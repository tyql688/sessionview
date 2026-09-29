use rusqlite::params;

use crate::models::SessionMeta;

use super::super::row_mapper::{SESSION_META_COLUMNS, row_to_session_meta};
use super::Database;

impl Database {
    pub(crate) fn add_favorite(&self, session_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_else(|e| {
                log::warn!("system clock before UNIX epoch in add_favorite: {e}");
                std::time::Duration::ZERO
            })
            .as_secs() as i64;
        conn.execute(
            "INSERT OR IGNORE INTO favorites (session_id, added_at) VALUES (?1, ?2)",
            params![session_id, now],
        )?;
        Ok(())
    }

    pub(crate) fn remove_favorite(&self, session_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        conn.execute(
            "DELETE FROM favorites WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    pub(crate) fn is_favorite(&self, session_id: &str) -> Result<bool, rusqlite::Error> {
        let conn = self.lock_read()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM favorites WHERE session_id = ?1)",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    pub(crate) fn list_favorites(&self) -> Result<Vec<SessionMeta>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_META_COLUMNS}
             FROM favorites f
             JOIN sessions s ON s.id = f.session_id
             ORDER BY f.added_at DESC"
        ))?;

        let rows = stmt.query_map([], row_to_session_meta)?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }
}
