use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::services::tail_reader::open_tail_reader;

use serde::Deserialize;
use serde_json::Value;

use crate::models::{Message, Provider, SessionMeta};
use crate::provider::{ParsedSession, UsageEvent};
use crate::provider_utils::{
    is_system_content, parse_rfc3339_timestamp, project_name_from_path, session_title, NO_PROJECT,
};

use super::tools::*;
use super::CodexProvider;

mod event_msg;
mod response_item;
mod usage;
mod value_helpers;

use usage::CodexRawUsageCounts;
use value_helpers::push_system_event;

#[derive(Deserialize)]
pub(super) struct CodexLine {
    pub(super) timestamp: Option<String>,
    #[serde(rename = "type")]
    line_type: String,
    payload: Option<Value>,
}

pub(super) struct PendingCodexUserMessage {
    pub(super) content: String,
    pub(super) timestamp: Option<String>,
    pub(super) image_segments: Vec<String>,
}

/// Per-scan accumulator shared between the full-file and tail-only
/// Codex parsers. Holds the cross-line state the dispatch loop walks
/// (parsed messages, call_id → message-index pairing, "first
/// occurrence" trackers for cwd/model/version, and the fork-context
/// skip flag used by subagent files) so the loop body can run against
/// either a full file or a seeked tail reader without duplication.
pub(super) struct CodexScanAccum {
    pub(super) messages: Vec<Message>,
    pub(super) usage_events: Vec<UsageEvent>,
    pub(super) first_user_message: Option<String>,
    first_timestamp: Option<String>,
    last_timestamp: Option<String>,
    pub(super) content_parts: Vec<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    /// call_id → message-index pairing for merging function_call_output
    /// into the matching function_call message.
    pub(super) call_id_map: crate::provider_utils::ToolCallPairer,
    model: Option<String>,
    model_provider: Option<String>,
    pub(super) thread_name: Option<String>,
    pub(super) current_model: Option<String>,
    pub(super) previous_token_totals: Option<CodexRawUsageCounts>,
    /// Codex re-emits some token_count events verbatim. Events identical in
    /// (timestamp, model, input, cached, output, reasoning, total) are counted
    /// once; this set tracks the ones already recorded.
    pub(super) seen_token_events:
        std::collections::HashSet<(String, String, u64, u64, u64, u64, u64)>,
    cc_version: Option<String>,
    git_branch: Option<String>,
    is_sidechain: bool,
    parent_id: Option<String>,
    agent_nickname: Option<String>,
    pub(super) pending_user_message: Option<PendingCodexUserMessage>,
    /// True while we're inside a subagent file's pre-fork parent context
    /// and must drop those lines before they leak into the subagent's
    /// own view of the conversation.
    skipping_fork_context: bool,
    subagent_start_seconds: Option<i64>,
    parse_warning_count: u32,
}

impl CodexScanAccum {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            usage_events: Vec::new(),
            first_user_message: None,
            first_timestamp: None,
            last_timestamp: None,
            content_parts: Vec::new(),
            session_id: None,
            cwd: None,
            call_id_map: crate::provider_utils::ToolCallPairer::default(),
            model: None,
            model_provider: None,
            thread_name: None,
            current_model: None,
            previous_token_totals: None,
            seen_token_events: std::collections::HashSet::new(),
            cc_version: None,
            git_branch: None,
            is_sidechain: false,
            parent_id: None,
            agent_nickname: None,
            pending_user_message: None,
            skipping_fork_context: false,
            subagent_start_seconds: None,
            parse_warning_count: 0,
        }
    }
    /// Run the per-line dispatch over `reader`, mutating `self` with
    /// the messages / tool-call pairings / first-occurrence trackers it
    /// observes. Called by both `parse_session_file` (full-file) and
    /// `parse_session_tail` (mmap-seeked) — they share the same loop body.
    fn scan_lines<R: BufRead>(&mut self, reader: R, path: &Path) {
        let stats =
            crate::provider_utils::for_each_jsonl_record(reader, path, |_, entry: CodexLine| {
                self.scan_line(&entry, path);
                ControlFlow::Continue(())
            });
        self.parse_warning_count = self
            .parse_warning_count
            .saturating_add(stats.parse_error_count);
    }

    /// Per-record body of `scan_lines`, lifted so the shared JSONL iteration
    /// helper (`provider_utils::for_each_jsonl_record`) owns the read/parse/
    /// skip loop. Every original `continue` became a `return` — it was the
    /// last action taken for the line, so returning advances identically.
    fn scan_line(&mut self, entry: &CodexLine, path: &Path) {
        if let Some(ref ts) = entry.timestamp {
            if self.first_timestamp.is_none() {
                self.first_timestamp = Some(ts.clone());
            }
            self.last_timestamp = Some(ts.clone());
        }

        let payload = match entry.payload {
            Some(ref p) => p,
            None => return,
        };

        // Skip forked parent context in subagent files. Clear the flag on
        // the first subagent-owned `task_started` event (its `started_at`
        // matches the subagent session's creation time). Older transcripts
        // don't carry that marker — fall back to the textual
        // `newly spawned agent` cue still present in their function-call
        // output.
        if self.skipping_fork_context {
            if entry.line_type == "event_msg"
                && payload.get("type").and_then(|v| v.as_str()) == Some("task_started")
            {
                if let (Some(started_at), Some(sub_sec)) = (
                    payload.get("started_at").and_then(|v| v.as_i64()),
                    self.subagent_start_seconds,
                ) {
                    if started_at >= sub_sec {
                        self.skipping_fork_context = false;
                        return;
                    }
                }
            } else if entry.line_type == "response_item"
                && payload.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            {
                let output = payload.get("output").and_then(|v| v.as_str()).unwrap_or("");
                if output.contains("newly spawned agent") {
                    self.skipping_fork_context = false;
                }
            }
            return;
        }

        match entry.line_type.as_str() {
            "session_meta" => self.handle_session_meta(entry, payload),
            "compacted" => self.handle_compacted(entry, payload),
            "response_item" => self.handle_response_item(entry, payload, path),
            "turn_context" => self.handle_turn_context(payload),
            "event_msg" => self.handle_event_msg(entry, payload, path),
            _ => {}
        }
    }

    /// Handle a `session_meta` line. The original arm's single early
    /// `continue` (2nd `session_meta` = forked parent context, skip the
    /// rest of the body) becomes a `return`; that is the last action
    /// `scan_lines` takes for the line, so returning advances the loop
    /// exactly as `continue` did.
    fn handle_session_meta(&mut self, entry: &CodexLine, payload: &Value) {
        // Only process the first session_meta; subagent JSONL files
        // contain a second session_meta for the parent context which
        // would overwrite the subagent's own id/self.cwd/source fields.
        if self.session_id.is_some() {
            // 2nd session_meta = start of forked parent context
            if self.is_sidechain {
                self.skipping_fork_context = true;
            }
            return;
        }
        if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
            self.session_id = Some(id.to_string());
        }
        if let Some(c) = payload.get("cwd").and_then(|v| v.as_str()) {
            self.cwd = Some(c.to_string());
        }
        if let Some(v) = payload.get("cli_version").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                self.cc_version = Some(v.to_string());
            }
        }
        if let Some(m) = payload.get("model_provider").and_then(|v| v.as_str()) {
            if !m.is_empty() {
                self.model_provider = Some(m.to_string());
            }
        }
        if let Some(b) = payload
            .get("git")
            .and_then(|g| g.get("branch"))
            .and_then(|v| v.as_str())
        {
            if !b.is_empty() && b != "HEAD" {
                self.git_branch = Some(b.to_string());
            }
        }
        // Detect subagent sessions: source.subagent.thread_spawn
        if let Some(spawn) = payload
            .get("source")
            .and_then(|s| s.get("subagent"))
            .and_then(|a| a.get("thread_spawn"))
        {
            self.is_sidechain = true;
            self.parent_id = spawn
                .get("parent_thread_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            self.agent_nickname = payload
                .get("agent_nickname")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let sub_ts = parse_rfc3339_timestamp(
                payload
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .or(entry.timestamp.as_deref()),
            );
            if sub_ts > 0 {
                self.subagent_start_seconds = Some(sub_ts);
            }
        } else if self.parent_id.is_none() {
            // Regular forks (source: "vscode" etc.) also carry
            // a `forked_from_id` we can use as the parent. We
            // intentionally leave is_sidechain=false so the
            // forked session shows in the main list, just with
            // provenance back to its origin.
            if let Some(id) = payload
                .get("forked_from_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                self.parent_id = Some(id.to_string());
            }
        }
    }

    /// Handle a top-level `compacted` line. Carries the post-compaction
    /// handoff summary in `payload.message`; surfaced as a System event
    /// so the user can see WHAT survived the compaction, not just that
    /// one happened. No control flow beyond a single push.
    fn handle_compacted(&mut self, entry: &CodexLine, payload: &Value) {
        let message = payload
            .get("message")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let content = match message {
            Some(text) => format!("[context_compacted]\n{text}"),
            None => "[context_compacted]".to_string(),
        };
        push_system_event(&mut self.messages, entry.timestamp.clone(), content);
    }

    /// Handle a `turn_context` line: flush any pending user message and
    /// capture the active model name. No control flow beyond the flush
    /// and field updates.
    fn handle_turn_context(&mut self, payload: &Value) {
        flush_pending_user_message(
            &mut self.pending_user_message,
            &mut self.messages,
            &mut self.content_parts,
            &mut self.first_user_message,
        );
        // Extract actual self.model name (e.g. "gpt-5.4") from turn_context
        if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
            if !m.is_empty() {
                self.current_model = Some(m.to_string());
                if self.model.is_none() {
                    self.model = Some(m.to_string());
                }
            }
        }
    }
}

impl CodexProvider {
    pub fn parse_session_file(&self, path: &PathBuf) -> Option<ParsedSession> {
        self.parse_session_file_with_index(path, &self.load_session_index())
    }

    /// Full-file parse with a pre-loaded `session_index.jsonl` title map
    /// (session id → thread name), so batch scans read the index once
    /// instead of per file.
    pub(crate) fn parse_session_file_with_index(
        &self,
        path: &PathBuf,
        index_titles: &std::collections::HashMap<String, String>,
    ) -> Option<ParsedSession> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) => {
                log::warn!("failed to open Codex session '{}': {error}", path.display());
                return None;
            }
        };
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                log::warn!(
                    "failed to read Codex session metadata '{}': {error}",
                    path.display()
                );
                return None;
            }
        };
        let file_size = metadata.len();

        let reader = BufReader::new(file);
        // Two Codex subagent JSONL layouts the parser has to handle.
        // `skipping_fork_context` drops the parent's forked history so
        // it doesn't leak into the subagent view:
        //   legacy: [sub_meta, parent_meta, ...parent_context...,
        //            function_call_output("newly spawned agent"), sub_turn]
        //   newer:  [sub_meta, parent_meta, ...sanitized_parent_history...,
        //            task_started(sub_turn), turn_context, sub_turn]
        //     The newer layout no longer carries the "newly spawned"
        //     textual marker; the fork boundary is the first
        //     `event_msg.task_started` whose `started_at` is at or
        //     after the subagent's own `session_meta.timestamp`.
        let mut accum = CodexScanAccum::new();

        accum.scan_lines(reader, path);

        // Hoist accumulator fields back to locals so the existing post-loop
        // finalization (title, project_path, content_text, meta assembly)
        // reads exactly like the pre-refactor code did.
        let CodexScanAccum {
            mut messages,
            usage_events,
            mut first_user_message,
            first_timestamp,
            last_timestamp,
            mut content_parts,
            session_id,
            cwd,
            call_id_map: _,
            model,
            model_provider,
            thread_name,
            current_model: _,
            previous_token_totals: _,
            seen_token_events: _,
            cc_version,
            git_branch,
            is_sidechain,
            parent_id,
            agent_nickname,
            mut pending_user_message,
            skipping_fork_context,
            subagent_start_seconds: _,
            parse_warning_count,
        } = accum;

        flush_pending_user_message(
            &mut pending_user_message,
            &mut messages,
            &mut content_parts,
            &mut first_user_message,
        );

        if skipping_fork_context && is_sidechain {
            log::warn!(
                "Codex subagent '{}' fork-context boundary never resolved (missing task_started.started_at or subagent timestamp); yielded 0 messages",
                path.display()
            );
        }

        if messages.is_empty() {
            return None;
        }

        // Session ID: from session_meta payload.id, fallback to filename parsing
        let session_id = session_id.unwrap_or_else(|| {
            path.file_stem().map_or_else(
                || "unknown".to_string(),
                |s| s.to_string_lossy().to_string(),
            )
        });

        // Title priority: the sidecar `~/.codex/session_index.jsonl` entry
        // for this session id (Codex rewrites it on rename, so it is the
        // freshest source), then the inline `thread_name_updated` event
        // (only a minority of rollouts carry one), then the subagent
        // nickname, then the first user message fallback. Index-first also
        // keeps `scan_incremental`'s stored-title-vs-index comparison
        // convergent: one re-parse after a rename, not one per scan.
        let title = index_titles
            .get(&session_id)
            .cloned()
            .or(thread_name)
            .or(agent_nickname.as_deref().map(|n| n.to_string()))
            .unwrap_or_else(|| session_title(first_user_message.as_deref()));

        let project_path = cwd.unwrap_or_else(|| NO_PROJECT.to_string());

        let project_name = project_name_from_path(&project_path);

        let created_at = parse_rfc3339_timestamp(first_timestamp.as_deref());

        let updated_at = parse_rfc3339_timestamp(last_timestamp.as_deref());

        let content_text = content_parts.join("\n");

        let meta = SessionMeta {
            id: session_id,
            provider: Provider::Codex,
            title,
            project_path,
            project_name,
            created_at,
            updated_at,
            message_count: messages.len() as u32,
            file_size_bytes: file_size,
            source_path: path.to_string_lossy().to_string(),
            is_sidechain,
            variant_name: None,
            model: model.or(model_provider),
            cc_version,
            git_branch,
            parent_id,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };

        Some(ParsedSession {
            meta,
            messages,
            content_text,
            parse_warning_count,
            child_session_ids: Vec::new(),
            usage_events,
            source_mtime: source_mtime_epoch_seconds(&metadata),
        })
    }
}

fn source_mtime_epoch_seconds(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(crate::provider::system_time_to_epoch_seconds)
        .unwrap_or(0)
}

/// Tail-only Codex parse result. Carries the most recent N messages
/// plus the warning count from the tail region so the caller can
/// assemble a `SessionMessagesWindow` without paying for a full-file
/// parse. The metadata bits (title / cwd / model) live on the DB-loaded
/// `SessionMeta` and are not re-derived here.
pub struct CodexTailResult {
    pub messages: Vec<Message>,
    pub parse_warning_count: u32,
    pub last_timestamp: Option<String>,
}

/// Parse only the tail of a Codex session file — the last
/// `target_messages` (or so) emitted messages — by mmap'ing the file
/// and seeking the BufReader past the byte offset of the first line
/// we want. Shares the per-line dispatch with `parse_session_file`
/// through `CodexScanAccum::scan_lines`.
///
/// Same caveats as the Claude tail entry point:
/// - Tool merging across lines is best-effort. A `function_call_output`
///   whose matching `function_call` was earlier in the file surfaces
///   as a standalone tool message; the background full-parse promote
///   replaces the cache once it completes.
/// - The Codex fork-context skip (used for subagent files whose JSONL
///   starts with the parent's history) is a no-op here because the
///   tail naturally starts past that region — `skipping_fork_context`
///   stays at its default `false` and the loop dispatches normally.
/// - No token-total computation: the caller pulls totals from the DB.
pub(crate) fn parse_session_tail(path: &Path, target_messages: usize) -> Option<CodexTailResult> {
    // Codex JSONL lines are noticeably bigger than Claude's (each turn
    // is ~10-20 KB of `response_item.message` content + tool calls),
    // and an event_msg.token_count plus its enclosing turn_context can
    // span ~50 raw lines between consecutive emitted messages. Pad the
    // tail window more generously than Claude's so we don't miss a
    // recent message whose surrounding metadata lines pushed the
    // actual message-emit further into the file than expected.
    let safety_buffer = target_messages / 2 + 100;
    let scan_lines = target_messages.saturating_add(safety_buffer);
    let (reader, _window) = open_tail_reader(path, scan_lines, "Codex")?;

    let mut accum = CodexScanAccum::new();
    accum.scan_lines(reader, path);

    flush_pending_user_message(
        &mut accum.pending_user_message,
        &mut accum.messages,
        &mut accum.content_parts,
        &mut accum.first_user_message,
    );

    if accum.messages.is_empty() {
        log::debug!(
            "Codex tail parse produced no messages for '{}'; falling back to full parse",
            path.display()
        );
        return None;
    }

    // Trim to exactly `target_messages` — we deliberately over-scan so
    // tool merging at the boundary works, but the caller asked for a
    // specific window size.
    let len = accum.messages.len();
    if len > target_messages {
        accum.messages.drain(0..(len - target_messages));
    }

    Some(CodexTailResult {
        messages: accum.messages,
        parse_warning_count: accum.parse_warning_count,
        last_timestamp: accum.last_timestamp,
    })
}

pub(super) fn append_user_message(
    messages: &mut Vec<Message>,
    content_parts: &mut Vec<String>,
    first_user_message: &mut Option<String>,
    content: String,
    timestamp: Option<String>,
) {
    let content = omit_base64_image_sources(&content);
    if content.is_empty() {
        return;
    }

    let normalized_text = strip_inline_image_sources(&content);
    let trimmed = normalized_text.trim_start();
    if is_system_content(trimmed) {
        return;
    }

    if first_user_message.is_none() {
        *first_user_message = Some(normalized_text.clone());
    }

    if !normalized_text.is_empty() {
        content_parts.push(normalized_text);
    }

    messages.push(Message {
        timestamp,
        ..Message::user(content)
    });
}

pub(super) fn flush_pending_user_message(
    pending_user_message: &mut Option<PendingCodexUserMessage>,
    messages: &mut Vec<Message>,
    content_parts: &mut Vec<String>,
    first_user_message: &mut Option<String>,
) {
    let Some(pending) = pending_user_message.take() else {
        return;
    };

    append_user_message(
        messages,
        content_parts,
        first_user_message,
        pending.content,
        pending.timestamp,
    );
}

#[cfg(test)]
mod tests;
