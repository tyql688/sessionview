use std::collections::HashMap;

use rusqlite::{params, params_from_iter};

use crate::models::{SearchFilters, SearchResult, SessionMeta, TokenTotals};

use super::row_mapper::row_to_session_meta;
use super::Database;

mod search;
mod usage;

use search::{build_fts_query, list_sessions_from_query, search_with_fts, search_with_like};

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
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens
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
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens
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

    /// Pre-fetch the `(source_path, size, mtime)` snapshot the indexer
    /// uses to decide which files can skip parsing. Returned `mtime` is
    /// the stored epoch seconds; rows that pre-date the migration have
    /// `mtime = 0`, which forces a reparse the first time they're seen.
    /// Rows with empty `source_path` (synthetic / detached sessions)
    /// are excluded — they wouldn't survive a file-level check anyway.
    pub fn source_states_for_provider(
        &self,
        provider_key: &str,
    ) -> Result<std::collections::HashMap<String, crate::provider::SourceState>, rusqlite::Error>
    {
        let conn = self.lock_read()?;
        let mut stmt = conn.prepare(
            "SELECT source_path, file_size_bytes, source_mtime
               FROM sessions
              WHERE provider = ?1 AND source_path != ''",
        )?;
        let rows = stmt.query_map(params![provider_key], |row| {
            let path: String = row.get(0)?;
            let size: i64 = row.get(1)?;
            let mtime: i64 = row.get(2)?;
            Ok((
                path,
                crate::provider::SourceState {
                    size: size.max(0) as u64,
                    mtime,
                },
            ))
        })?;
        let mut out = std::collections::HashMap::new();
        for row in rows {
            let (path, state) = row?;
            // Multiple session IDs can share one source_path (e.g. parent +
            // subagent rows backed by the same JSONL). Any of them sufficing
            // to mark the file "alive" — keep the first/latest state seen.
            out.entry(path).or_insert(state);
        }
        Ok(out)
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
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens
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
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens
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
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens
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
            source_mtime: 0,
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
            &[],
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
            &[],
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
            &[],
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
            &[],
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
