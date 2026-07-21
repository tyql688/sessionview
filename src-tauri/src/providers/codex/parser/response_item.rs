//! `response_item` line handler for the Codex per-line dispatch. Split out of
//! `parser/mod.rs` for size; the per-event-kind `match` stays one unit.

use std::path::Path;

use serde_json::{Value, json};

use crate::models::{Message, MessageRole, Provider};
use crate::provider_utils::{RenderedToolOutput, is_system_content};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata, set_tool_result_raw,
};

use super::super::tools::*;
use super::value_helpers::{
    codex_call_id, codex_content_items_text, codex_image_generation_input, codex_tool_input_value,
    codex_tool_result_value, enrich_existing_tool_message, push_system_event,
};
use super::{CodexLine, CodexScanAccum, PendingCodexUserMessage, flush_pending_user_message};

impl CodexScanAccum {
    /// Handle a `response_item` line. `return` means skip the line: this is
    /// the last action `scan_lines` takes for it.
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

                self.call_id_map.register(
                    payload.get("call_id").and_then(|v| v.as_str()),
                    self.messages.len(),
                );
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    tool_name: Some(display_name.to_string()),
                    tool_input,
                    tool_metadata: Some(metadata),
                    ..Message::new(MessageRole::Tool, String::new())
                });
            }
            "function_call_output" => {
                let output_value = payload.get("output");
                let raw_output = match output_value {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                let RenderedToolOutput {
                    text: output,
                    is_raw,
                    ..
                } = render_tool_output(output_value);

                if !output.is_empty() {
                    self.content_parts.push(output.clone());
                }

                // Merge output into the matching function_call message
                let call_id = payload.get("call_id").and_then(|v| v.as_str());
                if let Some(message) = self.call_id_map.message_mut(call_id, &mut self.messages) {
                    let result_value = codex_tool_result_value(&raw_output, &output);
                    message.content = output;
                    let is_error = result_value.as_ref().and_then(|value| {
                        value
                            .get("exitCode")
                            .and_then(|code| code.as_i64())
                            .map(|code| code != 0)
                    });
                    if let Some(result_value) = result_value {
                        enrich_existing_tool_message(message, result_value, is_error, None);
                    }
                    if let Some(metadata) = message.tool_metadata.as_mut() {
                        set_tool_result_raw(metadata, is_raw);
                    }
                    return;
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
                        raw_output: None,
                    },
                );
                if !query.is_empty() {
                    self.content_parts.push(query.to_string());
                }
                self.call_id_map.register(call_id, self.messages.len());
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
                self.call_id_map.register(call_id, self.messages.len());
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
            "tool_search_call" => {
                let input_value = payload.clone();
                let metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Codex,
                    raw_name: "ToolSearch",
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
            "tool_search_output" => {
                let output = codex_content_items_text(payload);
                if !output.is_empty() {
                    self.content_parts.push(output.clone());
                }
                if let Some(message) = self
                    .call_id_map
                    .message_mut(codex_call_id(payload), &mut self.messages)
                {
                    message.content = output;
                    enrich_existing_tool_message(
                        message,
                        payload.clone(),
                        None,
                        payload.get("status").and_then(|v| v.as_str()),
                    );
                }
            }
            // Inter-agent mail in multi-agent runs. The payload body is
            // encrypted by design; only the readable routing header renders.
            "agent_message" => {
                let author = payload
                    .get("author")
                    .and_then(|v| v.as_str())
                    .unwrap_or("agent");
                let recipient = payload
                    .get("recipient")
                    .and_then(|v| v.as_str())
                    .unwrap_or("agent");
                let text = codex_content_items_text(payload);
                let text = text.trim();
                if text.is_empty() {
                    return;
                }
                push_system_event(
                    &mut self.messages,
                    entry.timestamp.clone(),
                    format!("[agent_mail] {author} \u{2192} {recipient}\n{text}"),
                );
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

                self.call_id_map.register(
                    payload.get("call_id").and_then(|v| v.as_str()),
                    self.messages.len(),
                );
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    tool_name: Some(display_name.to_string()),
                    tool_input: input,
                    tool_metadata: Some(metadata),
                    ..Message::new(MessageRole::Tool, String::new())
                });
            }
            "custom_tool_call_output" => {
                let output_value = payload.get("output");
                let raw_output = match output_value {
                    Some(Value::String(text)) => text.clone(),
                    Some(value) => value.to_string(),
                    None => String::new(),
                };
                let RenderedToolOutput {
                    text: output,
                    is_raw,
                    ..
                } = render_tool_output(output_value);

                let call_id = payload.get("call_id").and_then(|v| v.as_str());
                if let Some(message) = self.call_id_map.message_mut(call_id, &mut self.messages) {
                    let result_value = codex_tool_result_value(&raw_output, &output);
                    message.content = output;
                    let is_error = result_value.as_ref().and_then(|value| {
                        value
                            .get("exitCode")
                            .and_then(|code| code.as_i64())
                            .map(|code| code != 0)
                    });
                    if let Some(result_value) = result_value {
                        enrich_existing_tool_message(message, result_value, is_error, None);
                    }
                    if let Some(metadata) = message.tool_metadata.as_mut() {
                        set_tool_result_raw(metadata, is_raw);
                    }
                    return;
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
