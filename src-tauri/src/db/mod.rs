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

    /// Reclaim file-level bloat after heavy churn. Skips quickly unless the
    /// freelist exceeds ~10% of the file; then merges the FTS index's
    /// incremental b-trees and VACUUMs, shrinking the file to its live data
    /// (observed 20x growth on long-lived DBs that never vacuumed). VACUUM
    /// waits on the busy timeout if a reader holds the file; callers treat
    /// failure as best-effort and retry on a later maintenance pass.
    pub fn compact_if_bloated(&self) -> Result<bool, rusqlite::Error> {
        let conn = self.lock_write()?;
        let (page_count, freelist_count): (i64, i64) = conn.query_row(
            "SELECT page_count, freelist_count FROM pragma_page_count, pragma_freelist_count",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if page_count == 0 || freelist_count * 10 < page_count {
            return Ok(false);
        }
        conn.execute_batch("INSERT INTO sessions_fts(sessions_fts) VALUES('optimize')")?;
        conn.execute("VACUUM", [])?;
        // In WAL mode the rewritten pages land in the WAL; the main file only
        // shrinks once they are checkpointed back and the WAL is truncated.
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(true)
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

        write_conn.execute_batch(
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
                content_text       TEXT NOT NULL DEFAULT '',
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

            CREATE TRIGGER IF NOT EXISTS sessions_ai AFTER INSERT ON sessions BEGIN
                INSERT INTO sessions_fts(rowid, title, content_text, project_name)
                VALUES (new.rowid, new.title, new.content_text, new.project_name);
            END;

            CREATE TRIGGER IF NOT EXISTS sessions_ad AFTER DELETE ON sessions BEGIN
                INSERT INTO sessions_fts(sessions_fts, rowid, title, content_text, project_name)
                VALUES ('delete', old.rowid, old.title, old.content_text, old.project_name);
            END;

            -- UPDATE OF: token-total/mtime-only updates must not churn the
            -- trigram index (it is ~10x the indexed content).
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

            CREATE TABLE IF NOT EXISTS session_token_stats (
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
        )?;

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
