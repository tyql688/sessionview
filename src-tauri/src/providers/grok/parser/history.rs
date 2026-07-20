//! Pre-compaction history reconstruction.
//!
//! Auto-compact rewrites `chat_history.jsonl`, but `updates.jsonl` is the
//! append-only ACP stream (it drives grok's own `/load`) and still carries
//! every prompt, thought, message, and tool call/result. On a compacted
//! session the parser replays it and rebuilds all messages before the
//! oldest prompt_index surviving in the transcript.
//!
//! Verified stream facts: each `agent_message_chunk` / `agent_thought_chunk`
//! is one COMPLETE message (no fragment merging); `tool_call` carries
//! `rawInput` + the raw name in `_meta."x.ai/tool".name`; `tool_call_update`
//! carries display `content` blocks and/or a variant-typed `rawOutput`
//! (last update wins); `turn_completed` totals attach to the turn's final
//! assistant message.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::models::{Message, MessageRole, Provider, TokenUsage};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

/// Fed one update at a time by the shared updates scan; collection stops
/// permanently at the first prompt that survived compaction.
pub(super) struct HistoryBuilder {
    cutoff_prompt: u64,
    done: bool,
    messages: Vec<Message>,
    tool_index: HashMap<String, usize>,
    last_assistant: Option<usize>,
}

impl HistoryBuilder {
    pub(super) fn new(cutoff_prompt: u64) -> Self {
        Self {
            cutoff_prompt,
            done: false,
            messages: Vec::new(),
            tool_index: HashMap::new(),
            last_assistant: None,
        }
    }

    pub(super) fn into_messages(self) -> Vec<Message> {
        self.messages
    }

    /// Feed one `params.update` object plus its wrapper timestamp.
    pub(super) fn push_update(&mut self, kind: &str, update: &Value, timestamp: Option<&str>) {
        if self.done {
            return;
        }
        match kind {
            "user_message_chunk" => {
                let Some(index) = update.pointer("/_meta/promptIndex").and_then(Value::as_u64)
                else {
                    return;
                };
                if index >= self.cutoff_prompt {
                    self.done = true;
                    return;
                }
                let text = update
                    .pointer("/content/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    return;
                }
                self.last_assistant = None;
                self.messages.push(Message {
                    timestamp: timestamp.map(str::to_string),
                    ..Message::user(text)
                });
            }
            "agent_thought_chunk" => {
                let Some(text) = update.pointer("/content/text").and_then(Value::as_str) else {
                    return;
                };
                if text.trim().is_empty() {
                    return;
                }
                self.messages.push(Message {
                    timestamp: timestamp.map(str::to_string),
                    ..Message::system(format!("[thinking]\n{text}"))
                });
            }
            "agent_message_chunk" => {
                let Some(text) = update.pointer("/content/text").and_then(Value::as_str) else {
                    return;
                };
                if text.trim().is_empty() {
                    return;
                }
                self.last_assistant = Some(self.messages.len());
                self.messages.push(Message {
                    timestamp: timestamp.map(str::to_string),
                    ..Message::assistant(text)
                });
            }
            "tool_call" => self.push_tool_call(update, timestamp),
            "tool_call_update" => self.apply_tool_update(update),
            "turn_completed" => self.attach_turn_usage(update),
            _ => {}
        }
    }

    fn push_tool_call(&mut self, update: &Value, timestamp: Option<&str>) {
        let Some(call_id) = update.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        let raw_name = update
            .pointer("/_meta/x.ai~1tool/name")
            .and_then(Value::as_str)
            .or_else(|| update.get("title").and_then(Value::as_str))
            .unwrap_or("unknown");
        let input = update.get("rawInput");
        let metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Grok,
            raw_name,
            input,
            call_id: Some(call_id),
            assistant_id: None,
        });
        let idx = self.messages.len();
        self.tool_index.insert(call_id.to_string(), idx);
        self.messages.push(Message {
            timestamp: timestamp.map(str::to_string),
            tool_name: Some(metadata.canonical_name.clone()),
            tool_input: input.map(Value::to_string),
            tool_metadata: Some(metadata),
            ..Message::new(MessageRole::Tool, String::new())
        });
    }

    fn apply_tool_update(&mut self, update: &Value) {
        let Some(idx) = update
            .get("toolCallId")
            .and_then(Value::as_str)
            .and_then(|id| self.tool_index.get(id).copied())
        else {
            return;
        };
        let Some(message) = self.messages.get_mut(idx) else {
            return;
        };

        let status = update.get("status").and_then(Value::as_str);
        let display_text = content_blocks_text(update);
        let raw_output = update.get("rawOutput");
        let (raw_text, result_extra) = raw_output.map(decode_raw_output).unwrap_or_default();

        let text = if !display_text.is_empty() {
            display_text
        } else {
            raw_text
        };
        if !text.is_empty() {
            message.content = text;
        }

        if let Some(metadata) = message.tool_metadata.as_mut() {
            let mut result = result_extra.unwrap_or_else(|| json!({}));
            if super::should_mirror_output_into_structured(&metadata.canonical_name)
                && !message.content.is_empty()
                && result.get("output").is_none()
            {
                result["output"] = Value::String(message.content.clone());
            }
            if metadata.canonical_name == "Agent"
                && let Some(child_id) = super::extract_subagent_id(&message.content)
            {
                result["agent_id"] = Value::String(child_id);
            }
            enrich_tool_metadata(
                metadata,
                ToolResultFacts {
                    raw_result: Some(&result),
                    is_error: status.map(|s| s == "failed"),
                    status,
                    artifact_path: None,
                },
            );
        }
    }

    fn attach_turn_usage(&mut self, update: &Value) {
        let Some(idx) = self.last_assistant else {
            return;
        };
        let Some(usage) = update.get("usage") else {
            return;
        };
        let (input, output, cache_read) = (
            usage
                .get("inputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage
                .get("outputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage
                .get("cachedReadTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        if let Some(message) = self.messages.get_mut(idx) {
            message.token_usage = Some(TokenUsage {
                input_tokens: input.saturating_sub(cache_read) as u32,
                output_tokens: output as u32,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: cache_read as u32,
            });
        }
        // One turn, one attachment.
        self.last_assistant = None;
    }
}

/// Display text from ACP `content` blocks.
fn content_blocks_text(update: &Value) -> String {
    let Some(blocks) = update.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    blocks
        .iter()
        .filter_map(|block| block.pointer("/content/text").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Decode a variant-typed `rawOutput` into (display text, structured extras).
/// Unknown variants fall back to compact JSON so nothing silently disappears.
fn decode_raw_output(raw: &Value) -> (String, Option<Value>) {
    if let Some(text) = raw.pointer("/Content/content").and_then(Value::as_str) {
        return (text.to_string(), None);
    }
    if let Some(summary) = raw
        .pointer("/TodosUpdated/summary_for_prompt")
        .and_then(Value::as_str)
    {
        return (summary.to_string(), None);
    }
    if let Some(edits) = raw.get("EditsApplied") {
        let (old, new) = (
            edits.get("old_string").and_then(Value::as_str),
            edits.get("new_string").and_then(Value::as_str),
        );
        if let (Some(old), Some(new)) = (old, new) {
            return (
                String::new(),
                Some(json!({ "oldString": old, "newString": new })),
            );
        }
    }
    if let Some(result) = raw.get("Result") {
        let text = result
            .get("output")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();
        return (text, Some(result.clone()));
    }
    (raw.to_string(), None)
}
