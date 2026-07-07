use rusqlite::params;

use super::Database;

mod favorites;
mod search;
mod sessions;
mod tree;
mod usage;

pub(crate) use usage::UsageDateBounds;

#[derive(Debug, Clone)]
pub(crate) struct UsageByModelRow {
    pub model: String,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageProjectModelDetailRow {
    pub project_path: String,
    pub project_name: String,
    pub provider: String,
    pub session_id: String,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct UsageSessionModelDetailRow {
    pub session_id: String,
    pub project_path: String,
    pub project_name: String,
    pub provider: String,
    pub updated_at: i64,
    pub model: String,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

pub(crate) type UsageTotalsRow = (u64, u64, u64, u64, u64, u64, f64);

impl Database {
    pub(crate) fn get_meta(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |row| row.get::<_, String>(0))?;
        match rows.next() {
            Some(Ok(val)) => Ok(Some(val)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub(crate) fn set_meta(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub(crate) fn db_size_bytes(&self) -> u64 {
        // Prefer (page_count - freelist_count) * page_size for actual data usage;
        // file size on disk includes free pages that can't be reclaimed while the
        // app is running. Falls back to file metadata if the pragma query fails.
        match self.lock_read() {
            Ok(conn) => {
                match conn.query_row(
                    "SELECT (page_count - freelist_count) * page_size FROM pragma_page_count, pragma_freelist_count, pragma_page_size",
                    [],
                    |row| row.get::<_, u64>(0),
                ) {
                    Ok(used) if used > 0 => return used,
                    Ok(_) => {}
                    Err(error) => {
                        log::warn!("db_size_bytes pragma query failed: {error}");
                    }
                }
            }
            Err(error) => {
                log::warn!("db_size_bytes lock_read failed: {error}");
            }
        }
        match std::fs::metadata(&self.db_path) {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                log::warn!(
                    "db_size_bytes fallback metadata failed for '{}': {error}",
                    self.db_path.display()
                );
                0
            }
        }
    }
}
