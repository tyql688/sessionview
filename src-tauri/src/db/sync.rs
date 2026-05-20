use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection};

use crate::models::{Message, MessageRole, Provider, SessionMeta};
use crate::provider::ParsedSession;
use crate::provider_utils::{truncate_to_bytes, FTS_CONTENT_LIMIT};

use super::Database;

/// Rebuild the FTS content text from typed messages, keeping only the
/// dialogue roles (user + assistant). This excludes tool calls, tool output,
/// and system/thinking messages so the global search matches what the user
/// actually said or what the model actually replied. Falls back to the
/// provider-supplied content_text when messages carry no real content
/// (e.g. OpenCode emits Assistant stubs only for token accounting).
fn dialogue_content_text(messages: &[Message], fallback: &str) -> String {
    let parts: Vec<&str> = messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant))
        .map(|m| m.content.as_str())
        .filter(|c| !c.trim().is_empty())
        .collect();

    if parts.is_empty() {
        return fallback.to_string();
    }

    truncate_to_bytes(&parts.join("\n"), FTS_CONTENT_LIMIT)
}

pub use crate::provider::TokenStatRow;

impl Database {
    pub fn sync_provider_snapshot(
        &self,
        provider: &Provider,
        sessions: &[ParsedSession],
        aggressive: bool,
        preserve_source_paths: &[String],
    ) -> Result<(), rusqlite::Error> {
        let provider_key = provider.key().to_string();
        let mut ids_by_source: HashMap<String, HashSet<String>> = HashMap::new();
        let mut source_paths = Vec::new();
        let mut seen_sources = HashSet::new();

        for parsed in sessions {
            let source_path = parsed.meta.source_path.clone();
            ids_by_source
                .entry(source_path.clone())
                .or_default()
                .insert(parsed.meta.id.clone());

            if seen_sources.insert(source_path.clone()) {
                source_paths.push(source_path);
            }
        }

        // Treat unchanged source files as still-alive when computing the
        // delete heuristic — otherwise an incremental scan that returned 0
        // changed sessions but covers 800 unchanged paths would look like
        // a near-empty result and trip the destructive-sync guard.
        for path in preserve_source_paths {
            if seen_sources.insert(path.clone()) {
                source_paths.push(path.clone());
            }
        }

        let current_count = self.count_sessions_for_provider(&provider_key)?;
        let scan_count = sessions.len() as u64;
        let alive_count = scan_count + preserve_source_paths.len() as u64;
        let should_delete = if aggressive {
            if alive_count == 0 {
                log::info!(
                    "provider {:?} aggressive reindex: scan returned 0 sessions, clearing stale entries",
                    provider
                );
            }
            true
        } else if alive_count == 0 {
            log::warn!(
                "provider {:?} scan returned 0 sessions, skipping deletion to protect index",
                provider
            );
            false
        } else {
            current_count <= 10 || (alive_count as f64 / current_count as f64) > 0.5
        };

        if !should_delete {
            log::warn!(
                "provider {:?} scan returned {} sessions ({} unchanged) but DB has {}, skipping destructive sync",
                provider, scan_count, preserve_source_paths.len(), current_count
            );
        }

        self.with_transaction(|conn| {
            for parsed in sessions {
                let content = dialogue_content_text(&parsed.messages, &parsed.content_text);
                upsert_session_on(
                    conn,
                    &parsed.meta,
                    &content,
                    &parsed.child_session_ids,
                    parsed.source_mtime,
                )?;
            }

            if should_delete {
                for (source_path, ids) in &ids_by_source {
                    delete_missing_sessions_for_source(conn, &provider_key, source_path, ids)?;
                }

                delete_missing_sources_for_provider(conn, &provider_key, &source_paths)?;
                conn.execute(
                    "DELETE FROM favorites WHERE session_id NOT IN (SELECT id FROM sessions)",
                    [],
                )?;
            }
            Ok(())
        })
    }

    pub fn sync_source_snapshot(
        &self,
        provider: &Provider,
        source_path: &str,
        sessions: &[ParsedSession],
    ) -> Result<(), rusqlite::Error> {
        let provider_key = provider.key().to_string();
        let ids: HashSet<String> = sessions
            .iter()
            .map(|parsed| parsed.meta.id.clone())
            .collect();

        let current_count = self.count_sessions_for_source(&provider_key, source_path)?;
        let scan_count = sessions.len() as u64;
        // For single-source sync, scan_count==0 is a valid signal (file deleted).
        // Only apply ratio guard when both sides are non-zero.
        let should_delete = scan_count == 0
            || current_count <= 10
            || (scan_count as f64 / current_count as f64) > 0.5;

        if !should_delete {
            log::warn!(
                "provider {:?} source {:?} scan returned {} sessions but DB has {}, skipping destructive sync",
                provider, source_path, scan_count, current_count
            );
        }

        self.with_transaction(|conn| {
            for parsed in sessions {
                let content = dialogue_content_text(&parsed.messages, &parsed.content_text);
                upsert_session_on(
                    conn,
                    &parsed.meta,
                    &content,
                    &parsed.child_session_ids,
                    parsed.source_mtime,
                )?;
            }

            if should_delete {
                delete_missing_sessions_for_source(conn, &provider_key, source_path, &ids)?;
                conn.execute(
                    "DELETE FROM favorites WHERE session_id NOT IN (SELECT id FROM sessions)",
                    [],
                )?;
            }
            Ok(())
        })
    }

    pub fn rename_session(&self, id: &str, new_title: &str) -> Result<(), rusqlite::Error> {
        let title = if new_title.chars().count() > 200 {
            new_title
                .chars()
                .take(200)
                .collect::<String>()
                .trim_end()
                .to_string()
        } else {
            new_title.to_string()
        };
        let conn = self.lock_write()?;
        conn.execute(
            "UPDATE sessions SET title = ?1, title_custom = 1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    pub fn clear_all(&self) -> Result<(), rusqlite::Error> {
        // Delete all data and rebuild FTS index.
        // Note: VACUUM is impossible while two connections are open,
        // so free pages remain in the file but get reused by subsequent writes.
        self.with_transaction(|conn| {
            conn.execute_batch(
                "DELETE FROM session_token_stats;
                 DELETE FROM favorites;
                 DELETE FROM sessions;
                 DELETE FROM meta;
                 INSERT INTO sessions_fts(sessions_fts) VALUES('rebuild');",
            )
        })
    }

    pub fn clear_usage_stats(&self) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        conn.execute("DELETE FROM session_token_stats", [])?;
        // Keep the denormalized totals consistent with the now-empty stats.
        conn.execute(
            "UPDATE sessions SET
                input_tokens = 0,
                output_tokens = 0,
                cache_read_tokens = 0,
                cache_write_tokens = 0",
            [],
        )?;
        Ok(())
    }

    /// Delete this session and all its children from DB.
    pub fn delete_session(&self, id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        conn.execute("DELETE FROM favorites WHERE session_id IN (SELECT id FROM sessions WHERE parent_id = ?1)", params![id])?;
        conn.execute("DELETE FROM sessions WHERE parent_id = ?1", params![id])?;
        conn.execute("DELETE FROM favorites WHERE session_id = ?1", params![id])?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Replace all token stats for a session. Called during indexing.
    /// Deletes existing rows first, then inserts new per-(date, model) aggregates.
    /// Also refreshes the denormalized totals on `sessions` so list/search
    /// queries can avoid correlated `SELECT SUM(...)` subqueries.
    pub fn replace_token_stats(
        &self,
        session_id: &str,
        stats: &[TokenStatRow],
    ) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        conn.execute(
            "DELETE FROM session_token_stats WHERE session_id = ?1",
            params![session_id],
        )?;
        let mut stmt = conn.prepare_cached(
            "INSERT INTO session_token_stats
                (session_id, date, model, turn_count, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        let mut totals = SessionTotals::default();
        for row in stats {
            stmt.execute(params![
                session_id,
                row.date,
                row.model,
                row.turn_count as i64,
                row.input_tokens as i64,
                row.output_tokens as i64,
                row.cache_read_tokens as i64,
                row.cache_write_tokens as i64,
                row.cost_usd,
            ])?;
            totals.add_row(row);
        }
        update_session_totals(&conn, session_id, &totals)?;
        Ok(())
    }

    /// Replace token stats for multiple sessions atomically within a single
    /// transaction.  The frontend reads via a separate connection, so without
    /// a transaction the reader can observe a partially-updated state (e.g.
    /// after a DELETE but before the matching INSERTs), causing usage numbers
    /// to "jump" on every poll cycle. The denormalized per-session totals on
    /// `sessions` are updated in the same transaction so search/list queries
    /// never see a stale aggregate.
    pub fn replace_token_stats_batch(
        &self,
        batch: &[(&str, &[TokenStatRow])],
    ) -> Result<(), rusqlite::Error> {
        self.with_transaction(|conn| {
            let mut insert = conn.prepare_cached(
                "INSERT INTO session_token_stats
                    (session_id, date, model, turn_count, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for &(session_id, stats) in batch {
                conn.execute(
                    "DELETE FROM session_token_stats WHERE session_id = ?1",
                    params![session_id],
                )?;
                let mut totals = SessionTotals::default();
                for row in stats {
                    insert.execute(params![
                        session_id,
                        row.date,
                        row.model,
                        row.turn_count as i64,
                        row.input_tokens as i64,
                        row.output_tokens as i64,
                        row.cache_read_tokens as i64,
                        row.cache_write_tokens as i64,
                        row.cost_usd,
                    ])?;
                    totals.add_row(row);
                }
                update_session_totals(conn, session_id, &totals)?;
            }
            Ok(())
        })
    }
}

#[derive(Default)]
struct SessionTotals {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
}

impl SessionTotals {
    fn add_row(&mut self, row: &TokenStatRow) {
        self.input_tokens = self.input_tokens.saturating_add(row.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(row.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(row.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(row.cache_write_tokens);
    }
}

fn update_session_totals(
    conn: &Connection,
    session_id: &str,
    totals: &SessionTotals,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE sessions SET
            input_tokens = ?1,
            output_tokens = ?2,
            cache_read_tokens = ?3,
            cache_write_tokens = ?4
         WHERE id = ?5",
        params![
            totals.input_tokens as i64,
            totals.output_tokens as i64,
            totals.cache_read_tokens as i64,
            totals.cache_write_tokens as i64,
            session_id,
        ],
    )?;
    Ok(())
}

fn upsert_session_on(
    conn: &Connection,
    meta: &SessionMeta,
    content_text: &str,
    child_session_ids: &[String],
    source_mtime: i64,
) -> Result<(), rusqlite::Error> {
    let provider_str = meta.provider.key();

    conn.execute(
        "INSERT INTO sessions (id, provider, title, project_path, project_name,
            created_at, updated_at, message_count, file_size_bytes, source_path, content_text, is_sidechain,
            variant_name, model, cc_version, git_branch, parent_id, source_mtime)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT(id) DO UPDATE SET
            provider = excluded.provider,
            title = CASE WHEN sessions.title_custom = 1 THEN sessions.title ELSE excluded.title END,
            project_path = CASE WHEN excluded.project_path != '' THEN excluded.project_path ELSE sessions.project_path END,
            project_name = CASE WHEN excluded.project_name != 'Unknown Project' AND excluded.project_name != '' THEN excluded.project_name ELSE sessions.project_name END,
            created_at = excluded.created_at,
            updated_at = excluded.updated_at,
            message_count = excluded.message_count,
            file_size_bytes = excluded.file_size_bytes,
            source_path = excluded.source_path,
            content_text = excluded.content_text,
            is_sidechain = CASE WHEN excluded.is_sidechain = 1 OR sessions.is_sidechain = 1 THEN 1 ELSE 0 END,
            variant_name = excluded.variant_name,
            model = excluded.model,
            cc_version = excluded.cc_version,
            git_branch = excluded.git_branch,
            parent_id = COALESCE(excluded.parent_id, sessions.parent_id),
            source_mtime = excluded.source_mtime",
        params![
            meta.id,
            provider_str,
            meta.title,
            meta.project_path,
            meta.project_name,
            meta.created_at,
            meta.updated_at,
            meta.message_count,
            meta.file_size_bytes,
            meta.source_path,
            content_text,
            meta.is_sidechain as i64,
            meta.variant_name,
            meta.model,
            meta.cc_version,
            meta.git_branch,
            meta.parent_id,
            source_mtime,
        ],
    )?;

    // Back-fill parent_id / is_sidechain / project metadata on already-indexed
    // child rows. `child_session_ids` is populated by providers that surface
    // structured parent→child links in the transcript itself (today only
    // Antigravity via its `INVOKE_SUBAGENT` step type). For other providers
    // the list is empty and this loop is a no-op.
    for child_id in child_session_ids {
        if child_id == &meta.id {
            continue;
        }
        conn.execute(
            "UPDATE sessions
             SET parent_id = ?1,
                 is_sidechain = 1,
                 project_path = CASE WHEN project_path = '' OR project_path IS NULL THEN ?2 ELSE project_path END,
                 project_name = CASE WHEN project_name = 'Unknown Project' OR project_name IS NULL THEN ?3 ELSE project_name END
             WHERE id = ?4 AND (parent_id IS NULL OR parent_id = '')",
            params![meta.id, meta.project_path, meta.project_name, child_id],
        )?;
    }

    Ok(())
}

fn delete_missing_sessions_for_source(
    conn: &Connection,
    provider_key: &str,
    source_path: &str,
    ids: &HashSet<String>,
) -> Result<(), rusqlite::Error> {
    let mut sql = String::from("DELETE FROM sessions WHERE provider = ?1 AND source_path = ?2");
    let mut params_refs: Vec<&dyn rusqlite::types::ToSql> = vec![&provider_key, &source_path];
    let mut ids_vec: Vec<&String> = ids.iter().collect();
    ids_vec.sort();

    if !ids_vec.is_empty() {
        sql.push_str(" AND id NOT IN (");
        sql.push_str(&repeat_vars(ids_vec.len()));
        sql.push(')');
        for id in &ids_vec {
            params_refs.push(*id);
        }
    }

    conn.execute(&sql, params_refs.as_slice())?;
    Ok(())
}

fn delete_missing_sources_for_provider(
    conn: &Connection,
    provider_key: &str,
    source_paths: &[String],
) -> Result<(), rusqlite::Error> {
    if source_paths.is_empty() {
        conn.execute(
            "DELETE FROM sessions WHERE provider = ?1",
            params![provider_key],
        )?;
        return Ok(());
    }

    let mut sql = String::from("DELETE FROM sessions WHERE provider = ?1 AND source_path NOT IN (");
    sql.push_str(&repeat_vars(source_paths.len()));
    sql.push(')');

    let mut params_refs: Vec<&dyn rusqlite::types::ToSql> = vec![&provider_key];
    for source_path in source_paths {
        params_refs.push(source_path);
    }

    conn.execute(&sql, params_refs.as_slice())?;
    Ok(())
}

fn repeat_vars(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::{Database, SessionMeta, TokenStatRow};
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
            source_path: "/tmp/source.jsonl".into(),
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

    #[test]
    fn replace_token_stats_clears_existing_rows_when_empty() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let meta = sample_meta("session-1");
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[ParsedSession {
                meta: meta.clone(),
                messages: Vec::new(),
                content_text: String::new(),
                parse_warning_count: 0,
                child_session_ids: Vec::new(),
                usage_events: Vec::new(),
                source_mtime: 0,
            }],
            true,
            &[],
        )
        .unwrap();

        db.replace_token_stats(
            &meta.id,
            &[TokenStatRow {
                date: "2026-04-09".into(),
                model: "claude-opus-4-6".into(),
                turn_count: 1,
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.01,
            }],
        )
        .unwrap();

        db.replace_token_stats(&meta.id, &[]).unwrap();

        let conn = db.lock_read().unwrap();
        let count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_token_stats WHERE session_id = ?1",
                [meta.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn clear_usage_stats_preserves_sessions() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let meta = sample_meta("session-2");
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[ParsedSession {
                meta: meta.clone(),
                messages: Vec::new(),
                content_text: String::new(),
                parse_warning_count: 0,
                child_session_ids: Vec::new(),
                usage_events: Vec::new(),
                source_mtime: 0,
            }],
            true,
            &[],
        )
        .unwrap();
        db.replace_token_stats(
            &meta.id,
            &[TokenStatRow {
                date: "2026-04-10".into(),
                model: "claude-opus-4-6".into(),
                turn_count: 1,
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.001,
            }],
        )
        .unwrap();

        db.clear_usage_stats().unwrap();

        assert!(db.get_session(&meta.id).unwrap().is_some());
        let conn = db.lock_read().unwrap();
        let usage_rows: u64 = conn
            .query_row("SELECT COUNT(*) FROM session_token_stats", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(usage_rows, 0);
    }

    #[test]
    fn child_session_counts_returns_counts_for_requested_parents() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let parent_a = sample_meta("parent-a");
        let parent_b = sample_meta("parent-b");
        let mut child_a1 = sample_meta("child-a-1");
        child_a1.parent_id = Some(parent_a.id.clone());
        child_a1.is_sidechain = true;
        let mut child_a2 = sample_meta("child-a-2");
        child_a2.parent_id = Some(parent_a.id.clone());
        child_a2.is_sidechain = true;
        let mut child_b1 = sample_meta("child-b-1");
        child_b1.parent_id = Some(parent_b.id.clone());
        child_b1.is_sidechain = true;

        let parsed = [
            parent_a.clone(),
            parent_b.clone(),
            child_a1,
            child_a2,
            child_b1,
        ]
        .into_iter()
        .map(|meta| ParsedSession {
            meta,
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 0,
        })
        .collect::<Vec<_>>();
        db.sync_provider_snapshot(&Provider::Claude, &parsed, true, &[])
            .unwrap();

        let counts = db
            .child_session_counts(&[parent_a.id.clone(), parent_b.id.clone()])
            .unwrap();
        assert_eq!(counts.get(&parent_a.id), Some(&2));
        assert_eq!(counts.get(&parent_b.id), Some(&1));
    }

    #[test]
    fn parent_backfills_child_when_parser_declares_child_ids() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();

        let child_id = "22222222-2222-4222-a222-222222222222";
        let parent_id = "11111111-1111-4111-a111-111111111111";

        // 1. Child indexed first with no parent / Unknown project (mirrors the
        //    watcher firing on the child transcript before the parent's).
        let mut child_meta = sample_meta(child_id);
        child_meta.project_path = String::new();
        child_meta.project_name = "Unknown Project".into();
        child_meta.parent_id = None;
        child_meta.is_sidechain = false;

        db.sync_provider_snapshot(
            &Provider::Antigravity,
            &[ParsedSession {
                meta: child_meta.clone(),
                messages: Vec::new(),
                content_text: String::new(),
                parse_warning_count: 0,
                child_session_ids: Vec::new(),
                usage_events: Vec::new(),
                source_mtime: 0,
            }],
            true,
            &[],
        )
        .unwrap();

        let loaded_child = db.get_session(child_id).unwrap().unwrap();
        assert_eq!(loaded_child.parent_id, None);
        assert!(!loaded_child.is_sidechain);
        assert_eq!(loaded_child.project_name, "Unknown Project");

        // 2. Parent indexed with explicit child id list (the antigravity parser
        //    populates this from INVOKE_SUBAGENT step content). The child row
        //    must be back-filled with parent_id + inherited project metadata.
        let mut parent_meta = sample_meta(parent_id);
        parent_meta.project_path = "/tmp/ccsession".into();
        parent_meta.project_name = "ccsession".into();

        db.sync_provider_snapshot(
            &Provider::Antigravity,
            &[ParsedSession {
                meta: parent_meta.clone(),
                messages: Vec::new(),
                content_text: String::new(),
                parse_warning_count: 0,
                child_session_ids: vec![child_id.to_string()],
                usage_events: Vec::new(),
                source_mtime: 0,
            }],
            true,
            &[],
        )
        .unwrap();

        let loaded_child_after = db.get_session(child_id).unwrap().unwrap();
        assert_eq!(loaded_child_after.parent_id, Some(parent_id.to_string()));
        assert!(loaded_child_after.is_sidechain);
        assert_eq!(loaded_child_after.project_path, "/tmp/ccsession");
        assert_eq!(loaded_child_after.project_name, "ccsession");

        // 3. A later incremental sync of the child (no parent info) must not
        //    clobber parent_id / is_sidechain / project metadata.
        db.sync_provider_snapshot(
            &Provider::Antigravity,
            &[ParsedSession {
                meta: child_meta.clone(),
                messages: Vec::new(),
                content_text: "Child updated content".into(),
                parse_warning_count: 0,
                child_session_ids: Vec::new(),
                usage_events: Vec::new(),
                source_mtime: 0,
            }],
            true,
            &[],
        )
        .unwrap();

        let loaded_child_final = db.get_session(child_id).unwrap().unwrap();
        assert_eq!(loaded_child_final.parent_id, Some(parent_id.to_string()));
        assert!(loaded_child_final.is_sidechain);
        assert_eq!(loaded_child_final.project_path, "/tmp/ccsession");
        assert_eq!(loaded_child_final.project_name, "ccsession");
    }

    #[test]
    fn upsert_does_not_relink_when_child_already_has_parent() {
        // Regression guard: random UUIDs that happen to match an existing
        // session id must NOT steal it away from its real parent.
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();

        let child_id = "22222222-2222-4222-a222-222222222222";
        let true_parent = "11111111-1111-4111-a111-111111111111";
        let other = "44444444-4444-4444-a444-444444444444";

        // Real parent claims the child.
        let mut child_meta = sample_meta(child_id);
        child_meta.parent_id = Some(true_parent.into());
        child_meta.is_sidechain = true;
        let mut true_parent_meta = sample_meta(true_parent);
        true_parent_meta.project_path = "/tmp/real".into();
        true_parent_meta.project_name = "real".into();

        db.sync_provider_snapshot(
            &Provider::Antigravity,
            &[
                ParsedSession {
                    meta: child_meta,
                    messages: Vec::new(),
                    content_text: String::new(),
                    parse_warning_count: 0,
                    child_session_ids: Vec::new(),
                    usage_events: Vec::new(),
                    source_mtime: 0,
                },
                ParsedSession {
                    meta: true_parent_meta,
                    messages: Vec::new(),
                    content_text: String::new(),
                    parse_warning_count: 0,
                    child_session_ids: vec![child_id.into()],
                    usage_events: Vec::new(),
                    source_mtime: 0,
                },
            ],
            true,
            &[],
        )
        .unwrap();

        // An unrelated session that happens to mention the child id (e.g. a
        // long-running session whose transcript copy-pasted that uuid) must
        // not be allowed to override the existing parent.
        let other_meta = sample_meta(other);
        db.sync_provider_snapshot(
            &Provider::Antigravity,
            &[ParsedSession {
                meta: other_meta,
                messages: Vec::new(),
                content_text: String::new(),
                parse_warning_count: 0,
                child_session_ids: vec![child_id.into()],
                usage_events: Vec::new(),
                source_mtime: 0,
            }],
            true,
            &[],
        )
        .unwrap();

        let loaded = db.get_session(child_id).unwrap().unwrap();
        assert_eq!(loaded.parent_id, Some(true_parent.to_string()));
    }
}
