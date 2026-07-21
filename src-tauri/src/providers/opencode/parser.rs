use crate::models::{Message, MessageRole, Provider, TokenUsage};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

fn opencode_tool_input_value(state: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let input = state?.get("input")?;
    if let Some(text) = input.as_str() {
        serde_json::from_str(text)
            .ok()
            .or_else(|| Some(serde_json::json!({ "input": text })))
    } else {
        Some(input.clone())
    }
}

fn opencode_tool_result_value(
    state: Option<&serde_json::Value>,
    output: &str,
) -> Option<serde_json::Value> {
    let state = state?;
    let mut result = state.clone();
    if let Some(obj) = result.as_object_mut()
        && !output.is_empty()
        && !obj.contains_key("output")
    {
        obj.insert("output".to_string(), serde_json::json!(output));
    }
    Some(result)
}

fn opencode_patch_part_value(part: &serde_json::Value) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "hash": part.get("hash")?.as_str()?,
        "files": part.get("files")?.clone(),
    }))
}

/// Build the user-facing messages for one OpenCode `role: user` message:
/// the joined text parts as one message, plus one image message per
/// `file` part with an `image/*` mime.
pub(super) fn build_user_messages(
    parts: &[serde_json::Value],
    timestamp: Option<&str>,
) -> Vec<Message> {
    let mut messages = Vec::new();

    let text_content: Vec<&str> = parts
        .iter()
        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
        .collect();

    if !text_content.is_empty() {
        messages.push(Message {
            timestamp: timestamp.map(str::to_string),
            ..Message::user(text_content.join("\n"))
        });
    }

    for part in parts {
        if part.get("type").and_then(|t| t.as_str()) == Some("file") {
            let mime = part.get("mime").and_then(|m| m.as_str()).unwrap_or("");
            let url = part.get("url").and_then(|u| u.as_str()).unwrap_or("");
            if mime.starts_with("image/") {
                if !url.is_empty() {
                    messages.push(Message {
                        timestamp: timestamp.map(str::to_string),
                        ..Message::user(format!("[Image: source: {url}]"))
                    });
                }
            } else {
                // Non-image attachments still belong in the transcript.
                let filename = part
                    .get("filename")
                    .and_then(|f| f.as_str())
                    .filter(|f| !f.is_empty())
                    .unwrap_or("attachment");
                messages.push(Message {
                    timestamp: timestamp.map(str::to_string),
                    ..Message::user(format!("[File: {filename}]"))
                });
            }
        }
    }

    messages
}

/// Build one tool message from an OpenCode assistant `tool` part, pairing
/// input with output/error (per `state.status`) plus enriched metadata.
fn build_tool_message(part: &serde_json::Value, msg_id: &str, timestamp: Option<&str>) -> Message {
    let tool_name = part
        .get("tool")
        .and_then(|t| t.as_str())
        .unwrap_or("tool")
        .to_string();
    let state = part.get("state");
    let status = state
        .and_then(|s| s.get("status"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    let input_value = opencode_tool_input_value(state);
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::OpenCode,
        raw_name: &tool_name,
        input: input_value.as_ref(),
        call_id: part
            .get("callID")
            .or_else(|| part.get("id"))
            .and_then(|v| v.as_str()),
        assistant_id: Some(msg_id),
    });

    let tool_input = state.and_then(|s| s.get("input")).map(|i| {
        i.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| i.to_string())
    });

    let (output, is_raw) = match (status, state) {
        ("completed", Some(state)) => render_opencode_result(state.get("output"), false),
        ("error", Some(state)) => render_opencode_result(state.get("error"), true),
        _ => (String::new(), false),
    };

    let result_value = opencode_tool_result_value(state, &output);
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_output: Some(is_raw),
            raw_result: result_value.as_ref(),
            is_error: Some(status == "error"),
            status: (!status.is_empty()).then_some(status),
            artifact_path: None,
        },
    );

    let tool_ts = state
        .and_then(|s| s.get("time"))
        .and_then(|t| t.get("start"))
        .and_then(|s| s.as_i64())
        .and_then(crate::provider::util::epoch_ms_to_rfc3339)
        .or_else(|| timestamp.map(str::to_string));

    Message {
        timestamp: tool_ts,
        tool_name: Some(metadata.canonical_name.clone()),
        tool_input,
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, output)
    }
}

fn render_opencode_result(value: Option<&serde_json::Value>, prefix_error: bool) -> (String, bool) {
    match value {
        Some(serde_json::Value::String(text)) if prefix_error => (format!("[Error] {text}"), false),
        Some(serde_json::Value::String(text)) => (text.clone(), false),
        Some(serde_json::Value::Null) | None => (String::new(), false),
        Some(value) => (value.to_string(), true),
    }
}

/// Build the messages for one OpenCode `role: assistant` message: reasoning
/// (`[thinking]`) parts, one text message, and the tool messages, threading
/// token usage onto the turn-final message.
pub(super) fn build_assistant_messages(
    parts: &[serde_json::Value],
    msg_json: &serde_json::Value,
    msg_id: &str,
    timestamp: Option<&str>,
) -> Vec<Message> {
    let mut messages = Vec::new();
    let token_usage = extract_tokens(msg_json);

    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_messages: Vec<Message> = Vec::new();
    let mut patch_parts: Vec<serde_json::Value> = Vec::new();

    for part in parts {
        let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match part_type {
            "text" => {
                if let Some(text) = part.get("text").and_then(|t| t.as_str())
                    && !text.is_empty()
                {
                    text_parts.push(text.to_string());
                }
            }
            "reasoning" => {
                if let Some(text) = part.get("text").and_then(|t| t.as_str())
                    && !text.trim().is_empty()
                {
                    let reasoning_ts = part
                        .get("time")
                        .and_then(|t| t.get("start"))
                        .and_then(|s| s.as_i64())
                        .and_then(crate::provider::util::epoch_ms_to_rfc3339)
                        .or_else(|| timestamp.map(str::to_string));
                    messages.push(Message {
                        timestamp: reasoning_ts,
                        ..Message::system(format!("[thinking]\n{text}"))
                    });
                }
            }
            "tool" => {
                tool_messages.push(build_tool_message(part, msg_id, timestamp));
            }
            "patch" => {
                if let Some(patch) = opencode_patch_part_value(part) {
                    patch_parts.push(patch);
                }
            }
            // Step markers and snapshots carry no transcript content.
            "step-start" | "step-finish" | "snapshot" => {}
            unknown => {
                log::warn!("skipping unknown OpenCode part type '{unknown}'");
            }
        }
    }

    if !patch_parts.is_empty() {
        for tool_message in tool_messages.iter_mut().rev() {
            let Some(metadata) = tool_message.tool_metadata.as_mut() else {
                continue;
            };
            if metadata.raw_name != "apply_patch" {
                continue;
            }

            let mut structured = metadata
                .structured
                .take()
                .unwrap_or_else(|| serde_json::json!({}));
            if !structured.is_object() {
                structured = serde_json::json!({});
            }
            if let Some(obj) = structured.as_object_mut() {
                if patch_parts.len() == 1 {
                    obj.insert("patch".to_string(), patch_parts[0].clone());
                } else {
                    obj.insert(
                        "patches".to_string(),
                        serde_json::Value::Array(patch_parts.clone()),
                    );
                }
            }
            metadata.structured = Some(structured);
            break;
        }
    }

    // Emit text message first (with token usage on last text msg of this turn)
    let msg_model = msg_json
        .get("modelID")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());

    if !text_parts.is_empty() {
        messages.push(Message {
            timestamp: timestamp.map(str::to_string),
            token_usage: if tool_messages.is_empty() {
                token_usage.clone()
            } else {
                None
            },
            model: msg_model.clone(),
            usage_hash: if tool_messages.is_empty() {
                Some(msg_id.to_string())
            } else {
                None
            },
            ..Message::assistant(text_parts.join("\n"))
        });
    }

    if !tool_messages.is_empty() {
        let last_idx = tool_messages.len() - 1;
        for (i, mut tool_msg) in tool_messages.into_iter().enumerate() {
            if i == last_idx {
                tool_msg.token_usage = token_usage.clone();
                tool_msg.model = msg_model.clone();
                tool_msg.usage_hash = Some(msg_id.to_string());
            }
            messages.push(tool_msg);
        }
    }

    // If assistant message had no text and no tools (rare), still emit for token tracking.
    if text_parts.is_empty()
        && !parts
            .iter()
            .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("tool"))
        && token_usage.is_some()
    {
        messages.push(Message {
            timestamp: timestamp.map(str::to_string),
            token_usage,
            model: msg_model,
            usage_hash: Some(msg_id.to_string()),
            ..Message::assistant(String::new())
        });
    }

    messages
}

/// Extract token usage from an assistant message's `data.tokens` JSON.
pub(crate) fn extract_tokens(msg_json: &serde_json::Value) -> Option<TokenUsage> {
    let tokens = msg_json.get("tokens")?;
    let count = |field: &str, value: Option<&serde_json::Value>, required: bool| {
        let raw = match value {
            Some(value) => value.as_u64(),
            None if !required => Some(0),
            None => None,
        };
        let Some(raw) = raw else {
            log::warn!("skipping OpenCode usage with invalid {field}");
            return None;
        };
        match u32::try_from(raw) {
            Ok(value) => Some(value),
            Err(_) => {
                log::warn!(
                    "skipping OpenCode usage with {field}={raw} outside the supported range"
                );
                None
            }
        }
    };
    let input_tokens = count("tokens.input", tokens.get("input"), true)?;
    let output_tokens = count("tokens.output", tokens.get("output"), true)?;
    let reasoning_tokens = count("tokens.reasoning", tokens.get("reasoning"), false)?;
    let output_tokens = output_tokens.checked_add(reasoning_tokens).or_else(|| {
        log::warn!("skipping OpenCode usage whose output and reasoning total exceeds u32");
        None
    })?;
    let cache = tokens.get("cache");
    if cache.is_some_and(|value| !value.is_object()) {
        log::warn!("skipping OpenCode usage with invalid tokens.cache");
        return None;
    }
    Some(TokenUsage {
        input_tokens,
        output_tokens,
        cache_read_input_tokens: count(
            "tokens.cache.read",
            cache.and_then(|value| value.get("read")),
            false,
        )?,
        cache_creation_input_tokens: count(
            "tokens.cache.write",
            cache.and_then(|value| value.get("write")),
            false,
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ToolResultMode;
    use serde_json::json;

    const TS: &str = "2026-01-02T03:04:05+00:00";

    #[test]
    fn build_user_messages_joins_text_parts() {
        let parts = vec![
            json!({ "type": "text", "text": "first line" }),
            json!({ "type": "text", "text": "second line" }),
        ];
        let msgs = build_user_messages(&parts, Some(TS));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, MessageRole::User);
        assert_eq!(msgs[0].content, "first line\nsecond line");
        assert_eq!(msgs[0].timestamp.as_deref(), Some(TS));
    }

    #[test]
    fn build_user_messages_emits_image_marker_for_image_file() {
        let parts = vec![
            json!({ "type": "text", "text": "look at this" }),
            json!({ "type": "file", "mime": "image/png", "url": "/tmp/cache/a.png" }),
            // Non-image files stay visible as attachment markers.
            json!({ "type": "file", "mime": "application/pdf", "filename": "doc.pdf", "url": "/tmp/doc.pdf" }),
        ];
        let msgs = build_user_messages(&parts, Some(TS));
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].content, "look at this");
        assert_eq!(msgs[1].content, "[Image: source: /tmp/cache/a.png]");
        assert_eq!(msgs[2].content, "[File: doc.pdf]");
    }

    #[test]
    fn build_user_messages_empty_when_no_text_or_image() {
        let parts = vec![json!({ "type": "step-start" })];
        assert!(build_user_messages(&parts, Some(TS)).is_empty());
    }

    #[test]
    fn build_tool_message_pairs_completed_output() {
        let part = json!({
            "type": "tool",
            "tool": "bash",
            "callID": "call-1",
            "state": {
                "status": "completed",
                "input": { "command": "ls" },
                "output": "a.txt\nb.txt",
            },
        });
        let msg = build_tool_message(&part, "msg-1", Some(TS));
        assert_eq!(msg.role, MessageRole::Tool);
        // canonical_name for opencode "bash" → "Bash".
        assert_eq!(msg.tool_name.as_deref(), Some("Bash"));
        assert_eq!(msg.content, "a.txt\nb.txt");
        let metadata = msg.tool_metadata.as_ref().expect("metadata");
        assert_eq!(metadata.status.as_deref(), Some("completed"));
    }

    #[test]
    fn build_tool_message_prefixes_error_output() {
        let part = json!({
            "type": "tool",
            "tool": "bash",
            "id": "fallback-id",
            "state": {
                "status": "error",
                "input": { "command": "false" },
                "error": "command failed",
            },
        });
        let msg = build_tool_message(&part, "msg-1", Some(TS));
        assert_eq!(msg.content, "[Error] command failed");
        let metadata = msg.tool_metadata.as_ref().expect("metadata");
        assert_eq!(metadata.status.as_deref(), Some("error"));
    }

    #[test]
    fn build_tool_message_preserves_non_string_future_output_as_raw() {
        let part = json!({
            "type": "tool",
            "tool": "read",
            "callID": "call-raw",
            "state": {
                "status": "completed",
                "input": { "path": "future.json" },
                "output": { "future": true },
            },
        });
        let msg = build_tool_message(&part, "msg-1", Some(TS));
        assert_eq!(msg.content, r#"{"future":true}"#);
        assert_eq!(
            msg.tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.presentation.as_ref())
                .map(|presentation| presentation.result_mode),
            Some(ToolResultMode::Raw)
        );
    }

    #[test]
    fn build_assistant_messages_emits_text_with_usage_when_no_tools() {
        let parts = vec![json!({ "type": "text", "text": "here is the answer" })];
        let msg_json = json!({
            "role": "assistant",
            "modelID": "gpt-test",
            "tokens": { "input": 10, "output": 5, "cache": { "read": 2, "write": 1 } },
        });
        let msgs = build_assistant_messages(&parts, &msg_json, "msg-9", Some(TS));
        assert_eq!(msgs.len(), 1);
        let m = &msgs[0];
        assert_eq!(m.role, MessageRole::Assistant);
        assert_eq!(m.content, "here is the answer");
        assert_eq!(m.model.as_deref(), Some("gpt-test"));
        // No tool parts → usage + hash ride on the text message.
        assert_eq!(m.usage_hash.as_deref(), Some("msg-9"));
        let usage = m.token_usage.as_ref().expect("usage");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_input_tokens, 2);
        assert_eq!(usage.cache_creation_input_tokens, 1);
    }

    #[test]
    fn build_assistant_messages_promotes_reasoning_to_thinking_system() {
        let parts = vec![
            json!({ "type": "reasoning", "text": "let me think" }),
            json!({ "type": "text", "text": "final reply" }),
        ];
        let msg_json = json!({ "role": "assistant" });
        let msgs = build_assistant_messages(&parts, &msg_json, "msg-2", Some(TS));
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, MessageRole::System);
        assert_eq!(msgs[0].content, "[thinking]\nlet me think");
        assert_eq!(msgs[1].role, MessageRole::Assistant);
        assert_eq!(msgs[1].content, "final reply");
    }

    #[test]
    fn build_assistant_messages_attaches_usage_to_last_tool_when_no_text() {
        let parts = vec![
            json!({
                "type": "tool",
                "tool": "read",
                "callID": "c1",
                "state": { "status": "completed", "input": {}, "output": "ok" },
            }),
            json!({
                "type": "tool",
                "tool": "bash",
                "callID": "c2",
                "state": { "status": "completed", "input": {}, "output": "done" },
            }),
        ];
        let msg_json = json!({
            "role": "assistant",
            "tokens": { "input": 3, "output": 7 },
        });
        let msgs = build_assistant_messages(&parts, &msg_json, "msg-3", Some(TS));
        assert_eq!(msgs.len(), 2);
        // Both are tool messages; usage rides on the LAST one only.
        assert!(msgs[0].token_usage.is_none());
        assert_eq!(msgs[0].usage_hash, None);
        let last = &msgs[1];
        assert_eq!(last.role, MessageRole::Tool);
        assert_eq!(last.usage_hash.as_deref(), Some("msg-3"));
        assert_eq!(last.token_usage.as_ref().expect("usage").output_tokens, 7);
    }

    #[test]
    fn build_assistant_messages_attaches_usage_to_last_tool_after_text() {
        let parts = vec![
            json!({ "type": "text", "text": "checking" }),
            json!({
                "type": "tool",
                "tool": "read",
                "callID": "c1",
                "state": { "status": "completed", "input": {}, "output": "ok" },
            }),
        ];
        let msg_json = json!({
            "role": "assistant",
            "modelID": "gpt-test",
            "tokens": { "input": 3, "output": 7, "reasoning": 2 },
        });

        let messages = build_assistant_messages(&parts, &msg_json, "msg-10", Some(TS));

        assert_eq!(messages.len(), 2);
        assert!(messages[0].token_usage.is_none());
        assert_eq!(messages[1].usage_hash.as_deref(), Some("msg-10"));
        assert_eq!(messages[1].model.as_deref(), Some("gpt-test"));
        assert_eq!(
            messages[1]
                .token_usage
                .as_ref()
                .expect("usage")
                .output_tokens,
            9
        );
    }

    #[test]
    fn extract_tokens_rejects_counts_outside_u32() {
        let message = json!({
            "tokens": {
                "input": u64::from(u32::MAX) + 1,
                "output": 1,
            }
        });

        assert!(extract_tokens(&message).is_none());
    }

    #[test]
    fn build_assistant_messages_emits_usage_only_message_when_no_content() {
        let parts: Vec<serde_json::Value> = vec![json!({ "type": "step-finish" })];
        let msg_json = json!({
            "role": "assistant",
            "modelID": "gpt-test",
            "tokens": { "input": 1, "output": 1 },
        });
        let msgs = build_assistant_messages(&parts, &msg_json, "msg-4", Some(TS));
        // Rare case: no text, no tools, but usage present → one empty
        // assistant message carrying the usage for token tracking.
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "");
        assert_eq!(msgs[0].usage_hash.as_deref(), Some("msg-4"));
        assert!(msgs[0].token_usage.is_some());
    }

    #[test]
    fn build_assistant_messages_merges_patch_into_apply_patch_structured() {
        let parts = vec![
            json!({
                "type": "tool",
                "tool": "apply_patch",
                "callID": "c1",
                "state": { "status": "completed", "input": {}, "output": "patched" },
            }),
            json!({
                "type": "patch",
                "hash": "abc123",
                "files": ["a.txt"],
            }),
        ];
        let msg_json = json!({ "role": "assistant" });
        let msgs = build_assistant_messages(&parts, &msg_json, "msg-5", Some(TS));
        assert_eq!(msgs.len(), 1);
        let metadata = msgs[0].tool_metadata.as_ref().expect("metadata");
        let structured = metadata.structured.as_ref().expect("structured");
        let patch = structured.get("patch").expect("patch attached");
        assert_eq!(patch.get("hash").and_then(|h| h.as_str()), Some("abc123"));
        assert_eq!(patch.get("files"), Some(&json!(["a.txt"])));
    }
}
