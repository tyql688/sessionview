use std::collections::HashMap;

use rusqlite::params;

use crate::models::{SearchFilters, SearchResult, SessionMeta, TokenTotals};

use super::super::row_mapper::row_to_session_meta;
use super::search::{build_fts_query, list_sessions_from_query, search_with_fts, search_with_like};
use super::Database;

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
            match search_with_fts(&conn, filters, &query) {
                Ok(results) => Ok(results),
                Err(err) => {
                    // Don't silently swallow the FTS failure (no-silent-fallback
                    // rule): record why we degraded to the slower LIKE scan.
                    log::warn!(
                        "FTS search failed for query {:?}, falling back to LIKE: {err}",
                        filters.query
                    );
                    search_with_like(&conn, filters)
                }
            }
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
    fn search_filtered_caps_fts_results_at_100() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        // The 100-row hard cap in db/queries/search.rs is a deliberate design
        // choice — search and the Cmd+K panel intentionally do NOT paginate.
        // 105 sessions all match the same FTS term, so the result must come
        // back truncated to exactly 100.
        let sessions: Vec<ParsedSession> = (0..105)
            .map(|i| {
                parsed_session(
                    sample_meta(&format!("session-cap-{i:03}")),
                    "searchterm content body".into(),
                )
            })
            .collect();
        db.sync_provider_snapshot(&Provider::Claude, &sessions, true, &[])
            .unwrap();

        let results = db
            .search_filtered(&SearchFilters {
                query: "searchterm".into(),
                ..SearchFilters::default()
            })
            .unwrap();

        assert_eq!(
            results.len(),
            100,
            "FTS search must cap at 100 results (no pagination by design)"
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

    #[test]
    fn like_search_ranks_title_match_above_newer_content_match() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();

        let mut title_hit = sample_meta("session-title-rank");
        title_hit.title = "中文标题命中".into();
        let mut content_hit = sample_meta("session-content-rank");
        content_hit.title = "无关标题".into();

        // Both match the 2-char CJK query "中文" via the LIKE fallback. Under the
        // old pure-recency ORDER BY the content-only hit could outrank the title
        // hit; relevance ordering must surface the title match first.
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[
                parsed_session(title_hit, "正文没有目标词".into()),
                parsed_session(content_hit, "正文里有中文这个词".into()),
            ],
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

        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].session.id, "session-title-rank",
            "title match must rank above a content-only match"
        );
    }

    #[test]
    fn search_matches_whole_query_as_literal_phrase_not_and_tokens() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[
                // Contiguous phrase — must match.
                parsed_session(sample_meta("session-phrase"), "前缀 347 测试 后缀".into()),
                // Both words present but NOT adjacent — must NOT match, because
                // search is a literal substring, not an AND of the two words.
                parsed_session(
                    sample_meta("session-scattered"),
                    "这里有 347 然后一堆别的内容 接着 测试 出现".into(),
                ),
            ],
            true,
            &[],
        )
        .unwrap();

        let results = db
            .search_filtered(&SearchFilters {
                query: "347 测试".into(),
                ..SearchFilters::default()
            })
            .unwrap();
        let ids: Vec<&str> = results.iter().map(|r| r.session.id.as_str()).collect();

        assert!(
            ids.contains(&"session-phrase"),
            "the literal contiguous phrase must match"
        );
        assert!(
            !ids.contains(&"session-scattered"),
            "scattered words must NOT match — search is literal, not AND"
        );
    }
}
