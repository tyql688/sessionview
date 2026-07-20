use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params};

use crate::models::{Message, MessageRole, Provider, SessionMeta};
use crate::provider::ParsedSession;

use super::Database;

/// Prefix marking thinking content stored as `MessageRole::System`.
const THINKING_PREFIX: &str = "[thinking]";
/// Max chars of each thinking block indexed for search.
const THINKING_INDEX_CHARS: usize = 1000;
/// Max chars of a tool's input summary / result content indexed for search.
const TOOL_INDEX_CHARS: usize = 300;

/// Truncate to at most `max_chars` characters on a char boundary (never
/// slices inside a multi-byte character).
fn truncate_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((byte_index, _)) => &text[..byte_index],
        None => text,
    }
}

fn append_indexable_part(content: &mut String, part: &str) {
    if part.is_empty() {
        return;
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(part);
}

/// Build the FTS content text from typed messages: full user + assistant
/// dialogue, plus truncated excerpts of thinking blocks (`[thinking]`-prefixed
/// System messages) and tool calls (tool name + compact input summary + result
/// snippet), so global search also reaches tool activity and reasoning.
/// Falls back to the provider-supplied content_text when messages carry no
/// indexable content (e.g. OpenCode emits Assistant stubs only for token
/// accounting).
fn indexable_content_text(messages: &[Message], fallback: &str) -> String {
    let mut content = String::new();
    for message in messages {
        match message.role {
            MessageRole::User | MessageRole::Assistant => {
                if !message.content.trim().is_empty() {
                    append_indexable_part(&mut content, &message.content);
                }
            }
            MessageRole::System => {
                let Some(thinking) = message.content.strip_prefix(THINKING_PREFIX) else {
                    continue;
                };
                let thinking = thinking.trim_start();
                if !thinking.is_empty() {
                    append_indexable_part(
                        &mut content,
                        truncate_chars(thinking, THINKING_INDEX_CHARS),
                    );
                }
            }
            MessageRole::Tool => {
                if let Some(name) = message.tool_name.as_deref() {
                    append_indexable_part(&mut content, name);
                }
                if let Some(input) = message.tool_input.as_deref()
                    && !input.trim().is_empty()
                {
                    append_indexable_part(&mut content, truncate_chars(input, TOOL_INDEX_CHARS));
                }
                let tool_output = message.content.trim();
                if !tool_output.is_empty() {
                    append_indexable_part(
                        &mut content,
                        truncate_chars(tool_output, TOOL_INDEX_CHARS),
                    );
                }
            }
        }
    }

    if content.is_empty() {
        return fallback.to_string();
    }

    content
}

pub use crate::provider::TokenStatRow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolStatRow {
    pub key: String,
    pub label: String,
    pub category: String,
    pub count: u64,
}

pub(crate) fn build_tool_stats(messages: &[Message]) -> Vec<ToolStatRow> {
    let mut counters: HashMap<String, ToolStatRow> = HashMap::new();
    for message in messages {
        if message.role != MessageRole::Tool {
            continue;
        }
        let Some((key, label, category)) = tool_identity(message) else {
            continue;
        };
        let entry = counters.entry(key.clone()).or_insert(ToolStatRow {
            key,
            label,
            category,
            count: 0,
        });
        entry.count = entry.count.saturating_add(1);
    }

    let mut rows: Vec<ToolStatRow> = counters.into_values().collect();
    rows.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.label.cmp(&right.label))
    });
    rows
}

fn tool_identity(message: &Message) -> Option<(String, String, String)> {
    if let Some(metadata) = message.tool_metadata.as_ref() {
        let key = if metadata.canonical_name.trim().is_empty() {
            metadata.raw_name.trim()
        } else {
            metadata.canonical_name.trim()
        };
        if key.is_empty() {
            return None;
        }
        let label = if metadata.display_name.trim().is_empty() {
            key.to_string()
        } else {
            metadata.display_name.clone()
        };
        return Some((key.to_string(), label, metadata.category.clone()));
    }

    let name = message.tool_name.as_deref()?.trim();
    if name.is_empty() {
        return None;
    }
    Some((name.to_string(), name.to_string(), "tool".to_string()))
}

struct ProviderSnapshotSources {
    ids_by_source: HashMap<String, HashSet<String>>,
    source_paths: Vec<String>,
    seen_sources: HashSet<String>,
}

impl ProviderSnapshotSources {
    fn from_sessions(sessions: &[ParsedSession]) -> Self {
        let mut sources = Self {
            ids_by_source: HashMap::new(),
            source_paths: Vec::new(),
            seen_sources: HashSet::new(),
        };

        for parsed in sessions {
            let source_path = parsed.meta.source_path.clone();
            sources
                .ids_by_source
                .entry(source_path.clone())
                .or_default()
                .insert(parsed.meta.id.clone());
            sources.push_source_path(source_path);
        }

        sources
    }

    fn preserve_paths(&mut self, paths: &[String]) {
        for path in paths {
            self.push_source_path(path.clone());
        }
    }

    fn push_source_path(&mut self, source_path: String) {
        if self.seen_sources.insert(source_path.clone()) {
            self.source_paths.push(source_path);
        }
    }
}

fn should_delete_provider_snapshot(
    provider: &Provider,
    aggressive: bool,
    current_count: u64,
    scan_count: u64,
    preserved_source_count: u64,
) -> bool {
    let alive_count = scan_count + preserved_source_count;
    if aggressive {
        if alive_count == 0 {
            log::info!(
                "provider {provider:?} aggressive reindex: scan returned 0 sessions, clearing stale entries"
            );
        }
        return true;
    }

    if alive_count == 0 {
        log::warn!(
            "provider {provider:?} scan returned 0 sessions, skipping deletion to protect index"
        );
        return false;
    }

    current_count <= 10 || (alive_count as f64 / current_count as f64) > 0.5
}

impl Database {
    pub fn sync_provider_snapshot(
        &self,
        provider: &Provider,
        sessions: &[ParsedSession],
        aggressive: bool,
        preserve_source_paths: &[String],
    ) -> Result<(), rusqlite::Error> {
        self.sync_provider_snapshot_with_token_stats(
            provider,
            sessions,
            aggressive,
            preserve_source_paths,
            &[],
        )
    }

    pub(crate) fn sync_provider_snapshot_with_token_stats(
        &self,
        provider: &Provider,
        sessions: &[ParsedSession],
        aggressive: bool,
        preserve_source_paths: &[String],
        token_stats: &[(&str, &[TokenStatRow])],
    ) -> Result<(), rusqlite::Error> {
        let provider_key = provider.key().to_string();
        let mut snapshot_sources = ProviderSnapshotSources::from_sessions(sessions);

        // Treat unchanged source files as still-alive when computing the
        // delete heuristic — otherwise an incremental scan that returned 0
        // changed sessions but covers 800 unchanged paths would look like
        // a near-empty result and trip the destructive-sync guard.
        snapshot_sources.preserve_paths(preserve_source_paths);

        let current_count = self.count_sessions_for_provider(&provider_key)?;
        let scan_count = sessions.len() as u64;
        let should_delete = should_delete_provider_snapshot(
            provider,
            aggressive,
            current_count,
            scan_count,
            preserve_source_paths.len() as u64,
        );

        if !should_delete {
            log::warn!(
                "provider {provider:?} scan returned {scan_count} sessions ({} unchanged) but DB has {current_count}, skipping destructive sync",
                preserve_source_paths.len()
            );
        }

        self.with_transaction(|conn| {
            upsert_parsed_sessions_on(conn, sessions)?;
            for &(session_id, stats) in token_stats {
                replace_token_stats_on(conn, session_id, stats)?;
            }

            if should_delete {
                for (source_path, ids) in &snapshot_sources.ids_by_source {
                    delete_missing_sessions_for_source(conn, &provider_key, source_path, ids)?;
                }

                delete_vanished_sources_for_provider(
                    conn,
                    &provider_key,
                    &snapshot_sources.source_paths,
                )?;
                conn.execute(
                    "DELETE FROM favorites WHERE session_id NOT IN (SELECT id FROM sessions)",
                    [],
                )?;
            }
            Ok(())
        })
    }

    pub(crate) fn rename_session(&self, id: &str, new_title: &str) -> Result<(), rusqlite::Error> {
        let conn = self.lock_write()?;
        conn.execute(
            "UPDATE sessions SET title = ?1, title_custom = 1 WHERE id = ?2",
            params![new_title, id],
        )?;
        Ok(())
    }

    pub(crate) fn clear_all(&self) -> Result<(), rusqlite::Error> {
        // Delete all data and rebuild FTS index.
        // Note: VACUUM is impossible while two connections are open,
        // so free pages remain in the file but get reused by subsequent writes.
        self.with_transaction(|conn| {
            conn.execute_batch(
                "DELETE FROM session_token_stats;
                 DELETE FROM session_tool_stats;
                 DELETE FROM session_tool_index;
                 DELETE FROM favorites;
                 DELETE FROM sessions;
                 DELETE FROM meta;
                 INSERT INTO sessions_fts(sessions_fts) VALUES('rebuild');",
            )
        })
    }

    pub(crate) fn clear_usage_stats(&self) -> Result<(), rusqlite::Error> {
        self.with_transaction(|conn| {
            conn.execute_batch(
                "DELETE FROM session_token_stats;
                 DELETE FROM session_tool_stats;
                 DELETE FROM session_tool_index;
                 UPDATE sessions SET
                    input_tokens = 0,
                    output_tokens = 0,
                    cache_read_tokens = 0,
                    cache_write_tokens = 0,
                    source_mtime = 0;",
            )
        })
    }

    /// Replace all token stats for a session. Called during indexing.
    /// Deletes existing rows first, then inserts new per-(bucket, model) aggregates.
    /// Also refreshes the denormalized totals on `sessions` so list/search
    /// queries can avoid correlated `SELECT SUM(...)` subqueries.
    pub fn replace_token_stats(
        &self,
        session_id: &str,
        stats: &[TokenStatRow],
    ) -> Result<(), rusqlite::Error> {
        self.with_transaction(|conn| replace_token_stats_on(conn, session_id, stats))
    }

    pub(crate) fn replace_tool_stats(
        &self,
        session_id: &str,
        stats: &[ToolStatRow],
    ) -> Result<(), rusqlite::Error> {
        self.with_transaction(|conn| replace_tool_stats_on(conn, session_id, stats))
    }
}

fn replace_token_stats_on(
    conn: &Connection,
    session_id: &str,
    stats: &[TokenStatRow],
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM session_token_stats WHERE session_id = ?1",
        params![session_id],
    )?;
    let mut insert = conn.prepare_cached(
        "INSERT INTO session_token_stats
            (session_id, bucket, model, turn_count, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;
    let mut totals = SessionTotals::default();
    for row in stats {
        insert.execute(params![
            session_id,
            row.bucket,
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
    update_session_totals(conn, session_id, &totals)
}

fn replace_tool_stats_on(
    conn: &Connection,
    session_id: &str,
    stats: &[ToolStatRow],
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM session_tool_stats WHERE session_id = ?1",
        params![session_id],
    )?;
    conn.execute(
        "INSERT INTO session_tool_index (session_id) VALUES (?1)
         ON CONFLICT(session_id) DO NOTHING",
        params![session_id],
    )?;
    let mut insert = conn.prepare_cached(
        "INSERT INTO session_tool_stats
            (session_id, tool_key, label, category, count)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for row in stats {
        insert.execute(params![
            session_id,
            row.key,
            row.label,
            row.category,
            row.count as i64,
        ])?;
    }
    Ok(())
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

fn upsert_parsed_sessions_on(
    conn: &Connection,
    sessions: &[ParsedSession],
) -> Result<(), rusqlite::Error> {
    let mut slim = 0usize;
    for parsed in sessions {
        let content = indexable_content_text(&parsed.messages, &parsed.content_text);
        let outcome = upsert_session_on(
            conn,
            &parsed.meta,
            &content,
            &parsed.child_session_ids,
            parsed.source_mtime,
        )?;
        let tool_stats = build_tool_stats(&parsed.messages);
        replace_tool_stats_on(conn, &parsed.meta.id, &tool_stats)?;
        if outcome == UpsertOutcome::MetadataOnly {
            slim += 1;
        }
    }
    if slim > 0 {
        log::debug!(
            "upsert batch: {slim}/{} sessions had unchanged searchable content (FTS skipped)",
            sessions.len()
        );
    }
    Ok(())
}

/// How `upsert_session_on` wrote the row. `MetadataOnly` means the searchable
/// columns were untouched, so the (expensive) FTS trigram update never fired.
#[derive(Debug, PartialEq)]
enum UpsertOutcome {
    IndexedContent,
    MetadataOnly,
}

/// Upsert one session row. When the row already exists and its effective
/// searchable columns (title / content_text / project_name, after the
/// keep-old-value rules below) are unchanged, only the metadata columns are
/// updated — the SET list omits the FTS-indexed columns so the `UPDATE OF`
/// trigger does not rewrite ~content-size trigram postings per session.
/// Measured on a 2.3k-session library: this is roughly half the cost of a
/// full usage refresh.
fn upsert_session_on(
    conn: &Connection,
    meta: &SessionMeta,
    content_text: &str,
    child_session_ids: &[String],
    source_mtime: i64,
) -> Result<UpsertOutcome, rusqlite::Error> {
    let provider_str = meta.provider.key();

    let existing: Option<(String, String, String, i64)> = conn
        .prepare_cached(
            "SELECT title, content_text, project_name, title_custom FROM sessions WHERE id = ?1",
        )?
        .query_row(params![meta.id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;

    let searchable_unchanged = existing.as_ref().is_some_and(
        |(old_title, old_content, old_project_name, title_custom)| {
            // Mirror the CASE rules of the full upsert below: a custom title
            // always wins, and placeholder project names keep the old value.
            let effective_title = if *title_custom == 1 {
                old_title.as_str()
            } else {
                meta.title.as_str()
            };
            let effective_project_name =
                if !meta.project_name.is_empty() && meta.project_name != "Unknown Project" {
                    meta.project_name.as_str()
                } else {
                    old_project_name.as_str()
                };
            effective_title == old_title
                && effective_project_name == old_project_name
                && content_text == old_content
        },
    );

    if searchable_unchanged {
        conn.execute(
            "UPDATE sessions SET
                provider = ?2,
                project_path = CASE WHEN ?3 != '' THEN ?3 ELSE project_path END,
                created_at = ?4,
                updated_at = ?5,
                message_count = ?6,
                file_size_bytes = ?7,
                source_path = ?8,
                is_sidechain = CASE
                    WHEN ?2 = 'pi' THEN ?9
                    WHEN ?9 = 1 OR is_sidechain = 1 THEN 1
                    ELSE 0
                END,
                variant_name = ?10,
                model = ?11,
                cc_version = ?12,
                git_branch = ?13,
                parent_id = CASE
                    WHEN ?2 = 'pi' THEN ?14
                    ELSE COALESCE(?14, parent_id)
                END,
                source_mtime = ?15
             WHERE id = ?1",
            params![
                meta.id,
                provider_str,
                meta.project_path,
                meta.created_at,
                meta.updated_at,
                meta.message_count,
                meta.file_size_bytes,
                meta.source_path,
                meta.is_sidechain as i64,
                meta.variant_name,
                meta.model,
                meta.cc_version,
                meta.git_branch,
                meta.parent_id,
                source_mtime,
            ],
        )?;
        backfill_children_on(conn, meta, child_session_ids)?;
        return Ok(UpsertOutcome::MetadataOnly);
    }

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
            is_sidechain = CASE
                WHEN excluded.provider = 'pi' THEN excluded.is_sidechain
                WHEN excluded.is_sidechain = 1 OR sessions.is_sidechain = 1 THEN 1
                ELSE 0
            END,
            variant_name = excluded.variant_name,
            model = excluded.model,
            cc_version = excluded.cc_version,
            git_branch = excluded.git_branch,
            parent_id = CASE
                WHEN excluded.provider = 'pi' THEN excluded.parent_id
                ELSE COALESCE(excluded.parent_id, sessions.parent_id)
            END,
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

    backfill_children_on(conn, meta, child_session_ids)?;

    Ok(UpsertOutcome::IndexedContent)
}

/// Back-fill parent_id / is_sidechain / project metadata on already-indexed
/// child rows. `child_session_ids` is populated by providers that surface
/// structured parent→child links in the transcript itself (today only
/// Antigravity via its `INVOKE_SUBAGENT` step type). For other providers
/// the list is empty and this loop is a no-op.
fn backfill_children_on(
    conn: &Connection,
    meta: &SessionMeta,
    child_session_ids: &[String],
) -> Result<(), rusqlite::Error> {
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

/// Remove sessions whose source file is gone. A source missing from the scan
/// output is only a HINT — scans silently skip files that fail to open/stat
/// (transient I/O pressure, permissions), and treating those as deleted once
/// wiped a whole library. Deletion therefore requires positive proof: the
/// path must stat as `NotFound`. Any other outcome (exists, EMFILE, EPERM…)
/// preserves the rows and logs a warning.
fn delete_vanished_sources_for_provider(
    conn: &Connection,
    provider_key: &str,
    scanned_source_paths: &[String],
) -> Result<(), rusqlite::Error> {
    let mut sql = String::from("SELECT DISTINCT source_path FROM sessions WHERE provider = ?1");
    let mut params_refs: Vec<&dyn rusqlite::types::ToSql> = vec![&provider_key];
    if !scanned_source_paths.is_empty() {
        sql.push_str(" AND source_path NOT IN (");
        sql.push_str(&repeat_vars(scanned_source_paths.len()));
        sql.push(')');
        for source_path in scanned_source_paths {
            params_refs.push(source_path);
        }
    }

    let mut stmt = conn.prepare(&sql)?;
    let candidates: Vec<String> = stmt
        .query_map(params_refs.as_slice(), |row| row.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;

    for source_path in candidates {
        match std::fs::symlink_metadata(&source_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                conn.execute(
                    "DELETE FROM sessions WHERE provider = ?1 AND source_path = ?2",
                    params![provider_key, source_path],
                )?;
            }
            Err(error) => {
                log::warn!(
                    "source {source_path} missing from {provider_key} scan but unverifiable ({error}); keeping its sessions"
                );
            }
            Ok(_) => {
                log::warn!(
                    "source {source_path} missing from {provider_key} scan but still on disk; keeping its sessions"
                );
            }
        }
    }
    Ok(())
}

fn repeat_vars(count: usize) -> String {
    let mut sql = String::with_capacity(count.saturating_mul(3));
    for i in 0..count {
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push('?');
    }
    sql
}

#[cfg(test)]
mod tests;
