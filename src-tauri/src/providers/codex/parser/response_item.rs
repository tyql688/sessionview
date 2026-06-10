//! `response_item` line handler for the Codex per-line dispatch.
//!
//! Holds the single `CodexScanAccum::handle_response_item` method,
//! relocated verbatim from `scan_lines`'s `"response_item"` arm. The
//! method stays one cohesive unit (the natural per-event-kind `match`);
//! it lives in its own file purely to keep `parser/mod.rs` under the
//! module size limit. `scan_lines` (in `mod.rs`) calls it unchanged.

use std::path::Path;

use serde_json::{json, Value};

use crate::models::{Message, MessageRole, Provider};
use crate::provider_utils::is_system_content;
use crate::tool_metadata::{
    build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

use super::super::tools::*;
use super::value_helpers::{
    codex_call_id, codex_content_items_text, codex_image_generation_input, codex_tool_input_value,
    codex_tool_result_value, enrich_existing_tool_message,
};
use super::{flush_pending_user_message, CodexLine, CodexScanAccum, PendingCodexUserMessage};

impl CodexScanAccum {
    /// Handle a `response_item` line. Lifted verbatim from the
    /// `scan_lines` dispatch arm: every `continue` (skip-this-line) in
    /// the original arm becomes a `return`, since this method is the
    /// last action `scan_lines` takes for the line — returning lets the
    /// loop advance to the next line exactly as `continue` did.
    pub(super) fn handle_response_item(&mut self, entry: &CodexLine, payload: &Value, path: &Path) {
        // Skip developer role and reasoning type
        let role_str = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let item_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");

        if role_str == "developer" || item_type == "reasoning" {
            return;
        }

        if !(item_type == "message" && role_str == "user") {
            flush_pending_user_message(
                &mut self.pending_user_message,
                &mut self.messages,
                &mut self.content_parts,
                &mut self.first_user_message,
            );
        }

        match item_type {
            "message" => {
                let text = omit_base64_image_sources(&extract_codex_content(payload));
                let normalized_text = strip_inline_image_sources(&text);
                let role = match role_str {
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    _ => return,
                };

                // Skip empty self.messages and system/environment XML content
                if text.is_empty() {
                    return;
                }
                let trimmed = normalized_text.trim_start();
                if is_system_content(trimmed) {
                    return;
                }

                if role == MessageRole::User {
                    let image_segments = extract_image_source_segments(&text);
                    flush_pending_user_message(
                        &mut self.pending_user_message,
                        &mut self.messages,
                        &mut self.content_parts,
                        &mut self.first_user_message,
                    );
                    self.pending_user_message = Some(PendingCodexUserMessage {
                        content: text,
                        timestamp: entry.timestamp.clone(),
                        image_segments,
                    });
                    return;
                }

                let msg_model = if role == MessageRole::Assistant {
                    self.current_model.clone()
                } else {
                    None
                };
                if !normalized_text.is_empty() {
                    self.content_parts.push(normalized_text);
                }
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    model: msg_model,
                    ..Message::new(role, text)
                });
            }
            "function_call" => {
                let raw_name = payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let arguments_str = payload.get("arguments").and_then(|v| v.as_str());

                // For exec_command, remap arguments to match Bash tool format
                let tool_input = match raw_name {
                    "exec_command" | "shell_command" => {
                        // Remap {"cmd": "..."} to {"command": "..."}; keep already-normalized command args.
                        arguments_str.and_then(|s| {
                            let v: Value = match serde_json::from_str(s) {
                                Ok(value) => value,
                                Err(error) => {
                                    log::warn!(
                                        "failed to parse Codex tool arguments in '{}': {error}",
                                        path.display()
                                    );
                                    return None;
                                }
                            };
                            let cmd = v
                                .get("cmd")
                                .or_else(|| v.get("command"))
                                .and_then(|c| c.as_str())?;
                            Some(json!({"command": cmd}).to_string())
                        })
                    }
                    "view_image" => {
                        // Emit as image message instead of tool
                        if let Some(path) =
                            arguments_str.and_then(|s| match serde_json::from_str::<Value>(s) {
                                Ok(value) => value
                                    .get("path")
                                    .and_then(|p| p.as_str())
                                    .map(|s| s.to_string()),
                                Err(error) => {
                                    log::warn!(
                                        "failed to parse Codex view_image arguments in '{}': {error}", path.display());
                                    None
                                }
                            })
                        {
                            self.messages.push(Message {
                                timestamp: entry.timestamp.clone(),
                                ..Message::assistant(format!("[Image: source: {path}]"))
                            });
                            return;
                        }
                        None
                    }
                    "write_stdin" => {
                        // Skip empty stdin writes (just polling)
                        let is_empty = arguments_str
                            .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            .and_then(|v| {
                                v.get("chars")
                                    .and_then(|c| c.as_str())
                                    .map(|s| s.is_empty())
                            })
                            .unwrap_or(true);
                        if is_empty {
                            return;
                        }
                        arguments_str.map(|s| s.to_string())
                    }
                    _ => arguments_str.map(|s| s.to_string()),
                };
                let input_value =
                    codex_tool_input_value(raw_name, arguments_str, tool_input.as_deref());
                let metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Codex,
                    raw_name,
                    input: input_value.as_ref(),
                    call_id: payload.get("call_id").and_then(|v| v.as_str()),
                    assistant_id: None,
                });
                let display_name = metadata.canonical_name.clone();

                let idx = self.messages.len();
                if let Some(cid) = payload.get("call_id").and_then(|v| v.as_str()) {
                    self.call_id_map.insert(cid.to_string(), idx);
                }
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    tool_name: Some(display_name.to_string()),
                    tool_input,
                    tool_metadata: Some(metadata),
                    ..Message::new(MessageRole::Tool, String::new())
                });
            }
            "function_call_output" => {
                let raw_output = match payload.get("output") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => serde_json::to_string(other).unwrap_or_default(),
                    None => String::new(),
                };
                let output = extract_tool_output(&raw_output);

                if !output.is_empty() {
                    self.content_parts.push(output.clone());
                }

                // Merge output into the matching function_call message
                let call_id = payload.get("call_id").and_then(|v| v.as_str());
                if let Some(idx) = call_id.and_then(|cid| self.call_id_map.get(cid)).copied() {
                    if idx < self.messages.len() {
                        let result_value = codex_tool_result_value(&raw_output, &output);
                        self.messages[idx].content = output;
                        let is_error = result_value.as_ref().and_then(|value| {
                            value
                                .get("exitCode")
                                .and_then(|code| code.as_i64())
                                .map(|code| code != 0)
                        });
                        if let Some(result_value) = result_value {
                            enrich_existing_tool_message(
                                &mut self.messages[idx],
                                result_value,
                                is_error,
                                None,
                            );
                        }
                        return;
                    }
                }
                // Fallback: standalone output message
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    ..Message::new(MessageRole::Tool, output)
                });
            }
            "web_search_call" => {
                let action = payload.get("action");
                let call_id = payload.get("id").and_then(|v| v.as_str());
                let query = action
                    .and_then(|a| a.get("query"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
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
                        raw_result: Some(payload),
                        is_error: payload
                            .get("status")
                            .and_then(|v| v.as_str())
                            .map(|status| matches!(status, "failed" | "error")),
                        status: payload.get("status").and_then(|v| v.as_str()),
                        artifact_path: None,
                    },
                );
                if !query.is_empty() {
                    self.content_parts.push(query.to_string());
                }
                let idx = self.messages.len();
                if let Some(call_id) = call_id {
                    self.call_id_map.insert(call_id.to_string(), idx);
                }
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    tool_name: Some(metadata.canonical_name.clone()),
                    tool_input: action.map(|value| value.to_string()),
                    tool_metadata: Some(metadata),
                    ..Message::new(MessageRole::Tool, query.to_string())
                });
            }
            "image_generation_call" => {
                let call_id = codex_call_id(payload);
                let input_value = codex_image_generation_input(payload);
                let metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Codex,
                    raw_name: "image_generation_call",
                    input: Some(&input_value),
                    call_id,
                    assistant_id: None,
                });
                let idx = self.messages.len();
                if let Some(call_id) = call_id {
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
            // Older Codex rollouts emit `local_shell_call` as
            // a `response_item` payload. Current rollouts in
            // this repo's local data corpus don't, but the
            // canonical-name table still maps it to Bash, so
            // we keep the snake_case dispatch for backward
            // compatibility with archived sessions. PascalCase
            // (`LocalShellCall`) is genuinely dead.
            "local_shell_call" => {
                let input_value = payload.clone();
                let metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Codex,
                    raw_name: "LocalShellCall",
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
            "tool_search_call" => {
                let input_value = payload.clone();
                let metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Codex,
                    raw_name: "ToolSearch",
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
            "tool_search_output" => {
                let output = codex_content_items_text(payload);
                if !output.is_empty() {
                    self.content_parts.push(output.clone());
                }
                if let Some(idx) = codex_call_id(payload)
                    .and_then(|cid| self.call_id_map.get(cid))
                    .copied()
                {
                    self.messages[idx].content = output;
                    enrich_existing_tool_message(
                        &mut self.messages[idx],
                        payload.clone(),
                        None,
                        payload.get("status").and_then(|v| v.as_str()),
                    );
                }
            }
            "custom_tool_call" => {
                let raw_name = payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                let input = payload.get("input").map(|v| {
                    if let Some(s) = v.as_str() {
                        s.to_string()
                    } else {
                        serde_json::to_string(v).unwrap_or_default()
                    }
                });
                let input_value = codex_tool_input_value(raw_name, None, input.as_deref());
                let metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Codex,
                    raw_name,
                    input: input_value.as_ref(),
                    call_id: payload.get("call_id").and_then(|v| v.as_str()),
                    assistant_id: None,
                });
                let display_name = metadata.canonical_name.clone();

                let idx = self.messages.len();
                if let Some(cid) = payload.get("call_id").and_then(|v| v.as_str()) {
                    self.call_id_map.insert(cid.to_string(), idx);
                }
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    tool_name: Some(display_name.to_string()),
                    tool_input: input,
                    tool_metadata: Some(metadata),
                    ..Message::new(MessageRole::Tool, String::new())
                });
            }
            "custom_tool_call_output" => {
                let raw_output = payload
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let output = extract_tool_output(&raw_output);

                let call_id = payload.get("call_id").and_then(|v| v.as_str());
                if let Some(idx) = call_id.and_then(|cid| self.call_id_map.get(cid)).copied() {
                    if idx < self.messages.len() {
                        let result_value = codex_tool_result_value(&raw_output, &output);
                        self.messages[idx].content = output;
                        let is_error = result_value.as_ref().and_then(|value| {
                            value
                                .get("exitCode")
                                .and_then(|code| code.as_i64())
                                .map(|code| code != 0)
                        });
                        if let Some(result_value) = result_value {
                            enrich_existing_tool_message(
                                &mut self.messages[idx],
                                result_value,
                                is_error,
                                None,
                            );
                        }
                        return;
                    }
                }
                if !output.is_empty() {
                    self.messages.push(Message {
                        timestamp: entry.timestamp.clone(),
                        ..Message::new(MessageRole::Tool, output)
                    });
                }
            }
            _ => (),
        }
    }
}
