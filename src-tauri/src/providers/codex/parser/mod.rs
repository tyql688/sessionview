use std::fs::{self, File};
use std::io::{BufRead, Read};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use crate::services::tail_reader::open_tail_reader;

use memchr::memrchr;
use memmap2::Mmap;
use serde::Deserialize;
use serde_json::Value;

mod completed_item;
#[cfg(test)]
mod completed_item_tests;

use crate::models::{Message, MessageRole, Provider, SessionMeta};
use crate::provider::util::{
    NO_PROJECT, is_system_content, parse_rfc3339_timestamp, project_name_from_path, session_title,
};
use crate::provider::{ParsedSession, UsageEvent};
use crate::tool_metadata::{ToolCallFacts, build_tool_metadata};

use super::CodexProvider;
use super::tools::*;

mod dispatch;
mod event_msg;
mod metadata;
mod response_item;
mod usage;
mod value_helpers;

use usage::{CodexRawUsageCounts, CodexUsageFingerprint};
use value_helpers::push_system_event;

#[derive(Deserialize)]
pub(super) struct CodexLine {
    ordinal: Option<u64>,
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
    /// Metadata identities of validated physical history segments.
    history_segment_headers: std::collections::HashSet<(String, u64)>,
    owned_session_id: Option<String>,
    segment_owns_usage: bool,
    usage_start_ordinal: Option<u64>,
    transcript_start_ordinal: Option<u64>,
    target_is_subagent: bool,
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
    pub(super) call_id_map: crate::provider::util::ToolCallPairer,
    model: Option<String>,
    model_provider: Option<String>,
    pub(super) thread_name: Option<String>,
    pub(super) current_model: Option<String>,
    /// Every distinct model named by this file (turn_context or resolved
    /// token_count). Lets `scan_lines` backfill usage events that arrived
    /// before the file's first turn_context when the answer is unambiguous.
    pub(super) models_seen: std::collections::BTreeSet<String>,
    /// token_count events with real totals but no resolvable model yet.
    pub(super) pending_unresolved_usage: Vec<(
        String,
        Option<String>,
        CodexRawUsageCounts,
        Option<CodexRawUsageCounts>,
    )>,
    /// Copied parent usage forms a dense burst in legacy fork files. Skip
    /// that burst while priming the cumulative baseline. Typed history and
    /// task boundaries take precedence over this legacy timing check.
    pub(super) replay_usage_skip: bool,
    /// Timestamp of the latest inherited usage in the legacy replay burst.
    pub(super) replay_last_timestamp_ms: Option<i64>,
    pub(super) previous_token_totals: Option<CodexRawUsageCounts>,
    /// Codex re-emits some token_count events verbatim. Events identical in
    /// timestamp, model, per-response usage, and cumulative snapshot are counted
    /// once. Advancing totals distinguish equal-sized responses at the same time.
    pub(super) seen_token_events: std::collections::HashSet<(
        String,
        String,
        CodexRawUsageCounts,
        Option<CodexRawUsageCounts>,
    )>,
    seen_completed_items: std::collections::HashSet<String>,
    seen_assistant_items: std::collections::HashSet<String>,
    /// Active turn identity from `turn_context`, used to pair the new
    /// token_usage_record channel with its legacy token_count duplicate.
    pub(super) current_turn_id: Option<String>,
    pub(super) unmatched_token_count_usage:
        std::collections::HashMap<CodexUsageFingerprint, Vec<usize>>,
    pub(super) unmatched_token_usage_records:
        std::collections::HashMap<CodexUsageFingerprint, Vec<usize>>,
    pub(super) seen_token_usage_record_ids:
        std::collections::HashMap<String, CodexUsageFingerprint>,
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
    unmatched_tool_event_count: u32,
    pub(super) unresolved_usage_event_count: u32,
    parse_warning_count: u32,
}

impl CodexScanAccum {
    fn set_history_header(&mut self, header: &super::history::Header) {
        self.owned_session_id = Some(header.id.clone());
        self.usage_start_ordinal = header
            .usage_start_ordinal
            .or(header.transcript_start_ordinal);
        self.transcript_start_ordinal = header.transcript_start_ordinal;
        self.target_is_subagent = header.is_subagent;
    }

    fn new() -> Self {
        Self {
            history_segment_headers: std::collections::HashSet::new(),
            owned_session_id: None,
            segment_owns_usage: true,
            usage_start_ordinal: None,
            transcript_start_ordinal: None,
            target_is_subagent: false,
            messages: Vec::new(),
            usage_events: Vec::new(),
            first_user_message: None,
            first_timestamp: None,
            last_timestamp: None,
            content_parts: Vec::new(),
            session_id: None,
            cwd: None,
            call_id_map: crate::provider::util::ToolCallPairer::default(),
            model: None,
            model_provider: None,
            thread_name: None,
            current_model: None,
            models_seen: std::collections::BTreeSet::new(),
            pending_unresolved_usage: Vec::new(),
            replay_usage_skip: false,
            replay_last_timestamp_ms: None,
            previous_token_totals: None,
            seen_token_events: std::collections::HashSet::new(),
            seen_completed_items: std::collections::HashSet::new(),
            seen_assistant_items: std::collections::HashSet::new(),
            current_turn_id: None,
            unmatched_token_count_usage: std::collections::HashMap::new(),
            unmatched_token_usage_records: std::collections::HashMap::new(),
            seen_token_usage_record_ids: std::collections::HashMap::new(),
            cc_version: None,
            git_branch: None,
            is_sidechain: false,
            parent_id: None,
            agent_nickname: None,
            pending_user_message: None,
            skipping_fork_context: false,
            subagent_start_seconds: None,
            unmatched_tool_event_count: 0,
            unresolved_usage_event_count: 0,
            parse_warning_count: 0,
        }
    }

    /// Materialize a tool call whose only on-disk record is a lifecycle
    /// event (no response_item pair): build the tool message and register
    /// its call_id so the caller's enrichment path finds it.
    pub(super) fn push_event_only_tool_call(
        &mut self,
        raw_name: &str,
        call_id: &str,
        input: Option<serde_json::Value>,
        timestamp: Option<String>,
    ) {
        let metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Codex,
            raw_name,
            input: input.as_ref(),
            call_id: Some(call_id),
            assistant_id: None,
        });
        let idx = self.messages.len();
        self.call_id_map.register(Some(call_id), idx);
        self.messages.push(Message {
            timestamp,
            tool_name: Some(metadata.canonical_name.clone()),
            tool_input: input.map(|value| value.to_string()),
            tool_metadata: Some(metadata),
            ..Message::new(MessageRole::Tool, String::new())
        });
    }

    pub(super) fn record_unmatched_tool_event(
        &mut self,
        kind: &'static str,
        call_id: &str,
        path: &Path,
    ) {
        if self.unmatched_tool_event_count == 0 {
            log::debug!(
                "first unmatched Codex {kind} event has call_id {call_id} in '{}'",
                path.display()
            );
        }
        self.unmatched_tool_event_count = self.unmatched_tool_event_count.saturating_add(1);
    }

    /// Run the per-line dispatch over `reader`, mutating `self` with
    /// the messages / tool-call pairings / first-occurrence trackers it
    /// observes. Called by both `parse_session_file` (full-file) and
    /// `parse_session_tail` (mmap-seeked) — they share the same loop body.
    fn scan_lines<R: BufRead>(&mut self, reader: R, path: &Path) {
        let stats =
            crate::provider::util::for_each_jsonl_record(reader, path, |_, entry: CodexLine| {
                self.scan_line(&entry, path);
                ControlFlow::Continue(())
            });
        let unmatched_tool_events = std::mem::take(&mut self.unmatched_tool_event_count);
        if unmatched_tool_events > 0 {
            log::warn!(
                "skipped {unmatched_tool_events} unmatched Codex tool result event(s) in '{}'",
                path.display()
            );
        }
        // Usage that arrived before the file's first turn_context: when the
        // whole file names exactly one model, that model is the answer;
        // ambiguity (or no model at all) stays a counted warning.
        let pending = std::mem::take(&mut self.pending_unresolved_usage);
        if !pending.is_empty() {
            if self.models_seen.len() == 1 {
                let model = self.models_seen.iter().next().cloned().unwrap_or_default();
                for (timestamp, turn_id, counts, total_counts) in pending {
                    let key = (timestamp.clone(), model.clone(), counts, total_counts);
                    if !self.seen_token_events.insert(key) {
                        continue;
                    }
                    self.ingest_token_count_usage(&timestamp, model.clone(), counts, turn_id);
                }
            } else {
                self.unresolved_usage_event_count = self
                    .unresolved_usage_event_count
                    .saturating_add(u32::try_from(pending.len()).unwrap_or(u32::MAX));
            }
        }
        let unresolved_usage_events = std::mem::take(&mut self.unresolved_usage_event_count);
        if unresolved_usage_events > 0 {
            log::warn!(
                "skipped {unresolved_usage_events} Codex token_count event(s) without resolvable models in '{}'",
                path.display()
            );
        }
        self.parse_warning_count = self
            .parse_warning_count
            .saturating_add(stats.parse_error_count)
            .saturating_add(unmatched_tool_events)
            .saturating_add(unresolved_usage_events);
    }

    /// Handle a top-level `compacted` line. Carries the post-compaction
    /// handoff summary in `payload.message`; surfaced as a System event
    /// so the user can see WHAT survived the compaction, not just that
    /// one happened. The boundary also ends response/snapshot pairing.
    fn handle_compacted(&mut self, entry: &CodexLine, payload: &Value) {
        self.unmatched_token_count_usage.clear();
        self.unmatched_token_usage_records.clear();
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
        self.begin_usage_turn(payload.get("turn_id").and_then(Value::as_str));
        // Extract actual self.model name (e.g. "gpt-5.4") from turn_context
        if let Some(m) = payload.get("model").and_then(|v| v.as_str())
            && !m.is_empty()
        {
            self.current_model = Some(m.to_string());
            self.models_seen.insert(m.to_string());
            if self.model.is_none() {
                self.model = Some(m.to_string());
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

        let history = match super::history::open_reader(self, path, file, file_size) {
            Ok(history) => history,
            Err(error) => {
                log::warn!(
                    "cannot resolve Codex history '{}': {error:#}",
                    path.display()
                );
                return None;
            }
        };
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
        if let Some(header) = &history.header {
            accum.set_history_header(header);
        }
        accum.history_segment_headers = history.segment_headers;
        accum.scan_lines(history.reader, path);

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
            models_seen: _,
            pending_unresolved_usage: _,
            replay_usage_skip: _,
            replay_last_timestamp_ms: _,
            previous_token_totals: _,
            seen_token_events: _,
            seen_completed_items: _,
            seen_assistant_items: _,
            current_turn_id: _,
            unmatched_token_count_usage: _,
            unmatched_token_usage_records: _,
            seen_token_usage_record_ids: _,
            cc_version,
            git_branch,
            is_sidechain,
            parent_id,
            agent_nickname,
            mut pending_user_message,
            skipping_fork_context,
            subagent_start_seconds: _,
            unmatched_tool_event_count: _,
            unresolved_usage_event_count: _,
            parse_warning_count,
            ..
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

/// Seed a partial tail parse from the nearest preceding turn context. Current
/// `token_usage_record` rows intentionally omit the model, so starting midway
/// through a long turn without this context would misclassify valid usage as
/// malformed. The reverse mmap walk touches only the pages needed to reach the
/// preceding context and preserves the tail fast path for very large rollouts.
fn prime_tail_turn_context(path: &Path, start_offset: u64, accum: &mut CodexScanAccum) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    // SAFETY: `file` remains open for the lifetime of this read-only mapping,
    // and the mapping is dropped before `file` when the function returns.
    let Ok(mmap) = (unsafe { Mmap::map(&file) }) else {
        return false;
    };
    let Ok(mut line_end) = usize::try_from(start_offset) else {
        return false;
    };
    let bytes: &[u8] = mmap.as_ref();
    if line_end > bytes.len() {
        return false;
    }

    while line_end > 0 && bytes[line_end - 1] == b'\n' {
        line_end -= 1;
    }
    while line_end > 0 {
        let previous_newline = memrchr(b'\n', &bytes[..line_end]);
        let line_start = previous_newline.map_or(0, |index| index + 1);
        if let Ok(entry) = serde_json::from_slice::<CodexLine>(&bytes[line_start..line_end])
            && entry.line_type == "turn_context"
            && let Some(payload) = entry.payload.as_ref()
        {
            let has_model = payload
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|model| !model.is_empty());
            if !has_model {
                return false;
            }
            accum.handle_turn_context(payload);
            return true;
        }
        let Some(newline) = previous_newline else {
            break;
        };
        line_end = newline;
        while line_end > 0 && bytes[line_end - 1] == b'\n' {
            line_end -= 1;
        }
    }
    false
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
/// - Typed inherited-history boundaries are enforced in both whole-file and
///   partial windows, using the canonical physical file header.
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
    let (reader, window) = open_tail_reader(path, scan_lines, "Codex")?;
    let header = match super::history::read_header(path) {
        Ok(header) => header,
        Err(error) => {
            log::warn!(
                "cannot read Codex history header '{}': {error:#}",
                path.display()
            );
            return None;
        }
    };
    if window.covers_whole_file
        && header
            .as_ref()
            .is_some_and(|header| header.is_continuation())
    {
        return None;
    }

    let mut accum = CodexScanAccum::new();
    if let Some(header) = &header {
        accum.set_history_header(header);
        if !window.covers_whole_file {
            accum.session_id = Some(header.id.clone());
            accum.is_sidechain = header.is_subagent;
        }
    }
    if !window.covers_whole_file && !prime_tail_turn_context(path, window.start_offset, &mut accum)
    {
        log::debug!(
            "Codex tail parse could not resolve preceding turn context for '{}'; falling back to full parse",
            path.display()
        );
        return None;
    }
    // Freeze the same file prefix used to calculate the tail offset. A live
    // Codex process may append another record while this reader is active;
    // consuming beyond the captured size would reintroduce a partial-line race.
    let stable_tail_bytes = window.file_size.saturating_sub(window.start_offset);
    accum.scan_lines(reader.take(stable_tail_bytes), path);

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
