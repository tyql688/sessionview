pub(crate) mod queries;
mod row_mapper;
pub(crate) mod sync;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rusqlite::{params_from_iter, Connection};

/// Number of read-only connections in the pool. SQLite WAL allows
/// concurrent readers across distinct connections, so each connection in
/// the pool can serve a query in parallel; we serialize within a single
/// connection via its Mutex.
const READ_POOL_SIZE: usize = 4;

pub struct Database {
    write_conn: Mutex<Connection>,
    read_pool: Vec<Mutex<Connection>>,
    read_cursor: AtomicUsize,
    db_path: std::path::PathBuf,
}

impl Database {
    /// Acquire the write connection lock, recovering from mutex poisoning.
    fn lock_write(&self) -> Result<std::sync::MutexGuard<'_, Connection>, rusqlite::Error> {
        self.write_conn.lock().map_err(|_| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_LOCKED),
                Some("write mutex poisoned".to_string()),
            )
        })
    }

    /// Acquire a read connection from the pool. Try each slot once with
    /// try_lock starting at a rotating cursor; fall back to a blocking lock
    /// on the cursor slot if every connection is busy.
    fn lock_read(&self) -> Result<std::sync::MutexGuard<'_, Connection>, rusqlite::Error> {
        let n = self.read_pool.len();
        let start = self.read_cursor.fetch_add(1, Ordering::Relaxed) % n;

        for offset in 0..n {
            let idx = (start + offset) % n;
            if let Ok(guard) = self.read_pool[idx].try_lock() {
                return Ok(guard);
            }
        }

        // All busy — block on the rotating slot. Poisoning is recovered as
        // SQLITE_LOCKED so callers fall through their existing error paths.
        self.read_pool[start].lock().map_err(|_| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_LOCKED),
                Some("read mutex poisoned".to_string()),
            )
        })
    }
}

impl Database {
    /// Fold the WAL back into the main file and truncate it. Heavy sync
    /// passes append hundreds of MB of WAL, and the passive autocheckpoint
    /// never wins against steady read traffic — left alone the WAL grows
    /// unbounded (observed >1GB) and every reader pays to scan it. Called
    /// after maintenance work; best-effort (TRUNCATE yields to active
    /// readers rather than erroring).
    pub fn checkpoint_truncate(&self) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    pub fn with_transaction<T, F>(&self, f: F) -> Result<T, rusqlite::Error>
    where
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    {
        let conn = self.lock_write()?;
        conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;
        match f(&conn) {
            Ok(value) => {
                conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(err) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    pub fn open(data_dir: &Path) -> Result<Self, rusqlite::Error> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let db_path = data_dir.join("sessions.db");

        let write_conn = Connection::open(&db_path)?;

        write_conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA cache_size = -2000;
             PRAGMA busy_timeout = 5000;",
        )?;

        write_conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id              TEXT PRIMARY KEY,
                provider        TEXT NOT NULL,
                title           TEXT NOT NULL DEFAULT '',
                project_path    TEXT NOT NULL DEFAULT '',
                project_name    TEXT NOT NULL DEFAULT '',
                created_at      INTEGER NOT NULL DEFAULT 0,
                updated_at      INTEGER NOT NULL DEFAULT 0,
                message_count   INTEGER NOT NULL DEFAULT 0,
                file_size_bytes INTEGER NOT NULL DEFAULT 0,
                source_path     TEXT NOT NULL DEFAULT '',
                content_text    TEXT NOT NULL DEFAULT ''
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_provider ON sessions(provider);
            CREATE INDEX IF NOT EXISTS idx_sessions_project_name ON sessions(project_name);
            CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_sessions_provider_updated ON sessions(provider, updated_at DESC);

            CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(
                title, content_text, project_name,
                content='sessions',
                content_rowid='rowid',
                tokenize='trigram'
            );

            CREATE TRIGGER IF NOT EXISTS sessions_ai AFTER INSERT ON sessions BEGIN
                INSERT INTO sessions_fts(rowid, title, content_text, project_name)
                VALUES (new.rowid, new.title, new.content_text, new.project_name);
            END;

            CREATE TRIGGER IF NOT EXISTS sessions_ad AFTER DELETE ON sessions BEGIN
                INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
                VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
            END;

            -- UPDATE OF: token-total/mtime-only updates must not churn the
            -- trigram index (it is ~10x the indexed content). Keep the column
            -- list in sync with the two re-creation sites below.
            CREATE TRIGGER IF NOT EXISTS sessions_au
            AFTER UPDATE OF title, content_text, project_name ON sessions BEGIN
                INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
                VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
                INSERT INTO sessions_fts(rowid, title, content_text, project_name)
                VALUES (new.rowid, new.title, new.content_text, new.project_name);
            END;

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT
            );

            CREATE TABLE IF NOT EXISTS favorites (
                session_id TEXT PRIMARY KEY,
                added_at   INTEGER NOT NULL
            );

            -- Add title_custom column for user-renamed sessions (safe to re-run)
            ",
        )?;

        // Migration: add title_custom column if not exists
        let has_title_custom: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'title_custom'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_title_custom {
            write_conn.execute_batch(
                "ALTER TABLE sessions ADD COLUMN title_custom INTEGER NOT NULL DEFAULT 0;",
            )?;
        }

        // Migration: add is_sidechain column if not exists
        let has_is_sidechain: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'is_sidechain'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_is_sidechain {
            write_conn.execute_batch(
                "ALTER TABLE sessions ADD COLUMN is_sidechain INTEGER NOT NULL DEFAULT 0;",
            )?;
        }

        // Migration: add variant_name column if not exists
        let has_variant_name: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'variant_name'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_variant_name {
            write_conn.execute_batch("ALTER TABLE sessions ADD COLUMN variant_name TEXT;")?;
        }

        // Migration: add model column if not exists
        let has_model: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'model'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_model {
            write_conn.execute_batch("ALTER TABLE sessions ADD COLUMN model TEXT;")?;
        }

        // Migration: add cc_version column if not exists
        let has_cc_version: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'cc_version'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_cc_version {
            write_conn.execute_batch("ALTER TABLE sessions ADD COLUMN cc_version TEXT;")?;
        }

        // Migration: add git_branch column if not exists
        let has_git_branch: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'git_branch'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_git_branch {
            write_conn.execute_batch("ALTER TABLE sessions ADD COLUMN git_branch TEXT;")?;
        }

        // Migration: add parent_id column if not exists
        let has_parent_id: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'parent_id'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_parent_id {
            write_conn.execute_batch("ALTER TABLE sessions ADD COLUMN parent_id TEXT;")?;
        }

        // Migration: add source_mtime column for the incremental indexer.
        // 0 = unknown / forces a reparse on next scan (acts as a sentinel
        // for pre-migration rows). New rows get the real epoch seconds.
        let has_source_mtime: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'source_mtime'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_source_mtime {
            write_conn.execute_batch(
                "ALTER TABLE sessions ADD COLUMN source_mtime INTEGER NOT NULL DEFAULT 0;",
            )?;
        }

        // Migration: denormalize per-session token totals into the sessions
        // table. Previously every list / search query recomputed these via
        // four correlated `SELECT COALESCE(SUM(...))` subqueries against
        // `session_token_stats`, paying a B-tree probe per row even though
        // the totals only change when stats are rewritten. The aggregates
        // are kept in sync inside `replace_token_stats_batch`'s transaction
        // and backfilled here on first run.
        let has_input_tokens_col: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'input_tokens'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_input_tokens_col {
            write_conn.execute_batch(
                "ALTER TABLE sessions ADD COLUMN input_tokens INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE sessions ADD COLUMN output_tokens INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE sessions ADD COLUMN cache_read_tokens INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE sessions ADD COLUMN cache_write_tokens INTEGER NOT NULL DEFAULT 0;",
            )?;
            // Backfill from `session_token_stats` happens further down,
            // after the table is guaranteed to exist (see below).
        }
        write_conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_sessions_parent_updated
                ON sessions(parent_id, updated_at DESC);
             CREATE INDEX IF NOT EXISTS idx_sessions_parent_created
                ON sessions(parent_id, created_at);",
        )?;

        write_conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS session_token_stats (
                session_id          TEXT    NOT NULL,
                date                TEXT    NOT NULL,
                model               TEXT    NOT NULL DEFAULT '',
                turn_count          INTEGER NOT NULL DEFAULT 0,
                input_tokens        INTEGER NOT NULL DEFAULT 0,
                output_tokens       INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
                cost_usd            REAL    NOT NULL DEFAULT 0,
                PRIMARY KEY (session_id, date, model)
            );

            CREATE INDEX IF NOT EXISTS idx_token_stats_date
                ON session_token_stats(date);

            CREATE TRIGGER IF NOT EXISTS trg_token_stats_cascade
            AFTER DELETE ON sessions
            BEGIN
                DELETE FROM session_token_stats WHERE session_id = OLD.id;
            END;",
        )?;

        if !has_input_tokens_col {
            // Backfill the newly-added per-session token totals from the
            // existing stats rows. Runs once, immediately after the
            // sessions.input_tokens column is created and session_token_stats
            // is guaranteed to exist.
            write_conn.execute_batch(
                "UPDATE sessions SET
                    input_tokens = COALESCE((SELECT SUM(input_tokens) FROM session_token_stats WHERE session_id = sessions.id), 0),
                    output_tokens = COALESCE((SELECT SUM(output_tokens) FROM session_token_stats WHERE session_id = sessions.id), 0),
                    cache_read_tokens = COALESCE((SELECT SUM(cache_read_tokens) FROM session_token_stats WHERE session_id = sessions.id), 0),
                    cache_write_tokens = COALESCE((SELECT SUM(cache_write_tokens) FROM session_token_stats WHERE session_id = sessions.id), 0);",
            )?;
        }

        let has_token_cost: bool = {
            let mut stmt = write_conn.prepare(
                "SELECT COUNT(*) FROM pragma_table_info('session_token_stats') WHERE name = 'cost_usd'",
            )?;
            let count: i64 = stmt.query_row([], |row| row.get(0))?;
            count > 0
        };
        if !has_token_cost {
            write_conn.execute_batch(
                "ALTER TABLE session_token_stats ADD COLUMN cost_usd REAL NOT NULL DEFAULT 0;",
            )?;
        }

        // Migration: rebuild FTS index when tokenizer configuration changes.
        // Bump FTS_TOKENIZER_VERSION whenever the tokenizer config in the CREATE VIRTUAL TABLE above changes.
        const FTS_TOKENIZER_VERSION: &str = "trigram_v1";
        let current_fts_version: Option<String> = {
            let mut stmt =
                write_conn.prepare("SELECT value FROM meta WHERE key = 'fts_tokenizer_version'")?;
            stmt.query_row([], |row| row.get(0)).ok()
        };
        if current_fts_version.as_deref() != Some(FTS_TOKENIZER_VERSION) {
            write_conn.execute_batch(
                "DROP TRIGGER IF EXISTS sessions_ai;
                 DROP TRIGGER IF EXISTS sessions_ad;
                 DROP TRIGGER IF EXISTS sessions_au;
                 DROP TABLE IF EXISTS sessions_fts;

                 CREATE VIRTUAL TABLE sessions_fts USING fts5(
                     title, content_text, project_name,
                     content='sessions',
                     content_rowid='rowid',
                     tokenize='trigram'
                 );

                 CREATE TRIGGER sessions_ai AFTER INSERT ON sessions BEGIN
                     INSERT INTO sessions_fts(rowid, title, content_text, project_name)
                     VALUES (new.rowid, new.title, new.content_text, new.project_name);
                 END;

                 CREATE TRIGGER sessions_ad AFTER DELETE ON sessions BEGIN
                     INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
                     VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
                 END;

                 CREATE TRIGGER sessions_au
                 AFTER UPDATE OF title, content_text, project_name ON sessions BEGIN
                     INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
                     VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
                     INSERT INTO sessions_fts(rowid, title, content_text, project_name)
                     VALUES (new.rowid, new.title, new.content_text, new.project_name);
                 END;

                 INSERT INTO sessions_fts(sessions_fts) VALUES('rebuild');

                 INSERT INTO meta (key, value) VALUES ('fts_tokenizer_version', 'trigram_v1')
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            )?;
        }

        // Migration: narrow the FTS update trigger to the indexed columns.
        // Existing DBs carry the old `AFTER UPDATE` form, which rewrote the
        // session's trigram postings on EVERY row update (token totals,
        // source_mtime, ...). Cheap swap — the index itself is untouched.
        const SESSIONS_AU_VERSION: &str = "update_of_v1";
        let current_au_version: Option<String> = {
            let mut stmt =
                write_conn.prepare("SELECT value FROM meta WHERE key = 'sessions_au_version'")?;
            stmt.query_row([], |row| row.get(0)).ok()
        };
        if current_au_version.as_deref() != Some(SESSIONS_AU_VERSION) {
            write_conn.execute_batch(
                "DROP TRIGGER IF EXISTS sessions_au;

                 CREATE TRIGGER sessions_au
                 AFTER UPDATE OF title, content_text, project_name ON sessions BEGIN
                     INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
                     VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
                     INSERT INTO sessions_fts(rowid, title, content_text, project_name)
                     VALUES (new.rowid, new.title, new.content_text, new.project_name);
                 END;

                 INSERT INTO meta (key, value) VALUES ('sessions_au_version', 'update_of_v1')
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            )?;
        }

        // Migration: force a one-time re-parse of Claude-format sessions after
        // the usage accounting fixes (cumulative streamed usage now keeps the
        // largest entry per messageId:requestId; subagents/workflows/ files are
        // now scanned). Token stats are only rewritten when a file is re-parsed,
        // and the incremental scan skips files whose (size, mtime) match the DB,
        // so resetting source_mtime is what makes the next scan revisit them.
        // Bump CLAUDE_USAGE_STATS_VERSION whenever stats need a full rebuild.
        const CLAUDE_USAGE_STATS_VERSION: &str = "max_usage_v1";
        let current_usage_stats_version: Option<String> = {
            let mut stmt =
                write_conn.prepare("SELECT value FROM meta WHERE key = 'usage_stats_version'")?;
            stmt.query_row([], |row| row.get(0)).ok()
        };
        if current_usage_stats_version.as_deref() != Some(CLAUDE_USAGE_STATS_VERSION) {
            write_conn.execute_batch(&format!(
                "UPDATE sessions SET source_mtime = 0 WHERE provider IN ('claude', 'cc-mirror');
                 INSERT INTO meta (key, value) VALUES ('usage_stats_version', '{CLAUDE_USAGE_STATS_VERSION}')
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            ))?;
        }

        // Migration: Pi session metadata timestamps were initially parsed as
        // epoch milliseconds, while every `sessions.created_at/updated_at`
        // consumer expects epoch seconds. `updated_at` also needs to match
        // Pi's session-list `modified` semantics: latest user/assistant
        // message timestamp, not the last JSONL entry. Correct rows that
        // already contain millisecond values and invalidate all Pi freshness
        // snapshots so the fixed parser rewrites activity times.
        const PI_TIMESTAMP_VERSION: &str = "message_activity_epoch_seconds_v1";
        let current_pi_timestamp_version: Option<String> = {
            let mut stmt =
                write_conn.prepare("SELECT value FROM meta WHERE key = 'pi_timestamp_version'")?;
            stmt.query_row([], |row| row.get(0)).ok()
        };
        if current_pi_timestamp_version.as_deref() != Some(PI_TIMESTAMP_VERSION) {
            let fixed_rows = write_conn.execute(
                "UPDATE sessions
                    SET created_at = CASE
                            WHEN created_at > 20000000000 THEN created_at / 1000
                            ELSE created_at
                        END,
                        updated_at = CASE
                            WHEN updated_at > 20000000000 THEN updated_at / 1000
                            ELSE updated_at
                        END
                  WHERE provider = 'pi'
                    AND (created_at > 20000000000 OR updated_at > 20000000000)",
                [],
            )?;
            let invalidated_rows = write_conn.execute(
                "UPDATE sessions SET source_mtime = 0 WHERE provider = 'pi'",
                [],
            )?;
            if fixed_rows > 0 || invalidated_rows > 0 {
                log::info!(
                    "corrected {fixed_rows} Pi timestamp rows and invalidated {invalidated_rows} Pi source snapshots"
                );
            }
            write_conn.execute_batch(&format!(
                "INSERT INTO meta (key, value)
                     VALUES ('pi_timestamp_version', '{PI_TIMESTAMP_VERSION}')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            ))?;
        }

        // Migration: early Pi parent linking stored header.parentSession
        // directly, but Pi writes that field as a JSONL file path. The app's
        // tree expects parent_id to be the parent session id, so clear path-like
        // values and force a Pi reparse that resolves the parent header id.
        const PI_PARENT_SESSION_VERSION: &str = "parent_session_id_v1";
        let current_pi_parent_session_version: Option<String> = {
            let mut stmt = write_conn
                .prepare("SELECT value FROM meta WHERE key = 'pi_parent_session_version'")?;
            stmt.query_row([], |row| row.get(0)).ok()
        };
        if current_pi_parent_session_version.as_deref() != Some(PI_PARENT_SESSION_VERSION) {
            let repaired_rows = write_conn.execute(
                "UPDATE sessions
                    SET parent_id = NULL,
                        is_sidechain = 0
                  WHERE provider = 'pi'
                    AND parent_id IS NOT NULL
                    AND (parent_id LIKE '%/%'
                         OR parent_id LIKE '%\\%'
                         OR parent_id LIKE '%.jsonl')",
                [],
            )?;
            let invalidated_rows = write_conn.execute(
                "UPDATE sessions SET source_mtime = 0 WHERE provider = 'pi'",
                [],
            )?;
            if repaired_rows > 0 || invalidated_rows > 0 {
                log::info!(
                    "repaired {repaired_rows} Pi parent links and invalidated {invalidated_rows} Pi source snapshots"
                );
            }
            write_conn.execute_batch(&format!(
                "INSERT INTO meta (key, value)
                     VALUES ('pi_parent_session_version', '{PI_PARENT_SESSION_VERSION}')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            ))?;
        }

        // Migration: Kimi AgentSwarm subagents now use state.json's structured
        // `swarmItem` value as their title source. Existing rows may have been
        // indexed before the parent swarm result finished, leaving the long
        // generated prompt as the title. Invalidate Kimi freshness snapshots so
        // the next scan replays the fixed parser.
        const KIMI_SWARM_ITEM_VERSION: &str = "state_swarm_item_titles_v1";
        let current_kimi_swarm_item_version: Option<String> = {
            let mut stmt = write_conn
                .prepare("SELECT value FROM meta WHERE key = 'kimi_swarm_item_version'")?;
            stmt.query_row([], |row| row.get(0)).ok()
        };
        if current_kimi_swarm_item_version.as_deref() != Some(KIMI_SWARM_ITEM_VERSION) {
            let invalidated_rows = write_conn.execute(
                "UPDATE sessions SET source_mtime = 0 WHERE provider = 'kimi'",
                [],
            )?;
            if invalidated_rows > 0 {
                log::info!(
                    "invalidated {invalidated_rows} Kimi source snapshots for AgentSwarm swarmItem titles"
                );
            }
            write_conn.execute_batch(&format!(
                "INSERT INTO meta (key, value)
                     VALUES ('kimi_swarm_item_version', '{KIMI_SWARM_ITEM_VERSION}')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            ))?;
        }

        // Migration: the FTS content text now also indexes thinking excerpts
        // and tool call summaries (see db/sync.rs::indexable_content_text).
        // `content_text` is only rewritten when a session is re-parsed, so
        // invalidate every freshness snapshot to force a full reindex; the
        // FTS triggers propagate the richer text into sessions_fts.
        // Bump CONTENT_INDEX_VERSION whenever indexable_content_text changes
        // what it emits.
        const CONTENT_INDEX_VERSION: &str = "thinking_tool_text_v1";
        let current_content_index_version: Option<String> = {
            let mut stmt =
                write_conn.prepare("SELECT value FROM meta WHERE key = 'content_index_version'")?;
            stmt.query_row([], |row| row.get(0)).ok()
        };
        if current_content_index_version.as_deref() != Some(CONTENT_INDEX_VERSION) {
            let invalidated_rows =
                write_conn.execute("UPDATE sessions SET source_mtime = 0", [])?;
            if invalidated_rows > 0 {
                log::info!(
                    "invalidated {invalidated_rows} source snapshots to rebuild search index with thinking/tool content"
                );
            }
            write_conn.execute_batch(&format!(
                "INSERT INTO meta (key, value)
                     VALUES ('content_index_version', '{CONTENT_INDEX_VERSION}')
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            ))?;
        }

        let supported_provider_keys: Vec<&str> = crate::models::Provider::all()
            .iter()
            .map(|p| p.key())
            .collect();
        let supported_provider_placeholders =
            std::iter::repeat_n("?", supported_provider_keys.len())
                .collect::<Vec<_>>()
                .join(", ");
        let unsupported_provider_filter =
            format!("provider NOT IN ({supported_provider_placeholders})");
        let removed_provider_rows: i64 = write_conn.query_row(
            &format!("SELECT COUNT(*) FROM sessions WHERE {unsupported_provider_filter}"),
            params_from_iter(supported_provider_keys.iter().copied()),
            |row| row.get(0),
        )?;
        if removed_provider_rows > 0 {
            write_conn.execute(
                &format!(
                    "DELETE FROM favorites
                        WHERE session_id IN (
                            SELECT id FROM sessions WHERE {unsupported_provider_filter}
                        )"
                ),
                params_from_iter(supported_provider_keys.iter().copied()),
            )?;
            write_conn.execute(
                &format!("DELETE FROM sessions WHERE {unsupported_provider_filter}"),
                params_from_iter(supported_provider_keys.iter().copied()),
            )?;
            write_conn.execute(
                "INSERT INTO sessions_fts(sessions_fts) VALUES('rebuild')",
                [],
            )?;
        }

        let mut read_pool = Vec::with_capacity(READ_POOL_SIZE);
        for _ in 0..READ_POOL_SIZE {
            let conn = Connection::open(&db_path)?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "query_only", "ON")?;
            conn.pragma_update(None, "busy_timeout", 5000)?;
            read_pool.push(Mutex::new(conn));
        }

        Ok(Self {
            write_conn: Mutex::new(write_conn),
            read_pool,
            read_cursor: AtomicUsize::new(0),
            db_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_migrates_pi_millisecond_session_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = Database::open(dir.path()).unwrap();
            let conn = db.lock_write().unwrap();
            conn.execute(
                "INSERT INTO sessions (
                    id, provider, title, project_path, project_name,
                    created_at, updated_at, message_count, file_size_bytes,
                    source_path, content_text, source_mtime
                ) VALUES (
                    'pi-session', 'pi', 'Pi session', '/tmp/project', 'project',
                    1781076452486, 1781081413238, 1, 123,
                    '/tmp/pi-session.jsonl', 'hello', 42
                )",
                [],
            )
            .unwrap();
            conn.execute("DELETE FROM meta WHERE key = 'pi_timestamp_version'", [])
                .unwrap();
        }

        let db = Database::open(dir.path()).unwrap();
        let conn = db.lock_read().unwrap();
        let (created_at, updated_at, source_mtime): (i64, i64, i64) = conn
            .query_row(
                "SELECT created_at, updated_at, source_mtime
                   FROM sessions
                  WHERE id = 'pi-session'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'pi_timestamp_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(created_at, 1_781_076_452);
        assert_eq!(updated_at, 1_781_081_413);
        assert_eq!(source_mtime, 0);
        assert_eq!(version, "message_activity_epoch_seconds_v1");
    }

    #[test]
    fn open_migrates_pi_parent_session_paths() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = Database::open(dir.path()).unwrap();
            let conn = db.lock_write().unwrap();
            conn.execute(
                "INSERT INTO sessions (
                    id, provider, title, project_path, project_name,
                    created_at, updated_at, message_count, file_size_bytes,
                    source_path, content_text, is_sidechain, parent_id, source_mtime
                ) VALUES (
                    'pi-child-path', 'pi', 'Pi child path', '/tmp/project', 'project',
                    1781076452, 1781081413, 1, 123,
                    '/tmp/child.jsonl', 'hello', 1,
                    '/Users/test/.pi/agent/sessions/--tmp-project--/parent.jsonl', 42
                )",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (
                    id, provider, title, project_path, project_name,
                    created_at, updated_at, message_count, file_size_bytes,
                    source_path, content_text, is_sidechain, parent_id, source_mtime
                ) VALUES (
                    'pi-child-id', 'pi', 'Pi child id', '/tmp/project', 'project',
                    1781076452, 1781081413, 1, 123,
                    '/tmp/child-id.jsonl', 'hello', 1,
                    'parent-session-id', 99
                )",
                [],
            )
            .unwrap();
            conn.execute(
                "DELETE FROM meta WHERE key = 'pi_parent_session_version'",
                [],
            )
            .unwrap();
        }

        let db = Database::open(dir.path()).unwrap();
        let conn = db.lock_read().unwrap();
        let path_row: (Option<String>, i64, i64) = conn
            .query_row(
                "SELECT parent_id, is_sidechain, source_mtime
                   FROM sessions
                  WHERE id = 'pi-child-path'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let id_row: (Option<String>, i64, i64) = conn
            .query_row(
                "SELECT parent_id, is_sidechain, source_mtime
                   FROM sessions
                  WHERE id = 'pi-child-id'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'pi_parent_session_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(path_row, (None, 0, 0));
        assert_eq!(id_row, (Some("parent-session-id".to_string()), 1, 0));
        assert_eq!(version, "parent_session_id_v1");
    }

    #[test]
    fn open_invalidates_kimi_rows_for_swarm_item_titles() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = Database::open(dir.path()).unwrap();
            let conn = db.lock_write().unwrap();
            conn.execute(
                "INSERT INTO sessions (
                    id, provider, title, project_path, project_name,
                    created_at, updated_at, message_count, file_size_bytes,
                    source_path, content_text, source_mtime
                ) VALUES (
                    'kimi-child', 'kimi', 'long generated prompt', '/tmp/teli', 'teli',
                    1781076452, 1781081413, 1, 123,
                    '/tmp/kimi/wire.jsonl', 'hello', 42
                )",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (
                    id, provider, title, project_path, project_name,
                    created_at, updated_at, message_count, file_size_bytes,
                    source_path, content_text, source_mtime
                ) VALUES (
                    'codex-session', 'codex', 'other', '/tmp/teli', 'teli',
                    1781076452, 1781081413, 1, 123,
                    '/tmp/codex/session.jsonl', 'hello', 77
                )",
                [],
            )
            .unwrap();
            conn.execute("DELETE FROM meta WHERE key = 'kimi_swarm_item_version'", [])
                .unwrap();
        }

        let db = Database::open(dir.path()).unwrap();
        let conn = db.lock_read().unwrap();
        let kimi_mtime: i64 = conn
            .query_row(
                "SELECT source_mtime FROM sessions WHERE id = 'kimi-child'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let codex_mtime: i64 = conn
            .query_row(
                "SELECT source_mtime FROM sessions WHERE id = 'codex-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'kimi_swarm_item_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(kimi_mtime, 0);
        assert_eq!(codex_mtime, 77);
        assert_eq!(version, "state_swarm_item_titles_v1");
    }
}
