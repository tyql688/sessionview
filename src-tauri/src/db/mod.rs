pub(crate) mod queries;
mod row_mapper;
pub(crate) mod sync;

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};

/// Number of read-only connections in the pool. SQLite WAL allows
/// concurrent readers across distinct connections, so each connection in
/// the pool can serve a query in parallel; we serialize within a single
/// connection via its Mutex.
const READ_POOL_SIZE: usize = 4;
/// Meta key holding the page count the last compaction left behind.
pub(crate) const COMPACTED_PAGE_COUNT_KEY: &str = "compacted_page_count";

/// `sessions.content_text` is a virtual generated column over the
/// zstd-compressed `content_zst` blob (the search text is ~3.8x smaller
/// compressed). Every connection registers this function before touching
/// the table; one that lacks it fails loudly instead of misreading content.
const CONTENT_TEXT_FUNCTION: &str = "session_content_text";

/// Keeps the external-content FTS index in step with `sessions`. The
/// `UPDATE OF` list keeps token-total/mtime-only updates from churning the
/// trigram index (it is ~2x the indexed content).
const SESSIONS_FTS_TRIGGERS: &str = "
    CREATE TRIGGER IF NOT EXISTS sessions_ai AFTER INSERT ON sessions BEGIN
        INSERT INTO sessions_fts(rowid, title, content_text, project_name)
        VALUES (new.rowid, new.title, new.content_text, new.project_name);
    END;

    CREATE TRIGGER IF NOT EXISTS sessions_ad AFTER DELETE ON sessions BEGIN
        INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
        VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
    END;

    CREATE TRIGGER IF NOT EXISTS sessions_au
    AFTER UPDATE OF title, content_zst, project_name ON sessions BEGIN
        INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
        VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
        INSERT INTO sessions_fts(rowid, title, content_text, project_name)
        VALUES (new.rowid, new.title, new.content_text, new.project_name);
    END;";

pub(crate) fn compress_content(text: &str) -> Result<Vec<u8>, rusqlite::Error> {
    zstd::bulk::compress(text.as_bytes(), zstd::DEFAULT_COMPRESSION_LEVEL)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

/// An empty blob is the column default and reads as empty text.
fn decompress_content(blob: &[u8]) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if blob.is_empty() {
        return Ok(String::new());
    }
    Ok(String::from_utf8(zstd::decode_all(blob)?)?)
}

fn register_content_function(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.create_scalar_function(
        CONTENT_TEXT_FUNCTION,
        1,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            let blob = ctx
                .get_raw(0)
                .as_blob()
                .map_err(|error| rusqlite::Error::UserFunctionError(error.into()))?;
            decompress_content(blob).map_err(rusqlite::Error::UserFunctionError)
        },
    )
}

/// Whether `sessions.content_text` is still a stored text column (false for
/// the generated column, and for a fresh file without a `sessions` table).
fn has_plain_content(conn: &Connection) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo('sessions')
                        WHERE name = 'content_text' AND hidden = 0)",
        [],
        |row| row.get(0),
    )
}

/// Move stored content into `content_zst` and turn `content_text` into the
/// generated column. The FTS index stays valid as is: the column reads back
/// the exact text it was built from.
fn compress_plain_content(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS sessions_ai;
         DROP TRIGGER IF EXISTS sessions_ad;
         DROP TRIGGER IF EXISTS sessions_au;
         ALTER TABLE sessions ADD COLUMN content_zst BLOB NOT NULL DEFAULT x'';",
    )?;
    {
        let rowids: Vec<i64> = conn
            .prepare("SELECT rowid FROM sessions")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let mut read = conn.prepare("SELECT content_text FROM sessions WHERE rowid = ?1")?;
        let mut write = conn.prepare("UPDATE sessions SET content_zst = ?1 WHERE rowid = ?2")?;
        for rowid in rowids {
            let text: String = read.query_row([rowid], |row| row.get(0))?;
            write.execute(params![compress_content(&text)?, rowid])?;
        }
    }
    conn.execute_batch(&format!(
        "ALTER TABLE sessions DROP COLUMN content_text;
         ALTER TABLE sessions ADD COLUMN content_text TEXT
             GENERATED ALWAYS AS ({CONTENT_TEXT_FUNCTION}(content_zst)) VIRTUAL;
         {SESSIONS_FTS_TRIGGERS}"
    ))
}

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

/// Whether `session_token_stats` still has the pre-bucket `date` column.
fn has_legacy_date_stats(conn: &Connection) -> Result<bool, rusqlite::Error> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('session_token_stats') WHERE name = 'date'",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Merge the FTS index's b-trees into one and VACUUM, then record the
/// resulting page count as the baseline `compact_if_bloated` measures growth
/// against.
fn compact_on(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("INSERT INTO sessions_fts(sessions_fts) VALUES('optimize')")?;
    conn.execute("VACUUM", [])?;
    // In WAL mode the rewritten pages land in the WAL; the main file only
    // shrinks once they are checkpointed back and the WAL is truncated.
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    let compacted: i64 = conn.query_row("SELECT page_count FROM pragma_page_count", [], |row| {
        row.get(0)
    })?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![COMPACTED_PAGE_COUNT_KEY, compacted.to_string()],
    )?;
    Ok(())
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

    /// Reclaim file-level bloat after heavy churn. Skips quickly unless the
    /// freelist exceeds ~10% of the file or the file has grown to twice the
    /// size the last compaction left (or was never compacted): FTS5 keeps
    /// deleted postings until its segments merge, so rewriting sessions — a
    /// live session on every change, every session after an index-content
    /// revision — grows the index without freeing a page. Then merges the FTS
    /// index's incremental b-trees and VACUUMs, shrinking the file to its live
    /// data (observed 20x growth on long-lived DBs that never vacuumed), and
    /// records the result as the next baseline. VACUUM waits on the busy
    /// timeout if a reader holds the file; callers treat failure as
    /// best-effort and retry on a later maintenance pass.
    pub fn compact_if_bloated(&self) -> Result<bool, rusqlite::Error> {
        let conn = self.lock_write()?;
        let (page_count, freelist_count): (i64, i64) = conn.query_row(
            "SELECT page_count, freelist_count FROM pragma_page_count, pragma_freelist_count",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let baseline: Option<i64> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [COMPACTED_PAGE_COUNT_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| value.parse().ok());
        let grown = baseline.is_none_or(|pages| page_count >= pages * 2);
        if page_count == 0 || (freelist_count * 10 < page_count && !grown) {
            return Ok(false);
        }
        compact_on(&conn)?;
        Ok(true)
    }

    /// Compact unconditionally: after a pass that rewrote every session's
    /// indexed content, the old postings sit in FTS segments below any growth
    /// threshold a compacted file would cross.
    pub fn compact(&self) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        compact_on(&conn)
    }

    pub fn with_transaction<T, F>(&self, f: F) -> Result<T, rusqlite::Error>
    where
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    {
        let mut conn = self.lock_write()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }

    pub fn open(data_dir: &Path) -> Result<Self, rusqlite::Error> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let db_path = data_dir.join("sessions.db");

        let write_conn = Connection::open(&db_path)?;

        // busy_timeout must come FIRST: on a fresh file the WAL switch takes
        // an exclusive lock, and with the default 0ms timeout a second
        // process opening the same brand-new DB (GUI + headless launched
        // together) fails with SQLITE_BUSY instead of waiting.
        write_conn.execute_batch(
            "PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA cache_size = -2000;",
        )?;
        register_content_function(&write_conn)?;

        // Stats are derived: drop the pre-bucket shape and reset the totals
        // and `source_mtime` so the next scan rebuilds them. The probe stays
        // outside the transaction — taking the write lock on every open makes
        // startup fail with SQLITE_BUSY while the other process indexes — and
        // the re-check inside keeps two upgrading processes from both acting.
        if has_legacy_date_stats(&write_conn)? {
            write_conn.execute_batch("BEGIN IMMEDIATE")?;
            let migration = (|| -> Result<(), rusqlite::Error> {
                if has_legacy_date_stats(&write_conn)? {
                    write_conn.execute_batch(
                        "DROP TABLE IF EXISTS session_token_stats;
                         UPDATE sessions SET
                            input_tokens = 0,
                            output_tokens = 0,
                            cache_read_tokens = 0,
                            cache_write_tokens = 0,
                            source_mtime = 0;",
                    )?;
                }
                Ok(())
            })();
            match migration {
                Ok(()) => write_conn.execute_batch("COMMIT")?,
                Err(err) => {
                    let _ = write_conn.execute_batch("ROLLBACK");
                    return Err(err);
                }
            }
        }

        // Same probe / re-check-under-lock shape as above. The freed text
        // pages return to the freelist; `compact_if_bloated` reclaims them
        // after the next index pass.
        if has_plain_content(&write_conn)? {
            let transaction =
                rusqlite::Transaction::new_unchecked(&write_conn, TransactionBehavior::Immediate)?;
            if has_plain_content(&transaction)? {
                compress_plain_content(&transaction)?;
            }
            transaction.commit()?;
        }

        write_conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS sessions (
                id                 TEXT PRIMARY KEY,
                provider           TEXT NOT NULL,
                title              TEXT NOT NULL DEFAULT '',
                project_path       TEXT NOT NULL DEFAULT '',
                project_name       TEXT NOT NULL DEFAULT '',
                created_at         INTEGER NOT NULL DEFAULT 0,
                updated_at         INTEGER NOT NULL DEFAULT 0,
                message_count      INTEGER NOT NULL DEFAULT 0,
                file_size_bytes    INTEGER NOT NULL DEFAULT 0,
                source_path        TEXT NOT NULL DEFAULT '',
                content_zst        BLOB NOT NULL DEFAULT x'',
                content_text       TEXT
                    GENERATED ALWAYS AS ({CONTENT_TEXT_FUNCTION}(content_zst)) VIRTUAL,
                title_custom       INTEGER NOT NULL DEFAULT 0,
                is_sidechain       INTEGER NOT NULL DEFAULT 0,
                variant_name       TEXT,
                model              TEXT,
                cc_version         TEXT,
                git_branch         TEXT,
                parent_id          TEXT,
                source_mtime       INTEGER NOT NULL DEFAULT 0,
                input_tokens       INTEGER NOT NULL DEFAULT 0,
                output_tokens      INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_provider ON sessions(provider);
            CREATE INDEX IF NOT EXISTS idx_sessions_project_name ON sessions(project_name);
            CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_sessions_provider_updated ON sessions(provider, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_sessions_parent_updated ON sessions(parent_id, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_sessions_parent_created ON sessions(parent_id, created_at);

            CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts USING fts5(
                title, content_text, project_name,
                content='sessions',
                content_rowid='rowid',
                tokenize='trigram'
            );
            {SESSIONS_FTS_TRIGGERS}

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT
            );

            CREATE TABLE IF NOT EXISTS favorites (
                session_id TEXT PRIMARY KEY,
                added_at   INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS session_token_stats (
                session_id          TEXT    NOT NULL,
                bucket              INTEGER NOT NULL,
                model               TEXT    NOT NULL DEFAULT '',
                turn_count          INTEGER NOT NULL DEFAULT 0,
                input_tokens        INTEGER NOT NULL DEFAULT 0,
                output_tokens       INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
                cost_usd            REAL    NOT NULL DEFAULT 0,
                estimated_turns     INTEGER NOT NULL DEFAULT 0,
                reported_turns      INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (session_id, bucket, model)
            );

            CREATE INDEX IF NOT EXISTS idx_token_stats_bucket
                ON session_token_stats(bucket);

            CREATE TABLE IF NOT EXISTS session_tool_stats (
                session_id          TEXT    NOT NULL,
                tool_key            TEXT    NOT NULL,
                label               TEXT    NOT NULL DEFAULT '',
                category            TEXT    NOT NULL DEFAULT '',
                count               INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (session_id, tool_key)
            );

            CREATE TABLE IF NOT EXISTS session_tool_index (
                session_id          TEXT PRIMARY KEY
            );

            CREATE INDEX IF NOT EXISTS idx_tool_stats_session
                ON session_tool_stats(session_id);

            CREATE TRIGGER IF NOT EXISTS trg_token_stats_cascade
            AFTER DELETE ON sessions
            BEGIN
                DELETE FROM session_token_stats WHERE session_id = OLD.id;
            END;

            CREATE TRIGGER IF NOT EXISTS trg_tool_stats_cascade
            AFTER DELETE ON sessions
            BEGIN
                DELETE FROM session_tool_stats WHERE session_id = OLD.id;
                DELETE FROM session_tool_index WHERE session_id = OLD.id;
            END;",
        ))?;

        // Additive migration: retain old totals until each provider is repriced.
        // Recheck under the write lock so simultaneous new-version opens agree.
        for column in ["estimated_turns", "reported_turns"] {
            let exists = |conn: &Connection| -> Result<bool, rusqlite::Error> {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_token_stats') WHERE name = ?1)",
                    [column], |row| row.get(0),
                )
            };
            if !exists(&write_conn)? {
                let transaction = rusqlite::Transaction::new_unchecked(
                    &write_conn,
                    TransactionBehavior::Immediate,
                )?;
                if !exists(&transaction)? {
                    transaction.execute_batch(&format!("ALTER TABLE session_token_stats ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0"))?;
                }
                transaction.commit()?;
            }
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
            conn.pragma_update(None, "busy_timeout", 5000)?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "query_only", "ON")?;
            register_content_function(&conn)?;
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
    use super::{Database, compress_content, has_plain_content};
    use crate::models::SearchFilters;

    const SESSION_ID: &str = "11111111-1111-4111-a111-111111111111";

    /// `sessions`, its FTS index and triggers as stored before `content_zst`.
    const PLAIN_CONTENT_SCHEMA: &str = "
        CREATE TABLE sessions (
            id TEXT PRIMARY KEY, provider TEXT NOT NULL, title TEXT NOT NULL DEFAULT '',
            project_path TEXT NOT NULL DEFAULT '', project_name TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0,
            message_count INTEGER NOT NULL DEFAULT 0, file_size_bytes INTEGER NOT NULL DEFAULT 0,
            source_path TEXT NOT NULL DEFAULT '', content_text TEXT NOT NULL DEFAULT '',
            title_custom INTEGER NOT NULL DEFAULT 0, is_sidechain INTEGER NOT NULL DEFAULT 0,
            variant_name TEXT, model TEXT, cc_version TEXT, git_branch TEXT, parent_id TEXT,
            source_mtime INTEGER NOT NULL DEFAULT 0, input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0, cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0);
        CREATE VIRTUAL TABLE sessions_fts USING fts5(title, content_text, project_name,
            content='sessions', content_rowid='rowid', tokenize='trigram');
        CREATE TRIGGER sessions_ai AFTER INSERT ON sessions BEGIN
            INSERT INTO sessions_fts(rowid, title, content_text, project_name)
            VALUES (new.rowid, new.title, new.content_text, new.project_name);
        END;
        CREATE TRIGGER sessions_ad AFTER DELETE ON sessions BEGIN
            INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
            VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
        END;
        CREATE TRIGGER sessions_au AFTER UPDATE OF title, content_text, project_name ON sessions
        BEGIN
            INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
            VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
            INSERT INTO sessions_fts(rowid, title, content_text, project_name)
            VALUES (new.rowid, new.title, new.content_text, new.project_name);
        END;
        CREATE TABLE favorites (session_id TEXT PRIMARY KEY, added_at INTEGER NOT NULL);
        INSERT INTO sessions (id, provider, title, project_name, content_text, title_custom)
            VALUES ('11111111-1111-4111-a111-111111111111', 'claude', 'Renamed', 'demo',
                    'alpha 需要 beta', 1);
        INSERT INTO favorites VALUES ('11111111-1111-4111-a111-111111111111', 1);";

    fn search(db: &Database, query: &str) -> Vec<(String, String)> {
        let filters = SearchFilters {
            query: query.into(),
            ..SearchFilters::default()
        };
        db.search_filtered(&filters)
            .unwrap()
            .into_iter()
            .map(|result| (result.session.id, result.snippet))
            .collect()
    }

    #[test]
    fn open_moves_plain_content_into_compressed_column_with_fts_intact() {
        let dir = tempfile::TempDir::new().unwrap();
        rusqlite::Connection::open(dir.path().join("sessions.db"))
            .unwrap()
            .execute_batch(PLAIN_CONTENT_SCHEMA)
            .unwrap();

        let db = Database::open(dir.path()).unwrap();

        // Trigram FTS serves >= 3-char queries, LIKE the shorter ones.
        assert_eq!(
            search(&db, "alpha"),
            [(
                SESSION_ID.to_string(),
                "<mark>alpha</mark> 需要 beta".to_string()
            )]
        );
        assert_eq!(
            search(&db, "需要"),
            [(
                SESSION_ID.to_string(),
                "alpha <mark>需要</mark> beta".to_string()
            )]
        );
        db.with_transaction(|conn| {
            assert!(!has_plain_content(conn)?);
            conn.execute(
                "INSERT INTO sessions_fts(sessions_fts, rank) VALUES('integrity-check', 1)",
                [],
            )?;
            let (title, favorites): (String, i64) = conn.query_row(
                "SELECT title, (SELECT COUNT(*) FROM favorites) FROM sessions WHERE title_custom = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!((title.as_str(), favorites), ("Renamed", 1));
            conn.execute(
                "UPDATE sessions SET content_zst = ?1",
                [compress_content("gamma delta")?],
            )
        })
        .unwrap();

        assert!(search(&db, "alpha").is_empty());
        assert_eq!(search(&db, "gamma").len(), 1);
    }
}
