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
             PRAGMA cache_size = -2000;",
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

            CREATE TRIGGER IF NOT EXISTS sessions_au AFTER UPDATE ON sessions BEGIN
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

                 CREATE TRIGGER sessions_au AFTER UPDATE ON sessions BEGIN
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
