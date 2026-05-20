use std::collections::HashMap;

use rusqlite::{params, params_from_iter, Connection};

use crate::models::{SearchFilters, SearchResult, SessionMeta, TokenTotals};

use super::row_mapper::row_to_session_meta;
use super::Database;

const LIKE_SNIPPET_CONTEXT_CHARS: usize = 80;

const LIKE_SNIPPET_MAX_CHARS: usize = 200;

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
    pub fn get_session(&self, id: &str) -> Result<Option<SessionMeta>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let mut stmt = conn.prepare(
            "SELECT id, provider, title, project_path, project_name,
                    created_at, updated_at, message_count, file_size_bytes, source_path, is_sidechain,
                    variant_name, model, cc_version, git_branch, parent_id,
                    (SELECT COALESCE(SUM(input_tokens), 0) FROM session_token_stats WHERE session_id = id) AS input_tokens,
                    (SELECT COALESCE(SUM(output_tokens), 0) FROM session_token_stats WHERE session_id = id) AS output_tokens,
                    (SELECT COALESCE(SUM(cache_read_tokens), 0) FROM session_token_stats WHERE session_id = id) AS cache_read_tokens,
                    (SELECT COALESCE(SUM(cache_write_tokens), 0) FROM session_token_stats WHERE session_id = id) AS cache_write_tokens
             FROM sessions WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], row_to_session_meta)?;
        match rows.next() {
            Some(Ok(meta)) => Ok(Some(meta)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn get_session_token_totals(
        &self,
        session_id: &str,
    ) -> Result<Option<TokenTotals>, rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COALESCE(SUM(cache_write_tokens), 0)
             FROM session_token_stats
             WHERE session_id = ?1",
            params![session_id],
            |row| {
                let count: u64 = row.get(0)?;
                if count == 0 {
                    return Ok(None);
                }
                Ok(Some(TokenTotals {
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_read_tokens: row.get(3)?,
                    cache_write_tokens: row.get(4)?,
                }))
            },
        )
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionMeta>, rusqlite::Error> {
        let conn = self.lock_read()?;
        list_sessions_from_query(
            &conn,
            "SELECT id, provider, title, project_path, project_name,
                    created_at, updated_at, message_count, file_size_bytes, source_path, is_sidechain,
                    variant_name, model, cc_version, git_branch, parent_id,
                    (SELECT COALESCE(SUM(input_tokens), 0) FROM session_token_stats WHERE session_id = id) AS input_tokens,
                    (SELECT COALESCE(SUM(output_tokens), 0) FROM session_token_stats WHERE session_id = id) AS output_tokens,
                    (SELECT COALESCE(SUM(cache_read_tokens), 0) FROM session_token_stats WHERE session_id = id) AS cache_read_tokens,
                    (SELECT COALESCE(SUM(cache_write_tokens), 0) FROM session_token_stats WHERE session_id = id) AS cache_write_tokens
             FROM sessions ORDER BY updated_at DESC",
            [],
        )
    }

    pub fn search_filtered(
        &self,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let has_query = !filters.query.trim().is_empty();
        let safe_query = build_fts_query(&filters.query);
        let has_filters = filters.provider.is_some()
            || filters.project.is_some()
            || filters.after.is_some()
            || filters.before.is_some();

        if !has_query && !has_filters {
            return Ok(Vec::new());
        }

        if let Some(query) = safe_query {
            search_with_fts(&conn, filters, &query).or_else(|_| search_with_like(&conn, filters))
        } else {
            search_with_like(&conn, filters)
        }
    }

    pub fn session_count(&self) -> Result<u64, rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
    }

    pub fn count_sessions_for_provider(&self, provider_key: &str) -> Result<u64, rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE provider = ?1",
            params![provider_key],
            |row| row.get(0),
        )
    }

    pub fn count_sessions_for_source(
        &self,
        provider_key: &str,
        source_path: &str,
    ) -> Result<u64, rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE provider = ?1 AND source_path = ?2",
            params![provider_key, source_path],
            |row| row.get(0),
        )
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |row| row.get::<_, String>(0))?;
        match rows.next() {
            Some(Ok(val)) => Ok(Some(val)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn db_size_bytes(&self) -> u64 {
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

    pub fn provider_session_counts(&self) -> Result<HashMap<String, u64>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let mut stmt = conn.prepare("SELECT provider, COUNT(*) FROM sessions GROUP BY provider")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?;

        let mut counts = HashMap::new();
        for row in rows {
            let (provider, count) = row?;
            counts.insert(provider, count);
        }
        Ok(counts)
    }

    pub fn list_recent_sessions(&self, limit: usize) -> Result<Vec<SessionMeta>, rusqlite::Error> {
        let conn = self.lock_read()?;
        list_sessions_from_query(
            &conn,
            "SELECT id, provider, title, project_path, project_name,
                    created_at, updated_at, message_count, file_size_bytes, source_path, is_sidechain,
                    variant_name, model, cc_version, git_branch, parent_id,
                    (SELECT COALESCE(SUM(input_tokens), 0) FROM session_token_stats WHERE session_id = id) AS input_tokens,
                    (SELECT COALESCE(SUM(output_tokens), 0) FROM session_token_stats WHERE session_id = id) AS output_tokens,
                    (SELECT COALESCE(SUM(cache_read_tokens), 0) FROM session_token_stats WHERE session_id = id) AS cache_read_tokens,
                    (SELECT COALESCE(SUM(cache_write_tokens), 0) FROM session_token_stats WHERE session_id = id) AS cache_write_tokens
             FROM sessions
             WHERE parent_id IS NULL
             ORDER BY updated_at DESC
             LIMIT ?1",
            params![limit as i64],
        )
    }

    /// Returns full SessionMeta for all children of a given parent session.
    pub fn get_child_sessions(&self, parent_id: &str) -> Result<Vec<SessionMeta>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let mut stmt = conn.prepare(
            "SELECT id, provider, title, project_path, project_name,
                    created_at, updated_at, message_count, file_size_bytes, source_path, is_sidechain,
                    variant_name, model, cc_version, git_branch, parent_id,
                    (SELECT COALESCE(SUM(input_tokens), 0) FROM session_token_stats WHERE session_id = id) AS input_tokens,
                    (SELECT COALESCE(SUM(output_tokens), 0) FROM session_token_stats WHERE session_id = id) AS output_tokens,
                    (SELECT COALESCE(SUM(cache_read_tokens), 0) FROM session_token_stats WHERE session_id = id) AS cache_read_tokens,
                    (SELECT COALESCE(SUM(cache_write_tokens), 0) FROM session_token_stats WHERE session_id = id) AS cache_write_tokens
             FROM sessions WHERE parent_id = ?1
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![parent_id], row_to_session_meta)?;
        let mut sessions = Vec::new();
        for row in rows {
            match row {
                Ok(meta) => sessions.push(meta),
                Err(e) => {
                    log::warn!(
                        "failed to map child session row for parent {}: {}",
                        parent_id,
                        e
                    );
                }
            }
        }
        Ok(sessions)
    }

    pub fn child_session_counts(
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

    pub fn add_favorite(&self, session_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT OR IGNORE INTO favorites (session_id, added_at) VALUES (?1, ?2)",
            params![session_id, now],
        )?;
        Ok(())
    }

    pub fn remove_favorite(&self, session_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        conn.execute(
            "DELETE FROM favorites WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    pub fn is_favorite(&self, session_id: &str) -> Result<bool, rusqlite::Error> {
        let conn = self.lock_read()?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM favorites WHERE session_id = ?1)",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    pub fn list_favorites(&self) -> Result<Vec<SessionMeta>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let mut stmt = conn.prepare(
            "SELECT s.id, s.provider, s.title, s.project_path, s.project_name,
                    s.created_at, s.updated_at, s.message_count, s.file_size_bytes, s.source_path, s.is_sidechain,
                    s.variant_name, s.model, s.cc_version, s.git_branch, s.parent_id,
                    (SELECT COALESCE(SUM(input_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS input_tokens,
                    (SELECT COALESCE(SUM(output_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS output_tokens,
                    (SELECT COALESCE(SUM(cache_read_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS cache_read_tokens,
                    (SELECT COALESCE(SUM(cache_write_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS cache_write_tokens
             FROM favorites f
             JOIN sessions s ON s.id = f.session_id
             ORDER BY f.added_at DESC",
        )?;

        let rows = stmt.query_map([], row_to_session_meta)?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    // ── Usage stats queries ──────────────────────────────────────────

    pub fn usage_session_count(
        &self,
        providers: &[String],
        cutoff_date: Option<&str>,
    ) -> Result<u64, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, cutoff_date);
        let sql = format!(
            "SELECT COUNT(DISTINCT s.session_id) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{}",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, param_refs.as_slice(), |row| row.get(0))
    }

    pub fn usage_session_count_by_provider(
        &self,
        providers: &[String],
        cutoff_date: Option<&str>,
    ) -> Result<Vec<(String, u64)>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, cutoff_date);
        let sql = format!(
            "SELECT sess.provider, COUNT(DISTINCT s.session_id) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY sess.provider",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?;
        rows.collect()
    }

    pub fn usage_totals(
        &self,
        providers: &[String],
        cutoff_date: Option<&str>,
    ) -> Result<(u64, u64, u64, u64, u64), rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, cutoff_date);
        let sql = format!(
            "SELECT COALESCE(SUM(s.turn_count),0), \
                    COALESCE(SUM(s.input_tokens),0), \
                    COALESCE(SUM(s.output_tokens),0), \
                    COALESCE(SUM(s.cache_read_tokens),0), \
                    COALESCE(SUM(s.cache_write_tokens),0) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{}",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, param_refs.as_slice(), |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
    }

    pub fn usage_daily(
        &self,
        providers: &[String],
        cutoff_date: Option<&str>,
    ) -> Result<Vec<(String, String, u64, f64)>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, cutoff_date);
        let sql = format!(
            "SELECT s.date, sess.provider, \
                    SUM(s.input_tokens + s.output_tokens + s.cache_read_tokens + s.cache_write_tokens), \
                    SUM(s.cost_usd) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY s.date, sess.provider \
             ORDER BY s.date, sess.provider",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub(crate) fn usage_by_model(
        &self,
        providers: &[String],
        cutoff_date: Option<&str>,
    ) -> Result<Vec<UsageByModelRow>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, cutoff_date);
        let sql = format!(
            "SELECT COALESCE(NULLIF(s.model, ''), sess.model, ''), \
                    SUM(s.turn_count), \
                    SUM(s.input_tokens), \
                    SUM(s.output_tokens), \
                    SUM(s.cache_read_tokens), \
                    SUM(s.cache_write_tokens), \
                    SUM(s.cost_usd) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY COALESCE(NULLIF(s.model, ''), sess.model, '') \
             ORDER BY SUM(s.input_tokens + s.output_tokens) DESC",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(UsageByModelRow {
                model: row.get(0)?,
                turns: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cache_read_tokens: row.get(4)?,
                cache_write_tokens: row.get(5)?,
                cost_usd: row.get(6)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Per-project cost detail grouped by (project_path, provider, session_id, model)
    /// so callers can deduplicate sessions exactly while still pricing by model.
    pub(crate) fn usage_project_model_detail(
        &self,
        providers: &[String],
        cutoff_date: Option<&str>,
    ) -> Result<Vec<UsageProjectModelDetailRow>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, cutoff_date);
        let sql = format!(
            "SELECT sess.project_path, sess.project_name, sess.provider, s.session_id, \
                    SUM(s.turn_count), \
                    SUM(s.input_tokens), \
                    SUM(s.output_tokens), \
                    SUM(s.cache_read_tokens), \
                    SUM(s.cache_write_tokens), \
                    SUM(s.cost_usd) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY sess.project_path, sess.project_name, sess.provider, s.session_id, \
                      COALESCE(NULLIF(s.model, ''), sess.model, '') \
             ORDER BY SUM(s.input_tokens + s.output_tokens) DESC",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(UsageProjectModelDetailRow {
                project_path: row.get(0)?,
                project_name: row.get(1)?,
                provider: row.get(2)?,
                session_id: row.get(3)?,
                turns: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                cache_read_tokens: row.get(7)?,
                cache_write_tokens: row.get(8)?,
                cost_usd: row.get(9)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Per-session token detail grouped by (session_id, model) for accurate cost calculation.
    pub(crate) fn usage_session_model_detail(
        &self,
        providers: &[String],
        cutoff_date: Option<&str>,
        limit: u32,
    ) -> Result<Vec<UsageSessionModelDetailRow>, rusqlite::Error> {
        let conn = self.lock_read()?;

        // Two-step approach: find the top N session IDs, then fetch per-model detail.
        let (where_clause, params) = build_usage_where(providers, cutoff_date);
        let session_sql = format!(
            "SELECT DISTINCT s.session_id, MAX(sess.updated_at) as max_updated \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
               AND sess.parent_id IS NULL \
             GROUP BY s.session_id \
             ORDER BY max_updated DESC \
             LIMIT ?{}",
            where_clause,
            params.len() + 1
        );
        let mut session_params = params;
        session_params.push(Box::new(limit));
        let session_refs: Vec<&dyn rusqlite::types::ToSql> =
            session_params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&session_sql)?;
        let rows = stmt.query_map(session_refs.as_slice(), |row| row.get::<_, String>(0))?;
        let mut session_ids = Vec::new();
        for row in rows {
            session_ids.push(row?);
        }

        if session_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Now query detail for those sessions
        let id_placeholders: Vec<String> = (0..session_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let detail_sql = format!(
            "SELECT s.session_id, sess.project_path, sess.project_name, sess.provider, sess.updated_at, \
                    COALESCE(NULLIF(s.model, ''), sess.model, ''), \
                    SUM(s.turn_count), \
                    SUM(s.input_tokens), \
                    SUM(s.output_tokens), \
                    SUM(s.cache_read_tokens), \
                    SUM(s.cache_write_tokens), \
                    SUM(s.cost_usd) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id \
             WHERE s.session_id IN ({}) \
             GROUP BY s.session_id, COALESCE(NULLIF(s.model, ''), sess.model, '') \
             ORDER BY sess.updated_at DESC, s.session_id",
            id_placeholders.join(",")
        );
        let detail_params: Vec<Box<dyn rusqlite::types::ToSql>> = session_ids
            .into_iter()
            .map(|id| Box::new(id) as _)
            .collect();
        let detail_refs: Vec<&dyn rusqlite::types::ToSql> =
            detail_params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&detail_sql)?;
        let rows = stmt.query_map(detail_refs.as_slice(), |row| {
            Ok(UsageSessionModelDetailRow {
                session_id: row.get(0)?,
                project_path: row.get(1)?,
                project_name: row.get(2)?,
                provider: row.get(3)?,
                updated_at: row.get(4)?,
                model: row.get(5)?,
                turns: row.get(6)?,
                input_tokens: row.get(7)?,
                output_tokens: row.get(8)?,
                cache_read_tokens: row.get(9)?,
                cache_write_tokens: row.get(10)?,
                cost_usd: row.get(11)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Totals for a specific date range [start, end).
    pub fn usage_totals_range(
        &self,
        providers: &[String],
        date_start: &str,
        date_end: &str,
    ) -> Result<UsageTotalsRow, rusqlite::Error> {
        let conn = self.lock_read()?;
        if providers.is_empty() {
            return Ok((0, 0, 0, 0, 0, 0, 0.0));
        }
        let placeholders: Vec<String> = (0..providers.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            providers.iter().map(|p| Box::new(p.clone()) as _).collect();
        params.push(Box::new(date_start.to_string()));
        params.push(Box::new(date_end.to_string()));
        let sql = format!(
            "SELECT COUNT(DISTINCT s.session_id), \
                    COALESCE(SUM(s.turn_count),0), \
                    COALESCE(SUM(s.input_tokens),0), \
                    COALESCE(SUM(s.output_tokens),0), \
                    COALESCE(SUM(s.cache_read_tokens),0), \
                    COALESCE(SUM(s.cache_write_tokens),0), \
                    COALESCE(SUM(s.cost_usd),0.0) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id \
             WHERE sess.provider IN ({}) AND s.date >= ?{} AND s.date < ?{}",
            placeholders.join(","),
            params.len() - 1,
            params.len()
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, param_refs.as_slice(), |row: &rusqlite::Row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, u64>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, u64>(5)?,
                row.get::<_, f64>(6)?,
            ))
        })
    }

    /// Total cost for a single date (all providers).
    pub fn cost_for_date(&self, date: &str) -> Result<f64, rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row(
            "SELECT COALESCE(SUM(cost_usd), 0.0) FROM session_token_stats WHERE date = ?1",
            [date],
            |row| row.get(0),
        )
    }

    /// Token breakdown for a single date (all providers).
    pub fn tokens_for_date(&self, date: &str) -> Result<(u64, u64, u64, u64), rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), \
                    COALESCE(SUM(output_tokens), 0), \
                    COALESCE(SUM(cache_read_tokens), 0), \
                    COALESCE(SUM(cache_write_tokens), 0) \
             FROM session_token_stats WHERE date = ?1",
            [date],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
    }
}

fn build_usage_where(
    providers: &[String],
    cutoff_date: Option<&str>,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    if providers.is_empty() {
        return (" WHERE 1 = 0".to_string(), Vec::new());
    }

    let mut conditions = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    let placeholders: Vec<String> = (0..providers.len())
        .map(|i| format!("?{}", i + 1))
        .collect();
    conditions.push(format!("sess.provider IN ({})", placeholders.join(",")));
    for p in providers {
        params.push(Box::new(p.clone()));
    }
    if let Some(date) = cutoff_date {
        params.push(Box::new(date.to_string()));
        conditions.push(format!("s.date >= ?{}", params.len()));
    }

    // conditions always has at least the provider IN clause (empty providers early-return above)
    let clause = format!(" WHERE {}", conditions.join(" AND "));
    (clause, params)
}

fn list_sessions_from_query<P>(
    conn: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<SessionMeta>, rusqlite::Error>
where
    P: rusqlite::Params,
{
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, row_to_session_meta)?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row?);
    }
    Ok(sessions)
}

fn search_with_fts(
    conn: &Connection,
    filters: &SearchFilters,
    query: &str,
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    let mut sql = String::from(
        "SELECT s.id, s.provider, s.title, s.project_path, s.project_name,
                s.created_at, s.updated_at, s.message_count, s.file_size_bytes, s.source_path, s.is_sidechain,
                s.variant_name, s.model, s.cc_version, s.git_branch, s.parent_id,
                (SELECT COALESCE(SUM(input_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS input_tokens,
                (SELECT COALESCE(SUM(output_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS output_tokens,
                (SELECT COALESCE(SUM(cache_read_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS cache_read_tokens,
                (SELECT COALESCE(SUM(cache_write_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS cache_write_tokens,
                snippet(sessions_fts, -1, '<mark>', '</mark>', '...', 64) AS snip
         FROM sessions_fts
         JOIN sessions s ON s.rowid = sessions_fts.rowid
         WHERE sessions_fts MATCH ?",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(query.to_string())];
    append_search_filters(&mut sql, &mut param_values, filters);
    sql.push_str(" ORDER BY bm25(sessions_fts, 10.0, 1.0, 5.0) LIMIT 100");
    query_search_results(conn, &sql, &param_values)
}

fn search_with_like(
    conn: &Connection,
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    let raw = filters.query.trim().to_string();
    // Split on whitespace so mixed queries like "auth ui" require both terms
    // to appear somewhere in the row. Without this we would only match rows
    // where the whole raw string appears as one contiguous substring, which
    // silently misses common mixed-token queries.
    let tokens: Vec<String> = raw
        .split_whitespace()
        .map(str::to_string)
        .filter(|token| !token.is_empty())
        .collect();

    let mut sql = String::from(
        "SELECT s.id, s.provider, s.title, s.project_path, s.project_name,
                s.created_at, s.updated_at, s.message_count, s.file_size_bytes, s.source_path, s.is_sidechain,
                s.variant_name, s.model, s.cc_version, s.git_branch, s.parent_id,
                (SELECT COALESCE(SUM(input_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS input_tokens,
                (SELECT COALESCE(SUM(output_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS output_tokens,
                (SELECT COALESCE(SUM(cache_read_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS cache_read_tokens,
                (SELECT COALESCE(SUM(cache_write_tokens), 0) FROM session_token_stats WHERE session_id = s.id) AS cache_write_tokens,
                CASE
                    WHEN ?1 <> '' THEN substr(s.content_text, 1, 200)
                    ELSE ''
                END AS snip,
                s.title AS like_title,
                s.content_text AS like_content_text,
                s.project_name AS like_project_name
         FROM sessions s
         WHERE 1=1",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(raw.clone())];

    for token in &tokens {
        let idx = param_values.len() + 1;
        sql.push_str(&format!(
            " AND (
                s.title LIKE '%' || ?{idx} || '%'
                OR s.content_text LIKE '%' || ?{idx} || '%'
                OR s.project_name LIKE '%' || ?{idx} || '%'
            )"
        ));
        param_values.push(Box::new(token.clone()));
    }

    let next_index = param_values.len() + 1;
    append_search_filters_numbered(&mut sql, &mut param_values, filters, next_index);
    sql.push_str(" ORDER BY s.created_at DESC LIMIT 100");
    query_like_search_results(conn, &sql, &param_values, &tokens)
}

fn append_search_filters(
    sql: &mut String,
    param_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    filters: &SearchFilters,
) {
    if let Some(ref provider) = filters.provider {
        sql.push_str(" AND s.provider = ?");
        param_values.push(Box::new(provider.clone()));
    }
    if let Some(ref project) = filters.project {
        sql.push_str(" AND s.project_name LIKE '%' || ? || '%'");
        param_values.push(Box::new(project.clone()));
    }
    if let Some(after) = filters.after {
        sql.push_str(" AND s.created_at > ?");
        param_values.push(Box::new(after));
    }
    if let Some(before) = filters.before {
        sql.push_str(" AND s.created_at < ?");
        param_values.push(Box::new(before));
    }
}

fn append_search_filters_numbered(
    sql: &mut String,
    param_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    filters: &SearchFilters,
    mut next_index: usize,
) {
    if let Some(ref provider) = filters.provider {
        sql.push_str(&format!(" AND s.provider = ?{next_index}"));
        param_values.push(Box::new(provider.clone()));
        next_index += 1;
    }
    if let Some(ref project) = filters.project {
        sql.push_str(&format!(
            " AND s.project_name LIKE '%' || ?{next_index} || '%'"
        ));
        param_values.push(Box::new(project.clone()));
        next_index += 1;
    }
    if let Some(after) = filters.after {
        sql.push_str(&format!(" AND s.created_at > ?{next_index}"));
        param_values.push(Box::new(after));
        next_index += 1;
    }
    if let Some(before) = filters.before {
        sql.push_str(&format!(" AND s.created_at < ?{next_index}"));
        param_values.push(Box::new(before));
    }
}

fn query_search_results(
    conn: &Connection,
    sql: &str,
    param_values: &[Box<dyn rusqlite::types::ToSql>],
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(SearchResult {
            session: row_to_session_meta(row)?,
            snippet: row.get(20)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

fn query_like_search_results(
    conn: &Connection,
    sql: &str,
    param_values: &[Box<dyn rusqlite::types::ToSql>],
    tokens: &[String],
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        let fallback_snippet: String = row.get(20)?;
        let title: String = row.get(21)?;
        let content_text: String = row.get(22)?;
        let project_name: String = row.get(23)?;
        let snippet = build_like_snippet(&title, &content_text, &project_name, tokens)
            .unwrap_or(fallback_snippet);

        Ok(SearchResult {
            session: row_to_session_meta(row)?,
            snippet,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

fn build_like_snippet(
    title: &str,
    content_text: &str,
    project_name: &str,
    tokens: &[String],
) -> Option<String> {
    if tokens.is_empty() {
        return Some(String::new());
    }

    for source in [title, content_text, project_name] {
        if source.trim().is_empty() {
            continue;
        }
        if let Some(match_start) = find_first_like_match(source, tokens) {
            return Some(snippet_around_match(source, match_start, tokens));
        }
    }

    None
}

fn snippet_around_match(source: &str, match_byte_start: usize, tokens: &[String]) -> String {
    let total_chars = source.chars().count();
    if total_chars <= LIKE_SNIPPET_MAX_CHARS {
        return highlight_like_tokens(source, tokens);
    }

    let match_char_start = source[..match_byte_start].chars().count();
    let mut start_char = match_char_start.saturating_sub(LIKE_SNIPPET_CONTEXT_CHARS);
    let mut end_char = (start_char + LIKE_SNIPPET_MAX_CHARS).min(total_chars);
    if end_char == total_chars {
        start_char = total_chars.saturating_sub(LIKE_SNIPPET_MAX_CHARS);
        end_char = total_chars;
    }

    let start_byte = byte_index_for_char(source, start_char);
    let end_byte = byte_index_for_char(source, end_char);
    let mut snippet = String::new();
    if start_char > 0 {
        snippet.push_str("...");
    }
    snippet.push_str(&source[start_byte..end_byte]);
    if end_char < total_chars {
        snippet.push_str("...");
    }

    highlight_like_tokens(&snippet, tokens)
}

fn byte_index_for_char(source: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }

    source
        .char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(source.len())
}

fn find_first_like_match(source: &str, tokens: &[String]) -> Option<usize> {
    tokens
        .iter()
        .filter(|token| !token.is_empty())
        .filter_map(|token| find_like_match(source, token))
        .min()
}

fn find_like_match(source: &str, token: &str) -> Option<usize> {
    source.find(token).or_else(|| {
        if token.is_ascii() {
            source
                .to_ascii_lowercase()
                .find(&token.to_ascii_lowercase())
        } else {
            None
        }
    })
}

fn highlight_like_tokens(snippet: &str, tokens: &[String]) -> String {
    let mut ranges = Vec::new();
    for token in tokens {
        collect_like_match_ranges(snippet, token, &mut ranges);
    }
    ranges.sort_by(|a, b| {
        let a_len = a.1 - a.0;
        let b_len = b.1 - b.0;
        a.0.cmp(&b.0).then_with(|| b_len.cmp(&a_len))
    });

    let mut selected = Vec::new();
    let mut covered_until = 0;
    for (start, end) in ranges {
        if start >= covered_until {
            selected.push((start, end));
            covered_until = end;
        }
    }

    if selected.is_empty() {
        return snippet.to_string();
    }

    let mut highlighted = String::with_capacity(snippet.len() + selected.len() * 13);
    let mut cursor = 0;
    for (start, end) in selected {
        highlighted.push_str(&snippet[cursor..start]);
        highlighted.push_str("<mark>");
        highlighted.push_str(&snippet[start..end]);
        highlighted.push_str("</mark>");
        cursor = end;
    }
    highlighted.push_str(&snippet[cursor..]);
    highlighted
}

fn collect_like_match_ranges(snippet: &str, token: &str, ranges: &mut Vec<(usize, usize)>) {
    if token.is_empty() {
        return;
    }

    if token.is_ascii() {
        let haystack = snippet.to_ascii_lowercase();
        let needle = token.to_ascii_lowercase();
        let mut offset = 0;
        while let Some(relative_start) = haystack[offset..].find(&needle) {
            let start = offset + relative_start;
            let end = start + token.len();
            ranges.push((start, end));
            offset = end;
        }
        return;
    }

    let mut offset = 0;
    while let Some(relative_start) = snippet[offset..].find(token) {
        let start = offset + relative_start;
        let end = start + token.len();
        ranges.push((start, end));
        offset = end;
    }
}

fn build_fts_query(raw: &str) -> Option<String> {
    // Trigram tokenizer requires each query term to have at least 3 characters
    // (codepoints). If any token is shorter we bail out so the caller falls
    // back to LIKE, which correctly handles short substrings (e.g. 2-char CJK).
    let tokens: Vec<String> = raw
        .split_whitespace()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| token.to_string())
        .collect();

    if tokens.is_empty() {
        return None;
    }
    if tokens.iter().any(|t| t.chars().count() < 3) {
        return None;
    }

    Some(
        tokens
            .iter()
            .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::sync::TokenStatRow;
    use crate::models::Provider;
    use crate::provider::ParsedSession;
    use tempfile::TempDir;

    fn sample_meta(session_id: &str) -> SessionMeta {
        SessionMeta {
            id: session_id.to_string(),
            provider: Provider::Claude,
            title: "Test".into(),
            project_path: "/tmp/project".into(),
            project_name: "project".into(),
            created_at: 1_775_635_200,
            updated_at: 1_775_635_200,
            message_count: 1,
            file_size_bytes: 0,
            source_path: format!("/tmp/{session_id}.jsonl"),
            is_sidechain: false,
            variant_name: None,
            model: Some("claude-opus-4-6".into()),
            cc_version: None,
            git_branch: None,
            parent_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }

    fn parsed_session(meta: SessionMeta, content_text: String) -> ParsedSession {
        ParsedSession {
            meta,
            messages: Vec::new(),
            content_text,
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
        }
    }

    #[test]
    fn get_session_token_totals_prefers_indexed_usage_rows() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let meta = sample_meta("session-usage");
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[parsed_session(meta.clone(), String::new())],
            true,
        )
        .unwrap();

        assert_eq!(db.get_session_token_totals(&meta.id).unwrap(), None);

        db.replace_token_stats(
            &meta.id,
            &[
                TokenStatRow {
                    date: "2026-04-09".into(),
                    model: "claude-opus-4-6".into(),
                    turn_count: 1,
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_tokens: 20,
                    cache_write_tokens: 10,
                    cost_usd: 0.01,
                },
                TokenStatRow {
                    date: "2026-04-10".into(),
                    model: "claude-opus-4-6".into(),
                    turn_count: 1,
                    input_tokens: 7,
                    output_tokens: 3,
                    cache_read_tokens: 2,
                    cache_write_tokens: 1,
                    cost_usd: 0.001,
                },
            ],
        )
        .unwrap();

        assert_eq!(
            db.get_session_token_totals(&meta.id).unwrap(),
            Some(TokenTotals {
                input_tokens: 107,
                output_tokens: 53,
                cache_read_tokens: 22,
                cache_write_tokens: 11,
            })
        );
    }

    #[test]
    fn like_search_centers_and_marks_short_chinese_match() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let prefix = "开头内容".repeat(70);
        let content = format!("{prefix}这里才出现中文搜索命中，后面还有内容。");
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[parsed_session(sample_meta("session-cn"), content)],
            true,
        )
        .unwrap();

        let results = db
            .search_filtered(&SearchFilters {
                query: "中文".into(),
                ..SearchFilters::default()
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.starts_with("..."));
        assert!(results[0].snippet.contains("<mark>中文</mark>"));
    }

    #[test]
    fn like_search_marks_short_chinese_title_match() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let mut meta = sample_meta("session-title-cn");
        meta.title = "中文搜索标题".into();
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[parsed_session(meta, "正文没有目标词".into())],
            true,
        )
        .unwrap();

        let results = db
            .search_filtered(&SearchFilters {
                query: "中文".into(),
                ..SearchFilters::default()
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].snippet, "<mark>中文</mark>搜索标题");
    }

    #[test]
    fn like_search_keeps_empty_snippet_for_filter_only_results() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[parsed_session(
                sample_meta("session-filter"),
                "中文正文".into(),
            )],
            true,
        )
        .unwrap();

        let results = db
            .search_filtered(&SearchFilters {
                provider: Some(Provider::Claude.key().to_string()),
                ..SearchFilters::default()
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.is_empty());
    }
}
