use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::services::tail_reader::open_tail_reader;

use serde::Deserialize;
use serde_json::Value;

use crate::models::{Message, Provider, SessionMeta};
use crate::provider::{ParsedSession, UsageEvent};
use crate::provider_utils::{
    is_system_content, parse_rfc3339_timestamp, project_name_from_path, session_title,
    truncate_to_bytes, FTS_CONTENT_LIMIT, NO_PROJECT,
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
    /// Map call_id -> message index for merging function_call_output
    /// into the matching function_call message.
    pub(super) call_id_map: std::collections::HashMap<String, usize>,
    model: Option<String>,
    model_provider: Option<String>,
    pub(super) thread_name: Option<String>,
    pub(super) current_model: Option<String>,
    pub(super) previous_token_totals: Option<CodexRawUsageCounts>,
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
            call_id_map: std::collections::HashMap::new(),
            model: None,
            model_provider: None,
            thread_name: None,
            current_model: None,
            previous_token_totals: None,
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
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(error) => {
                    log::warn!(
                        "failed to read Codex session line from '{}': {}",
                        path.display(),
                        error
                    );
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }

            let entry: CodexLine = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(error) => {
                    log::warn!(
                        "skipping malformed Codex JSONL in '{}': {}",
                        path.display(),
                        error
                    );
                    self.parse_warning_count = self.parse_warning_count.saturating_add(1);
                    continue;
                }
            };

            if let Some(ref ts) = entry.timestamp {
                if self.first_timestamp.is_none() {
                    self.first_timestamp = Some(ts.clone());
                }
                self.last_timestamp = Some(ts.clone());
            }

            let payload = match entry.payload {
                Some(ref p) => p,
                None => continue,
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
                            continue;
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
                continue;
            }

            match entry.line_type.as_str() {
                "session_meta" => self.handle_session_meta(&entry, payload),
                "compacted" => self.handle_compacted(&entry, payload),
                "response_item" => self.handle_response_item(&entry, payload, path),
                "turn_context" => self.handle_turn_context(payload),
                "event_msg" => self.handle_event_msg(&entry, payload, path),
                _ => continue,
            }
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
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) => {
                log::warn!(
                    "failed to open Codex session '{}': {}",
                    path.display(),
                    error
                );
                return None;
            }
        };
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                log::warn!(
                    "failed to read Codex session metadata '{}': {}",
                    path.display(),
                    error
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

        let title = thread_name
            .or(agent_nickname.as_deref().map(|n| n.to_string()))
            .unwrap_or_else(|| session_title(first_user_message.as_deref()));

        let project_path = cwd.unwrap_or_else(|| NO_PROJECT.to_string());

        let project_name = project_name_from_path(&project_path);

        let created_at = parse_rfc3339_timestamp(first_timestamp.as_deref());

        let updated_at = parse_rfc3339_timestamp(last_timestamp.as_deref());

        let full_content = content_parts.join("\n");
        let content_text = truncate_to_bytes(&full_content, FTS_CONTENT_LIMIT);

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
pub fn parse_session_tail(path: &Path, target_messages: usize) -> Option<CodexTailResult> {
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
mod tests {
    use super::{parse_session_tail, CodexProvider};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn parse_session_surfaces_top_level_compacted_handoff_summary() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-1\",\"cwd\":\"/tmp\",\"cli_version\":\"0.123.0\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"compacted\",\"payload\":{\"message\":\"Recap so far: did X and Y.\",\"replacement_history\":[]}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"after compaction\"}]}}\n"
            ),
        )
        .unwrap();
        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        let compacted = parsed
            .messages
            .iter()
            .find(|m| m.content.contains("[context_compacted]"))
            .expect("compacted system event");
        assert!(
            compacted.content.contains("Recap so far: did X and Y."),
            "compacted handoff summary missing from {:?}",
            compacted.content
        );
    }

    #[test]
    fn parse_session_skips_usage_event_with_no_resolvable_model() {
        // No turn_context, no info.model — resolved_model is None. We
        // must NOT fabricate "gpt-5"; we drop the usage event entirely.
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-2\",\"cwd\":\"/tmp\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":0,\"output_tokens\":10,\"reasoning_output_tokens\":0,\"total_tokens\":110}}}}\n"
            ),
        )
        .unwrap();
        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        assert!(
            parsed.usage_events.is_empty(),
            "must NOT fabricate a model name when none resolves; got {:?}",
            parsed.usage_events
        );
        // Both paths must skip together: assistant message also gets no
        // phantom usage stamp when the model is unresolvable.
        let assistant = parsed
            .messages
            .iter()
            .find(|m| m.role == crate::models::MessageRole::Assistant)
            .expect("assistant message");
        assert!(
            assistant.token_usage.is_none(),
            "assistant must not carry usage with no resolved model; got {:?}",
            assistant.token_usage
        );
    }

    #[test]
    fn parse_session_collects_usage_events_keeping_total_input_and_cached_input() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        let events = &parsed.usage_events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model, "gpt-5.4");
        assert_eq!(events[0].input_tokens, 1000);
        assert_eq!(events[0].cache_read_input_tokens, 600);
        assert_eq!(events[0].output_tokens, 50);
    }

    #[test]
    fn parse_session_prefers_last_token_usage_when_both_last_and_total_are_present() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1700,\"cached_input_tokens\":1000,\"output_tokens\":70,\"reasoning_output_tokens\":25,\"total_tokens\":1770},\"last_token_usage\":{\"input_tokens\":700,\"cached_input_tokens\":400,\"output_tokens\":20,\"reasoning_output_tokens\":0,\"total_tokens\":720}}}}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        let events = &parsed.usage_events;
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].input_tokens, 1000);
        assert_eq!(events[0].cache_read_input_tokens, 600);
        assert_eq!(events[0].output_tokens, 50);
        assert_eq!(events[1].input_tokens, 1000);
        assert_eq!(events[1].cache_read_input_tokens, 600);
        assert_eq!(events[1].output_tokens, 50);
        assert_eq!(events[2].input_tokens, 700);
        assert_eq!(events[2].cache_read_input_tokens, 400);
        assert_eq!(events[2].output_tokens, 20);
    }

    #[test]
    fn parse_session_file_accumulates_repeated_last_token_usage() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        let assistant = parsed
            .messages
            .iter()
            .find(|message| message.role == crate::models::MessageRole::Assistant)
            .expect("assistant message");
        let usage = assistant.token_usage.as_ref().expect("token usage");

        assert_eq!(assistant.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(usage.input_tokens, 2000);
        assert_eq!(usage.cache_read_input_tokens, 1200);
        assert_eq!(usage.output_tokens, 100);
    }

    #[test]
    fn parse_session_file_counts_malformed_lines_without_aborting() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"first\"}}\n",
                "{ this is not valid json\n",
                "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"second\"}}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        assert_eq!(
            parsed.parse_warning_count, 1,
            "the single malformed line must be counted"
        );
        // The two well-formed user events should still produce messages.
        assert!(
            parsed.messages.len() >= 2,
            "well-formed lines must still produce messages; got {}",
            parsed.messages.len()
        );
    }

    #[test]
    fn parse_session_file_emits_tool_metadata_for_web_search_end_event() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"search docs\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"web_search_end\",\"call_id\":\"ws_123\",\"query\":\"notify kqueue\",\"action\":{\"type\":\"search\",\"query\":\"notify kqueue\"}}}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        let tool = parsed
            .messages
            .iter()
            .find(|message| message.tool_metadata.is_some())
            .expect("web search tool message");
        let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

        assert_eq!(tool.tool_name.as_deref(), Some("WebSearch"));
        assert_eq!(tool.content, "notify kqueue");
        assert_eq!(metadata.raw_name, "web_search_call");
        assert_eq!(metadata.canonical_name, "WebSearch");
        assert_eq!(metadata.status.as_deref(), Some("success"));
        assert_eq!(metadata.summary.as_deref(), Some("notify kqueue"));
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("action"))
                .and_then(|value| value.get("query"))
                .and_then(|value| value.as_str()),
            Some("notify kqueue")
        );
    }

    #[test]
    fn parse_session_file_merges_exec_command_end_into_existing_tool_message() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"pwd\\\"}\",\"call_id\":\"exec_123\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"exec_123\",\"output\":\"{\\\"output\\\":\\\"/tmp/project\\n\\\",\\\"metadata\\\":{\\\"exit_code\\\":0,\\\"duration_seconds\\\":0.2}}\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"exec_command_end\",\"call_id\":\"exec_123\",\"process_id\":\"42\",\"turn_id\":\"turn_1\",\"command\":[\"pwd\"],\"cwd\":\"/tmp/project\",\"parsed_cmd\":[],\"source\":\"agent\",\"stdout\":\"/tmp/project\\n\",\"stderr\":\"\",\"aggregated_output\":\"/tmp/project\\n\",\"exit_code\":0,\"duration\":{\"secs\":1,\"nanos\":500000000},\"formatted_output\":\"/tmp/project\\n\",\"status\":\"completed\"}}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        let tool = parsed
            .messages
            .iter()
            .find(|message| message.tool_name.as_deref() == Some("Bash"))
            .expect("bash tool message");
        let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

        assert_eq!(tool.content, "/tmp/project\n");
        assert_eq!(metadata.status.as_deref(), Some("completed"));
        assert_eq!(metadata.result_kind.as_deref(), Some("terminal_output"));
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("cwd"))
                .and_then(|value| value.as_str()),
            Some("/tmp/project")
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("source"))
                .and_then(|value| value.as_str()),
            Some("agent")
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("exitCode"))
                .and_then(|value| value.as_i64()),
            Some(0)
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("durationSeconds"))
                .and_then(|value| value.as_f64()),
            Some(1.5)
        );
    }

    #[test]
    fn parse_session_file_merges_patch_apply_end_into_existing_tool_message() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call\",\"status\":\"completed\",\"call_id\":\"patch_123\",\"name\":\"apply_patch\",\"input\":\"*** Begin Patch\\n*** Update File: src/file.rs\\n@@\\n-old\\n+new\\n*** End Patch\\n\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call_output\",\"call_id\":\"patch_123\",\"output\":\"{\\\"output\\\":\\\"Success. Updated the following files:\\\\nM src/file.rs\\\\n\\\",\\\"metadata\\\":{\\\"exit_code\\\":0,\\\"duration_seconds\\\":0.0}}\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"patch_apply_end\",\"call_id\":\"patch_123\",\"turn_id\":\"turn_1\",\"stdout\":\"Success. Updated the following files:\\nM src/file.rs\\n\",\"stderr\":\"\",\"success\":true,\"changes\":{\"src/file.rs\":{\"type\":\"update\",\"unified_diff\":\"@@ -1 +1 @@\\n-old\\n+new\\n\",\"move_path\":null}},\"status\":\"completed\"}}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        let tool = parsed
            .messages
            .iter()
            .find(|message| message.tool_name.as_deref() == Some("Edit"))
            .expect("apply patch tool message");
        let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

        assert_eq!(metadata.status.as_deref(), Some("completed"));
        assert_eq!(metadata.result_kind.as_deref(), Some("file_patch"));
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("diff"))
                .and_then(|value| value.as_str())
                .map(|value| value.contains("*** Update File: src/file.rs")),
            Some(true)
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("patches"))
                .and_then(|value| value.as_array())
                .and_then(|patches| patches.first())
                .and_then(|patch| patch.get("files"))
                .and_then(|value| value.as_array())
                .and_then(|files| files.first())
                .and_then(|value| value.as_str()),
            Some("src/file.rs")
        );
    }

    #[test]
    fn parse_session_file_handles_recent_codex_events_without_base64_output() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-28T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\",\"cwd\":\"/tmp/project\"}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_name_updated\",\"thread_id\":\"thread-1\",\"thread_name\":\"Generated image task\"}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"see image\",\"local_images\":[\"data:image/png;base64,USER_IMAGE\"]}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"missing_image\",\"output\":\"[{\\\"detail\\\":\\\"original\\\",\\\"image_url\\\":\\\"data:image/png;base64,TOOL_IMAGE\\\"}]\"}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"status\":\"generating\",\"revised_prompt\":\"make an icon\"}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"image_generation_end\",\"call_id\":\"ig_1\",\"status\":\"completed\",\"revised_prompt\":\"make an icon\",\"saved_path\":\"/Users/alice/.codex/generated_images/ig_1.png\",\"base64\":\"SHOULD_NOT_APPEAR\"}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"dynamic_tool_call_request\",\"callId\":\"dyn_1\",\"tool\":\"load_workspace_dependencies\",\"arguments\":{}}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:06Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"dynamic_tool_call_response\",\"call_id\":\"dyn_1\",\"tool\":\"load_workspace_dependencies\",\"arguments\":{},\"content_items\":[{\"type\":\"inputText\",\"text\":\"Workspace dependencies are available\"}],\"success\":true,\"error\":null,\"duration\":{\"secs\":0,\"nanos\":1000000}}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:07Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"item_completed\",\"item\":{\"type\":\"Plan\",\"id\":\"plan_1\",\"text\":\"# Plan\\n- Do the work\"}}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:08Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"error\",\"message\":\"unexpected status 502 Bad Gateway\",\"codex_error_info\":\"other\"}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:09Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_aborted\",\"turn_id\":\"turn_1\",\"reason\":\"interrupted\",\"duration_ms\":1500}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:10Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"context_compacted\"}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:11Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"send_input\",\"arguments\":\"{\\\"target\\\":\\\"agent-1\\\",\\\"message\\\":\\\"continue\\\"}\",\"call_id\":\"send_1\"}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:12Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"collab_agent_interaction_end\",\"call_id\":\"send_1\",\"status\":\"completed\",\"receiver_thread_id\":\"agent-1\"}}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        assert_eq!(parsed.meta.title, "Generated image task");
        assert!(
            !parsed
                .messages
                .iter()
                .any(|message| message.content.contains(";base64,")),
            "base64 image payloads should not be stored in message content"
        );

        let image = parsed
            .messages
            .iter()
            .find(|message| message.tool_name.as_deref() == Some("ImageGeneration"))
            .expect("image generation tool");
        assert_eq!(
            image.content,
            "[Image: source: /Users/alice/.codex/generated_images/ig_1.png]"
        );
        assert!(!image.content.contains("SHOULD_NOT_APPEAR"));
        let image_metadata = image.tool_metadata.as_ref().expect("image metadata");
        assert_eq!(image_metadata.category, "media");
        assert_eq!(image_metadata.result_kind.as_deref(), Some("image"));
        assert_eq!(
            image_metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("savedPath"))
                .and_then(|value| value.as_str()),
            Some("/Users/alice/.codex/generated_images/ig_1.png")
        );

        let dynamic = parsed
            .messages
            .iter()
            .find(|message| {
                message
                    .tool_metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.raw_name == "load_workspace_dependencies")
            })
            .expect("dynamic tool");
        assert_eq!(dynamic.tool_name.as_deref(), Some("DynamicTool"));
        assert_eq!(dynamic.content, "Workspace dependencies are available");
        assert_eq!(
            dynamic
                .tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.status.as_deref()),
            Some("success")
        );

        assert!(
            parsed.messages.iter().any(|message| message.role
                == crate::models::MessageRole::Assistant
                && message.content.starts_with("# Plan")),
            "Plan item should be emitted as a visible assistant message"
        );
        for marker in ["[error]", "[turn_aborted]", "[context_compacted]"] {
            assert!(
                parsed
                    .messages
                    .iter()
                    .any(|message| message.content.contains(marker)),
                "{marker} should be visible as a system event"
            );
        }

        let send_input = parsed
            .messages
            .iter()
            .find(|message| {
                message
                    .tool_metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.raw_name == "send_input")
            })
            .expect("send_input tool");
        assert_eq!(
            send_input
                .tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.status.as_deref()),
            Some("completed")
        );
        assert_eq!(
            send_input
                .tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.structured.as_ref())
                .and_then(|value| value.get("receiver_thread_id"))
                .and_then(|value| value.as_str()),
            Some("agent-1")
        );
    }

    #[test]
    fn parse_session_tail_returns_only_the_last_n_messages() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        let mut content = String::new();
        // Leading turn_context so the model is non-None for the bulk
        // of the file (matches real-world Codex JSONL layout).
        content.push_str(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
        );
        for i in 0..200 {
            let ts = format!("2026-04-10T10:00:{:02}Z", i % 60);
            content.push_str(&format!(
                "{{\"timestamp\":\"{ts}\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"msg-{i}\"}}]}}}}\n"
            ));
        }
        fs::write(&file, content).unwrap();

        let tail = parse_session_tail(&file, 20).expect("tail parse");
        assert_eq!(tail.messages.len(), 20);
        let first = tail.messages.first().expect("first").content.clone();
        let last = tail.messages.last().expect("last").content.clone();
        assert!(
            first.contains("msg-180"),
            "first tail message should be msg-180, got {first:?}"
        );
        assert!(
            last.contains("msg-199"),
            "last tail message should be msg-199, got {last:?}"
        );
    }

    #[test]
    fn parse_session_tail_returns_full_file_when_smaller_than_window() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        let mut content = String::new();
        content.push_str(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
        );
        for i in 0..5 {
            content.push_str(&format!(
                "{{\"timestamp\":\"2026-04-10T10:00:0{i}Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"only-{i}\"}}]}}}}\n"
            ));
        }
        fs::write(&file, content).unwrap();

        let tail = parse_session_tail(&file, 100).expect("tail parse");
        assert_eq!(
            tail.messages.len(),
            5,
            "tail must return all messages when the file is smaller than the requested window"
        );
    }
}
