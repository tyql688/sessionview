//! Accumulator and per-line dispatch — shared between full-file parse
//! and tail parse.

use serde_json::{Value, json};

use crate::models::{Message, MessageRole, Provider, TokenUsage, ToolMetadata};
use crate::provider::UsageEvent;
use crate::provider_utils::ToolCallPairer;
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, attach_call_metadata, build_tool_metadata, enrich_tool_metadata,
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
    call_id_map: ToolCallPairer,
    /// Fallback timestamp when individual lines do not carry `time`
    /// (migrated format): derived from `metadata.created_at`.
    fallback_time_secs: Option<i64>,
    fallback_time_rfc: Option<String>,
    /// Tracks the most recently observed model alias so usage records and
    /// assistant messages can be tagged correctly.
    pub(super) current_model: Option<String>,
    pub(super) usage_events: Vec<UsageEvent>,
    /// Message index that owns usage for the current turn.
    current_turn_usage_idx: Option<usize>,
    current_turn_usage_event_idx: Option<usize>,
    pub(super) parse_warning_count: u32,
    /// Snapshot of state at the last turn.prompt, used to roll back on
    /// turn.cancel. protocol_version 1.4+ emits turn.cancel when the
    /// user interrupts mid-turn; partial transcript content is discarded.
    turn_snapshot: Option<TurnSnapshot>,
    pub(super) cancel_without_snapshot: bool,
    /// Tail windows legitimately start mid-turn, so usage records that
    /// cannot resolve a model or anchor there are expected — don't count
    /// them toward the parse-warning badge.
    pub(super) is_tail: bool,
}

impl ScanAccum {
    pub(super) fn new() -> Self {
        Self {
            messages: Vec::new(),
            first_user_message: None,
            first_time_secs: None,
            last_time_secs: None,
            content_parts: Vec::new(),
            call_id_map: ToolCallPairer::default(),
            fallback_time_secs: None,
            fallback_time_rfc: None,
            current_model: None,
            usage_events: Vec::new(),
            current_turn_usage_idx: None,
            current_turn_usage_event_idx: None,
            parse_warning_count: 0,
            turn_snapshot: None,
            cancel_without_snapshot: false,
            is_tail: false,
        }
    }

    /// Capture a snapshot of current state at turn boundary (turn.prompt).
    fn snapshot_turn(&mut self) {
        self.turn_snapshot = Some(TurnSnapshot {
            messages_len: self.messages.len(),
            content_parts_len: self.content_parts.len(),
            first_user_message: self.first_user_message.clone(),
        });
        self.current_turn_usage_idx = None;
        self.current_turn_usage_event_idx = None;
    }

    /// Roll back to the last turn snapshot, discarding everything
    /// accumulated since the turn started. Called on turn.cancel.
    fn rollback_turn(&mut self) {
        let Some(snap) = self.turn_snapshot.take() else {
            self.cancel_without_snapshot = true;
            return;
        };
        self.messages.truncate(snap.messages_len);
        self.content_parts.truncate(snap.content_parts_len);
        // Rebuild call_id_map by keeping only entries whose message still exists.
        self.call_id_map.retain_below(snap.messages_len);
        self.first_user_message = snap.first_user_message;
        self.current_turn_usage_idx = None;
        self.current_turn_usage_event_idx = None;
    }

    fn note_time(&mut self, ms: Option<i64>) -> Option<String> {
        let (secs, rfc) = match ms {
            Some(ms) => {
                let Some(parts) = time_ms_to_parts(ms) else {
                    log::warn!("skipping out-of-range Kimi timestamp {ms}");
                    self.note_warning();
                    return None;
                };
                parts
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
            self.current_turn_usage_idx = None;
            self.current_turn_usage_event_idx = None;
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
        // The turn's usage belongs on its assistant text. If a step fallback
        // already landed it on a tool message, move it here so exactly one
        // message per turn carries the usage.
        let tool_owner = self
            .current_turn_usage_idx
            .and_then(|index| self.messages.get_mut(index))
            .filter(|message| message.role == MessageRole::Tool);
        let owner_is_tool = tool_owner.is_some();
        let moved_usage = tool_owner.and_then(|owner| owner.token_usage.take());
        if self.current_turn_usage_idx.is_none() || owner_is_tool {
            self.current_turn_usage_idx = Some(self.messages.len());
        }
        self.messages.push(Message {
            timestamp: ts,
            model: self.current_model.clone(),
            token_usage: moved_usage,
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
        if self.current_turn_usage_idx.is_none() {
            self.current_turn_usage_idx = Some(self.messages.len());
        }
        self.call_id_map.register(call_id, self.messages.len());
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
        if let Some(message) = self.call_id_map.message_mut(call_id, &mut self.messages) {
            if let Some(meta) = message.tool_metadata.as_mut() {
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
            message.content = rendered_output;
            return;
        }
        if self.current_turn_usage_idx.is_none() {
            self.current_turn_usage_idx = Some(self.messages.len());
        }
        self.messages.push(Message {
            timestamp: ts,
            ..Message::new(MessageRole::Tool, rendered_output)
        });
    }

    /// Attach token totals to the turn's first assistant text, or its
    /// trailing tool for tool-only turns. A step fallback keeps the target
    /// so the authoritative turn record overwrites it instead of duplicating it.
    fn attach_usage(&mut self, usage: TokenUsage, model: Option<&str>, finish_turn: bool) {
        let target_idx = if finish_turn {
            self.current_turn_usage_idx.take()
        } else {
            self.current_turn_usage_idx
        };
        let Some(idx) = target_idx else {
            // Usage record with no anchor message. Session totals still
            // come from usage_events; only the per-message badge is lost.
            if !self.is_tail {
                log::warn!(
                    "Kimi usage.record (output={}, input_other={}) had no assistant/tool message to attach to",
                    usage.output_tokens,
                    usage.input_tokens
                );
                self.note_warning();
            }
            return;
        };
        let Some(msg) = self.messages.get_mut(idx) else {
            return;
        };
        if !finish_turn {
            self.current_turn_usage_idx = Some(idx);
        }
        msg.token_usage = Some(usage);
        if let Some(m) = model {
            msg.model = Some(m.to_string());
        } else if msg.model.is_none() {
            msg.model = self.current_model.clone();
        }
    }

    /// Fold a usage record into the current turn's event. Per-step usages
    /// (`finish_turn == false`) accumulate, so a turn that never sees its
    /// closing record (crash, live session) still totals every step; the
    /// closing `usage.record` replaces the accumulated value with the
    /// authoritative turn total. Returns the usage to attach to the turn's
    /// owner message.
    fn record_usage_event(
        &mut self,
        usage: &TokenUsage,
        timestamp: Option<String>,
        model: Option<&str>,
        finish_turn: bool,
    ) -> Option<TokenUsage> {
        let (Some(timestamp), Some(model)) = (timestamp, model) else {
            if !self.is_tail {
                log::warn!("skipping Kimi usage record without timestamp or model");
                self.note_warning();
            }
            return None;
        };
        let mut event = UsageEvent {
            timestamp,
            model: model.to_string(),
            input_tokens: u64::from(usage.input_tokens),
            output_tokens: u64::from(usage.output_tokens),
            cache_read_input_tokens: u64::from(usage.cache_read_input_tokens),
            cache_creation_input_tokens: u64::from(usage.cache_creation_input_tokens),
            usage_hash: None,
        };
        if let Some(index) = self.current_turn_usage_event_idx.take() {
            if !finish_turn {
                let prev = &self.usage_events[index];
                event.input_tokens += prev.input_tokens;
                event.output_tokens += prev.output_tokens;
                event.cache_read_input_tokens += prev.cache_read_input_tokens;
                event.cache_creation_input_tokens += prev.cache_creation_input_tokens;
                self.current_turn_usage_event_idx = Some(index);
            }
            self.usage_events[index] = event;
        } else {
            self.usage_events.push(event);
            if !finish_turn {
                self.current_turn_usage_event_idx = Some(self.usage_events.len() - 1);
            }
        }
        let attached = self
            .current_turn_usage_event_idx
            .map_or(usage.clone(), |index| {
                let event = &self.usage_events[index];
                let clamp = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
                TokenUsage {
                    input_tokens: clamp(event.input_tokens),
                    output_tokens: clamp(event.output_tokens),
                    cache_read_input_tokens: clamp(event.cache_read_input_tokens),
                    cache_creation_input_tokens: clamp(event.cache_creation_input_tokens),
                }
            });
        Some(attached)
    }

    pub(super) fn note_warning(&mut self) {
        self.note_warnings(1);
    }

    pub(super) fn note_warnings(&mut self, count: u32) {
        self.parse_warning_count = self.parse_warning_count.saturating_add(count);
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
    fn turn_cancel_discards_content_but_preserves_usage() {
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
        dispatch_line(
            &mut accum,
            &json!({
                "type": "usage.record",
                "model": "kimi-test",
                "usage": {"inputOther": 10, "output": 5},
                "usageScope": "turn",
                "time": 1002
            }),
        );
        // User cancels
        dispatch_line(&mut accum, &json!({"type": "turn.cancel", "time": 1003}));

        assert_eq!(accum.messages.len(), 0);
        assert_eq!(accum.content_parts.len(), 0);
        assert_eq!(accum.usage_events.len(), 1);
        assert_eq!(accum.usage_events[0].input_tokens, 10);
        assert_eq!(accum.usage_events[0].output_tokens, 5);
        assert_eq!(accum.call_id_map.index_of(Some("tc_1")), None);
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
                "usageScope": "turn",
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
                    "usage": {"model": "kimi-test", "inputOther": 100, "output": 50, "inputCacheRead": 200, "inputCacheCreation": 0}
                },
                "time": 1003
            }),
        );

        let usage = accum.messages[1]
            .token_usage
            .as_ref()
            .expect("usage attached");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(accum.usage_events.len(), 1);
        assert_eq!(accum.usage_events[0].input_tokens, 100);
    }

    #[test]
    fn turn_usage_overwrites_step_fallback_once() {
        let mut accum = ScanAccum::new();
        accum.push_assistant_text("answer", Some("2026-07-19T00:00:00Z".into()));
        accum.push_tool_call("Read", Some("call-1"), None, None, None);
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {
                    "type": "step.end",
                    "usage": {"inputOther": 10, "output": 5, "inputCacheRead": 20}
                }
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "usage.record",
                "usageScope": "turn",
                "model": "kimi-test",
                "usage": {"inputOther": 12, "output": 6, "inputCacheRead": 24},
                "time": 1001
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "usage.record",
                "usageScope": "session",
                "model": "kimi-test",
                "usage": {"inputOther": 120, "output": 60, "inputCacheRead": 240}
            }),
        );

        assert_eq!(
            accum
                .messages
                .iter()
                .filter(|message| message.token_usage.is_some())
                .count(),
            1
        );
        let usage = accum.messages[0].token_usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.cache_read_input_tokens, 24);
        assert_eq!(accum.usage_events.len(), 1);
        assert_eq!(accum.usage_events[0].input_tokens, 12);
    }

    #[test]
    fn step_usage_accumulates_when_turn_record_never_arrives() {
        let mut accum = ScanAccum::new();
        accum.current_model = Some("kimi-test".into());
        accum.push_assistant_text("answer", Some("2026-07-19T00:00:00Z".into()));
        for step in 0..2 {
            dispatch_line(
                &mut accum,
                &json!({
                    "type": "context.append_loop_event",
                    "event": {
                        "type": "step.end",
                        "usage": {"inputOther": 100, "output": 50, "inputCacheRead": 30}
                    },
                    "time": 1001 + step
                }),
            );
        }

        assert_eq!(accum.usage_events.len(), 1);
        assert_eq!(accum.usage_events[0].input_tokens, 200);
        assert_eq!(accum.usage_events[0].output_tokens, 100);
        assert_eq!(accum.usage_events[0].cache_read_input_tokens, 60);
        let usage = accum.messages[0].token_usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 100);
    }

    #[test]
    fn assistant_text_takes_over_usage_from_tool_owner() {
        let mut accum = ScanAccum::new();
        accum.current_model = Some("kimi-test".into());
        accum.push_tool_call(
            "Read",
            Some("call-1"),
            None,
            Some("2026-07-19T00:00:00Z".into()),
            None,
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {
                    "type": "step.end",
                    "usage": {"inputOther": 10, "output": 5}
                },
                "time": 1001
            }),
        );
        accum.push_assistant_text("answer", Some("2026-07-19T00:00:02Z".into()));
        dispatch_line(
            &mut accum,
            &json!({
                "type": "usage.record",
                "usageScope": "turn",
                "model": "kimi-test",
                "usage": {"inputOther": 12, "output": 6},
                "time": 1002
            }),
        );

        let carriers: Vec<usize> = accum
            .messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| message.token_usage.is_some().then_some(index))
            .collect();
        assert_eq!(carriers, vec![1], "only the assistant text carries usage");
        let usage = accum.messages[1].token_usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(accum.usage_events.len(), 1);
        assert_eq!(accum.usage_events[0].input_tokens, 12);
    }

    #[test]
    fn real_user_boundary_finalizes_step_usage_fallback() {
        let mut accum = ScanAccum::new();
        for (text, time) in [("first", 1000), ("second", 2000)] {
            dispatch_line(
                &mut accum,
                &json!({
                    "type": "context.append_message",
                    "message": {
                        "role": "user",
                        "content": [{"type": "text", "text": text}]
                    },
                    "time": time
                }),
            );
            accum.push_assistant_text("answer", Some("2026-07-19T00:00:00Z".into()));
            dispatch_line(
                &mut accum,
                &json!({
                    "type": "context.append_loop_event",
                    "event": {
                        "type": "step.end",
                        "usage": {"model": "kimi-test", "inputOther": 10, "output": 5}
                    },
                    "time": time + 1
                }),
            );
        }

        assert_eq!(accum.usage_events.len(), 2);
    }

    #[test]
    fn usage_without_current_output_does_not_overwrite_previous_turn() {
        let mut accum = ScanAccum::new();
        accum.current_model = Some("kimi-test".into());
        accum.push_assistant_text("first", Some("2026-07-19T00:00:00Z".into()));
        dispatch_line(
            &mut accum,
            &json!({"type":"usage.record","usageScope":"turn","model":"kimi-test","usage":{"output":1},"time":1000}),
        );
        accum.push_user_text("next", Some("2026-07-19T00:00:01Z".into()), true);
        dispatch_line(
            &mut accum,
            &json!({"type":"usage.record","usageScope":"turn","model":"kimi-test","usage":{"output":2},"time":2000}),
        );

        assert_eq!(
            accum.messages[0]
                .token_usage
                .as_ref()
                .unwrap()
                .output_tokens,
            1
        );
        assert!(accum.messages[1].token_usage.is_none());
        assert_eq!(accum.usage_events.len(), 2);
        assert_eq!(accum.parse_warning_count, 1);
    }

    #[test]
    fn invalid_metadata_timestamp_is_reported() {
        let mut accum = ScanAccum::new();

        dispatch_line(
            &mut accum,
            &json!({"type": "metadata", "created_at": i64::MAX}),
        );

        assert_eq!(accum.parse_warning_count, 1);
        assert_eq!(accum.first_time_secs, None);
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
                if let Some((secs, rfc)) = time_ms_to_parts(ms) {
                    accum.fallback_time_secs = Some(secs);
                    accum.fallback_time_rfc = Some(rfc);
                    if accum.first_time_secs.is_none() {
                        accum.first_time_secs = Some(secs);
                    }
                    accum.last_time_secs = Some(secs);
                } else {
                    log::warn!("skipping out-of-range Kimi metadata timestamp {ms}");
                    accum.note_warning();
                }
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
            let timestamp = accum.note_time(line_time_ms);
            if entry.get("usageScope").and_then(Value::as_str) != Some("turn") {
                return;
            }
            let model = entry
                .get("model")
                .and_then(|v| v.as_str())
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .or_else(|| accum.current_model.clone());
            if let Some(u) = entry.get("usage")
                && let Some(usage) = parse_usage(u)
                && let Some(total) =
                    accum.record_usage_event(&usage, timestamp, model.as_deref(), true)
            {
                accum.attach_usage(total, model.as_deref(), true);
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
            if let Some(u) = event.get("usage")
                && let Some(usage) = parse_usage(u)
                && let Some(total) = accum.record_usage_event(&usage, ts.clone(), model_ref, false)
            {
                accum.attach_usage(total, model_ref, false);
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
    let usage = crate::provider_utils::token_usage_from(
        value,
        &crate::provider_utils::UsageKeys {
            input: &["inputOther"],
            output: &["output"],
            cache_read: &["inputCacheRead"],
            cache_write: &["inputCacheCreation"],
        },
    )?;
    if usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_read_input_tokens == 0
        && usage.cache_creation_input_tokens == 0
    {
        return None;
    }
    Some(usage)
}
