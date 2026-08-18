//! Wire-format parser for MiniMax Code (mcode) `messages.jsonl`.
//!
//! MiniMax Code stores one session per directory under
//! `{dataDir}/v2/sessions/YYYY/MM/DD/HH-MM-SS-mmm-session_<b64(sessionId)>/`.
//! Session metadata lives in `sqlite/runtime-state.sqlite` (table
//! `local_runtime_sessions`), but the full per-message wire is only in
//! `messages.jsonl` — the SQLite `local_runtime_message_rows` mirror
//! truncates assistant content and drops the `usage` blob. Load-time
//! therefore reads the jsonl, never `data_json`.
//!
//! ## Wire format (one JSON object per line)
//!
//! ```text
//! {
//!   "message_id": "msg-..." | "msg-user-v1-...",
//!   "turn_id":   "turn_<run>_<step>",
//!   "message": {
//!     "role": "user" | "assistant" | "toolResult" | "system",
//!     "content": [
//!       { "type": "text",      "text": "..." },
//!       { "type": "thinking",  "thinking": "..." },
//!       { "type": "toolCall",  "id": "call_...", "name": "bash",
//!         "arguments": { ... } | "<json-string>" },
//!       { "type": "image",     "data": "<base64>", "mimeType": "image/jpeg" }
//!     ],
//!     "model":       "MiniMax-M3",
//!     "usage": { "input": u32, "output": u32, "cacheRead": u32,
//!                "cacheWrite": u32,
//!                "cost": { "total": f64, ... } },
//!     "timestamp":   <epoch ms>,
//!     "canonicalTextRange": { "startOffset": u, "endOffset": u }
//!   }
//! }
//! ```
//!
//! `canonicalTextRange` is a character-offset slice into the concatenated
//! text parts. The runtime prefixes every user turn with a
//! `<system-reminder>` block; the range points at the user's actual
//! prompt. Offsets are Unicode scalar values, not bytes.
//!
//! For `toolResult` rows the shape collapses to:
//! ```text
//! { "role": "toolResult", "toolCallId": "...", "toolName": "bash",
//!   "content": [ { "type": "text", "text": "..." } | { "type": "image", ... } ],
//!   "isError": false, "timestamp": <epoch ms> }
//! ```
//!
//! ## Translation into the canonical `Message` shape
//!
//! - `user` text            → `Message::user` of the `canonicalTextRange`
//!   slice (or the full text when the range is absent). Images become
//!   `[Image: source: data:{mime};base64,{data}]` markers. A turn that
//!   is only the injected reminder (empty range, no images) is dropped.
//! - `assistant` parts      → emitted in wire order. `thinking` →
//!   `Message::system("[thinking]\n{text}")`; `text` →
//!   `Message::assistant`; `toolCall` → a Tool row via
//!   `build_tool_metadata`. Usage attaches to the first assistant text
//!   row of that wire entry.
//! - `toolResult`           → folded into the matching `toolCall` by
//!   `toolCallId`. Orphan results surface as a system note. A `task`
//!   result's `details.sub_session_id` (or `<task_result session_id>`)
//!   is the hidden child Agent id.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::models::{Message, MessageRole, TokenUsage};
use crate::provider::UsageEvent;
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

/// The on-disk shape of a single `messages.jsonl` line.
#[derive(Debug, Deserialize)]
pub(crate) struct WireLine {
    pub message_id: String,
    pub message: WireMessage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireMessage {
    pub role: String,
    #[serde(default)]
    pub content: Vec<WireContent>,
    pub model: Option<String>,
    pub usage: Option<WireUsage>,
    pub timestamp: Option<i64>,
    pub canonical_text_range: Option<CanonicalTextRange>,
    // toolResult-only:
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub is_error: Option<bool>,
    /// Typed payload on `task` (and similar) results. Carries
    /// `sub_session_id` for the hidden child Agent.
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

/// Character offsets into the concatenated text parts of a user turn.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CanonicalTextRange {
    pub start_offset: usize,
    pub end_offset: usize,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum WireContent {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    ToolCall {
        id: String,
        name: String,
        /// Tool arguments arrive as either an object (newer sessions) or a
        /// raw JSON-encoded string (legacy / partial-stream chunks). Accept
        /// both without forcing the parser to pick.
        arguments: serde_json::Value,
    },
    Image {
        #[serde(default)]
        data: Option<String>,
        #[serde(default, rename = "mimeType")]
        mime_type: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WireUsage {
    #[serde(default)]
    pub input: u32,
    #[serde(default)]
    pub output: u32,
    #[serde(default, rename = "cacheRead")]
    pub cache_read: u32,
    #[serde(default, rename = "cacheWrite")]
    pub cache_write: u32,
    #[serde(default)]
    pub cost: Option<WireCost>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WireCost {
    #[serde(default)]
    pub total: f64,
}

impl WireUsage {
    fn into_token_usage(self) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input,
            output_tokens: self.output,
            cache_creation_input_tokens: self.cache_write,
            cache_read_input_tokens: self.cache_read,
        }
    }
}

/// Outcome of parsing one full `messages.jsonl` file.
#[derive(Debug)]
pub(crate) struct ParsedMessages {
    pub messages: Vec<Message>,
    pub parse_warning_count: u32,
    /// First non-empty `message.model` we saw on an assistant turn — the
    /// provider module uses this as the `SessionMeta::model` fallback when
    /// the SQLite row has no `effectiveModel`.
    pub first_assistant_model: Option<String>,
    /// One event per assistant wire entry that carried a `usage` blob
    /// and a model name. Authoritative for indexer stats.
    pub usage_events: Vec<UsageEvent>,
    /// Child session ids from `task` results (`details.sub_session_id`).
    pub child_session_ids: Vec<String>,
}

/// Parse the entire `messages.jsonl`. Malformed individual lines are dropped
/// with a warning and a `parse_warning_count` bump; the load still succeeds
/// so the session is browseable (matches DSH / OpenCode tolerance).
pub(crate) fn parse_messages_file(path: &Path) -> Option<ParsedMessages> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            log::warn!(
                "failed to read mcode messages file '{}': {error}",
                path.display()
            );
            return None;
        }
    };

    let mut state = ParseState::default();
    let mut seen_message_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut parse_warning_count: u32 = 0;

    for (line_no, raw) in content.lines().enumerate() {
        if raw.is_empty() {
            continue;
        }
        let wire: WireLine = match serde_json::from_str(raw) {
            Ok(value) => value,
            Err(error) => {
                log::warn!(
                    "skipping malformed mcode record at line {} in '{}': {error}",
                    line_no + 1,
                    path.display()
                );
                parse_warning_count += 1;
                continue;
            }
        };

        // The runtime occasionally writes a stream-finalization duplicate of
        // the last assistant message (same message_id) when an interrupted
        // run is reconciled. Keep the first occurrence.
        if !seen_message_ids.insert(wire.message_id.clone()) {
            log::debug!(
                "skipping duplicate mcode message_id '{}' at line {} in '{}'",
                wire.message_id,
                line_no + 1,
                path.display()
            );
            continue;
        }

        if let Err(error) = apply(&mut state, wire) {
            log::warn!(
                "skipping unconvertible mcode record at line {} in '{}': {error}",
                line_no + 1,
                path.display()
            );
            parse_warning_count += 1;
        }
    }

    Some(ParsedMessages {
        messages: state.messages,
        parse_warning_count: state.parse_warning_count + parse_warning_count,
        first_assistant_model: state.first_assistant_model,
        usage_events: state.usage_events,
        child_session_ids: state.child_session_ids,
    })
}

#[derive(Default)]
struct ParseState {
    messages: Vec<Message>,
    /// index by `tool_call_id` → position in `messages` of the call we
    /// emitted (so the later `toolResult` can append to it).
    tool_call_index: HashMap<String, usize>,
    parse_warning_count: u32,
    first_assistant_model: Option<String>,
    usage_events: Vec<UsageEvent>,
    child_session_ids: Vec<String>,
}

impl ParseState {
    fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    fn make_tool_call(
        &mut self,
        call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
        timestamp: Option<String>,
    ) {
        let arguments_raw = match arguments {
            serde_json::Value::String(raw) => raw.clone(),
            other => other.to_string(),
        };
        let arguments_value: Option<serde_json::Value> = match arguments {
            serde_json::Value::String(raw) => serde_json::from_str(raw).ok(),
            other => Some(other.clone()),
        };
        let metadata = build_tool_metadata(ToolCallFacts {
            provider: crate::models::Provider::Mcode,
            raw_name: name,
            input: arguments_value.as_ref(),
            call_id: Some(call_id),
            assistant_id: None,
        });
        let canonical_name = metadata.canonical_name.clone();
        let idx = self.messages.len();
        self.messages.push(Message {
            timestamp,
            tool_name: Some(canonical_name),
            tool_input: Some(arguments_raw),
            tool_metadata: Some(metadata),
            ..Message::new(MessageRole::Tool, String::new())
        });
        self.tool_call_index.insert(call_id.to_string(), idx);
    }

    fn attach_tool_result(
        &mut self,
        call_id: Option<&str>,
        tool_name_fallback: Option<&str>,
        text: &str,
        is_error: Option<bool>,
        timestamp: Option<String>,
        details: Option<&serde_json::Value>,
    ) {
        if let Some(child_id) = child_session_id_from_result(details, text)
            && !self.child_session_ids.iter().any(|id| id == &child_id)
        {
            self.child_session_ids.push(child_id);
        }
        let Some(call_id) = call_id else {
            // Orphan result: render as a system note so the timeline
            // surfaces the result without crashing the parser.
            self.push(Message::system(format!("[tool result]\n{text}")));
            return;
        };
        let Some(&idx) = self.tool_call_index.get(call_id) else {
            self.push(Message::system(format!("[tool result]\n{text}")));
            return;
        };
        let message = &mut self.messages[idx];
        message.content = text.to_string();
        if timestamp.is_some() {
            message.timestamp = timestamp;
        }
        // tool_name was already canonicalized at make_tool_call time. The
        // wire-side result carries a redundant raw_name; only use it to
        // disambiguate if we somehow missed the call.
        if message.tool_name.is_none() {
            message.tool_name = tool_name_fallback.map(str::to_string);
        }
        if let Some(metadata) = message.tool_metadata.as_mut() {
            // Prefer the typed `details` object (`sub_session_id`, status,
            // agent_name) so Agent metadata can resolve the child. Fall
            // back to the rendered text when details is absent.
            let text_value = serde_json::Value::String(text.to_string());
            let raw_result = match details {
                Some(value) if value.is_object() => value,
                _ => &text_value,
            };
            enrich_tool_metadata(
                metadata,
                ToolResultFacts {
                    raw_result: Some(raw_result),
                    is_error,
                    status: None,
                    artifact_path: None,
                    raw_output: Some(false),
                },
            );
        }
    }
}

fn apply(state: &mut ParseState, wire: WireLine) -> Result<(), String> {
    let role = match wire.message.role.as_str() {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "toolResult" | "tool_result" => MessageRole::Tool,
        "system" => MessageRole::System,
        other => return Err(format!("unknown mcode role '{other}'")),
    };

    let timestamp = wire
        .message
        .timestamp
        .and_then(crate::provider::util::epoch_ms_to_rfc3339);

    match role {
        MessageRole::User => apply_user(state, &wire.message, timestamp),
        MessageRole::System => {
            let mut m = Message::system(collect_text(&wire.message.content));
            m.timestamp = timestamp;
            state.push(m);
        }
        MessageRole::Assistant => {
            apply_assistant(state, &wire.message_id, &wire.message, timestamp)
        }
        MessageRole::Tool => {
            let text = collect_user_visible(&wire.message.content);
            state.attach_tool_result(
                wire.message.tool_call_id.as_deref(),
                wire.message.tool_name.as_deref(),
                &text,
                wire.message.is_error,
                timestamp,
                wire.message.details.as_ref(),
            );
        }
    }

    Ok(())
}

fn apply_user(state: &mut ParseState, message: &WireMessage, timestamp: Option<String>) {
    let full_text = collect_text(&message.content);
    let user_text = match &message.canonical_text_range {
        Some(range) => slice_char_range(&full_text, range.start_offset, range.end_offset)
            .trim()
            .to_string(),
        None => full_text,
    };
    let images = collect_image_markers(&message.content);
    let mut parts: Vec<String> = Vec::new();
    if !user_text.is_empty() {
        parts.push(user_text);
    }
    parts.extend(images);
    if parts.is_empty() {
        // Reminder-only injection with no user prompt and no image — not
        // a conversation turn.
        return;
    }
    let mut m = Message::user(parts.join("\n"));
    m.timestamp = timestamp;
    state.push(m);
}

fn apply_assistant(
    state: &mut ParseState,
    message_id: &str,
    message: &WireMessage,
    timestamp: Option<String>,
) {
    // Assistant content mixes thinking + text + tool calls in one wire
    // entry. Emit in the order the wire gives so the timeline matches
    // what the model produced (thinking, then prose, then the tool it
    // decided to run — not prose after the tool).
    let mut pending_text = String::new();
    let mut usage_attached = false;
    let token_usage = message.usage.as_ref().map(|u| u.clone().into_token_usage());

    for part in &message.content {
        match part {
            WireContent::Text { text } => {
                if !pending_text.is_empty() {
                    pending_text.push('\n');
                }
                pending_text.push_str(text);
            }
            WireContent::Thinking { thinking } => {
                flush_assistant_text(
                    state,
                    &mut pending_text,
                    &timestamp,
                    message.model.as_deref(),
                    token_usage.as_ref(),
                    &mut usage_attached,
                );
                if !thinking.trim().is_empty() {
                    let mut m = Message::system(format!("[thinking]\n{thinking}"));
                    m.timestamp = timestamp.clone();
                    state.push(m);
                }
            }
            WireContent::ToolCall {
                id,
                name,
                arguments,
            } => {
                flush_assistant_text(
                    state,
                    &mut pending_text,
                    &timestamp,
                    message.model.as_deref(),
                    token_usage.as_ref(),
                    &mut usage_attached,
                );
                state.make_tool_call(id, name, arguments, timestamp.clone());
            }
            WireContent::Image { data, mime_type } => {
                if let Some(marker) = image_marker(data.as_deref(), mime_type.as_deref()) {
                    if !pending_text.is_empty() {
                        pending_text.push('\n');
                    }
                    pending_text.push_str(&marker);
                }
            }
            WireContent::Unknown => {}
        }
    }

    flush_assistant_text(
        state,
        &mut pending_text,
        &timestamp,
        message.model.as_deref(),
        token_usage.as_ref(),
        &mut usage_attached,
    );

    if let Some(usage) = &message.usage {
        record_usage_event(state, message_id, message, timestamp.as_deref(), usage);
    }

    if state.first_assistant_model.is_none() {
        state.first_assistant_model = message
            .model
            .as_deref()
            .filter(|model| !model.is_empty())
            .map(str::to_string);
    }
}

fn flush_assistant_text(
    state: &mut ParseState,
    pending_text: &mut String,
    timestamp: &Option<String>,
    model: Option<&str>,
    token_usage: Option<&TokenUsage>,
    usage_attached: &mut bool,
) {
    if pending_text.is_empty() {
        return;
    }
    let mut m = Message::assistant(std::mem::take(pending_text));
    m.timestamp = timestamp.clone();
    m.model = model.filter(|name| !name.is_empty()).map(str::to_string);
    if !*usage_attached {
        m.token_usage = token_usage.cloned();
        *usage_attached = true;
    }
    state.push(m);
}

fn record_usage_event(
    state: &mut ParseState,
    message_id: &str,
    message: &WireMessage,
    timestamp: Option<&str>,
    usage: &WireUsage,
) {
    let Some(model) = message
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        log::debug!("skipping mcode usage event '{message_id}': missing model");
        return;
    };
    let Some(timestamp) = timestamp.filter(|ts| !ts.is_empty()) else {
        log::debug!("skipping mcode usage event '{message_id}': missing timestamp");
        return;
    };
    let cost_usd = usage
        .cost
        .as_ref()
        .map(|cost| cost.total)
        .filter(|total| *total > 0.0);
    state.usage_events.push(UsageEvent {
        timestamp: timestamp.to_string(),
        model: model.to_string(),
        turn_count: 1,
        input_tokens: u64::from(usage.input),
        output_tokens: u64::from(usage.output),
        cache_read_input_tokens: u64::from(usage.cache_read),
        cache_creation_input_tokens: u64::from(usage.cache_write),
        usage_hash: Some(message_id.to_string()),
        cost_usd,
    });
}

fn collect_text(content: &[WireContent]) -> String {
    let mut out = String::new();
    for part in content {
        if let WireContent::Text { text } = part {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    out
}

fn collect_user_visible(content: &[WireContent]) -> String {
    let mut out = String::new();
    for part in content {
        match part {
            WireContent::Text { text } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            WireContent::Image { data, mime_type } => {
                if let Some(marker) = image_marker(data.as_deref(), mime_type.as_deref()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&marker);
                }
            }
            _ => {}
        }
    }
    out
}

fn collect_image_markers(content: &[WireContent]) -> Vec<String> {
    content
        .iter()
        .filter_map(|part| match part {
            WireContent::Image { data, mime_type } => {
                image_marker(data.as_deref(), mime_type.as_deref())
            }
            _ => None,
        })
        .collect()
}

fn image_marker(data: Option<&str>, mime_type: Option<&str>) -> Option<String> {
    let data = data.map(str::trim).filter(|d| !d.is_empty())?;
    let mime = mime_type
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("image/png");
    Some(format!("[Image: source: data:{mime};base64,{data}]"))
}

/// Child session id from a `task` result. Prefer the typed
/// `details.sub_session_id`; fall back to the `<task_result session_id>`
/// attribute in the rendered text.
fn child_session_id_from_result(details: Option<&serde_json::Value>, text: &str) -> Option<String> {
    if let Some(details) = details {
        for key in [
            "sub_session_id",
            "childSessionId",
            "child_session_id",
            "session_id",
        ] {
            if let Some(id) = details
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|id| id.starts_with("mvs_"))
            {
                return Some(id.to_string());
            }
        }
    }
    session_id_from_task_result_text(text)
}

fn session_id_from_task_result_text(text: &str) -> Option<String> {
    let marker = "session_id=\"";
    let start = text.find(marker)? + marker.len();
    let rest = text.get(start..)?;
    let end = rest.find('"')?;
    let id = rest.get(..end)?.trim();
    id.starts_with("mvs_").then(|| id.to_string())
}

/// Slice `text` by Unicode scalar offsets (what mcode writes in
/// `canonicalTextRange`). Out-of-range ends clamp to the string.
fn slice_char_range(text: &str, start: usize, end: usize) -> &str {
    let start = start.min(end);
    let mut start_byte = text.len();
    let mut end_byte = text.len();
    for (index, (byte, _)) in text.char_indices().enumerate() {
        if index == start {
            start_byte = byte;
        }
        if index == end {
            end_byte = byte;
            break;
        }
    }
    if start_byte > end_byte {
        return "";
    }
    &text[start_byte..end_byte]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_messages(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("messages.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn parses_user_assistant_tool_assistant_full_wire() {
        let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"user","content":[{"type":"text","text":"list /tmp"}],"timestamp":1787049058794}}
{"message_id":"m2","turn_id":"t1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"run ls"},{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls -la /tmp"}}],"api":"anthropic-messages","provider":"minimax","model":"MiniMax-M3","usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"toolUse","timestamp":1787049058887}}
{"message_id":"m3","turn_id":"t1","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","content":[{"type":"text","text":"file1\nfile2"}],"isError":false,"timestamp":1787049060860}}
{"message_id":"m4","turn_id":"t1","message":{"role":"assistant","content":[{"type":"text","text":"here you go"}],"model":"MiniMax-M3","usage":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":3},"stopReason":"endTurn","timestamp":1787049060900}}
"#;
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        // Expected layout: user, [thinking], tool(call+result merged),
        // assistant — 4 message rows. The toolResult wire entry is folded
        // into the matching toolCall row rather than emitted separately.
        assert_eq!(parsed.messages.len(), 4);
        assert_eq!(parsed.parse_warning_count, 0);
        assert_eq!(parsed.first_assistant_model.as_deref(), Some("MiniMax-M3"));

        let thinking = &parsed.messages[1];
        assert_eq!(thinking.role, MessageRole::System);
        assert!(thinking.content.starts_with("[thinking]\n"));

        let tool_call = &parsed.messages[2];
        assert_eq!(tool_call.role, MessageRole::Tool);
        assert_eq!(tool_call.tool_name.as_deref(), Some("Bash"));
        assert!(tool_call.tool_input.as_deref().unwrap().contains("ls -la"));
        assert!(tool_call.tool_metadata.is_some());
        // The wire's toolResult row was folded into this row.
        assert_eq!(tool_call.content, "file1\nfile2");
        let metadata = tool_call.tool_metadata.as_ref().unwrap();
        assert_eq!(metadata.status.as_deref(), Some("success"));

        let final_assistant = &parsed.messages[3];
        assert_eq!(final_assistant.role, MessageRole::Assistant);
        assert_eq!(final_assistant.content, "here you go");
        assert_eq!(
            final_assistant.token_usage.as_ref().unwrap().input_tokens,
            1
        );
    }

    #[test]
    fn normalizes_stringified_tool_arguments() {
        let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"read","arguments":"{\"path\":\"/etc/hosts\"}"}],"timestamp":1}}
{"message_id":"m2","turn_id":"t1","message":{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"127.0.0.1 localhost"}],"isError":false,"timestamp":2}}
"#;
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        // 1 tool call + result merged = 1 row.
        assert_eq!(parsed.messages.len(), 1);
        let tool = &parsed.messages[0];
        let input = tool.tool_input.as_deref().unwrap();
        assert!(
            input.contains("/etc/hosts"),
            "raw stringified args preserved: {input}"
        );
    }

    #[test]
    fn skips_malformed_lines_and_keeps_the_rest() {
        let jsonl = "not json\n{\"message_id\":\"m1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1}}\n";
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.parse_warning_count, 1);
    }

    #[test]
    fn deduplicates_repeated_message_id() {
        let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"text","text":"a"}],"timestamp":1}}
{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"text","text":"a-dup"}],"timestamp":2}}
"#;
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        assert_eq!(parsed.messages.len(), 1, "duplicate message_id dropped");
        assert_eq!(parsed.messages[0].content, "a");
        assert_eq!(
            parsed.parse_warning_count, 0,
            "stream-reconciliation duplicates are expected, not warnings"
        );
    }

    #[test]
    fn orphan_tool_result_becomes_system_note() {
        // Result arrives without a matching call (e.g. session truncated).
        let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"toolResult","toolCallId":"unknown","toolName":"bash","content":[{"type":"text","text":"orphan"}],"isError":false,"timestamp":1}}
"#;
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::System);
        assert!(parsed.messages[0].content.contains("orphan"));
    }

    #[test]
    fn emits_assistant_parts_in_wire_order() {
        // Wire order is thinking, then prose, then the tool call.
        let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"look first"},{"type":"text","text":"here is /tmp:"},{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls"}}],"model":"MiniMax-M3","usage":{"input":10,"output":5,"cacheRead":2,"cacheWrite":0},"timestamp":1000}}
"#;
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        assert_eq!(parsed.messages.len(), 3);
        assert_eq!(parsed.messages[0].role, MessageRole::System);
        assert!(parsed.messages[0].content.starts_with("[thinking]\n"));
        assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
        assert_eq!(parsed.messages[1].content, "here is /tmp:");
        assert_eq!(
            parsed.messages[1]
                .token_usage
                .as_ref()
                .unwrap()
                .input_tokens,
            10
        );
        assert_eq!(parsed.messages[2].role, MessageRole::Tool);
        assert_eq!(parsed.usage_events.len(), 1);
        assert_eq!(parsed.usage_events[0].model, "MiniMax-M3");
        assert_eq!(parsed.usage_events[0].input_tokens, 10);
        assert_eq!(parsed.usage_events[0].cache_read_input_tokens, 2);
        assert_eq!(parsed.usage_events[0].usage_hash.as_deref(), Some("m1"));
    }

    #[test]
    fn uses_canonical_text_range_for_user_prompt() {
        let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>\nagent: Mavis\n</system-reminder>\n\nlist /tmp"}],"canonicalTextRange":{"startOffset":51,"endOffset":60},"timestamp":1}}
"#;
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content, "list /tmp");
        assert!(
            !parsed.messages[0].content.contains("<system-reminder>"),
            "injected reminder must not ride along on the user bubble"
        );
    }

    #[test]
    fn drops_reminder_only_user_turn() {
        let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>only</system-reminder>"}],"canonicalTextRange":{"startOffset":39,"endOffset":39},"timestamp":1}}
"#;
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        assert!(parsed.messages.is_empty());
    }

    #[test]
    fn embeds_user_image_as_data_uri_marker() {
        let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>x</system-reminder>see this"},{"type":"image","data":"AAAA","mimeType":"image/jpeg"}],"canonicalTextRange":{"startOffset":36,"endOffset":44},"timestamp":1}}
"#;
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(
            parsed.messages[0].content,
            "see this\n[Image: source: data:image/jpeg;base64,AAAA]"
        );
    }

    #[test]
    fn slice_char_range_handles_multibyte() {
        let text = "你好世界";
        assert_eq!(slice_char_range(text, 0, 2), "你好");
        assert_eq!(slice_char_range(text, 2, 4), "世界");
        assert_eq!(slice_char_range(text, 4, 4), "");
        assert_eq!(slice_char_range(text, 3, 99), "界");
    }

    #[test]
    fn task_result_exposes_child_session_id() {
        let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"task","arguments":{"description":"Inspect workspace","prompt":"list files","agent_name":"explore"}}],"model":"MiniMax-M3","timestamp":1}}
{"message_id":"m2","turn_id":"t1","message":{"role":"toolResult","toolCallId":"c1","toolName":"task","content":[{"type":"text","text":"<task_result task_id=\"bg_1\" session_id=\"mvs_child1\">\nrun_status: succeeded\nfinal_text:\nok\n</task_result>"}],"isError":false,"details":{"agent_name":"explore","status":"succeeded","task_id":"bg_1","sub_session_id":"mvs_child1","resolved_agent_name":"explore"},"timestamp":2}}
"#;
        let (_dir, path) = temp_messages(jsonl);
        let parsed = parse_messages_file(&path).expect("parse ok");
        assert_eq!(parsed.child_session_ids, vec!["mvs_child1".to_string()]);
        assert_eq!(parsed.messages.len(), 1);
        let tool = &parsed.messages[0];
        assert_eq!(tool.tool_name.as_deref(), Some("Agent"));
        let structured = tool
            .tool_metadata
            .as_ref()
            .and_then(|m| m.structured.as_ref())
            .expect("structured");
        assert_eq!(
            structured.get("agentId").and_then(|v| v.as_str()),
            Some("mvs_child1")
        );
        assert_eq!(
            structured.get("sub_session_id").and_then(|v| v.as_str()),
            Some("mvs_child1")
        );
    }
}
