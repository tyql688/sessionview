//! `event_msg` line handler for the Codex per-line dispatch.
//!
//! Holds the single `CodexScanAccum::handle_event_msg` method,
//! relocated verbatim from `scan_lines`'s `"event_msg"` arm. The method
//! stays one cohesive unit (the natural per-event-kind `match`); it
//! lives in its own file purely to keep `parser/mod.rs` under the
//! module size limit. `scan_lines` (in `mod.rs`) calls it unchanged.

use std::path::Path;

use serde_json::Value;

use crate::models::{Message, MessageRole, Provider};
use crate::provider::UsageEvent;
use crate::tool_metadata::{
    build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
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
use super::{append_user_message, flush_pending_user_message, CodexLine, CodexScanAccum};

impl CodexScanAccum {
    /// Handle an `event_msg` line. Lifted verbatim from the `scan_lines`
    /// dispatch arm: every `continue` (skip-this-line) in the original
    /// arm becomes a `return`, since this method is the last action
    /// `scan_lines` takes for the line — returning lets the loop advance
    /// to the next line exactly as `continue` did.
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
            "context_compacted" => {
                push_system_event(
                    &mut self.messages,
                    entry.timestamp.clone(),
                    "[context_compacted]".to_string(),
                );
            }
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
                    // Capture the event for the indexer's per-date stats
                    // in the same pass — replaces a second file read.
                    // No silent fallback: cost attribution
                    // requires a real model name. If none
                    // resolves we'd mislabel usage as GPT-5,
                    // so we drop BOTH the indexer event and
                    // the per-message attachment together —
                    // keeping only one half would leave the
                    // UI showing tokens with no provenance
                    // while the daily totals undercount.
                    let Some(resolved_model) = resolved_model else {
                        if any_nonzero {
                            log::warn!(
                                "Codex token_count event at {:?} has no resolvable model — skipping usage record",
                                entry.timestamp
                            );
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
                    if any_nonzero {
                        if let Some(ts) = entry.timestamp.as_ref() {
                            self.usage_events.push(UsageEvent {
                                timestamp: ts.clone(),
                                model: resolved_model.clone(),
                                input_tokens: input,
                                output_tokens: output,
                                cache_read_input_tokens: cached.min(input),
                            });
                        }
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

                if let Some(idx) = call_id.and_then(|cid| self.call_id_map.get(cid)).copied() {
                    if idx < self.messages.len() {
                        self.messages[idx].content = query.to_string();
                        if self.messages[idx].tool_input.is_none() {
                            self.messages[idx].tool_input = action.map(|value| value.to_string());
                        }
                        if let Some(metadata) = self.messages[idx].tool_metadata.as_mut() {
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
                let idx = self.messages.len();
                if let Some(call_id) = call_id {
                    self.call_id_map.insert(call_id.to_string(), idx);
                }
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
                let Some(idx) = self.call_id_map.get(call_id).copied() else {
                    log::warn!(
                        "missing Codex image generation tool message for event call_id {call_id} in '{}'", path.display());
                    return;
                };
                if idx >= self.messages.len() {
                    return;
                }

                if let Some(saved_path) = payload.get("saved_path").and_then(|v| v.as_str()) {
                    self.messages[idx].content = format!("[Image: source: {saved_path}]");
                }
                let result_value = codex_image_generation_result(payload);
                enrich_existing_tool_message(
                    &mut self.messages[idx],
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
                let idx = self.messages.len();
                if let Some(call_id) = codex_call_id(payload) {
                    self.call_id_map.insert(call_id.to_string(), idx);
                }
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

                if let Some(idx) = codex_call_id(payload)
                    .and_then(|cid| self.call_id_map.get(cid))
                    .copied()
                {
                    if idx < self.messages.len() {
                        self.messages[idx].content = output;
                        enrich_existing_tool_message(
                            &mut self.messages[idx],
                            result_value,
                            is_error,
                            status,
                        );
                        return;
                    }
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
                let Some(idx) = self.call_id_map.get(call_id).copied() else {
                    log::warn!(
                        "missing Codex exec_command tool message for event call_id {call_id} in '{}'", path.display());
                    return;
                };
                if idx >= self.messages.len() {
                    return;
                }

                let result_value =
                    codex_exec_command_event_result(payload, &self.messages[idx].content);
                let status = payload.get("status").and_then(|v| v.as_str());
                let is_error = status.map(|status| matches!(status, "failed" | "declined"));
                if self.messages[idx].content.is_empty()
                    || self.messages[idx].content.trim_start().starts_with('{')
                {
                    if let Some(formatted_output) = result_value
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
                        self.messages[idx].content = formatted_output.to_string();
                    }
                }
                enrich_existing_tool_message(
                    &mut self.messages[idx],
                    result_value,
                    is_error,
                    status,
                );
            }
            "mcp_tool_call_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(idx) = self.call_id_map.get(call_id).copied() else {
                    log::warn!(
                        "missing Codex MCP tool message for event call_id {call_id} in '{}'",
                        path.display()
                    );
                    return;
                };
                if idx >= self.messages.len() {
                    return;
                }

                let result_value = codex_mcp_tool_call_event_result(payload);
                let is_error = result_value
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .map(|success| !success);
                enrich_existing_tool_message(&mut self.messages[idx], result_value, is_error, None);
            }
            "patch_apply_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(idx) = self.call_id_map.get(call_id).copied() else {
                    log::warn!(
                        "missing Codex apply_patch tool message for event call_id {call_id} in '{}'", path.display());
                    return;
                };
                if idx >= self.messages.len() {
                    return;
                }

                let result_value = codex_patch_event_result(payload);
                let status = payload.get("status").and_then(|v| v.as_str());
                let is_error = payload.get("success").and_then(|v| v.as_bool()).map(|v| !v);
                if self.messages[idx].content.is_empty()
                    || self.messages[idx].content.trim_start().starts_with('{')
                {
                    if let Some(stdout) = result_value
                        .get("stdout")
                        .and_then(|v| v.as_str())
                        .filter(|v| !v.is_empty())
                    {
                        self.messages[idx].content = stdout.to_string();
                    }
                }
                enrich_existing_tool_message(
                    &mut self.messages[idx],
                    result_value,
                    is_error,
                    status,
                );
            }
            "collab_agent_spawn_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(idx) = self.call_id_map.get(call_id).copied() else {
                    log::warn!(
                        "missing Codex spawn_agent tool message for event call_id {call_id} in '{}'", path.display());
                    return;
                };
                if idx >= self.messages.len() {
                    return;
                }

                let status = payload.get("status").and_then(|v| v.as_str());
                enrich_existing_tool_message(
                    &mut self.messages[idx],
                    payload.clone(),
                    None,
                    status,
                );
            }
            "collab_waiting_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(idx) = self.call_id_map.get(call_id).copied() else {
                    log::warn!(
                        "missing Codex wait_agent tool message for event call_id {call_id} in '{}'",
                        path.display()
                    );
                    return;
                };
                if idx >= self.messages.len() {
                    return;
                }

                let status = self.messages[idx]
                    .tool_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.structured.as_ref())
                    .and_then(|value| value.get("timed_out"))
                    .and_then(|value| value.as_bool())
                    .map(|timed_out| if timed_out { "timed_out" } else { "completed" });
                let is_error = status.map(|status| status == "timed_out");
                enrich_existing_tool_message(
                    &mut self.messages[idx],
                    payload.clone(),
                    is_error,
                    status,
                );
            }
            "collab_agent_interaction_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(idx) = self.call_id_map.get(call_id).copied() else {
                    log::warn!(
                        "missing Codex send_input tool message for event call_id {call_id} in '{}'",
                        path.display()
                    );
                    return;
                };
                if idx >= self.messages.len() {
                    return;
                }

                let status = payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .or(Some("completed"));
                enrich_existing_tool_message(
                    &mut self.messages[idx],
                    payload.clone(),
                    None,
                    status,
                );
            }
            "collab_close_end" => {
                let Some(call_id) = payload.get("call_id").and_then(|v| v.as_str()) else {
                    return;
                };
                let Some(idx) = self.call_id_map.get(call_id).copied() else {
                    log::warn!(
                        "missing Codex close_agent tool message for event call_id {call_id} in '{}'", path.display());
                    return;
                };
                if idx >= self.messages.len() {
                    return;
                }

                enrich_existing_tool_message(
                    &mut self.messages[idx],
                    payload.clone(),
                    None,
                    Some("completed"),
                );
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
