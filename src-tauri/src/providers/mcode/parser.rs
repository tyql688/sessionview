//! Wire-format parser for MiniMax Code (mcode) `messages.jsonl`.
//!
//! MiniMax Code stores one session per directory under
//! `{dataDir}/v2/sessions/YYYY/MM/DD/HH-MM-SS-mmm-session_<b64(sessionId)>/`.
//! Session metadata lives in `sqlite/runtime-state.sqlite` (table
//! `local_runtime_sessions`). `messages.jsonl` is the canonical ordered wire
//! used for transcript parsing; the runtime's SQLite message mirror is not a
//! substitute for that source contract.
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
//!   `toolCallId`. Orphan results surface as standalone Tool messages. A `task`
//!   result's `details.sub_session_id` (or `<task_result session_id>`)
//!   is the hidden child Agent id.

use std::collections::HashMap;
use std::path::Path;

use crate::models::{Message, MessageRole, TokenUsage};
use crate::provider::UsageEvent;
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

use super::types::{KnownWireContent, WireContent, WireLine, WireMessage, WireUsage};

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

        if let Err(error) = apply(&mut state, wire, path, line_no + 1) {
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

#[derive(Clone, Copy)]
struct RecordContext<'a> {
    path: &'a Path,
    line_no: usize,
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
        context: RecordContext<'_>,
    ) {
        if let Some(child_id) = child_session_id_from_result(details, text)
            && !self.child_session_ids.iter().any(|id| id == &child_id)
        {
            self.child_session_ids.push(child_id);
        }
        let Some(call_id) = call_id else {
            self.push_orphan_tool_result(
                None,
                tool_name_fallback,
                text,
                is_error,
                timestamp,
                details,
                context,
            );
            return;
        };
        let Some(&idx) = self.tool_call_index.get(call_id) else {
            self.push_orphan_tool_result(
                Some(call_id),
                tool_name_fallback,
                text,
                is_error,
                timestamp,
                details,
                context,
            );
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

    fn push_orphan_tool_result(
        &mut self,
        call_id: Option<&str>,
        raw_name: Option<&str>,
        text: &str,
        is_error: Option<bool>,
        timestamp: Option<String>,
        details: Option<&serde_json::Value>,
        context: RecordContext<'_>,
    ) {
        log::warn!(
            "mcode tool result has no matching call at line {} in '{}'",
            context.line_no,
            context.path.display()
        );
        self.parse_warning_count = self.parse_warning_count.saturating_add(1);

        let mut message = Message {
            timestamp,
            ..Message::new(MessageRole::Tool, text.to_string())
        };
        if let Some(raw_name) = raw_name.map(str::trim).filter(|name| !name.is_empty()) {
            let mut metadata = build_tool_metadata(ToolCallFacts {
                provider: crate::models::Provider::Mcode,
                raw_name,
                input: None,
                call_id,
                assistant_id: None,
            });
            let text_value = serde_json::Value::String(text.to_string());
            let raw_result = match details {
                Some(value) if value.is_object() => value,
                _ => &text_value,
            };
            enrich_tool_metadata(
                &mut metadata,
                ToolResultFacts {
                    raw_result: Some(raw_result),
                    is_error,
                    raw_output: Some(false),
                    ..ToolResultFacts::default()
                },
            );
            message.tool_name = Some(metadata.canonical_name.clone());
            message.tool_metadata = Some(metadata);
        }
        self.push(message);
    }
}

fn apply(
    state: &mut ParseState,
    wire: WireLine,
    path: &Path,
    line_no: usize,
) -> Result<(), String> {
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

    if role != MessageRole::System {
        warn_unknown_content(state, &wire.message.content, path, line_no);
    }

    match role {
        MessageRole::User => apply_user(state, &wire.message, timestamp, path, line_no),
        MessageRole::System => {
            let mut m = Message::system(collect_text(&wire.message.content));
            m.timestamp = timestamp;
            state.push(m);
        }
        MessageRole::Assistant => apply_assistant(
            state,
            &wire.message_id,
            &wire.message,
            timestamp,
            path,
            line_no,
        ),
        MessageRole::Tool => {
            let text = collect_user_visible(state, &wire.message.content, path, line_no);
            state.attach_tool_result(
                wire.message.tool_call_id.as_deref(),
                wire.message.tool_name.as_deref(),
                &text,
                wire.message.is_error,
                timestamp,
                wire.message.details.as_ref(),
                RecordContext { path, line_no },
            );
        }
    }

    Ok(())
}

fn apply_user(
    state: &mut ParseState,
    message: &WireMessage,
    timestamp: Option<String>,
    path: &Path,
    line_no: usize,
) {
    let full_text = collect_text(&message.content);
    let user_text = match &message.canonical_text_range {
        Some(range) => slice_char_range(&full_text, range.start_offset, range.end_offset)
            .trim()
            .to_string(),
        None => full_text,
    };
    let images = collect_image_markers(state, &message.content, path, line_no);
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
    path: &Path,
    line_no: usize,
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
            WireContent::Known(KnownWireContent::Text { text }) => {
                if !pending_text.is_empty() {
                    pending_text.push('\n');
                }
                pending_text.push_str(text);
            }
            WireContent::Known(KnownWireContent::Thinking { thinking }) => {
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
            WireContent::Known(KnownWireContent::ToolCall {
                id,
                name,
                arguments,
            }) => {
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
            WireContent::Known(KnownWireContent::Image { data, mime_type }) => {
                if let Some(marker) = resolve_image_marker(
                    state,
                    data.as_deref(),
                    mime_type.as_deref(),
                    path,
                    line_no,
                ) {
                    if !pending_text.is_empty() {
                        pending_text.push('\n');
                    }
                    pending_text.push_str(&marker);
                }
            }
            WireContent::Unknown(_) => {}
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
        record_usage_event(
            state,
            message_id,
            message,
            timestamp.as_deref(),
            usage,
            path,
            line_no,
        );
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
    path: &Path,
    line_no: usize,
) {
    let model = message
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or_default();
    if model.is_empty() {
        log::warn!(
            "mcode usage has no model at line {line_no} in '{}'",
            path.display()
        );
        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
    }
    let timestamp = timestamp.filter(|ts| !ts.is_empty()).unwrap_or_default();
    if timestamp.is_empty() {
        log::warn!(
            "mcode usage has no timestamp at line {line_no} in '{}'",
            path.display()
        );
        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
    }
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
        if let WireContent::Known(KnownWireContent::Text { text }) = part {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    out
}

fn collect_user_visible(
    state: &mut ParseState,
    content: &[WireContent],
    path: &Path,
    line_no: usize,
) -> String {
    let mut out = String::new();
    for part in content {
        match part {
            WireContent::Known(KnownWireContent::Text { text }) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            WireContent::Known(KnownWireContent::Image { data, mime_type }) => {
                if let Some(marker) = resolve_image_marker(
                    state,
                    data.as_deref(),
                    mime_type.as_deref(),
                    path,
                    line_no,
                ) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&marker);
                }
            }
            WireContent::Unknown(value) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&serde_json::to_string(value).unwrap_or_default());
            }
            _ => {}
        }
    }
    out
}

fn collect_image_markers(
    state: &mut ParseState,
    content: &[WireContent],
    path: &Path,
    line_no: usize,
) -> Vec<String> {
    content
        .iter()
        .filter_map(|part| match part {
            WireContent::Known(KnownWireContent::Image { data, mime_type }) => {
                resolve_image_marker(state, data.as_deref(), mime_type.as_deref(), path, line_no)
            }
            _ => None,
        })
        .collect()
}

fn image_marker(data: Option<&str>, mime_type: Option<&str>) -> Result<String, &'static str> {
    let data = data
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .ok_or("image block has no base64 payload")?;
    let mime = mime_type
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or("image block has no MIME type")?;
    Ok(format!("[Image: source: data:{mime};base64,{data}]"))
}

fn resolve_image_marker(
    state: &mut ParseState,
    data: Option<&str>,
    mime_type: Option<&str>,
    path: &Path,
    line_no: usize,
) -> Option<String> {
    match image_marker(data, mime_type) {
        Ok(marker) => Some(marker),
        Err(reason) => {
            log::warn!(
                "malformed mcode image at line {line_no} in '{}': {reason}",
                path.display()
            );
            state.parse_warning_count = state.parse_warning_count.saturating_add(1);
            None
        }
    }
}

fn warn_unknown_content(
    state: &mut ParseState,
    content: &[WireContent],
    path: &Path,
    line_no: usize,
) {
    for part in content {
        let WireContent::Unknown(value) = part else {
            continue;
        };
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing>");
        log::warn!(
            "unknown mcode content type '{kind}' at line {line_no} in '{}'",
            path.display()
        );
        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
    }
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
mod tests;
