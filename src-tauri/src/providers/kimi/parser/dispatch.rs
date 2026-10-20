//! Accumulator and per-line dispatch — shared between full-file parse
//! and tail parse.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::models::{Message, MessageRole, Provider, TokenUsage, ToolMetadata};
use crate::provider::UsageEvent;
use crate::provider::util::{ContentPartsRender, ToolCallPairer, render_content_parts};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, attach_call_metadata, build_tool_metadata, enrich_tool_metadata,
};

use super::super::tools::{render_format_a_tool_output, render_format_b_tool_output};
use super::subagents::parse_agent_swarm_children;
use super::time_ms_to_parts;

// ---------------------------------------------------------------------------
// Accumulator: shared per-line state for full-file and tail parse.
// ---------------------------------------------------------------------------

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
    /// Effective kimi-code profile from config/profile binding records.
    pub(super) current_profile: Option<String>,
    pub(super) usage_events: Vec<UsageEvent>,
    /// Message index and fallback event that own usage for the current model
    /// step. A user turn can contain several steps separated by tool calls.
    current_step_usage_idx: Option<usize>,
    current_step_usage_event_idx: Option<usize>,
    /// Newer kimi-code versions persist `usage.record` before the assistant
    /// content it belongs to. Hold the display copy until that message or
    /// tool call arrives; session accounting is already in `usage_events`.
    pending_message_usage: Option<(TokenUsage, Option<String>)>,
    /// Once the authoritative per-step `usage.record` has arrived, ignore a
    /// later `step.end.usage` replay of the same counts. Reset at `step.begin`.
    current_step_has_authoritative_usage: bool,
    pub(super) parse_warning_count: u32,
    /// Whether the agent's first `turn.prompt` has been seen. A forked agent
    /// (`/btw`) copies its parent's history before it, so only what follows
    /// is the agent's own ask.
    prompted: bool,
    /// Ids of `context.append_message` messages already rendered. kimi-code
    /// writes a steering message there and then in `turn.steer`.
    context_message_ids: HashSet<String>,
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
            current_profile: None,
            usage_events: Vec::new(),
            current_step_usage_idx: None,
            current_step_usage_event_idx: None,
            pending_message_usage: None,
            current_step_has_authoritative_usage: false,
            parse_warning_count: 0,
            prompted: false,
            context_message_ids: HashSet::new(),
            is_tail: false,
        }
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

    /// A `turn.prompt` opens a turn. Before the agent's first one, every
    /// message is inherited history, so the title candidate restarts there.
    fn begin_prompted_turn(&mut self) {
        if !self.prompted {
            self.prompted = true;
            self.first_user_message = None;
        }
        self.begin_visible_turn();
    }

    /// A session fork (`kimi fork`) copies its source's records, usage
    /// included, and then appends `forked`. That usage is the source's, so
    /// it is not counted again; the copied transcript stays.
    fn drop_inherited_usage(&mut self) {
        self.usage_events.clear();
        for message in &mut self.messages {
            message.token_usage = None;
        }
        self.begin_visible_turn();
    }

    fn begin_visible_turn(&mut self) {
        self.finish_pending_usage();
        self.current_step_usage_idx = None;
        self.current_step_usage_event_idx = None;
        self.pending_message_usage = None;
        self.current_step_has_authoritative_usage = false;
    }

    /// A Kimi user turn can run multiple LLM steps around tool calls. Both
    /// `usage.record` and `step.end.usage` describe one such step, so their
    /// pairing/attachment state must not leak into the next `step.begin`.
    fn begin_model_step(&mut self) {
        self.finish_pending_usage();
        self.current_step_usage_idx = None;
        self.current_step_usage_event_idx = None;
        self.pending_message_usage = None;
        self.current_step_has_authoritative_usage = false;
    }

    fn note_title_candidate(&mut self, text: &str) {
        if self.first_user_message.is_some() {
            return;
        }
        // Match the title heuristic used elsewhere: pick the first
        // non-image line as the title.
        let title = text
            .lines()
            .find(|line| !line.starts_with("[Image:"))
            .unwrap_or(text)
            .to_string();
        self.first_user_message = Some(title);
    }

    fn push_user_text(&mut self, text: &str, ts: Option<String>) {
        if text.is_empty() {
            return;
        }
        self.begin_visible_turn();
        self.note_title_candidate(text);
        self.content_parts.push(text.to_string());
        self.messages.push(Message {
            timestamp: ts,
            ..Message::user(text.to_string())
        });
    }

    fn push_system_context(&mut self, content: String, indexed_text: &str, ts: Option<String>) {
        if indexed_text.is_empty() {
            return;
        }
        self.content_parts.push(indexed_text.to_string());
        self.messages.push(Message {
            timestamp: ts,
            ..Message::system(content)
        });
    }

    fn push_assistant_text(&mut self, text: &str, ts: Option<String>) {
        if text.is_empty() {
            return;
        }
        self.content_parts.push(text.to_string());
        // The step's usage belongs on its assistant text. If a step fallback
        // already landed it on a tool message, move it here so exactly one
        // message per model step carries the usage.
        let tool_owner = self
            .current_step_usage_idx
            .and_then(|index| self.messages.get_mut(index))
            .filter(|message| message.role == MessageRole::Tool);
        let owner_is_tool = tool_owner.is_some();
        let moved_usage = tool_owner.and_then(|owner| owner.token_usage.take());
        if self.current_step_usage_idx.is_none() || owner_is_tool {
            self.current_step_usage_idx = Some(self.messages.len());
        }
        let index = self.messages.len();
        self.messages.push(Message {
            timestamp: ts,
            model: self.current_model.clone(),
            token_usage: moved_usage,
            ..Message::assistant(text.to_string())
        });
        self.attach_pending_usage(index);
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
        if self.current_step_usage_idx.is_none() {
            self.current_step_usage_idx = Some(self.messages.len());
        }
        let index = self.messages.len();
        self.call_id_map.register(call_id, index);
        self.messages.push(Message {
            timestamp: ts,
            tool_name: Some(display_name),
            tool_input,
            model: self.current_model.clone(),
            tool_metadata: Some(metadata),
            ..Message::new(MessageRole::Tool, String::new())
        });
        self.attach_pending_usage(index);
    }

    /// Merge a tool result onto the matching call, or push a standalone
    /// tool-result message if no matching call was seen yet (tail parse
    /// or out-of-order recovery).
    fn merge_tool_result(
        &mut self,
        call_id: Option<&str>,
        rendered_output: String,
        is_error: Option<bool>,
        is_raw: bool,
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
                        raw_output: Some(is_raw),
                    },
                );
                attach_agent_swarm_children(meta, &rendered_output);
            }
            message.content = rendered_output;
            return;
        }
        if self.current_step_usage_idx.is_none() {
            self.current_step_usage_idx = Some(self.messages.len());
        }
        let index = self.messages.len();
        self.messages.push(Message {
            timestamp: ts,
            ..Message::new(MessageRole::Tool, rendered_output)
        });
        self.attach_pending_usage(index);
    }

    fn attach_pending_usage(&mut self, index: usize) {
        let Some((usage, model)) = self.pending_message_usage.take() else {
            return;
        };
        let Some(message) = self.messages.get_mut(index) else {
            return;
        };
        message.token_usage = Some(usage);
        if message.model.is_none() {
            message.model = model.or_else(|| self.current_model.clone());
        }
    }

    pub(super) fn finish_pending_usage(&mut self) {
        if self.pending_message_usage.take().is_some() && !self.is_tail {
            log::warn!("Kimi usage.record had no assistant/tool message in its turn");
            self.note_warning();
        }
    }

    /// Attach token totals to the step's first assistant text, or its
    /// trailing tool for tool-only turns. If usage precedes content, retain
    /// the display copy until the first eligible message arrives.
    fn attach_usage(&mut self, usage: TokenUsage, model: Option<&str>, authoritative: bool) {
        let target_idx = if authoritative {
            self.current_step_usage_idx.take()
        } else {
            self.current_step_usage_idx
        };
        let Some(idx) = target_idx else {
            self.pending_message_usage = Some((usage, model.map(str::to_string)));
            return;
        };
        let Some(msg) = self.messages.get_mut(idx) else {
            return;
        };
        if !authoritative {
            self.current_step_usage_idx = Some(idx);
        }
        msg.token_usage = Some(usage);
        if let Some(m) = model {
            msg.model = Some(m.to_string());
        } else if msg.model.is_none() {
            msg.model = self.current_model.clone();
        }
    }

    /// Fold usage into the current model step's event. `step.end.usage`
    /// (`authoritative == false`) is a fallback; a neighboring `usage.record`
    /// replaces it with the authoritative counts. Consecutive fallbacks
    /// without an explicit `step.begin` still accumulate for legacy logs.
    /// Returns the usage to attach to the step's owner message.
    fn record_usage_event(
        &mut self,
        usage: &TokenUsage,
        timestamp: Option<String>,
        model: Option<&str>,
        authoritative: bool,
    ) -> Option<TokenUsage> {
        if !authoritative && self.current_step_has_authoritative_usage {
            return None;
        }
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
            turn_count: 1,
            input_tokens: u64::from(usage.input_tokens),
            output_tokens: u64::from(usage.output_tokens),
            cache_read_input_tokens: u64::from(usage.cache_read_input_tokens),
            cache_creation_input_tokens: u64::from(usage.cache_creation_input_tokens),
            usage_hash: None,
            cost_is_estimate: false,
            cost_usd: None,
        };
        if let Some(index) = self.current_step_usage_event_idx.take() {
            if !authoritative {
                let prev = &self.usage_events[index];
                event.input_tokens += prev.input_tokens;
                event.output_tokens += prev.output_tokens;
                event.cache_read_input_tokens += prev.cache_read_input_tokens;
                event.cache_creation_input_tokens += prev.cache_creation_input_tokens;
                self.current_step_usage_event_idx = Some(index);
            }
            self.usage_events[index] = event;
        } else {
            self.usage_events.push(event);
            if !authoritative {
                self.current_step_usage_event_idx = Some(self.usage_events.len() - 1);
            }
        }
        if authoritative {
            self.current_step_has_authoritative_usage = true;
        }
        let attached = self
            .current_step_usage_event_idx
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

    /// Count a model call made outside any turn (`usageScope: "session"`:
    /// compaction, titles, …). It is real spend but belongs to no message,
    /// so it neither annotates the transcript nor touches step pairing.
    fn record_session_usage(
        &mut self,
        usage: &TokenUsage,
        timestamp: Option<String>,
        model: Option<&str>,
    ) {
        let (Some(timestamp), Some(model)) = (timestamp, model) else {
            if !self.is_tail {
                log::warn!("skipping Kimi session usage record without timestamp or model");
                self.note_warning();
            }
            return;
        };
        self.usage_events.push(UsageEvent {
            timestamp,
            model: model.to_string(),
            turn_count: 0,
            input_tokens: u64::from(usage.input_tokens),
            output_tokens: u64::from(usage.output_tokens),
            cache_read_input_tokens: u64::from(usage.cache_read_input_tokens),
            cache_creation_input_tokens: u64::from(usage.cache_creation_input_tokens),
            usage_hash: None,
            cost_is_estimate: false,
            cost_usd: None,
        });
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
#[allow(clippy::items_after_test_module)]
mod tests;

// ---------------------------------------------------------------------------
// Per-line dispatch — shared between full-file parse and tail parse.
// ---------------------------------------------------------------------------

/// Render a content-part array with the shared text/media renderer, so
/// images, audio, and video all surface as `[Kind: source: …]` markers. A
/// part type the renderer does not know is skipped with a counted warning.
fn text_from_parts(accum: &mut ScanAccum, parts: &[Value]) -> String {
    match render_content_parts(parts) {
        ContentPartsRender::Rendered(text) => text,
        ContentPartsRender::Empty => String::new(),
        ContentPartsRender::Unsupported => {
            let rendered: Vec<String> = parts
                .iter()
                .filter_map(
                    |part| match render_content_parts(std::slice::from_ref(part)) {
                        ContentPartsRender::Rendered(text) => Some(text),
                        ContentPartsRender::Empty => None,
                        ContentPartsRender::Unsupported => {
                            let kind = part
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown");
                            log::warn!("skipping unsupported Kimi content part '{kind}'");
                            accum.note_warning();
                            None
                        }
                    },
                )
                .collect();
            rendered.join("\n")
        }
    }
}

fn handle_turn_ended(accum: &mut ScanAccum, entry: &Value, line_time_ms: Option<i64>) {
    let timestamp = accum.note_time(line_time_ms);
    let Some(reason) = entry
        .get("reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    else {
        log::warn!("Kimi turn.ended without a reason");
        accum.note_warning();
        return;
    };
    if reason == "completed" {
        return;
    }

    let detail = entry
        .pointer("/error/message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .or_else(|| {
            entry
                .get("interruptReason")
                .and_then(Value::as_str)
                .filter(|message| !message.is_empty())
        });
    let content = detail.map_or_else(
        || format!("[turn_{reason}]"),
        |detail| format!("[turn_{reason}]\n{detail}"),
    );
    accum.push_system_context(content, detail.unwrap_or(reason), timestamp);
}

fn handle_step_interrupted(accum: &mut ScanAccum, entry: &Value, line_time_ms: Option<i64>) {
    let timestamp = accum.note_time(line_time_ms);
    let Some(reason) = entry
        .get("reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    else {
        log::warn!("Kimi turn.step.interrupted without a reason");
        accum.note_warning();
        return;
    };
    let detail = entry
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty());
    let content = detail.map_or_else(
        || format!("[step_interrupted] {reason}"),
        |detail| format!("[step_interrupted] {reason}\n{detail}"),
    );
    accum.push_system_context(content, detail.unwrap_or(reason), timestamp);
}

fn handle_step_retrying(accum: &mut ScanAccum, entry: &Value, line_time_ms: Option<i64>) {
    let timestamp = accum.note_time(line_time_ms);
    let fields = (
        entry.get("nextAttempt").and_then(Value::as_u64),
        entry.get("maxAttempts").and_then(Value::as_u64),
        entry
            .get("delayMs")
            .and_then(Value::as_f64)
            .filter(|delay| delay.is_finite() && *delay >= 0.0),
        entry
            .get("errorName")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty()),
        entry
            .get("errorMessage")
            .and_then(Value::as_str)
            .filter(|message| !message.is_empty()),
    );
    let (Some(next_attempt), Some(max_attempts), Some(delay_ms), Some(error_name), error_message) =
        fields
    else {
        log::warn!("Kimi turn.step.retrying has malformed retry details");
        accum.note_warning();
        return;
    };
    let summary =
        format!("attempt {next_attempt}/{max_attempts} after {delay_ms} ms ({error_name})");
    let content = error_message.map_or_else(
        || format!("[retry] {summary}"),
        |message| format!("[retry] {summary}\n{message}"),
    );
    accum.push_system_context(content, error_message.unwrap_or(&summary), timestamp);
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

        "config.update" | "profile.bind" => {
            if let Some(model) = entry
                .get("modelAlias")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                accum.current_model = Some(model.to_string());
            }
            if let Some(profile) = entry
                .get("profileName")
                .and_then(Value::as_str)
                .filter(|profile| !profile.is_empty())
            {
                accum.current_profile = Some(profile.to_string());
            }
            // Soak up the time anyway so first/last span the whole file.
            let _ = accum.note_time(line_time_ms);
        }

        // ---- Format A & B: user prompt + injected reminders ----
        "context.append_message" => handle_migrated_line(accum, entry, line_time_ms),

        // ---- Format B: streaming events ----
        "context.append_loop_event" => handle_native_event(accum, entry, line_time_ms),

        // One record per model call, folded additively upstream. `turn`
        // usage belongs to the current model step; `session` usage is a call
        // outside any turn. Records without a scope come from releases whose
        // per-step totals `step.end.usage` already carries.
        "usage.record" => {
            let timestamp = accum.note_time(line_time_ms);
            let scope = entry.get("usageScope").and_then(Value::as_str);
            if !matches!(scope, Some("turn" | "session")) {
                return;
            }
            let model = entry
                .get("model")
                .and_then(|v| v.as_str())
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .or_else(|| accum.current_model.clone());
            let Some(usage) = entry.get("usage").and_then(parse_usage) else {
                return;
            };
            if scope == Some("session") {
                accum.record_session_usage(&usage, timestamp, model.as_deref());
            } else if let Some(total) =
                accum.record_usage_event(&usage, timestamp, model.as_deref(), true)
            {
                accum.attach_usage(total, model.as_deref(), true);
            }
        }

        // ---- Turn boundaries (protocol_version 1.4+) ----
        // A new turn starts: close the previous turn's usage pairing.
        "turn.prompt" => {
            let _ = accum.note_time(line_time_ms);
            accum.begin_prompted_turn();
        }

        // ---- Mid-turn steering (protocol_version 1.4+) ----
        // `turn.steer` carries input injected while a turn is running:
        // either a human steering message or a runtime notification. It is
        // transcript content, but NOT a turn boundary — usage anchoring
        // must not reset.
        "turn.steer" => {
            let ts = accum.note_time(line_time_ms);
            // Current kimi-code first writes the steering message as a
            // `context.append_message` with the same id.
            if entry
                .get("messageId")
                .and_then(Value::as_str)
                .is_some_and(|id| accum.context_message_ids.contains(id))
            {
                return;
            }
            let Some(parts) = entry.get("input").and_then(Value::as_array) else {
                log::warn!("Kimi turn.steer without input parts");
                accum.note_warning();
                return;
            };
            let text = text_from_parts(accum, parts);
            if text.is_empty() {
                return;
            }
            if text.trim_start().starts_with("<notification")
                || crate::provider::util::is_system_content(text.trim_start())
            {
                accum.push_system_context(format!("[kimi_context] steer\n{text}"), &text, ts);
            } else {
                accum.note_title_candidate(&text);
                accum.content_parts.push(text.clone());
                accum.messages.push(Message {
                    timestamp: ts,
                    ..Message::user(text)
                });
            }
        }

        // Compaction rewrote the context; the summary is what the model
        // sees afterwards, so it belongs in the transcript.
        "context.apply_compaction" => {
            let ts = accum.note_time(line_time_ms);
            let Some(summary) = entry
                .get("summary")
                .and_then(Value::as_str)
                .filter(|summary| !summary.is_empty())
            else {
                log::warn!("Kimi context.apply_compaction without summary");
                accum.note_warning();
                return;
            };
            accum.push_system_context(format!("[context_compacted]\n{summary}"), summary, ts);
        }

        // Abnormal lifecycle records carry user-visible status. Completed
        // turns are ordinary boundaries, but failures, interruptions, and
        // retries retain their reason/error text in the transcript.
        "turn.ended" => handle_turn_ended(accum, entry, line_time_ms),

        // Undo rewrites only the model's context. The transcript keeps every
        // message and marks where the context changed.
        "context.undo" => {
            let ts = accum.note_time(line_time_ms);
            let Some(count) = entry.get("count").and_then(Value::as_u64) else {
                log::warn!("Kimi context.undo without a turn count");
                accum.note_warning();
                return;
            };
            let detail = format!("undid the last {count} turn(s); they stay above");
            accum.push_system_context(format!("[kimi_context] undo\n{detail}"), &detail, ts);
        }
        "turn.step.interrupted" => handle_step_interrupted(accum, entry, line_time_ms),
        "turn.step.retrying" => handle_step_retrying(accum, entry, line_time_ms),

        // ---- Events that produce no visible transcript content ----
        "tools.set_active_tools"
        | "tools.update_store"
        | "plan_mode.enter"
        | "plan_mode.cancel"
        | "plan_mode.exit"
        | "permission.set_mode"
        | "permission.record_approval_result"
        | "llm.request"
        | "llm.tools_snapshot"
        | "goal.create"
        | "goal.update"
        | "goal.clear"
        | "full_compaction.begin"
        | "full_compaction.complete"
        | "full_compaction.cancel"
        | "swarm_mode.enter"
        | "swarm_mode.exit"
        // Current kimi-code runtime, replay, prompt-queue, and measurement
        // state. None adds unique transcript or billable usage: steering is
        // rendered from `turn.steer`, interactions/tasks from their tool and
        // context records, and usage from `usage.record`/`step.end.usage`.
        | "runtime.set_binding"
        | "prompt.accepted"
        | "prompt.aborted"
        | "prompt.completed"
        | "prompt.steered"
        | "token_counting.measured"
        | "token_counting.truncated"
        | "token_counting.rebased"
        | "token_counting.turn_recorded"
        | "plugin.session_start"
        | "interaction.request"
        | "interaction.resolved"
        | "task.started"
        | "task.terminated"
        | "task.waitDelivered"
        | "cron.add"
        | "cron.cursor"
        | "cron.delete"
        | "plan.revision"
        | "staleGuard.recorded"
        | "staleGuard.cleared"
        | "interruptionReminder.recorded"
        | "tools.register_user_tool"
        | "tools.unregister_user_tool"
        | "tools.reset_active_tools"
        | "mcp.tools_discovered"
        // A cancelled turn keeps its partial output, as kimi-code's context
        // does; its `turn.ended` reason marks the cancellation.
        | "turn.cancel"
        // Undo bookkeeping beside `context.undo`: the wire-tree branch edge
        // and the undone turn range.
        | "agent.switched"
        | "context.undone"
        // The loop engine's journal: turn bookkeeping. Its messages are
        // handled below.
        | "agent.turn.started"
        | "agent.turn.ended"
        // Subagent lifecycle mirrors in the parent's wire. Each child's
        // transcript and usage live in its own wire.
        | "subagent.spawned"
        | "subagent.started"
        | "subagent.completed"
        | "subagent.failed"
        | "subagent.cancelled"
        | "file_history.checkpoint"
        | "file_history.tracked"
        | "tower_mode.enter"
        | "tower_mode.exit" => {
            // These are UI/state bookkeeping events; they don't carry
            // messages we want in the transcript. Soak up the time so
            // first/last timestamps still span the whole file.
            let _ = accum.note_time(line_time_ms);
        }

        // The last `forked` closes a session fork's copy of its source.
        "forked" => {
            let _ = accum.note_time(line_time_ms);
            accum.drop_inherited_usage();
        }

        // The journal mirrors the input, model, and tool messages that the
        // `context.*` records carry. Only the partial output of an
        // interrupted model step is journal-only (`source: "salvaged"`).
        "agent.message.appended" => {
            let ts = accum.note_time(line_time_ms);
            if entry.pointer("/message/meta/source").and_then(Value::as_str) == Some("salvaged") {
                let parts = entry
                    .pointer("/message/message/content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                push_assistant_parts(accum, &parts, ts);
            }
        }

        unknown => {
            log::warn!("skipping unknown Kimi record type '{unknown}'");
            accum.note_warning();
        }
    }
}

/// Assistant content parts in order: thinking, text, and media markers.
fn push_assistant_parts(accum: &mut ScanAccum, parts: &[Value], ts: Option<String>) {
    for part in parts {
        match part.get("type").and_then(Value::as_str).unwrap_or("") {
            "think" => {
                let text = part.get("think").and_then(Value::as_str).unwrap_or("");
                accum.push_thinking(text, ts.clone());
            }
            "text" => {
                let text = part.get("text").and_then(Value::as_str).unwrap_or("");
                accum.push_assistant_text(text, ts.clone());
            }
            _ => {
                let text = text_from_parts(accum, std::slice::from_ref(part));
                accum.push_assistant_text(&text, ts.clone());
            }
        }
    }
}

/// Handle a `context.append_message` line shared by both wire formats:
/// human prompts and runtime-origin context, plus migrated assistant/tool
/// messages.
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
    if let Some(id) = message.get("id").and_then(Value::as_str) {
        accum.context_message_ids.insert(id.to_string());
    }
    let origin = message.get("origin");
    let origin_kind = origin
        .and_then(|value| value.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("");

    match role {
        "user" => {
            let text = text_from_parts(accum, &content_array);
            match origin_kind {
                // Migrated transcripts predate PromptOrigin. A missing origin
                // there still represents a genuine user prompt.
                "" | "user" => accum.push_user_text(&text, ts),
                // Permission banners and tool reminders are intentionally not
                // transcript content.
                "injection" => {}
                // Kimi serializes asynchronous task lifecycle notifications as
                // role=user because they are fed back into the model context.
                // In the transcript they are status events, never human text.
                "task" | "background_task" => {
                    let task_id = origin
                        .and_then(|value| value.get("taskId"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty());
                    let status = origin
                        .and_then(|value| value.get("status"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty());
                    let (Some(task_id), Some(status)) = (task_id, status) else {
                        log::warn!(
                            "Kimi {origin_kind} context missing taskId or status; rendering as generic context"
                        );
                        accum.note_warning();
                        accum.push_system_context(
                            format!("[kimi_context] {origin_kind}\n{text}"),
                            &text,
                            ts,
                        );
                        return;
                    };
                    let subtype = if matches!(status, "completed" | "running") {
                        "task_status"
                    } else {
                        "task_status_error"
                    };
                    accum.push_system_context(
                        format!("[{subtype}] {status} · {task_id}\n{text}"),
                        &text,
                        ts,
                    );
                }
                "system_trigger" => {
                    let Some(name) = origin
                        .and_then(|value| value.get("name"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                    else {
                        log::warn!(
                            "Kimi system_trigger context missing name; rendering as generic context"
                        );
                        accum.note_warning();
                        accum.push_system_context(
                            format!("[kimi_context] system_trigger\n{text}"),
                            &text,
                            ts,
                        );
                        return;
                    };
                    if name == "subagent" {
                        // Keep this as a title fallback when the parent Agent
                        // call is unavailable, but don't attribute it to the
                        // human user in the child transcript.
                        accum.note_title_candidate(&text);
                        accum.push_system_context(format!("[subagent_task] {text}"), &text, ts);
                    } else {
                        accum.push_system_context(
                            format!("[kimi_context] {name}\n{text}"),
                            &text,
                            ts,
                        );
                    }
                }
                "skill_activation" => {
                    let Some(skill_name) = origin
                        .and_then(|value| value.get("skillName"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                    else {
                        log::warn!(
                            "Kimi skill_activation context missing skillName; rendering as generic context"
                        );
                        accum.note_warning();
                        accum.push_system_context(
                            format!("[kimi_context] skill_activation\n{text}"),
                            &text,
                            ts,
                        );
                        return;
                    };
                    if origin
                        .and_then(|value| value.get("trigger"))
                        .and_then(Value::as_str)
                        == Some("user-slash")
                    {
                        accum.begin_visible_turn();
                    }
                    accum.push_system_context(
                        format!("[skill_activation] {skill_name}\n{text}"),
                        &text,
                        ts,
                    );
                }
                "plugin_command" => {
                    if origin
                        .and_then(|value| value.get("trigger"))
                        .and_then(Value::as_str)
                        == Some("user-slash")
                    {
                        accum.begin_visible_turn();
                    }
                    accum.push_system_context(
                        format!("[kimi_context] plugin command\n{text}"),
                        &text,
                        ts,
                    );
                }
                "shell_command" => {
                    let phase = origin
                        .and_then(|value| value.get("phase"))
                        .and_then(Value::as_str);
                    let message = match phase {
                        Some("input") => {
                            accum.begin_visible_turn();
                            accum.note_title_candidate(&text);
                            Message::command_input(text.clone())
                        }
                        Some("output") => Message::command_output(text.clone()),
                        _ => {
                            log::warn!(
                                "Kimi shell_command context missing phase; rendering as generic context"
                            );
                            accum.note_warning();
                            accum.push_system_context(
                                format!("[kimi_context] shell_command\n{text}"),
                                &text,
                                ts,
                            );
                            return;
                        }
                    };
                    if !text.is_empty() {
                        accum.content_parts.push(text);
                        accum.messages.push(Message {
                            timestamp: ts,
                            ..message
                        });
                    }
                }
                "compaction_summary" => {
                    accum.push_system_context(format!("[context_compacted]\n{text}"), &text, ts)
                }
                "cron_job" | "cron_missed" | "hook_result" | "retry" => accum.push_system_context(
                    format!("[kimi_context] {origin_kind}\n{text}"),
                    &text,
                    ts,
                ),
                // Unknown kinds are future protocol, not malformed data:
                // render the text under a generic label so nothing the model
                // saw is missing from the transcript.
                unknown => {
                    log::warn!("rendering Kimi context with unknown origin.kind '{unknown}'");
                    accum.push_system_context(
                        format!("[kimi_context] {unknown}\n{text}"),
                        &text,
                        ts,
                    );
                }
            }
        }
        "assistant" => {
            // Format A puts assistant think/text under content[],
            // tool calls under message.toolCalls[]. Emit them in
            // the order the on-disk message implies: think/text
            // first, then tool calls.
            push_assistant_parts(accum, &content_array, ts.clone());
            if let Some(calls) = message.get("toolCalls").and_then(|v| v.as_array()) {
                for tc in calls {
                    let id = tc.get("id").and_then(|v| v.as_str());
                    // Wire 1.1 flattened `{function: {name, arguments}}`
                    // onto the call itself; 1.0 files keep the nested form.
                    let field = |key: &str| {
                        tc.get(key)
                            .or_else(|| tc.get("function").and_then(|f| f.get(key)))
                    };
                    let name = field("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                    // Format A serialises args as a JSON string;
                    // try to parse it back into a Value so the
                    // metadata builder can structure-inspect it.
                    let arg_string = field("arguments").and_then(|v| v.as_str());
                    let arg_value: Option<Value> =
                        arg_string.and_then(|s| serde_json::from_str::<Value>(s).ok());
                    accum.push_tool_call(name, id, arg_value.as_ref(), ts.clone(), None);
                }
            }
        }
        "tool" => {
            let call_id = message.get("toolCallId").and_then(|v| v.as_str());
            let rendered = render_format_a_tool_output(&content_array);
            accum.merge_tool_result(
                call_id,
                rendered.text,
                rendered.is_error,
                rendered.is_raw,
                None,
                ts,
            );
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
                _ => {
                    let text = text_from_parts(accum, std::slice::from_ref(part));
                    accum.push_assistant_text(&text, ts);
                }
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
            let rendered = render_format_b_tool_output(result);
            accum.merge_tool_result(
                id,
                rendered.text,
                rendered.is_error,
                rendered.is_raw,
                result,
                ts,
            );
        }
        "step.end" => {
            // `usage.record` carries the same totals plus the canonical model
            // alias. Kimi versions place it either before the streamed content
            // or just after step.end; prefer it whenever present, with
            // `step.end.usage` as the legacy/crash fallback.
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
        // `step.begin` carries no transcript content, but it is the boundary
        // between model calls inside one user turn. Reset usage pairing here
        // so an authoritative record from the previous step cannot suppress
        // or overwrite this step's usage.
        "step.begin" => accum.begin_model_step(),
        unknown => {
            log::warn!("skipping unknown Kimi loop event type '{unknown}'");
            accum.note_warning();
        }
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
    let usage = crate::provider::util::token_usage_from(
        value,
        &crate::provider::util::UsageKeys {
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
