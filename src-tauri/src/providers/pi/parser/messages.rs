//! Message construction for the active Pi branch: assistant/tool/system
//! entries flatten into the universal Message shape.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::models::{Message, MessageRole, Provider, TokenUsage};
use crate::tool_metadata::{
    build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

use super::super::types::*;
use super::{format_millis_timestamp, format_rfc3339_timestamp, get_entry_id};

pub(super) fn extract_messages(entries: &[PiEntry], branch: &[String]) -> Vec<Message> {
    let entry_by_id: HashMap<String, &PiEntry> = entries
        .iter()
        .filter_map(|entry| get_entry_id(entry).map(|id| (id, entry)))
        .collect();
    let mut messages = Vec::new();
    let mut call_id_to_idx: HashMap<String, usize> = HashMap::new();

    for entry_id in branch {
        let Some(entry) = entry_by_id.get(entry_id).copied() else {
            continue;
        };

        match entry {
            PiEntry::Message(msg_entry) => {
                push_agent_messages(&msg_entry.message, &mut messages, &mut call_id_to_idx);
            }
            PiEntry::Compaction(compaction) => {
                push_system_message(
                    &mut messages,
                    format!("[Compaction] {}", compaction.summary),
                    format_rfc3339_timestamp(&compaction.base.timestamp),
                    None,
                );
            }
            PiEntry::BranchSummary(summary) => {
                push_system_message(
                    &mut messages,
                    format!("[Branch Summary] {}", summary.summary),
                    format_rfc3339_timestamp(&summary.base.timestamp),
                    None,
                );
            }
            PiEntry::CustomMessage(custom) if custom.display => {
                let content = extract_content_text(&custom.content);
                push_system_message(
                    &mut messages,
                    format!("[{}] {}", custom.custom_type, content),
                    format_rfc3339_timestamp(&custom.base.timestamp),
                    None,
                );
            }
            _ => {}
        }
    }

    messages
}

/// Convert Pi agent messages to CC Session messages.
fn push_agent_messages(
    msg: &PiAgentMessage,
    messages: &mut Vec<Message>,
    call_id_to_idx: &mut HashMap<String, usize>,
) {
    match msg {
        PiAgentMessage::User(user) => {
            let content = extract_content_text(&user.content);
            if !content.is_empty() {
                messages.push(Message {
                    timestamp: format_millis_timestamp(user.timestamp),
                    ..Message::user(content)
                });
            }
        }
        PiAgentMessage::Assistant(assistant) => {
            push_assistant_message(assistant, messages, call_id_to_idx);
        }
        PiAgentMessage::ToolResult(result) => {
            merge_tool_result(result, messages, call_id_to_idx);
        }
        PiAgentMessage::BashExecution(bash) => {
            push_bash_execution(bash, messages);
        }
        PiAgentMessage::Custom(custom) => {
            if custom.display {
                let content = extract_content_text(&custom.content);
                push_system_message(
                    messages,
                    format!("[{}] {}", custom.custom_type, content),
                    format_millis_timestamp(custom.timestamp),
                    None,
                );
            }
        }
        PiAgentMessage::BranchSummary(summary) => {
            push_system_message(
                messages,
                format!("[Branch Summary] {}", summary.summary),
                format_millis_timestamp(summary.timestamp),
                None,
            );
        }
        PiAgentMessage::CompactionSummary(compaction) => {
            push_system_message(
                messages,
                format!("[Compaction] {}", compaction.summary),
                format_millis_timestamp(compaction.timestamp),
                None,
            );
        }
    }
}

fn push_assistant_message(
    assistant: &PiAssistantMessage,
    messages: &mut Vec<Message>,
    call_id_to_idx: &mut HashMap<String, usize>,
) {
    let timestamp = format_millis_timestamp(assistant.timestamp);
    let mut usage_target_idx: Option<usize> = None;
    let token_usage = assistant.usage.as_ref().map(|u| TokenUsage {
        input_tokens: u.input as u32,
        output_tokens: u.output as u32,
        cache_creation_input_tokens: u.cache_write as u32,
        cache_read_input_tokens: u.cache_read as u32,
    });

    let mut text_parts: Vec<String> = Vec::new();
    for block in &assistant.content {
        match block {
            PiContentBlock::Text { text } => {
                if !text.is_empty() {
                    text_parts.push(text.clone());
                }
            }
            PiContentBlock::Image { .. } => {
                text_parts.push("[Image]".to_string());
            }
            PiContentBlock::Thinking { thinking } => {
                flush_assistant_text(
                    &mut text_parts,
                    messages,
                    timestamp.clone(),
                    assistant.model.clone(),
                    &mut usage_target_idx,
                );
                if !thinking.trim().is_empty() {
                    push_system_message(
                        messages,
                        format!("[thinking]\n{thinking}"),
                        timestamp.clone(),
                        assistant.model.clone(),
                    );
                }
            }
            PiContentBlock::ToolCall {
                id,
                name,
                arguments,
            } => {
                flush_assistant_text(
                    &mut text_parts,
                    messages,
                    timestamp.clone(),
                    assistant.model.clone(),
                    &mut usage_target_idx,
                );
                let idx = push_tool_call(
                    messages,
                    name,
                    Some(id),
                    arguments,
                    timestamp.clone(),
                    assistant.model.clone(),
                );
                call_id_to_idx.insert(id.clone(), idx);
                if usage_target_idx.is_none() {
                    usage_target_idx = Some(idx);
                }
            }
        }
    }

    flush_assistant_text(
        &mut text_parts,
        messages,
        timestamp,
        assistant.model.clone(),
        &mut usage_target_idx,
    );

    if let (Some(idx), Some(usage)) = (usage_target_idx, token_usage) {
        if let Some(message) = messages.get_mut(idx) {
            message.token_usage = Some(usage);
            if message.model.is_none() {
                message.model = assistant.model.clone();
            }
        }
    }
}

fn flush_assistant_text(
    text_parts: &mut Vec<String>,
    messages: &mut Vec<Message>,
    timestamp: Option<String>,
    model: Option<String>,
    usage_target_idx: &mut Option<usize>,
) {
    if text_parts.is_empty() {
        return;
    }
    let content = text_parts.join("\n");
    text_parts.clear();
    if content.trim().is_empty() {
        return;
    }
    let idx = messages.len();
    messages.push(Message {
        timestamp,
        model,
        ..Message::assistant(content)
    });
    if usage_target_idx.is_none() {
        *usage_target_idx = Some(idx);
    }
}

fn push_tool_call(
    messages: &mut Vec<Message>,
    raw_name: &str,
    call_id: Option<&str>,
    arguments: &Value,
    timestamp: Option<String>,
    model: Option<String>,
) -> usize {
    let metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Pi,
        raw_name,
        input: Some(arguments),
        call_id,
        assistant_id: None,
    });
    let idx = messages.len();
    messages.push(Message {
        timestamp,
        tool_name: Some(metadata.canonical_name.clone()),
        tool_input: Some(arguments.to_string()),
        tool_metadata: Some(metadata),
        model,
        ..Message::new(MessageRole::Tool, String::new())
    });
    idx
}

fn merge_tool_result(
    result: &PiToolResultMessage,
    messages: &mut Vec<Message>,
    call_id_to_idx: &HashMap<String, usize>,
) {
    let content = extract_content_blocks_text(&result.content);
    let result_value = tool_result_value(result, &content);
    let artifact_path = tool_result_artifact_path(result);

    if let Some(idx) = call_id_to_idx.get(&result.tool_call_id).copied() {
        if let Some(message) = messages.get_mut(idx) {
            message.content = content;
            if let Some(metadata) = message.tool_metadata.as_mut() {
                enrich_tool_metadata(
                    metadata,
                    ToolResultFacts {
                        raw_result: Some(&result_value),
                        is_error: Some(result.is_error),
                        status: None,
                        artifact_path,
                    },
                );
            }
            return;
        }
    }

    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Pi,
        raw_name: &result.tool_name,
        input: None,
        call_id: Some(&result.tool_call_id),
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&result_value),
            is_error: Some(result.is_error),
            status: None,
            artifact_path,
        },
    );
    messages.push(Message {
        timestamp: format_millis_timestamp(result.timestamp),
        tool_name: Some(metadata.canonical_name.clone()),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, content)
    });
}

fn push_bash_execution(bash: &PiBashExecutionMessage, messages: &mut Vec<Message>) {
    let input = serde_json::json!({ "command": bash.command });
    let result_value = serde_json::json!({
        "command": bash.command,
        "output": bash.output,
        "exitCode": bash.exit_code,
        "cancelled": bash.cancelled,
        "truncated": bash.truncated,
        "fullOutputPath": bash.full_output_path,
    });
    let is_error = bash.cancelled || bash.exit_code.is_some_and(|code| code != 0);
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Pi,
        raw_name: "bash",
        input: Some(&input),
        call_id: None,
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&result_value),
            is_error: Some(is_error),
            status: None,
            artifact_path: bash.full_output_path.as_deref(),
        },
    );
    messages.push(Message {
        timestamp: format_millis_timestamp(bash.timestamp),
        tool_name: Some(metadata.canonical_name.clone()),
        tool_input: Some(input.to_string()),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, bash.output.clone())
    });
}

fn push_system_message(
    messages: &mut Vec<Message>,
    content: String,
    timestamp: Option<String>,
    model: Option<String>,
) {
    if content.trim().is_empty() {
        return;
    }
    messages.push(Message {
        timestamp,
        model,
        ..Message::system(content)
    });
}

fn tool_result_value(result: &PiToolResultMessage, output: &str) -> Value {
    let mut obj = match result.details.clone() {
        Some(Value::Object(map)) => map,
        Some(value) => {
            let mut map = Map::new();
            map.insert("details".to_string(), value);
            map
        }
        None => Map::new(),
    };
    obj.insert(
        "toolCallId".to_string(),
        Value::String(result.tool_call_id.clone()),
    );
    obj.insert(
        "toolName".to_string(),
        Value::String(result.tool_name.clone()),
    );
    obj.insert("isError".to_string(), Value::Bool(result.is_error));
    if !output.is_empty() && !obj.contains_key("output") {
        obj.insert("output".to_string(), Value::String(output.to_string()));
    }
    if let Some(path) = tool_result_artifact_path(result) {
        obj.insert(
            "persistedOutputPath".to_string(),
            Value::String(path.to_string()),
        );
    }
    Value::Object(obj)
}

fn tool_result_artifact_path(result: &PiToolResultMessage) -> Option<&str> {
    let details = result.details.as_ref()?;
    details
        .get("fullOutputPath")
        .and_then(Value::as_str)
        .or_else(|| {
            details
                .get("truncation")
                .and_then(|value| value.get("fullOutputPath"))
                .and_then(Value::as_str)
        })
}

/// Extract text from content
pub(super) fn extract_content_text(content: &PiContent) -> String {
    match content {
        PiContent::Text(text) => text.clone(),
        PiContent::Blocks(blocks) => extract_content_blocks_text(blocks),
    }
}

/// Extract visible text from content blocks.
fn extract_content_blocks_text(blocks: &[PiContentBlock]) -> String {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            PiContentBlock::Text { text } => parts.push(text.clone()),
            PiContentBlock::Image { .. } => {
                parts.push("[Image]".to_string());
            }
            PiContentBlock::Thinking { .. } | PiContentBlock::ToolCall { .. } => {}
        }
    }
    parts.join("\n")
}
