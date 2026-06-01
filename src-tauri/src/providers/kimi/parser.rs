//! Kimi-code wire.jsonl parser.
//!
//! Handles two on-disk wire formats — both start with
//! `{"type":"metadata","protocol_version":"1.0",...}` and live under
//! `~/.kimi-code/sessions/wd_*/<session-dir>/agents/<name>/wire.jsonl`:
//!
//! * **Migrated** (from legacy kimi-cli protocol 1.9): only `metadata` +
//!   `context.append_message` lines. Messages carry `role` and structured
//!   `content[]`/`toolCalls[]` arrays. No per-line `time` field.
//! * **Native** (kimi-code 0.1.1+): events split into `metadata`,
//!   `config.update`, `turn.prompt`, `context.append_message` (user
//!   prompts only), `context.append_loop_event` (assistant
//!   `content.part` / `tool.call` / `tool.result` / step bookkeeping),
//!   `usage.record`. Each event-bearing line carries `"time"` in epoch
//!   milliseconds.
//!
//! The parser walks the file once, dispatching per-line by `type`, and
//! reuses a single accumulator so the message order matches on-disk
//! order regardless of which format the line uses.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::models::{Message, MessageRole, Provider, SessionMeta, TokenUsage};
use crate::provider::ParsedSession;
use crate::provider_utils::{
    project_name_from_path, session_title, truncate_to_bytes, FTS_CONTENT_LIMIT, NO_PROJECT,
};
use crate::services::tail_reader::tail_byte_offset;
use crate::tool_metadata::{
    build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

use super::tools::{render_format_a_tool_output, render_format_b_tool_output};

// ---------------------------------------------------------------------------
// session_index.jsonl: sessionId → workDir map written by kimi-code itself.
// Each line is `{"sessionId":"...","sessionDir":"...","workDir":"..."}`.
// We key by both sessionId (e.g. `session_<uuid>`) and sessionDir absolute
// path so a lookup can succeed from either side.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct SessionIndex {
    by_id: HashMap<String, String>,
    by_dir: HashMap<String, String>,
}

impl SessionIndex {
    pub(crate) fn load(path: &Path) -> Self {
        let mut index = Self::default();
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) => {
                // Missing file is fine on first run; log at debug so it
                // doesn't clutter normal operation.
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "failed to read Kimi session index '{}': {}",
                        path.display(),
                        error
                    );
                }
                return index;
            }
        };
        for (line_no, line) in BufReader::new(file).lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(error) => {
                    log::warn!(
                        "failed to read Kimi session_index.jsonl line {} from '{}': {}",
                        line_no + 1,
                        path.display(),
                        error
                    );
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(error) => {
                    log::warn!(
                        "skipping malformed Kimi session_index.jsonl line {} in '{}': {}",
                        line_no + 1,
                        path.display(),
                        error
                    );
                    continue;
                }
            };
            let work_dir = value
                .get("workDir")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let Some(work_dir) = work_dir else {
                continue;
            };
            if let Some(id) = value.get("sessionId").and_then(|v| v.as_str()) {
                index.by_id.insert(id.to_string(), work_dir.clone());
            }
            if let Some(dir) = value.get("sessionDir").and_then(|v| v.as_str()) {
                index.by_dir.insert(dir.to_string(), work_dir);
            }
        }
        index
    }

    fn lookup_workdir(&self, session_id: &str, session_dir: &Path) -> Option<String> {
        if let Some(wd) = self.by_id.get(session_id) {
            return Some(wd.clone());
        }
        // Try canonicalised first so `/var/...` ↔ `/private/var/...`
        // symlinks (macOS `/tmp` etc.) and trailing-slash mismatches
        // resolve; fall back to the raw path string for the common
        // case where both sides already match.
        let canon = std::fs::canonicalize(session_dir)
            .ok()
            .map(|p| p.to_string_lossy().to_string());
        if let Some(c) = &canon {
            if let Some(wd) = self.by_dir.get(c) {
                return Some(wd.clone());
            }
        }
        let raw = session_dir.to_string_lossy().to_string();
        let trimmed = raw.trim_end_matches('/');
        self.by_dir
            .get(trimmed)
            .or_else(|| self.by_dir.get(&raw))
            .cloned()
    }
}

// ---------------------------------------------------------------------------
// Path → identity helpers
// ---------------------------------------------------------------------------

/// Extract `(session_dir, agent_name)` from a wire.jsonl path.
/// Returns None if the path doesn't match the expected layout
/// `<session_dir>/agents/<agent>/wire.jsonl`.
fn split_session_path(path: &Path) -> Option<(PathBuf, String)> {
    let agent_dir = path.parent()?; // <session_dir>/agents/<agent>
    let agents_dir = agent_dir.parent()?; // <session_dir>/agents
    if agents_dir.file_name() != Some(std::ffi::OsStr::new("agents")) {
        return None;
    }
    let session_dir = agents_dir.parent()?.to_path_buf();
    let agent_name = agent_dir.file_name()?.to_string_lossy().to_string();
    Some((session_dir, agent_name))
}

/// Derive the on-disk session id (e.g. `session_<uuid>` or `ses_<uuid>`)
/// from a wire.jsonl path. Used by mod.rs to assemble parent ids for
/// subagents and by the source-sync layer to look up DB rows by path.
pub fn session_id_for_path(path: &Path) -> Option<String> {
    let (session_dir, _agent) = split_session_path(path)?;
    Some(session_dir.file_name()?.to_string_lossy().to_string())
}

/// state.json companion file produced by kimi-code alongside each session.
/// We only consume a few fields here; the schema may grow.
#[derive(Debug, Default)]
struct StateJson {
    /// Display title kimi-code stores after the first prompt.
    title: Option<String>,
    /// ISO-8601 (UTC) creation time, e.g. `"2026-05-25T09:26:36.474Z"`.
    created_at: Option<String>,
    /// ISO-8601 (UTC) last-update time.
    updated_at: Option<String>,
    /// Map of agent-name → parent-agent-name (None for `main`).
    /// Used to identify which wire.jsonl is the parent vs. subagent.
    agents: HashMap<String, Option<String>>,
}

impl StateJson {
    fn load(session_dir: &Path) -> Self {
        let path = session_dir.join("state.json");
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "failed to read Kimi state.json '{}': {}",
                        path.display(),
                        error
                    );
                }
                return Self::default();
            }
        };
        let value: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(error) => {
                log::warn!(
                    "failed to parse Kimi state.json '{}': {}",
                    path.display(),
                    error
                );
                return Self::default();
            }
        };
        let title = value
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let created_at = value
            .get("createdAt")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let updated_at = value
            .get("updatedAt")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let mut agents = HashMap::new();
        if let Some(map) = value.get("agents").and_then(|v| v.as_object()) {
            for (name, entry) in map {
                let parent = entry
                    .get("parentAgentId")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                agents.insert(name.clone(), parent);
            }
        }
        Self {
            title,
            created_at,
            updated_at,
            agents,
        }
    }
}

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

/// `time` fields in the new wire format are epoch milliseconds.
/// `metadata.created_at` is also epoch milliseconds. We treat both
/// uniformly: convert to (epoch_seconds, rfc3339_string).
fn time_ms_to_parts(ms: i64) -> (i64, String) {
    let secs = ms.div_euclid(1000);
    let nanos = (ms.rem_euclid(1000) * 1_000_000) as u32;
    let rfc = chrono::DateTime::from_timestamp(secs, nanos)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();
    (secs, rfc)
}

/// Parse ISO-8601 (e.g. state.json's `createdAt`) into epoch seconds.
fn iso_to_epoch_secs(iso: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|dt| dt.timestamp())
}

// ---------------------------------------------------------------------------
// Accumulator: shared per-line state for full-file and tail parse.
// ---------------------------------------------------------------------------

struct ScanAccum {
    messages: Vec<Message>,
    first_user_message: Option<String>,
    first_time_secs: Option<i64>,
    last_time_secs: Option<i64>,
    content_parts: Vec<String>,
    /// toolCallId → message index, used to merge tool.result onto the
    /// matching tool.call message.
    call_id_map: HashMap<String, usize>,
    /// Fallback timestamp when individual lines do not carry `time`
    /// (migrated format): derived from `metadata.created_at`.
    fallback_time_secs: Option<i64>,
    fallback_time_rfc: Option<String>,
    /// Tracks the most recently observed model alias so usage records and
    /// assistant messages can be tagged correctly.
    current_model: Option<String>,
    /// Message index of the first assistant text/think emitted in the
    /// current turn. `attach_usage` writes the turn's token totals here
    /// (rather than the trailing tool message) so the UI shows the
    /// model + cost on the actual assistant output. Reset to `None`
    /// after each `usage.record` / `step.end` is consumed.
    current_turn_assistant_idx: Option<usize>,
    parse_warning_count: u32,
}

impl ScanAccum {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            first_user_message: None,
            first_time_secs: None,
            last_time_secs: None,
            content_parts: Vec::new(),
            call_id_map: HashMap::new(),
            fallback_time_secs: None,
            fallback_time_rfc: None,
            current_model: None,
            current_turn_assistant_idx: None,
            parse_warning_count: 0,
        }
    }

    fn note_time(&mut self, ms: Option<i64>) -> Option<String> {
        let (secs, rfc) = match ms {
            Some(ms) => {
                let (s, r) = time_ms_to_parts(ms);
                (s, r)
            }
            None => match (self.fallback_time_secs, self.fallback_time_rfc.as_ref()) {
                (Some(s), Some(r)) => (s, r.clone()),
                _ => return None,
            },
        };
        if self.first_time_secs.is_none() {
            self.first_time_secs = Some(secs);
        }
        self.last_time_secs = Some(secs);
        Some(rfc)
    }

    fn push_user_text(&mut self, text: &str, ts: Option<String>, is_real_user: bool) {
        if text.is_empty() {
            return;
        }
        // Only a REAL user prompt (origin.kind == "user") marks a turn
        // boundary. `system_trigger` injections (subagent spawn, etc.)
        // can fire mid-turn and clearing the index would let the next
        // usage.record land on the wrong message.
        if is_real_user {
            self.current_turn_assistant_idx = None;
        }
        if self.first_user_message.is_none() {
            // Match the title heuristic used elsewhere: pick the first
            // non-image line as the title.
            let title = text
                .lines()
                .find(|l| !l.starts_with("[Image:"))
                .unwrap_or(text)
                .to_string();
            self.first_user_message = Some(title);
        }
        self.content_parts.push(text.to_string());
        self.messages.push(Message {
            role: MessageRole::User,
            content: text.to_string(),
            timestamp: ts,
            tool_name: None,
            tool_input: None,
            token_usage: None,
            model: None,
            usage_hash: None,
            tool_metadata: None,
        });
    }

    fn push_assistant_text(&mut self, text: &str, ts: Option<String>) {
        if text.is_empty() {
            return;
        }
        self.content_parts.push(text.to_string());
        if self.current_turn_assistant_idx.is_none() {
            self.current_turn_assistant_idx = Some(self.messages.len());
        }
        self.messages.push(Message {
            role: MessageRole::Assistant,
            content: text.to_string(),
            timestamp: ts,
            tool_name: None,
            tool_input: None,
            token_usage: None,
            model: self.current_model.clone(),
            usage_hash: None,
            tool_metadata: None,
        });
    }

    fn push_thinking(&mut self, text: &str, ts: Option<String>) {
        if text.is_empty() {
            return;
        }
        // Don't bind the turn's usage target to a thinking message —
        // [thinking] renders under MessageRole::System and the model
        // badge belongs on the real Assistant text that follows.
        self.messages.push(Message {
            role: MessageRole::System,
            content: format!("[thinking]\n{text}"),
            timestamp: ts,
            tool_name: None,
            tool_input: None,
            token_usage: None,
            model: self.current_model.clone(),
            usage_hash: None,
            tool_metadata: None,
        });
    }

    /// Append a tool call message. Stores call_id → idx for later
    /// pairing with a tool.result event.
    fn push_tool_call(
        &mut self,
        raw_name: &str,
        call_id: Option<&str>,
        args: Option<&Value>,
        ts: Option<String>,
    ) {
        let metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Kimi,
            raw_name,
            input: args,
            call_id,
            assistant_id: None,
        });
        let display_name = metadata.canonical_name.clone();
        let tool_input = args.map(|v| v.to_string());
        let idx = self.messages.len();
        if let Some(cid) = call_id {
            self.call_id_map.insert(cid.to_string(), idx);
        }
        self.messages.push(Message {
            role: MessageRole::Tool,
            content: String::new(),
            timestamp: ts,
            tool_name: Some(display_name),
            tool_input,
            token_usage: None,
            model: self.current_model.clone(),
            usage_hash: None,
            tool_metadata: Some(metadata),
        });
    }

    /// Merge a tool result onto the matching call, or push a standalone
    /// tool-result message if no matching call was seen yet (tail parse
    /// or out-of-order recovery).
    fn merge_tool_result(
        &mut self,
        call_id: Option<&str>,
        rendered_output: String,
        is_error: Option<bool>,
        raw_result: Option<&Value>,
        ts: Option<String>,
    ) {
        if !rendered_output.is_empty() {
            self.content_parts.push(rendered_output.clone());
        }
        let target_idx = call_id.and_then(|cid| self.call_id_map.get(cid)).copied();
        if let Some(idx) = target_idx {
            if idx < self.messages.len() {
                self.messages[idx].content = rendered_output;
                if let Some(meta) = self.messages[idx].tool_metadata.as_mut() {
                    enrich_tool_metadata(
                        meta,
                        ToolResultFacts {
                            raw_result,
                            is_error,
                            status: None,
                            artifact_path: None,
                        },
                    );
                }
                return;
            }
        }
        self.messages.push(Message {
            role: MessageRole::Tool,
            content: rendered_output,
            timestamp: ts,
            tool_name: None,
            tool_input: None,
            token_usage: None,
            model: None,
            usage_hash: None,
            tool_metadata: None,
        });
    }

    /// Attach the turn's token totals to its FIRST assistant text/think
    /// message (set by push_assistant_text / push_thinking). Falls back
    /// to the trailing assistant/tool message if we never saw an
    /// assistant-side message in the turn (e.g. tool-only step). After
    /// attachment, the per-turn index is cleared so the next step's
    /// usage lands on the next turn's first assistant message.
    fn attach_usage(&mut self, usage: TokenUsage, model: Option<&str>) {
        let target_idx = self.current_turn_assistant_idx.take().or_else(|| {
            // No assistant text/think this turn — fall back to the
            // trailing assistant/tool message so usage doesn't get lost.
            self.messages.iter().enumerate().rev().find_map(|(i, m)| {
                matches!(m.role, MessageRole::Assistant | MessageRole::Tool).then_some(i)
            })
        });
        let Some(idx) = target_idx else {
            // Wire stream gave us a usage record with no anchor message
            // — e.g. tail parse that started after the assistant text
            // and before any tool. Log so token totals that quietly fail
            // to land are visible in the parse-warning surface.
            log::warn!(
                "Kimi usage.record (output={}, input_other+cache={}) had no assistant/tool message to attach to",
                usage.output_tokens,
                usage.input_tokens
            );
            self.note_warning();
            return;
        };
        let Some(msg) = self.messages.get_mut(idx) else {
            return;
        };
        msg.token_usage = Some(usage);
        if let Some(m) = model {
            msg.model = Some(m.to_string());
        } else if msg.model.is_none() {
            msg.model = self.current_model.clone();
        }
    }

    fn note_warning(&mut self) {
        self.parse_warning_count = self.parse_warning_count.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Per-line dispatch — shared between full-file parse and tail parse.
// ---------------------------------------------------------------------------

/// Pull text out of an assistant content array (Format A `message.content`
/// or Format B `event.part`/`turn.prompt.input`). Returns plain text and
/// reformatted image placeholders so the FTS / title heuristics see them
/// uniformly.
fn text_from_parts(parts: &[Value]) -> String {
    let mut chunks: Vec<String> = Vec::new();
    let has_image = parts
        .iter()
        .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("image_url"));
    for part in parts {
        let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("text");
        match part_type {
            "text" => {
                let text = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                // Inline `<image path="..">…</image>` wrappers around an
                // image url were already represented as image_url parts;
                // drop them to avoid duplicating the marker.
                if has_image && (text.contains("<image path=") || text.trim() == "</image>") {
                    continue;
                }
                if !text.is_empty() {
                    chunks.push(text.to_string());
                }
            }
            "image_url" => {
                // Native kimi-code uses `imageUrl` (camelCase);
                // migrated wire still uses `image_url` (snake_case).
                // Accept both so format A/B share one code path.
                let url = part
                    .get("imageUrl")
                    .or_else(|| part.get("image_url"))
                    .and_then(|iu| iu.get("url"))
                    .and_then(|v| v.as_str());
                match url {
                    Some(url) => chunks.push(format!("[Image: source: {url}]")),
                    None => {
                        // URL field missing — surface a marker rather
                        // than silently dropping the image part.
                        log::warn!("Kimi image_url part has no resolvable URL");
                        chunks.push("[Image: source: unknown]".to_string());
                    }
                }
            }
            _ => {}
        }
    }
    chunks.join("\n")
}

fn dispatch_line(accum: &mut ScanAccum, entry: &Value) {
    let line_type = match entry.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return,
    };
    let line_time_ms = entry.get("time").and_then(|v| v.as_i64());

    match line_type {
        "metadata" => {
            // `created_at` is the only timestamp available on migrated
            // sessions (each subsequent line lacks `time`). Cache it so
            // `note_time(None)` can hand it back.
            if let Some(ms) = entry.get("created_at").and_then(|v| v.as_i64()) {
                let (secs, rfc) = time_ms_to_parts(ms);
                accum.fallback_time_secs = Some(secs);
                accum.fallback_time_rfc = Some(rfc);
                if accum.first_time_secs.is_none() {
                    accum.first_time_secs = Some(secs);
                }
                accum.last_time_secs = Some(secs);
            }
        }

        "config.update" => {
            if let Some(model) = entry
                .get("modelAlias")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                accum.current_model = Some(model.to_string());
            }
            // Soak up the time anyway so first/last span the whole file.
            let _ = accum.note_time(line_time_ms);
        }

        // ---- Format A & B: user prompt + injected reminders ----
        "context.append_message" => {
            let ts = accum.note_time(line_time_ms);
            let Some(message) = entry.get("message") else {
                accum.note_warning();
                return;
            };
            let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let content_array = message
                .get("content")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // kimi-code auto-injects user-role messages for permission
            // mode banners and similar system reminders. They carry
            // `origin.kind = "injection"` and the content is pure
            // `<system-reminder>` noise — drop them so the transcript
            // (and title heuristic) doesn't surface them as real user
            // input. `system_trigger` (subagent spawn etc.) is kept,
            // since that text drives the conversation.
            let origin_kind = message
                .get("origin")
                .and_then(|o| o.get("kind"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if origin_kind == "injection" {
                return;
            }

            match role {
                "user" => {
                    let text = text_from_parts(&content_array);
                    // Only origin.kind == "user" (or missing, for the
                    // migrated format) is a turn boundary. Treat
                    // `system_trigger` (subagent spawn etc.) as mid-
                    // turn content that must not reset usage tracking.
                    let is_real_user = matches!(origin_kind, "user" | "");
                    accum.push_user_text(&text, ts, is_real_user);
                }
                "assistant" => {
                    // Format A puts assistant think/text under content[],
                    // tool calls under message.toolCalls[]. Emit them in
                    // the order the on-disk message implies: think/text
                    // first, then tool calls.
                    for part in &content_array {
                        let pt = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        match pt {
                            "think" => {
                                let text = part.get("think").and_then(|v| v.as_str()).unwrap_or("");
                                accum.push_thinking(text, ts.clone());
                            }
                            "text" => {
                                let text = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                accum.push_assistant_text(text, ts.clone());
                            }
                            _ => {}
                        }
                    }
                    if let Some(calls) = message.get("toolCalls").and_then(|v| v.as_array()) {
                        for tc in calls {
                            let id = tc.get("id").and_then(|v| v.as_str());
                            let func = tc.get("function");
                            let name = func
                                .and_then(|f| f.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            // Format A serialises args as a JSON string;
                            // try to parse it back into a Value so the
                            // metadata builder can structure-inspect it.
                            let arg_string = func
                                .and_then(|f| f.get("arguments"))
                                .and_then(|v| v.as_str());
                            let arg_value: Option<Value> =
                                arg_string.and_then(|s| serde_json::from_str::<Value>(s).ok());
                            accum.push_tool_call(name, id, arg_value.as_ref(), ts.clone());
                        }
                    }
                }
                "tool" => {
                    let call_id = message.get("toolCallId").and_then(|v| v.as_str());
                    let (rendered, is_error) = render_format_a_tool_output(&content_array);
                    accum.merge_tool_result(call_id, rendered, is_error, None, ts);
                }
                _ => {}
            }
        }

        // ---- Format B: streaming events ----
        "context.append_loop_event" => {
            let ts = accum.note_time(line_time_ms);
            let Some(event) = entry.get("event") else {
                return;
            };
            let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match event_type {
                "content.part" => {
                    let part = event.get("part").unwrap_or(event);
                    let pt = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match pt {
                        "think" => {
                            let text = part.get("think").and_then(|v| v.as_str()).unwrap_or("");
                            accum.push_thinking(text, ts);
                        }
                        "text" => {
                            let text = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            accum.push_assistant_text(text, ts);
                        }
                        _ => {}
                    }
                }
                "tool.call" => {
                    let id = event
                        .get("toolCallId")
                        .or_else(|| event.get("uuid"))
                        .and_then(|v| v.as_str());
                    let name = event
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let args = event.get("args");
                    accum.push_tool_call(name, id, args, ts);
                }
                "tool.result" => {
                    let id = event
                        .get("toolCallId")
                        .or_else(|| event.get("parentUuid"))
                        .and_then(|v| v.as_str());
                    let result = event.get("result");
                    let (rendered, is_error) = render_format_b_tool_output(result);
                    accum.merge_tool_result(id, rendered, is_error, result, ts);
                }
                "step.end" => {
                    // `usage.record` carries the same totals plus the
                    // canonical model alias and fires right after
                    // step.end. Attaching here too would consume the
                    // per-turn assistant index, leaving the follow-up
                    // record to land on the trailing tool message
                    // instead. Skip — let `usage.record` do the work.
                }
                _ => {}
            }
        }

        "usage.record" => {
            let _ = accum.note_time(line_time_ms);
            let model = entry.get("model").and_then(|v| v.as_str());
            if let Some(u) = entry.get("usage") {
                if let Some(usage) = parse_usage(u) {
                    accum.attach_usage(usage, model);
                }
            }
        }

        _ => {}
    }
}

fn parse_usage(value: &Value) -> Option<TokenUsage> {
    let input_other = value
        .get("inputOther")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let output = value.get("output").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let cache_read = value
        .get("inputCacheRead")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_creation = value
        .get("inputCacheCreation")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    if input_other == 0 && output == 0 && cache_read == 0 && cache_creation == 0 {
        return None;
    }
    Some(TokenUsage {
        input_tokens: input_other + cache_read + cache_creation,
        output_tokens: output,
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: cache_creation,
    })
}

fn scan_lines<R: BufRead>(reader: R, path: &Path, accum: &mut ScanAccum) {
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(error) => {
                log::warn!(
                    "failed to read Kimi wire.jsonl line from '{}': {}",
                    path.display(),
                    error
                );
                accum.note_warning();
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let entry: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(error) => {
                log::warn!(
                    "skipping malformed Kimi wire.jsonl line in '{}': {}",
                    path.display(),
                    error
                );
                accum.note_warning();
                continue;
            }
        };
        dispatch_line(accum, &entry);
    }
}

// ---------------------------------------------------------------------------
// Subagent title resolution
//
// kimi-code spawns subagents via an `Agent` tool call in the parent's
// wire.jsonl whose `args.description` is the short, intentional label
// the parent (LLM) chose for the subtask — e.g. "Find .toml files".
// The subagent's *own* first user message is a much larger blob —
// `<git-context>…</git-context><environment>…</environment>` plus the
// prompt — so using it as a tree title clutters the UI.
//
// At parse time we don't know which Agent tool.call produced a given
// `agent-N` directory until we see its tool.result, which carries
// `agent_id: agent-N` in the rendered text. We scan the parent's wire
// once per session dir and build an `agent-N → description` map.
// ---------------------------------------------------------------------------

/// Scan a parent `wire.jsonl` and build the `agent-N → description`
/// map produced by Agent tool calls. Returns an empty map if the file
/// is missing or contains no Agent invocations.
fn collect_subagent_descriptions(parent_wire: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let file = match File::open(parent_wire) {
        Ok(f) => f,
        Err(_) => return map,
    };
    // toolCallId → description from the spawning Agent tool.call,
    // resolved to its agent_id once we see the matching tool.result.
    let mut pending: HashMap<String, String> = HashMap::new();
    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                log::warn!(
                    "kimi: failed to read line from {}: {e}",
                    parent_wire.display()
                );
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let entry: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "kimi: skipping malformed JSON in {}: {e}",
                    parent_wire.display()
                );
                continue;
            }
        };
        if entry.get("type").and_then(|v| v.as_str()) != Some("context.append_loop_event") {
            continue;
        }
        let Some(ev) = entry.get("event") else {
            continue;
        };
        let ev_type = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ev_type {
            "tool.call" if ev.get("name").and_then(|v| v.as_str()) == Some("Agent") => {
                let Some(call_id) = ev.get("toolCallId").and_then(|v| v.as_str()) else {
                    continue;
                };
                let args = ev.get("args");
                let description = args
                    .and_then(|a| a.get("description"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                if let Some(desc) = description {
                    pending.insert(call_id.to_string(), desc);
                }
            }
            "tool.result" => {
                let Some(call_id) = ev.get("toolCallId").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Some(desc) = pending.remove(call_id) else {
                    continue;
                };
                // The result output text starts with `agent_id: <name>`
                // when the spawned agent is local (subagent). A single
                // Agent tool.call can dispatch to multiple targets and
                // the result lists each agent_id on its own line:
                //   agent_id: agent-0
                //   …
                //   agent_id: agent-1
                //   …
                // Map all matched ids to the same description so each
                // subagent gets a meaningful title.
                let output = ev
                    .get("result")
                    .and_then(|r| r.get("output"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                for raw_line in output.lines() {
                    if let Some(rest) = raw_line.strip_prefix("agent_id:") {
                        let agent_id = rest.trim();
                        if !agent_id.is_empty() {
                            map.insert(agent_id.to_string(), desc.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Full-file parse entry point
// ---------------------------------------------------------------------------

/// Parse one `<session>/agents/<name>/wire.jsonl`. Returns `None` only
/// for non-recoverable issues (file open failure, unrecognised layout)
/// — file-level recoverable problems return Some(...) with
/// `parse_warning_count > 0`.
pub(crate) fn parse_session(path: &Path, index: &SessionIndex) -> Option<ParsedSession> {
    let (session_dir, agent_name) = match split_session_path(path) {
        Some(parts) => parts,
        None => {
            log::warn!(
                "Kimi wire.jsonl path '{}' does not match <session_dir>/agents/<name>/wire.jsonl",
                path.display()
            );
            return None;
        }
    };

    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "failed to open Kimi wire.jsonl '{}': {}",
                path.display(),
                error
            );
            return None;
        }
    };
    let file_meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(error) => {
            log::warn!(
                "failed to stat Kimi wire.jsonl '{}': {}",
                path.display(),
                error
            );
            return None;
        }
    };

    let state = StateJson::load(&session_dir);
    let mut accum = ScanAccum::new();
    scan_lines(BufReader::new(file), path, &mut accum);

    let parent_agent = state.agents.get(&agent_name).cloned().unwrap_or(None);
    let is_subagent = parent_agent.is_some();
    let parent_is_main = parent_agent.as_deref() == Some("main");

    let session_dir_name = session_dir.file_name()?.to_string_lossy().to_string();
    let session_id = if is_subagent {
        // No "official" global id for subagents — kimi-code identifies
        // them only by name local to a session. Combining parent dir + agent
        // name keeps the DB primary key globally unique while still being
        // resolvable back to the on-disk path. resume_command in mod.rs
        // strips the suffix before passing to `kimi --session`.
        format!("{session_dir_name}:{agent_name}")
    } else {
        session_dir_name.clone()
    };

    if accum.messages.is_empty() {
        log::debug!(
            "Kimi session '{}' parsed to zero messages — skipping",
            path.display()
        );
        return None;
    }

    // Title resolution:
    //   * Parent: state.json.title (kimi-code's own display label) →
    //     first user message heuristic.
    //   * Subagent: state.json is shared with the parent so its title
    //     is useless. The parent's `Agent` tool.call carries the short,
    //     intentional `description` the LLM chose for the subtask —
    //     prefer that. Fall back to the heuristic over the subagent's
    //     own first user message, which is typically a 1k+ char blob
    //     of `<git-context>` + environment prefixed to the real prompt.
    let title = if is_subagent {
        // Walk to the DIRECT parent agent (could be main or another
        // subagent) and scan its wire.jsonl for the Agent tool.call
        // that spawned us. Falls back to the first user message
        // (`<git-context>…` blob) heuristic when the description is
        // unavailable.
        let parent_agent_name = parent_agent.clone().unwrap_or_else(|| "main".to_string());
        let parent_wire = session_dir
            .join("agents")
            .join(&parent_agent_name)
            .join("wire.jsonl");
        let descriptions = collect_subagent_descriptions(&parent_wire);
        descriptions
            .get(&agent_name)
            .cloned()
            .unwrap_or_else(|| session_title(accum.first_user_message.as_deref()))
    } else {
        state
            .title
            .clone()
            .unwrap_or_else(|| session_title(accum.first_user_message.as_deref()))
    };

    let project_path = index
        .lookup_workdir(&session_dir_name, &session_dir)
        .unwrap_or_else(|| NO_PROJECT.to_string());
    let project_name = project_name_from_path(&project_path);

    let state_created = state.created_at.as_deref().and_then(iso_to_epoch_secs);
    let state_updated = state.updated_at.as_deref().and_then(iso_to_epoch_secs);

    let Some(created_at) = accum.first_time_secs.or(state_created) else {
        log::warn!(
            "skipping Kimi session '{}': no usable timestamp found",
            path.display()
        );
        return None;
    };
    let Some(updated_at) = accum.last_time_secs.or(state_updated).or(Some(created_at)) else {
        log::warn!(
            "skipping Kimi session '{}': no usable updated timestamp",
            path.display()
        );
        return None;
    };

    let full_content = accum.content_parts.join("\n");
    let content_text = truncate_to_bytes(&full_content, FTS_CONTENT_LIMIT);

    let parent_id = if is_subagent {
        // Direct parent: the agent named in state.json.parentAgentId.
        // If that's `main`, the parent is the top-level session; for
        // any other agent (e.g. `agent-0`), the parent itself is a
        // subagent and its id is `<session_dir>:<agent>`.
        if parent_is_main {
            Some(session_dir_name.clone())
        } else {
            parent_agent
                .as_deref()
                .map(|a| format!("{session_dir_name}:{a}"))
        }
    } else {
        None
    };

    let meta = SessionMeta {
        id: session_id,
        provider: Provider::Kimi,
        title,
        project_path,
        project_name,
        created_at,
        updated_at,
        message_count: accum.messages.len() as u32,
        file_size_bytes: file_meta.len(),
        source_path: path.to_string_lossy().to_string(),
        is_sidechain: is_subagent,
        variant_name: None,
        model: accum.current_model.clone(),
        cc_version: None,
        git_branch: None,
        parent_id,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };

    let source_mtime = file_meta
        .modified()
        .ok()
        .and_then(crate::provider::system_time_to_epoch_seconds)
        .unwrap_or(0);

    Some(ParsedSession {
        meta,
        messages: accum.messages,
        content_text,
        parse_warning_count: accum.parse_warning_count,
        child_session_ids: Vec::new(),
        usage_events: Vec::new(),
        source_mtime,
    })
}

// ---------------------------------------------------------------------------
// Tail parse — fast path for SessionView's negative-offset windows.
// ---------------------------------------------------------------------------

pub struct KimiTailResult {
    pub messages: Vec<Message>,
    pub parse_warning_count: u32,
}

/// Parse only the last ~target_messages worth of lines from a
/// `wire.jsonl`. Returns None if the file cannot be opened or the tail
/// produced zero messages (caller should fall through to full parse).
///
/// Trade-offs match the other tail parsers: tool.call/tool.result pairs
/// that straddle the boundary surface as standalone tool messages until
/// the background full-parse replaces the cache.
pub fn parse_session_tail(path: &Path, target_messages: usize) -> Option<KimiTailResult> {
    let safety_buffer = target_messages / 4 + 50;
    let scan_lines_count = target_messages.saturating_add(safety_buffer);
    let window = match tail_byte_offset(path, scan_lines_count) {
        Ok(w) => w,
        Err(error) => {
            log::warn!(
                "failed to locate Kimi wire.jsonl tail in '{}': {}",
                path.display(),
                error
            );
            return None;
        }
    };
    let mut accum = ScanAccum::new();
    // Migrated (Format A) wires only carry a timestamp on the
    // `metadata` header line — every `context.append_message` after it
    // has no `time`. The tail seek skips that header, so prime the
    // accumulator's fallback timestamp by reading the file head first.
    // Cheap (one short read) and lets every tail-rendered message land
    // with a usable timestamp.
    if window.start_offset > 0 {
        if let Ok(head_file) = File::open(path) {
            let head_reader = BufReader::new(head_file);
            for line in head_reader.lines().take(4).map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<Value>(&line) {
                    if entry.get("type").and_then(|v| v.as_str()) == Some("metadata") {
                        dispatch_line(&mut accum, &entry);
                        break;
                    }
                }
            }
        }
    }
    let file = match File::open(path) {
        Ok(f) => f,
        Err(error) => {
            log::warn!(
                "failed to open Kimi wire.jsonl for tail '{}': {}",
                path.display(),
                error
            );
            return None;
        }
    };
    let mut reader = BufReader::new(file);
    if window.start_offset > 0 {
        if let Err(error) = reader.seek(SeekFrom::Start(window.start_offset)) {
            log::warn!(
                "failed to seek Kimi wire.jsonl tail in '{}': {}",
                path.display(),
                error
            );
            return None;
        }
    }
    scan_lines(reader, path, &mut accum);
    if accum.messages.is_empty() {
        return None;
    }
    let len = accum.messages.len();
    if len > target_messages {
        accum.messages.drain(0..(len - target_messages));
    }
    Some(KimiTailResult {
        messages: accum.messages,
        parse_warning_count: accum.parse_warning_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wire(dir: &Path, agent: &str, lines: &[&str]) -> PathBuf {
        let agent_dir = dir.join("agents").join(agent);
        std::fs::create_dir_all(&agent_dir).unwrap();
        let path = agent_dir.join("wire.jsonl");
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    fn write_state(dir: &Path, json: &str) {
        std::fs::write(dir.join("state.json"), json).unwrap();
    }

    #[test]
    fn parses_format_b_basic_session() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_demo_abc").join("session_aaaa");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{
                "createdAt": "2026-05-25T09:26:36.474Z",
                "updatedAt": "2026-05-25T09:26:40.000Z",
                "title": "Demo title",
                "agents": {
                    "main": { "type": "main", "parentAgentId": null }
                }
            }"#,
        );
        let path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"config.update","modelAlias":"kimi-code/kimi-for-coding","time":1779701196500}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"hi"}],"toolCalls":[]}}"#,
                r#"{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"think","think":"thinking..."}},"time":1779701200000}"#,
                r#"{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"Hello!"}},"time":1779701200500}"#,
                r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":10,"output":5,"inputCacheRead":100,"inputCacheCreation":0},"time":1779701200600}"#,
            ],
        );
        let parsed = parse_session(&path, &SessionIndex::default()).expect("parses");
        assert_eq!(parsed.meta.title, "Demo title");
        assert!(!parsed.meta.is_sidechain);
        // user, thinking (System), assistant
        assert_eq!(parsed.messages.len(), 3);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[1].role, MessageRole::System);
        assert!(parsed.messages[1].content.starts_with("[thinking]"));
        assert_eq!(parsed.messages[2].role, MessageRole::Assistant);
        assert_eq!(parsed.messages[2].content, "Hello!");
        assert_eq!(
            parsed.messages[2].model.as_deref(),
            Some("kimi-code/kimi-for-coding")
        );
        let usage = parsed.messages[2].token_usage.as_ref().expect("usage");
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_input_tokens, 100);
        // input_tokens = inputOther + cache_read + cache_creation
        assert_eq!(usage.input_tokens, 110);
    }

    #[test]
    fn pairs_format_b_tool_call_and_result() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_x_zz").join("session_bbbb");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{"agents":{"main":{"type":"main","parentAgentId":null}}}"#,
        );
        let path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"read file"}],"toolCalls":[]}}"#,
                r#"{"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"tc_1","name":"Read","args":{"path":"a.txt"},"description":"Reading a.txt"},"time":1779701197000}"#,
                r#"{"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"tc_1","result":{"output":"hello world"}},"time":1779701197500}"#,
            ],
        );
        let parsed = parse_session(&path, &SessionIndex::default()).expect("parses");
        // user + tool (call+result merged)
        assert_eq!(parsed.messages.len(), 2);
        let tool = &parsed.messages[1];
        assert_eq!(tool.role, MessageRole::Tool);
        assert_eq!(tool.tool_name.as_deref(), Some("Read"));
        assert_eq!(tool.content, "hello world");
        let input: Value = serde_json::from_str(tool.tool_input.as_ref().unwrap()).unwrap();
        assert_eq!(input["path"], "a.txt");
    }

    #[test]
    fn parses_format_a_migrated_session() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_y_yy").join("ses_cccc");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{
                "createdAt": "2026-05-01T08:24:04.612Z",
                "updatedAt": "2026-05-01T08:24:04.612Z",
                "title": "Migrated",
                "agents": {"main": {"type":"main","parentAgentId":null}}
            }"#,
        );
        let path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1777623844612}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"check files"}],"toolCalls":[]}}"#,
                r#"{"type":"context.append_message","message":{"role":"assistant","content":[{"type":"think","think":"let me look"}],"toolCalls":[{"type":"function","id":"tc_a","function":{"name":"Shell","arguments":"{\"command\":\"ls\"}"}}]}}"#,
                r#"{"type":"context.append_message","message":{"role":"tool","content":[{"type":"text","text":"file1\nfile2"}],"toolCalls":[],"toolCallId":"tc_a"}}"#,
            ],
        );
        let parsed = parse_session(&path, &SessionIndex::default()).expect("parses");
        // user + assistant thinking + tool (merged)
        assert_eq!(parsed.messages.len(), 3);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[1].role, MessageRole::System);
        let tool = &parsed.messages[2];
        assert_eq!(tool.role, MessageRole::Tool);
        // Shell → canonicalised to Bash
        assert_eq!(tool.tool_name.as_deref(), Some("Bash"));
        assert_eq!(tool.content, "file1\nfile2");
    }

    #[test]
    fn subagent_links_parent_via_state_json() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_z_zz").join("session_dddd");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{
                "agents": {
                    "main": {"type":"main","parentAgentId":null},
                    "agent-0": {"type":"sub","parentAgentId":"main"}
                }
            }"#,
        );
        // Both agents need at least one user message for the parser to keep them.
        write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"parent prompt"}],"toolCalls":[]}}"#,
            ],
        );
        let sub_path = write_wire(
            &session_dir,
            "agent-0",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196500}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"sub prompt"}],"toolCalls":[]}}"#,
            ],
        );
        let sub = parse_session(&sub_path, &SessionIndex::default()).expect("sub parses");
        assert!(sub.meta.is_sidechain);
        assert_eq!(sub.meta.id, "session_dddd:agent-0");
        assert_eq!(sub.meta.parent_id.as_deref(), Some("session_dddd"));
    }

    #[test]
    fn project_path_comes_from_session_index() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_proj_hash").join("session_eeee");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{"agents":{"main":{"type":"main","parentAgentId":null}}}"#,
        );
        let path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"hi"}],"toolCalls":[]}}"#,
            ],
        );
        let mut index = SessionIndex::default();
        index
            .by_id
            .insert("session_eeee".to_string(), "/home/user/proj".to_string());
        let parsed = parse_session(&path, &index).expect("parses");
        assert_eq!(parsed.meta.project_path, "/home/user/proj");
        assert_eq!(parsed.meta.project_name, "proj");
    }

    #[test]
    fn session_id_for_path_strips_layout() {
        let p = Path::new("/home/u/.kimi-code/sessions/wd_x_yy/session_abc/agents/main/wire.jsonl");
        assert_eq!(session_id_for_path(p).as_deref(), Some("session_abc"));
        let p2 = Path::new("/home/u/.kimi-code/sessions/wd_x_yy/ses_abc/agents/agent-0/wire.jsonl");
        assert_eq!(session_id_for_path(p2).as_deref(), Some("ses_abc"));
        let bogus = Path::new("/etc/passwd");
        assert!(session_id_for_path(bogus).is_none());
    }
}
