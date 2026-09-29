//! Desktop rollouts persist nested code-mode tools as `item_completed`.
//! These ids identify actual tool executions, independently of the outer
//! `exec` call. Never infer calls by parsing JavaScript or matching output
//! text. Older lifecycle events reuse their call ids and can enrich the
//! same rows. Assistant response mirrors share a response id; user and
//! compaction mirrors are already represented by their transcript records.
//! Web tools use either `WebSearch` or the older `Extension/web.search`
//! shape; both carry the query, action, and results of `web_search_end`.

use std::path::Path;

use serde_json::{Value, json};

use crate::models::{Message, Provider};
use crate::tool_metadata::{ToolCallFacts, build_tool_metadata};

use super::super::tools::render_tool_output;
use super::value_helpers::enrich_existing_tool_message;
use super::{CodexLine, CodexScanAccum};

impl CodexScanAccum {
    pub(super) fn handle_completed_item(
        &mut self,
        entry: &CodexLine,
        payload: &Value,
        path: &Path,
    ) {
        let Some(item) = payload.get("item").filter(|v| v.is_object()) else {
            self.warn_completed_item(path, entry, "missing item");
            return;
        };
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
        let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            self.warn_completed_item(path, entry, "missing item id");
            return;
        };
        if !self.seen_completed_items.insert(id.to_string()) {
            return;
        }
        let mut event = item.clone();
        event["call_id"] = json!(id);
        match kind {
            "CommandExecution" => {
                if !item
                    .get("command")
                    .is_some_and(|v| v.is_string() || v.is_array())
                {
                    self.warn_completed_item(path, entry, "command execution missing command");
                    return;
                }
                event["type"] = json!("exec_command_end");
            }
            "FileChange" => {
                if !item.get("changes").is_some_and(Value::is_object) {
                    self.warn_completed_item(path, entry, "file change missing changes");
                    return;
                }
                event["type"] = json!("patch_apply_end");
            }
            "McpToolCall" => {
                let (Some(server), Some(tool)) = (
                    item.get("server").and_then(Value::as_str),
                    item.get("tool").and_then(Value::as_str),
                ) else {
                    self.warn_completed_item(path, entry, "MCP call missing server or tool");
                    return;
                };
                event["type"] = json!("mcp_tool_call_end");
                event["invocation"] =
                    json!({"server": server, "tool": tool, "arguments": item.get("arguments")});
                if let Some(result) = item.get("result").filter(|v| !v.is_null()) {
                    event["result"] = json!({"Ok": result});
                } else if let Some(error) = item.get("error") {
                    event["result"] = json!({"Err": error});
                } else {
                    self.warn_completed_item(path, entry, "MCP call missing result or error");
                    return;
                }
            }
            "DynamicToolCall" => {
                let Some(tool) = item.get("tool").and_then(Value::as_str) else {
                    self.warn_completed_item(path, entry, "dynamic call missing tool");
                    return;
                };
                if self
                    .call_id_map
                    .message_mut(Some(id), &mut self.messages)
                    .is_none()
                {
                    self.push_event_only_tool_call(
                        tool,
                        id,
                        item.get("arguments").cloned(),
                        entry.timestamp.clone(),
                    );
                }
                event["type"] = json!("dynamic_tool_call_response");
            }
            "WebSearch" => event["type"] = json!("web_search_end"),
            "Extension" => match item.get("kind").and_then(Value::as_str) {
                Some("clock.sleep") => {
                    let Some(duration) = item.get("durationMs").and_then(Value::as_u64) else {
                        self.warn_completed_item(path, entry, "clock sleep missing valid duration");
                        return;
                    };
                    self.upsert_completed_tool(
                        entry,
                        id,
                        "clock.sleep",
                        Some(json!({"duration_ms": duration})),
                        String::new(),
                        item.clone(),
                    );
                    return;
                }
                Some("web.search") => event["type"] = json!("web_search_end"),
                Some("image_gen.generation") => {
                    if self
                        .call_id_map
                        .message_mut(Some(id), &mut self.messages)
                        .is_none()
                    {
                        self.push_event_only_tool_call(
                            "image_generation_call",
                            id,
                            item.get("revisedPrompt")
                                .map(|p| json!({"revised_prompt": p})),
                            entry.timestamp.clone(),
                        );
                    }
                    event["type"] = json!("image_generation_end");
                    for (from, to) in [
                        ("savedPath", "saved_path"),
                        ("revisedPrompt", "revised_prompt"),
                    ] {
                        if let Some(value) = item.get(from) {
                            event[to] = value.clone();
                        }
                    }
                }
                _ => {
                    self.warn_completed_item(path, entry, "unknown extension kind");
                    return;
                }
            },
            "ImageView" => {
                let Some(image_path) = item.get("path").and_then(Value::as_str) else {
                    self.warn_completed_item(path, entry, "image view missing path");
                    return;
                };
                self.upsert_completed_tool(
                    entry,
                    id,
                    "view_image",
                    Some(json!({"path": image_path})),
                    format!("[Image: source: {image_path}]"),
                    item.clone(),
                );
                return;
            }
            "FunctionCallOutput" => {
                let Some(name) = item.get("name").and_then(Value::as_str) else {
                    self.warn_completed_item(path, entry, "function output missing name");
                    return;
                };
                let output = render_tool_output(item.get("output"));
                self.upsert_completed_tool(entry, id, name, None, output.text, item.clone());
                return;
            }
            "SubAgentActivity" => {
                let Some(agent_id) = item.get("agent_thread_id").and_then(Value::as_str) else {
                    self.warn_completed_item(path, entry, "agent activity missing thread id");
                    return;
                };
                let mut result = item.clone();
                result["agentId"] = json!(agent_id);
                self.upsert_completed_tool(entry, id, "spawn_agent", None, String::new(), result);
                event["type"] = json!("sub_agent_activity");
                event["event_id"] = json!(id);
            }
            "CollabAgentToolCall" => {
                let name = match item.get("tool").and_then(Value::as_str) {
                    Some("spawn") => "spawn_agent",
                    Some("wait") => "wait_agent",
                    Some("send_input") => "send_input",
                    Some("close") => "close_agent",
                    Some("resume") => "resume_agent",
                    _ => {
                        self.warn_completed_item(path, entry, "unknown collaboration tool");
                        return;
                    }
                };
                let mut result = item.clone();
                if let Some(ids) = item.get("receiver_thread_ids").and_then(Value::as_array) {
                    result["childConversationIds"] = json!(
                        ids.iter()
                            .filter_map(Value::as_str)
                            .filter(|id| !id.is_empty())
                            .collect::<Vec<_>>()
                    );
                }
                self.upsert_completed_tool(entry, id, name, None, String::new(), result);
                return;
            }
            "AgentMessage" => {
                let mut response = item.clone();
                response["type"] = json!("message");
                response["role"] = json!("assistant");
                self.handle_response_item(entry, &response, path);
                return;
            }
            "Reasoning" => {
                let text = ["summary_text", "raw_content"].into_iter().find_map(|key| {
                    let parts = item.get(key)?.as_array()?;
                    let text = parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    (!text.trim().is_empty()).then_some(text)
                });
                if let Some(text) = text {
                    event["type"] = json!("agent_reasoning");
                    event["text"] = json!(text);
                } else {
                    return;
                }
            }
            "Plan" => {
                let Some(text) = item
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|v| !v.trim().is_empty())
                else {
                    self.warn_completed_item(path, entry, "plan missing text");
                    return;
                };
                self.content_parts.push(text.to_string());
                self.messages.push(Message {
                    timestamp: entry.timestamp.clone(),
                    model: self.current_model.clone(),
                    ..Message::assistant(text.to_string())
                });
                return;
            }
            // Mirrors of response_item/message/user and top-level compacted.
            "UserMessage" | "ContextCompaction" => return,
            _ => {
                self.warn_completed_item(path, entry, &format!("unknown item type '{kind}'"));
                return;
            }
        }
        self.handle_event_msg(entry, &event, path);
        if let Some(message) = self.call_id_map.message_mut(Some(id), &mut self.messages)
            && !message.content.is_empty()
        {
            self.content_parts.push(message.content.clone());
        }
    }

    fn warn_completed_item(&mut self, path: &Path, entry: &CodexLine, reason: &str) {
        log::warn!(
            "skipping Codex item_completed in '{}' at {:?}: {reason}",
            path.display(),
            entry.timestamp
        );
        self.parse_warning_count = self.parse_warning_count.saturating_add(1);
    }

    fn upsert_completed_tool(
        &mut self,
        entry: &CodexLine,
        id: &str,
        name: &str,
        input: Option<Value>,
        content: String,
        result: Value,
    ) {
        if self
            .call_id_map
            .message_mut(Some(id), &mut self.messages)
            .is_none()
        {
            self.push_event_only_tool_call(name, id, input.clone(), entry.timestamp.clone());
        }
        let Some(message) = self.call_id_map.message_mut(Some(id), &mut self.messages) else {
            return;
        };
        if message.tool_metadata.is_none() {
            let metadata = build_tool_metadata(ToolCallFacts {
                provider: Provider::Codex,
                raw_name: name,
                input: input.as_ref(),
                call_id: Some(id),
                assistant_id: None,
            });
            message.tool_name = Some(metadata.canonical_name.clone());
            message.tool_metadata = Some(metadata);
        }
        if !content.is_empty() {
            self.content_parts.push(content.clone());
            message.content = content;
        }
        enrich_existing_tool_message(message, result, None, None);
    }
}
