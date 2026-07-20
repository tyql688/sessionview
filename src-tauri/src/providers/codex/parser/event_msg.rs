//! `event_msg` line handler for the Codex per-line dispatch. Split out of
//! `parser/mod.rs` for size; the per-event-kind `match` stays one unit.

use std::path::Path;

use serde_json::Value;

use crate::models::{Message, MessageRole, Provider};
use crate::provider::UsageEvent;
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

use super::super::tools::*;
use super::usage::{
    add_usage_to_last_assistant, codex_token_usage_from_counts, codex_usage_from_info,
    extract_codex_model,
};
use super::value_helpers::{
    codex_call_id, codex_content_items_text, codex_exec_command_event_result,
    codex_image_generation_result, codex_mcp_tool_call_event_result, codex_patch_event_result,
    dynamic_tool_input, dynamic_tool_result, enrich_existing_tool_message, push_system_event,
};
use super::{CodexLine, CodexScanAccum, append_user_message, flush_pending_user_message};

impl CodexScanAccum {
    /// Handle an `event_msg` line. `return` means skip the line: this is the
    /// last action `scan_lines` takes for it.
    pub(super) fn handle_event_msg(&mut self, entry: &CodexLine, payload: &Value, path: &Path) {
        let event_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
        // agent_message is a duplicate of response_item/message/assistant — skip
        match event_type {
            "user_message" => {
                let pending = self.pending_user_message.take();
                let fallback_content = pending.as_ref().map(|message| message.content.clone());
                let response_image_segments = pending
                    .as_ref()
                    .map(|message| message.image_segments.clone())
                    .unwrap_or_default();
                let timestamp = entry
                    .timestamp
                    .clone()
                    .or_else(|| pending.and_then(|message| message.timestamp));
                let built_content = build_codex_user_message(payload, &response_image_segments);
                let content = if built_content.is_empty() {
                    fallback_content.unwrap_or_default()
                } else {
                    built_content
                };
                append_user_message(
                    &mut self.messages,
                    &mut self.content_parts,
                    &mut self.first_user_message,
                    content,
                    timestamp,
                );
            }
            "item_completed" => {
                flush_pending_user_message(
                    &mut self.pending_user_message,
                    &mut self.messages,
                    &mut self.content_parts,
                    &mut self.first_user_message,
                );
                let item = payload.get("item");
                if item.and_then(|v| v.get("type")).and_then(|v| v.as_str()) == Some("Plan") {
                    let text = item
                        .and_then(|v| v.get("text"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !text.trim().is_empty() {
                        self.content_parts.push(text.to_string());
                        self.messages.push(Message {
                            timestamp: entry.timestamp.clone(),
                            model: self.current_model.clone(),
                            ..Message::assistant(text.to_string())
                        });
                    }
                }
            }
            "thread_name_updated" => {
                if let Some(name) = payload
                    .get("thread_name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                {
                    self.thread_name = Some(name.to_string());
                }
            }
            "error" => {
                let message = payload
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                let info = payload
                    .get("codex_error_info")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let detail = if info.is_empty() {
                    message.to_string()
                } else {
                    format!("{info}: {message}")
                };
                push_system_event(
                    &mut self.messages,
                    entry.timestamp.clone(),
                    format!("[error] {detail}"),
                );
            }
            "turn_aborted" => {
                let reason = payload
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("aborted");
                let duration = payload
                    .get("duration_ms")
                    .and_then(|v| v.as_u64())
                    .map(|ms| format!(" ({:.1}s)", ms as f64 / 1000.0))
                    .unwrap_or_default();
                push_system_event(
                    &mut self.messages,
                    entry.timestamp.clone(),
                    format!("[turn_aborted] {reason}{duration}"),
                );
            }
            // A top-level `compacted` record carries the post-compaction
            // handoff summary. The event message is only a marker that
            // compaction happened, and rendering both creates a duplicate
            // "compacted" row with no useful body.
            "context_compacted" => {}
            "token_count" => {
                if let Some(info) = payload.get("info") {
                    let Some((usage_model, usage_counts)) =
                        codex_usage_from_info(info, &mut self.previous_token_totals)
                    else {
                        return;
                    };
                    let (input, cached, output, reasoning, total) = usage_counts;
                    let any_nonzero =
                        input != 0 || cached != 0 || output != 0 || reasoning != 0 || total != 0;
                    let resolved_model = extract_codex_model(info)
                        .or_else(|| extract_codex_model(payload))
                        .or_else(|| self.current_model.clone())
                        .or_else(|| usage_model.clone());
                    // Cost attribution needs a real model name, so an
                    // unresolvable one drops BOTH the indexer event and the
                    // per-message attachment: keeping one half would show
                    // tokens with no provenance while daily totals undercount.
                    let Some(resolved_model) = resolved_model else {
                        if any_nonzero {
                            if self.unresolved_usage_event_count == 0 {
                                log::debug!(
                                    "first Codex token_count event without a resolvable model is at {:?} in '{}'",
                                    entry.timestamp,
                                    path.display()
                                );
                            }
                            self.unresolved_usage_event_count =
                                self.unresolved_usage_event_count.saturating_add(1);
                        }
                        return;
                    };
                    // Codex re-emits some token_count events verbatim. Count an
                    // event identical in (timestamp, model, input, cached,
                    // output, reasoning, total) only once.
                    if let Some(ts) = entry.timestamp.as_ref() {
                        let key = (
                            ts.clone(),
                            resolved_model.clone(),
                            input,
                            cached,
                            output,
                            reasoning,
                            total,
                        );
                        if !self.seen_token_events.insert(key) {
                            return;
                        }
                    }
                    if any_nonzero && let Some(ts) = entry.timestamp.as_ref() {
                        self.usage_events.push(UsageEvent {
                            timestamp: ts.clone(),
                            model: resolved_model.clone(),
                            input_tokens: input.saturating_sub(cached.min(input)),
                            output_tokens: output,
                            cache_read_input_tokens: cached.min(input),
                            cache_creation_input_tokens: 0,
                            // Rollouts never repeat a token_count across
                            // files; batch-scoped dedup would only make
                            // incremental rescans nondeterministic.
                            usage_hash: None,
                        });
                    }
                    let Some(usage) = codex_token_usage_from_counts(usage_counts) else {
                        return;
                    };
                    self.current_model = Some(resolved_model.clone());
                    add_usage_to_last_assistant(&mut self.messages, usage, Some(resolved_model));
                }
            }
            "web_search_end" => {
                let call_id = payload.get("call_id").and_then(|v| v.as_str());
                let action = payload.get("action");
                let query = payload.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let result = Some(payload.clone());

                if let Some(message) = self.call_id_map.message_mut(call_id, &mut self.messages) {
                    message.content = query.to_string();
                    if message.tool_input.is_none() {
                        message.tool_input = action.map(|value| value.to_string());
                    }
                    if let Some(metadata) = message.tool_metadata.as_mut() {
                        enrich_tool_metadata(
                            metadata,
                            ToolResultFacts {
                                raw_result: result.as_ref(),
                                is_error: None,
                                status: None,
                                artifact_path: None,
                            },
                        );
                    }
                    return;
                }

                let mut metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Codex,
                    raw_name: "web_search_call",
                    input: action,
                    call_id,
                    assistant_id: None,
                });
                enrich_tool_metadata(
                    &mut metadata,
                    ToolResultFacts {
                        raw_result: result.as_ref(),
                        is_error: None,
                        status: None,
                        artifact_path: None,
                    },
                );
                self.call_id_map.register(call_id, self.messages.len());
                if !query.is_empty() {
                    self.content_parts.push(query.to_string());
                }
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    tool_name: Some(metadata.canonical_name.clone()),
                    tool_input: action.map(|value| value.to_string()),
                    tool_metadata: Some(metadata),
                    ..Message::new(MessageRole::Tool, query.to_string())
                });
            }
            "image_generation_end" => {
                let Some(call_id) = codex_call_id(payload) else {
                    return;
                };
                let Some(message) = self
                    .call_id_map
                    .message_mut(Some(call_id), &mut self.messages)
                else {
                    self.record_unmatched_tool_event("image generation", call_id, path);
                    return;
                };

                if let Some(saved_path) = payload.get("saved_path").and_then(|v| v.as_str()) {
                    message.content = format!("[Image: source: {saved_path}]");
                }
                let result_value = codex_image_generation_result(payload);
                enrich_existing_tool_message(
                    message,
                    result_value,
                    None,
                    payload.get("status").and_then(|v| v.as_str()),
                );
            }
            "dynamic_tool_call_request" => {
                let input_value = dynamic_tool_input(payload);
                let raw_name = input_value
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("dynamic_tool_call");
                let metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Codex,
                    raw_name,
                    input: Some(&input_value),
                    call_id: codex_call_id(payload),
                    assistant_id: None,
                });
                self.call_id_map
                    .register(codex_call_id(payload), self.messages.len());
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    tool_name: Some(metadata.canonical_name.clone()),
                    tool_input: Some(input_value.to_string()),
                    tool_metadata: Some(metadata),
                    ..Message::new(MessageRole::Tool, String::new())
                });
            }
            "dynamic_tool_call_response" => {
                let output = codex_content_items_text(payload);
                if !output.is_empty() {
                    self.content_parts.push(output.clone());
                }
                let result_value = dynamic_tool_result(payload);
                let is_error = payload
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .map(|success| !success);
                let status = payload
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .map(|success| if success { "success" } else { "error" });

                if let Some(message) = self
                    .call_id_map
                    .message_mut(codex_call_id(payload), &mut self.messages)
                {
                    message.content = output;
                    enrich_existing_tool_message(message, result_value, is_error, status);
                    return;
                }

                let input_value = dynamic_tool_input(payload);
                let raw_name = input_value
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("dynamic_tool_call");
                let mut metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Codex,
                    raw_name,
                    input: Some(&input_value),
                    call_id: codex_call_id(payload),
                    assistant_id: None,
                });
                enrich_tool_metadata(
                    &mut metadata,
                    ToolResultFacts {
                        raw_result: Some(&result_value),
                        is_error,
                        status,
                        artifact_path: None,
                    },
                );
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    tool_name: Some(metadata.canonical_name.clone()),
                    tool_input: Some(input_value.to_string()),
                    tool_metadata: Some(metadata),
                    ..Message::new(MessageRole::Tool, output)
                });
            }
            "exec_command_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(message) = self
                    .call_id_map
                    .message_mut(Some(call_id), &mut self.messages)
                else {
                    self.record_unmatched_tool_event("exec_command", call_id, path);
                    return;
                };

                let result_value = codex_exec_command_event_result(payload, &message.content);
                let status = payload.get("status").and_then(|v| v.as_str());
                let is_error = status.map(|status| matches!(status, "failed" | "declined"));
                if (message.content.is_empty() || message.content.trim_start().starts_with('{'))
                    && let Some(formatted_output) = result_value
                        .get("formattedOutput")
                        .and_then(|v| v.as_str())
                        .filter(|v| !v.is_empty())
                        .or_else(|| {
                            result_value
                                .get("aggregatedOutput")
                                .and_then(|v| v.as_str())
                                .filter(|v| !v.is_empty())
                        })
                        .or_else(|| {
                            result_value
                                .get("stdout")
                                .and_then(|v| v.as_str())
                                .filter(|v| !v.is_empty())
                        })
                {
                    message.content = formatted_output.to_string();
                }
                enrich_existing_tool_message(message, result_value, is_error, status);
            }
            "mcp_tool_call_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(message) = self
                    .call_id_map
                    .message_mut(Some(call_id), &mut self.messages)
                else {
                    self.record_unmatched_tool_event("MCP tool", call_id, path);
                    return;
                };

                let result_value = codex_mcp_tool_call_event_result(payload);
                let is_error = result_value
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .map(|success| !success);
                enrich_existing_tool_message(message, result_value, is_error, None);
            }
            "patch_apply_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(message) = self
                    .call_id_map
                    .message_mut(Some(call_id), &mut self.messages)
                else {
                    self.record_unmatched_tool_event("apply_patch", call_id, path);
                    return;
                };

                let result_value = codex_patch_event_result(payload);
                let status = payload.get("status").and_then(|v| v.as_str());
                let is_error = payload.get("success").and_then(|v| v.as_bool()).map(|v| !v);
                if (message.content.is_empty() || message.content.trim_start().starts_with('{'))
                    && let Some(stdout) = result_value
                        .get("stdout")
                        .and_then(|v| v.as_str())
                        .filter(|v| !v.is_empty())
                {
                    message.content = stdout.to_string();
                }
                enrich_existing_tool_message(message, result_value, is_error, status);
            }
            "collab_agent_spawn_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(message) = self
                    .call_id_map
                    .message_mut(Some(call_id), &mut self.messages)
                else {
                    self.record_unmatched_tool_event("spawn_agent", call_id, path);
                    return;
                };

                let status = payload.get("status").and_then(|v| v.as_str());
                enrich_existing_tool_message(message, payload.clone(), None, status);
            }
            "collab_waiting_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(message) = self
                    .call_id_map
                    .message_mut(Some(call_id), &mut self.messages)
                else {
                    self.record_unmatched_tool_event("wait_agent", call_id, path);
                    return;
                };

                let status = message
                    .tool_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.structured.as_ref())
                    .and_then(|value| value.get("timed_out"))
                    .and_then(|value| value.as_bool())
                    .map(|timed_out| if timed_out { "timed_out" } else { "completed" });
                let is_error = status.map(|status| status == "timed_out");
                enrich_existing_tool_message(message, payload.clone(), is_error, status);
            }
            // New multi-agent runtime (Codex 0.144+): subagent lifecycle
            // events. `event_id` is the triggering tool call's call_id;
            // `agent_thread_id` is the child SESSION id — surfacing it as
            // `agentId` lets the frontend's "Open subagent" resolve the child
            // directly (spawn_agent arguments are encrypted, so the old
            // nickname/description matching has nothing to work with).
            "sub_agent_activity" => {
                let Some(call_id) = payload.get("event_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(message) = self
                    .call_id_map
                    .message_mut(Some(call_id), &mut self.messages)
                else {
                    self.record_unmatched_tool_event("subagent", call_id, path);
                    return;
                };
                let kind = payload.get("kind").and_then(|v| v.as_str());
                let mut result = serde_json::Map::new();
                if let Some(id) = payload.get("agent_thread_id").and_then(|v| v.as_str()) {
                    result.insert("agentId".to_string(), Value::String(id.to_string()));
                }
                if let Some(agent_path) = payload.get("agent_path").and_then(|v| v.as_str()) {
                    result.insert(
                        "agentPath".to_string(),
                        Value::String(agent_path.to_string()),
                    );
                }
                enrich_existing_tool_message(message, Value::Object(result), None, kind);
            }
            // Reasoning stream sections; consecutive sections merge into one
            // thinking block so a single model turn doesn't shatter into
            // dozens of rows (a tool call in between naturally splits them).
            "agent_reasoning" => {
                let text = payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .replace("<!-- -->", "");
                let text = text.trim();
                if text.is_empty() {
                    return;
                }
                if let Some(last) = self.messages.last_mut()
                    && last.role == MessageRole::System
                    && last.content.starts_with("[thinking]\n")
                {
                    last.content.push_str("\n\n");
                    last.content.push_str(text);
                    return;
                }
                push_system_event(
                    &mut self.messages,
                    entry.timestamp.clone(),
                    format!("[thinking]\n{text}"),
                );
            }
            "thread_rolled_back" => {
                let turns = payload
                    .get("num_turns")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                push_system_event(
                    &mut self.messages,
                    entry.timestamp.clone(),
                    format!("[turn_aborted] rolled back {turns} turn(s)"),
                );
            }
            "collab_agent_interaction_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(message) = self
                    .call_id_map
                    .message_mut(Some(call_id), &mut self.messages)
                else {
                    self.record_unmatched_tool_event("send_input", call_id, path);
                    return;
                };

                let status = payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .or(Some("completed"));
                enrich_existing_tool_message(message, payload.clone(), None, status);
            }
            "collab_close_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(message) = self
                    .call_id_map
                    .message_mut(Some(call_id), &mut self.messages)
                else {
                    self.record_unmatched_tool_event("close_agent", call_id, path);
                    return;
                };

                enrich_existing_tool_message(message, payload.clone(), None, Some("completed"));
            }
            _ => {
                flush_pending_user_message(
                    &mut self.pending_user_message,
                    &mut self.messages,
                    &mut self.content_parts,
                    &mut self.first_user_message,
                );
            }
        }
    }
}
