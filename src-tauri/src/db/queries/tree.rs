use std::collections::HashMap;

use rusqlite::{params, params_from_iter};

use crate::models::SessionMeta;

use super::super::row_mapper::{SESSION_META_COLUMNS, row_to_session_meta};
use super::Database;
use super::search::list_sessions_from_query;

impl Database {
    pub fn list_recent_sessions(&self, limit: usize) -> Result<Vec<SessionMeta>, rusqlite::Error> {
        let conn = self.lock_read()?;
        list_sessions_from_query(
            &conn,
            &format!(
                "SELECT {SESSION_META_COLUMNS} FROM sessions s
                 WHERE s.parent_id IS NULL
                 ORDER BY s.updated_at DESC
                 LIMIT ?1"
            ),
            params![limit as i64],
        )
    }

    /// Returns full SessionMeta for all children of a given parent session.
    pub(crate) fn get_child_sessions(
        &self,
        parent_id: &str,
    ) -> Result<Vec<SessionMeta>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {SESSION_META_COLUMNS} FROM sessions s
             WHERE s.parent_id = ?1
             ORDER BY s.created_at"
        ))?;
        let rows = stmt.query_map(params![parent_id], row_to_session_meta)?;
        let mut sessions = Vec::new();
        for row in rows {
            match row {
                Ok(meta) => sessions.push(meta),
                Err(e) => {
                    log::warn!("failed to map child session row for parent {parent_id}: {e}");
                }
            }
        }
        Ok(sessions)
    }

    pub(crate) fn child_session_counts(
        &self,
        parent_ids: &[String],
    ) -> Result<HashMap<String, u64>, rusqlite::Error> {
        if parent_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let conn = self.lock_read()?;
        let placeholders = std::iter::repeat_n("?", parent_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT parent_id, COUNT(*)
             FROM sessions
             WHERE parent_id IN ({placeholders})
             GROUP BY parent_id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(parent_ids.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?;

        let mut counts = HashMap::new();
        for row in rows {
            let (parent_id, count) = row?;
            counts.insert(parent_id, count);
        }
        Ok(counts)
    }
}
