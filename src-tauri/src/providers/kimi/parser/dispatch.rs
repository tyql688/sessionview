//! Accumulator and per-line dispatch — shared between full-file parse
//! and tail parse.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::models::{Message, MessageRole, Provider, TokenUsage, ToolMetadata};
use crate::tool_metadata::{
    attach_call_metadata, build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

use super::super::tools::{render_format_a_tool_output, render_format_b_tool_output};
use super::subagents::parse_agent_swarm_children;
use super::time_ms_to_parts;

// ---------------------------------------------------------------------------
// Accumulator: shared per-line state for full-file and tail parse.
// ---------------------------------------------------------------------------

/// Snapshot of accumulator state at turn boundaries, used to roll back
/// when a turn is cancelled.
struct TurnSnapshot {
    messages_len: usize,
    content_parts_len: usize,
    first_user_message: Option<String>,
}

pub(super) struct ScanAccum {
    pub(super) messages: Vec<Message>,
    pub(super) first_user_message: Option<String>,
    pub(super) first_time_secs: Option<i64>,
    pub(super) last_time_secs: Option<i64>,
    pub(super) content_parts: Vec<String>,
    /// toolCallId → message index, used to merge tool.result onto the
    /// matching tool.call message.
    call_id_map: HashMap<String, usize>,
    /// Fallback timestamp when individual lines do not carry `time`
    /// (migrated format): derived from `metadata.created_at`.
    fallback_time_secs: Option<i64>,
    fallback_time_rfc: Option<String>,
    /// Tracks the most recently observed model alias so usage records and
    /// assistant messages can be tagged correctly.
    pub(super) current_model: Option<String>,
    /// Message index of the first assistant text/think emitted in the
    /// current turn. `attach_usage` writes the turn's token totals here
    /// (rather than the trailing tool message) so the UI shows the
    /// model + cost on the actual assistant output. Reset to `None`
    /// after each `usage.record` / `step.end` is consumed.
    current_turn_assistant_idx: Option<usize>,
    pub(super) parse_warning_count: u32,
    /// Snapshot of state at the last turn.prompt, used to roll back on
    /// turn.cancel. protocol_version 1.4+ emits turn.cancel when the
    /// user interrupts mid-turn; everything accumulated since the turn
    /// started (tool calls, partial content, etc.) should be discarded.
    turn_snapshot: Option<TurnSnapshot>,
}

impl ScanAccum {
    pub(super) fn new() -> Self {
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
            turn_snapshot: None,
        }
    }

    /// Capture a snapshot of current state at turn boundary (turn.prompt).
    fn snapshot_turn(&mut self) {
        self.turn_snapshot = Some(TurnSnapshot {
            messages_len: self.messages.len(),
            content_parts_len: self.content_parts.len(),
            first_user_message: self.first_user_message.clone(),
        });
    }

    /// Roll back to the last turn snapshot, discarding everything
    /// accumulated since the turn started. Called on turn.cancel.
    fn rollback_turn(&mut self) {
        let Some(snap) = self.turn_snapshot.take() else {
            return;
        };
        self.messages.truncate(snap.messages_len);
        self.content_parts.truncate(snap.content_parts_len);
        // Rebuild call_id_map by keeping only entries whose message still exists.
        self.call_id_map.retain(|_k, v| *v < snap.messages_len);
        self.first_user_message = snap.first_user_message;
        self.current_turn_assistant_idx = None;
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
            timestamp: ts,
            ..Message::user(text.to_string())
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
            timestamp: ts,
            model: self.current_model.clone(),
            ..Message::assistant(text.to_string())
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
            timestamp: ts,
            model: self.current_model.clone(),
            ..Message::system(format!("[thinking]\n{text}"))
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
        event: Option<&Value>,
    ) {
        let mut metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Kimi,
            raw_name,
            input: args,
            call_id,
            assistant_id: None,
        });
        if let Some(event) = event {
            attach_kimi_call_metadata(&mut metadata, event);
        }
        let display_name = metadata.canonical_name.clone();
        let tool_input = args.map(|v| v.to_string());
        let idx = self.messages.len();
        if let Some(cid) = call_id {
            self.call_id_map.insert(cid.to_string(), idx);
        }
        self.messages.push(Message {
            timestamp: ts,
            tool_name: Some(display_name),
            tool_input,
            model: self.current_model.clone(),
            tool_metadata: Some(metadata),
            ..Message::new(MessageRole::Tool, String::new())
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
                    attach_agent_swarm_children(meta, &rendered_output);
                }
                self.messages[idx].content = rendered_output;
                return;
            }
        }
        self.messages.push(Message {
            timestamp: ts,
            ..Message::new(MessageRole::Tool, rendered_output)
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

    pub(super) fn note_warning(&mut self) {
        self.parse_warning_count = self.parse_warning_count.saturating_add(1);
    }
}

fn attach_agent_swarm_children(metadata: &mut ToolMetadata, rendered_output: &str) {
    if metadata.raw_name != "AgentSwarm" {
        return;
    }
    let children = parse_agent_swarm_children(rendered_output);
    if children.is_empty() {
        return;
    }

    let mut structured = metadata
        .structured
        .take()
        .unwrap_or_else(|| Value::Object(Default::default()));
    if !structured.is_object() {
        log::warn!("Kimi AgentSwarm structured metadata was not an object; skipping child links");
        metadata.structured = Some(structured);
        return;
    }
    if let Some(obj) = structured.as_object_mut() {
        obj.insert(
            "childConversationIds".to_string(),
            Value::Array(
                children
                    .iter()
                    .map(|child| json!(child.agent_id.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "childPrompts".to_string(),
            Value::Array(
                children
                    .iter()
                    .map(|child| json!(child.prompt.clone()))
                    .collect(),
            ),
        );
    }
    metadata.structured = Some(structured);
}

#[cfg(test)]
// These rollback tests stay next to ScanAccum's private state instead of the
// file bottom; moving them would make the fixture setup harder to read.
#[allow(clippy::items_after_test_module)]
mod turn_cancel_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn turn_cancel_discards_accumulated_content() {
        let mut accum = ScanAccum::new();
        // Simulate a turn that gets cancelled
        dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {"type": "content.part", "part": {"type": "text", "text": "partial response..."}},
                "time": 1001
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {"type": "tool.call", "toolCallId": "tc_1", "name": "Read", "args": {"path": "a.txt"}},
                "time": 1002
            }),
        );
        // User cancels
        dispatch_line(&mut accum, &json!({"type": "turn.cancel", "time": 1003}));

        assert_eq!(accum.messages.len(), 0);
        assert_eq!(accum.content_parts.len(), 0);
        assert_eq!(accum.call_id_map.len(), 0);
    }

    #[test]
    fn turn_cancel_preserves_previous_turn() {
        let mut accum = ScanAccum::new();
        // First turn completes normally
        dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_message",
                "message": {"role": "user", "content": [{"type": "text", "text": "hello"}], "toolCalls": [], "origin": {"kind": "user"}},
                "time": 1001
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {"type": "content.part", "part": {"type": "text", "text": "Hi!"}},
                "time": 1002
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "usage.record",
                "model": "kimi-test",
                "usage": {"inputOther": 10, "output": 5, "inputCacheRead": 0, "inputCacheCreation": 0},
                "time": 1003
            }),
        );

        // Second turn starts then gets cancelled
        dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 2000}));
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {"type": "content.part", "part": {"type": "text", "text": "partial..."}},
                "time": 2001
            }),
        );
        dispatch_line(&mut accum, &json!({"type": "turn.cancel", "time": 2002}));

        // Should still have the first turn's messages
        assert_eq!(accum.messages.len(), 2); // user + assistant
        assert_eq!(accum.messages[0].role, MessageRole::User);
        assert_eq!(accum.messages[0].content, "hello");
        assert_eq!(accum.messages[1].role, MessageRole::Assistant);
        assert_eq!(accum.messages[1].content, "Hi!");
    }

    #[test]
    fn turn_prompt_without_cancel_keeps_content() {
        let mut accum = ScanAccum::new();
        dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_message",
                "message": {"role": "user", "content": [{"type": "text", "text": "query"}], "toolCalls": [], "origin": {"kind": "user"}},
                "time": 1001
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {"type": "content.part", "part": {"type": "text", "text": "answer"}},
                "time": 1002
            }),
        );
        // No cancel — content should be kept
        assert_eq!(accum.messages.len(), 2);
        assert_eq!(accum.messages[1].content, "answer");
    }

    #[test]
    fn step_end_usage_fallback_when_no_usage_record() {
        let mut accum = ScanAccum::new();
        dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_message",
                "message": {"role": "user", "content": [{"type": "text", "text": "hi"}], "toolCalls": [], "origin": {"kind": "user"}},
                "time": 1001
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {"type": "content.part", "part": {"type": "text", "text": "Hello!"}},
                "time": 1002
            }),
        );
        // step.end with usage but no usage.record
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {
                    "type": "step.end",
                    "usage": {"inputOther": 100, "output": 50, "inputCacheRead": 200, "inputCacheCreation": 0}
                },
                "time": 1003
            }),
        );

        let usage = accum.messages[1]
            .token_usage
            .as_ref()
            .expect("usage attached");
        assert_eq!(usage.input_tokens, 300); // 100 + 200
        assert_eq!(usage.output_tokens, 50);
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

pub(super) fn dispatch_line(accum: &mut ScanAccum, entry: &Value) {
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
        "context.append_message" => handle_migrated_line(accum, entry, line_time_ms),

        // ---- Format B: streaming events ----
        "context.append_loop_event" => handle_native_event(accum, entry, line_time_ms),

        "usage.record" => {
            let _ = accum.note_time(line_time_ms);
            let model = entry.get("model").and_then(|v| v.as_str());
            if let Some(u) = entry.get("usage") {
                if let Some(usage) = parse_usage(u) {
                    accum.attach_usage(usage, model);
                }
            }
        }

        // ---- Turn boundaries (protocol_version 1.4+) ----
        "turn.prompt" => {
            // A new turn is starting — snapshot state so we can roll back
            // if the turn is cancelled.
            let _ = accum.note_time(line_time_ms);
            accum.snapshot_turn();
        }

        "turn.cancel" => {
            // User cancelled the current turn (e.g. Ctrl+C, interrupt).
            // Discard everything accumulated since the turn started.
            let _ = accum.note_time(line_time_ms);
            accum.rollback_turn();
        }

        // ---- Events that produce no visible transcript content ----
        "tools.set_active_tools"
        | "tools.update_store"
        | "plan_mode.enter"
        | "plan_mode.cancel"
        | "permission.set_mode"
        | "permission.record_approval_result" => {
            // These are UI/state bookkeeping events; they don't carry
            // messages we want in the transcript. Soak up the time so
            // first/last timestamps still span the whole file.
            let _ = accum.note_time(line_time_ms);
        }

        _ => {}
    }
}

/// Handle a migrated (Format A) `context.append_message` line: user
/// prompts, assistant think/text + toolCalls, and tool results.
fn handle_migrated_line(accum: &mut ScanAccum, entry: &Value, line_time_ms: Option<i64>) {
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
                    accum.push_tool_call(name, id, arg_value.as_ref(), ts.clone(), None);
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

/// Handle a native (Format B) `context.append_loop_event` line:
/// streaming `content.part`, `tool.call`, `tool.result`, and step
/// bookkeeping.
fn handle_native_event(accum: &mut ScanAccum, entry: &Value, line_time_ms: Option<i64>) {
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
            accum.push_tool_call(name, id, args, ts, Some(event));
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
            // step.end. Prefer `usage.record` when present, but
            // fall back to `step.end.usage` when the record is
            // missing (older protocol versions or edge cases).
            let model = event
                .get("usage")
                .and_then(|u| u.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| accum.current_model.clone());
            let model_ref = model.as_deref();
            if let Some(u) = event.get("usage") {
                if let Some(usage) = parse_usage(u) {
                    accum.attach_usage(usage, model_ref);
                }
            }
        }
        _ => {}
    }
}

fn attach_kimi_call_metadata(metadata: &mut ToolMetadata, event: &Value) {
    let description = event.get("description").and_then(|v| v.as_str());
    let display = event.get("display");
    let mut ids = Vec::new();
    for (field, key) in [
        ("uuid", "kimi_uuid"),
        ("turnId", "turn_id"),
        ("stepUuid", "step_uuid"),
    ] {
        if let Some(value) = event.get(field).and_then(value_to_id_string) {
            ids.push((key, value));
        }
    }
    if let Some(value) = event.get("step").and_then(value_to_id_string) {
        ids.push(("step", value));
    }
    attach_call_metadata(metadata, description, display, ids);
}

fn value_to_id_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
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
