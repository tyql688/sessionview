use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::models::{Message, MessageRole, Provider, TokenUsage};
use crate::provider::util::{RenderedToolOutput, ToolCallPairer};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

use super::super::types::{CustomMessageEntry, Entry, MessageEntry};
use super::{ActiveEntry, NormalizedUsage, StoredEntry, resolve_model_for_entry};

pub(super) fn convert_messages(
    entries: &[StoredEntry],
    by_id: &HashMap<String, usize>,
    active: &[ActiveEntry<'_>],
    usage_by_entry: &HashMap<usize, NormalizedUsage>,
    agent_links: &HashMap<String, String>,
    path: &Path,
    parse_warning_count: &mut u32,
) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut pairer = ToolCallPairer::default();
    for active_entry in active {
        match &active_entry.stored.entry {
            Entry::Message(entry) => push_wire_message(
                entry,
                active_entry,
                entries,
                by_id,
                usage_by_entry.get(&active_entry.index),
                agent_links,
                &mut messages,
                &mut pairer,
                path,
                parse_warning_count,
            ),
            Entry::Compaction(entry) => push_system(
                &mut messages,
                format!("[context_compacted]\n{}", entry.summary),
                active_entry.timestamp.clone(),
                None,
            ),
            Entry::BranchSummary(entry) => push_system(
                &mut messages,
                format!("[Branch Summary] {}", entry.summary),
                active_entry.timestamp.clone(),
                None,
            ),
            Entry::CustomMessage(entry) if entry.display => push_custom_message(
                entry,
                active_entry,
                &mut messages,
                path,
                parse_warning_count,
            ),
            Entry::ModelChange(_)
            | Entry::SessionInfo(_)
            | Entry::Metadata(_)
            | Entry::CustomMessage(_) => {}
        }
    }
    messages
}

#[allow(clippy::too_many_arguments)]
// The arguments are the parser state needed to keep wire order, tool pairing,
// exact model ancestry, and warning accounting together at this boundary.
fn push_wire_message(
    entry: &MessageEntry,
    active: &ActiveEntry<'_>,
    entries: &[StoredEntry],
    by_id: &HashMap<String, usize>,
    usage: Option<&NormalizedUsage>,
    agent_links: &HashMap<String, String>,
    messages: &mut Vec<Message>,
    pairer: &mut ToolCallPairer,
    path: &Path,
    parse_warning_count: &mut u32,
) {
    if meta_flag(entry, "isMeta") {
        return;
    }
    if meta_flag(entry, "isSummary") {
        let text = render_visible_content(
            &Value::Array(entry.message.content.clone()),
            active.stored.line_no,
            path,
            parse_warning_count,
        );
        push_system(
            messages,
            format!("[context_compacted]\n{text}"),
            active.timestamp.clone(),
            None,
        );
        return;
    }

    match entry.message.role.as_str() {
        "user" => push_user_message(entry, active, messages, pairer, path, parse_warning_count),
        "assistant" => {
            let model = resolve_model_for_entry(entries, by_id, active.index);
            push_assistant_message(
                entry,
                active,
                usage,
                model,
                agent_links,
                messages,
                pairer,
                path,
                parse_warning_count,
            );
        }
        "system" => {
            let text = render_visible_content(
                &Value::Array(entry.message.content.clone()),
                active.stored.line_no,
                path,
                parse_warning_count,
            );
            push_system(messages, text, active.timestamp.clone(), None);
        }
        role => {
            log::warn!(
                "skipping unsupported Command Code message role '{role}' at line {} in '{}'",
                active.stored.line_no,
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
        }
    }
}

fn meta_flag(entry: &MessageEntry, key: &str) -> bool {
    entry
        .message
        .meta
        .as_ref()
        .and_then(|meta| meta.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn push_user_message(
    entry: &MessageEntry,
    active: &ActiveEntry<'_>,
    messages: &mut Vec<Message>,
    pairer: &ToolCallPairer,
    path: &Path,
    parse_warning_count: &mut u32,
) {
    let mut chunks = Vec::new();
    for block in &entry.message.content {
        if block.get("type").and_then(Value::as_str) == Some("tool_result") {
            flush_user_chunks(&mut chunks, messages, active.timestamp.clone());
            merge_tool_result(block, active, messages, pairer, path, parse_warning_count);
            continue;
        }
        match render_visible_block(block) {
            Some(chunk) => chunks.push(chunk),
            None => warn_content_block(block, active, path, parse_warning_count),
        }
    }
    flush_user_chunks(&mut chunks, messages, active.timestamp.clone());
}

fn flush_user_chunks(
    chunks: &mut Vec<String>,
    messages: &mut Vec<Message>,
    timestamp: Option<String>,
) {
    if chunks.is_empty() {
        return;
    }
    let text = std::mem::take(chunks).join("\n");
    if !text.trim().is_empty() {
        messages.push(Message {
            timestamp,
            ..Message::user(text)
        });
    }
}

#[allow(clippy::too_many_arguments)]
// This converter owns the ordered assistant block stream and the shared tool
// pairing state; splitting those would make usage attachment ambiguous.
fn push_assistant_message(
    entry: &MessageEntry,
    active: &ActiveEntry<'_>,
    usage: Option<&NormalizedUsage>,
    model: Option<String>,
    agent_links: &HashMap<String, String>,
    messages: &mut Vec<Message>,
    pairer: &mut ToolCallPairer,
    path: &Path,
    parse_warning_count: &mut u32,
) {
    let mut text_chunks = Vec::new();
    let mut usage_target = None;
    for block in &entry.message.content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") | Some("image") | Some("image_url") | Some("input_image") => {
                match render_visible_block(block) {
                    Some(chunk) => text_chunks.push(chunk),
                    None => warn_content_block(block, active, path, parse_warning_count),
                }
            }
            Some("thinking") => {
                flush_assistant_text(
                    &mut text_chunks,
                    messages,
                    active.timestamp.clone(),
                    model.clone(),
                    &mut usage_target,
                );
                match block.get("thinking").and_then(Value::as_str) {
                    Some(thinking) if !thinking.trim().is_empty() => push_system(
                        messages,
                        format!("[thinking]\n{thinking}"),
                        active.timestamp.clone(),
                        model.clone(),
                    ),
                    Some(_) => {}
                    None => warn_content_block(block, active, path, parse_warning_count),
                }
            }
            Some("tool_use") => {
                flush_assistant_text(
                    &mut text_chunks,
                    messages,
                    active.timestamp.clone(),
                    model.clone(),
                    &mut usage_target,
                );
                if let Some(index) = push_tool_call(
                    block,
                    active,
                    model.clone(),
                    agent_links,
                    messages,
                    pairer,
                    path,
                    parse_warning_count,
                ) && usage_target.is_none()
                {
                    usage_target = Some(index);
                }
            }
            _ => warn_content_block(block, active, path, parse_warning_count),
        }
    }
    flush_assistant_text(
        &mut text_chunks,
        messages,
        active.timestamp.clone(),
        model.clone(),
        &mut usage_target,
    );
    attach_message_usage(
        usage,
        usage_target,
        model,
        messages,
        active,
        path,
        parse_warning_count,
    );
}

fn flush_assistant_text(
    chunks: &mut Vec<String>,
    messages: &mut Vec<Message>,
    timestamp: Option<String>,
    model: Option<String>,
    usage_target: &mut Option<usize>,
) {
    if chunks.is_empty() {
        return;
    }
    let text = std::mem::take(chunks).join("\n");
    if text.trim().is_empty() {
        return;
    }
    let index = messages.len();
    messages.push(Message {
        timestamp,
        model,
        ..Message::assistant(text)
    });
    if usage_target.is_none() {
        *usage_target = Some(index);
    }
}

fn push_tool_call(
    block: &Value,
    active: &ActiveEntry<'_>,
    model: Option<String>,
    agent_links: &HashMap<String, String>,
    messages: &mut Vec<Message>,
    pairer: &mut ToolCallPairer,
    path: &Path,
    parse_warning_count: &mut u32,
) -> Option<usize> {
    let (Some(id), Some(name), Some(input)) = (
        block.get("id").and_then(Value::as_str),
        block.get("name").and_then(Value::as_str),
        block.get("input"),
    ) else {
        warn_content_block(block, active, path, parse_warning_count);
        return None;
    };
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::CommandCode,
        raw_name: name,
        input: Some(input),
        call_id: Some(id),
        assistant_id: None,
    });
    if name.eq_ignore_ascii_case("agent")
        && let Some(agent_id) = agent_links.get(id)
    {
        let link = serde_json::json!({ "agent_id": agent_id });
        enrich_tool_metadata(
            &mut metadata,
            ToolResultFacts {
                raw_result: Some(&link),
                ..ToolResultFacts::default()
            },
        );
    }
    let index = messages.len();
    messages.push(Message {
        timestamp: active.timestamp.clone(),
        tool_name: Some(metadata.canonical_name.clone()),
        tool_input: Some(match input {
            Value::String(text) => text.clone(),
            _ => input.to_string(),
        }),
        tool_metadata: Some(metadata),
        model,
        ..Message::new(MessageRole::Tool, String::new())
    });
    pairer.register(Some(id), index);
    Some(index)
}

fn merge_tool_result(
    block: &Value,
    active: &ActiveEntry<'_>,
    messages: &mut Vec<Message>,
    pairer: &ToolCallPairer,
    path: &Path,
    parse_warning_count: &mut u32,
) {
    let Some(call_id) = block.get("tool_use_id").and_then(Value::as_str) else {
        warn_content_block(block, active, path, parse_warning_count);
        return;
    };
    let Some(content) = block.get("content") else {
        warn_content_block(block, active, path, parse_warning_count);
        return;
    };
    let rendered = render_tool_result_content(content);
    let is_error = block
        .get("is_error")
        .or_else(|| block.get("isError"))
        .and_then(Value::as_bool);
    let status = block.get("status").and_then(Value::as_str);
    let facts = ToolResultFacts {
        raw_result: Some(block),
        is_error,
        status,
        artifact_path: None,
        raw_output: Some(rendered.is_raw),
    };
    if let Some(message) = pairer.message_mut(Some(call_id), messages) {
        message.content = rendered.text;
        if let Some(metadata) = message.tool_metadata.as_mut() {
            enrich_tool_metadata(metadata, facts);
        }
        return;
    }

    log::warn!(
        "orphan Command Code tool_result '{call_id}' at line {} in '{}'",
        active.stored.line_no,
        path.display()
    );
    *parse_warning_count = parse_warning_count.saturating_add(1);
    let raw_name = block
        .get("tool_name")
        .or_else(|| block.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("UnknownToolResult");
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::CommandCode,
        raw_name,
        input: None,
        call_id: Some(call_id),
        assistant_id: None,
    });
    enrich_tool_metadata(&mut metadata, facts);
    messages.push(Message {
        timestamp: active.timestamp.clone(),
        tool_name: Some(metadata.canonical_name.clone()),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, rendered.text)
    });
}

fn attach_message_usage(
    usage: Option<&NormalizedUsage>,
    target: Option<usize>,
    model: Option<String>,
    messages: &mut [Message],
    active: &ActiveEntry<'_>,
    path: &Path,
    parse_warning_count: &mut u32,
) {
    let (Some(usage), Some(target)) = (usage, target) else {
        return;
    };
    let converted = (
        u32::try_from(usage.input),
        u32::try_from(usage.output),
        u32::try_from(usage.cache_read),
        u32::try_from(usage.cache_write),
    );
    let (Ok(input), Ok(output), Ok(cache_read), Ok(cache_write)) = converted else {
        log::warn!(
            "Command Code message usage exceeds u32 at line {} in '{}'",
            active.stored.line_no,
            path.display()
        );
        *parse_warning_count = parse_warning_count.saturating_add(1);
        return;
    };
    if let Some(message) = messages.get_mut(target) {
        message.token_usage = Some(TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_write,
        });
        if message.model.is_none() {
            message.model = model;
        }
    }
}

pub(super) fn render_tool_result_content(content: &Value) -> RenderedToolOutput {
    match content {
        Value::String(text) => RenderedToolOutput::rendered(text.clone()),
        Value::Array(parts) => {
            let mut chunks = Vec::new();
            for part in parts {
                let Some(chunk) = render_visible_block(part) else {
                    return RenderedToolOutput::raw(content.to_string());
                };
                chunks.push(chunk);
            }
            RenderedToolOutput::rendered(chunks.join("\n"))
        }
        Value::Null => RenderedToolOutput::rendered(String::new()),
        _ => RenderedToolOutput::raw(content.to_string()),
    }
}

fn render_visible_content(
    content: &Value,
    line_no: usize,
    path: &Path,
    parse_warning_count: &mut u32,
) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            let mut chunks = Vec::new();
            for block in blocks {
                match render_visible_block(block) {
                    Some(chunk) => chunks.push(chunk),
                    None => {
                        log::warn!(
                            "skipping unsupported Command Code visible content at line {line_no} in '{}'",
                            path.display()
                        );
                        *parse_warning_count = parse_warning_count.saturating_add(1);
                    }
                }
            }
            chunks.join("\n")
        }
        Value::Null => String::new(),
        _ => {
            log::warn!(
                "preserving unsupported Command Code visible content as raw JSON at line {line_no} in '{}'",
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            content.to_string()
        }
    }
}

fn render_visible_block(block: &Value) -> Option<String> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") | Some("input_text") | Some("output_text") => block
            .get("text")
            .or_else(|| block.get("input_text"))
            .or_else(|| block.get("output_text"))
            .and_then(Value::as_str)
            .map(str::to_string),
        Some("image") | Some("image_url") | Some("input_image") => image_marker(block),
        _ => None,
    }
}

fn image_marker(block: &Value) -> Option<String> {
    if let Some(source) = block.get("source") {
        match source.get("type").and_then(Value::as_str) {
            Some("base64") => {
                let mime = source
                    .get("media_type")
                    .or_else(|| source.get("mimeType"))
                    .and_then(Value::as_str)?;
                let data = source.get("data").and_then(Value::as_str)?;
                return Some(format!("[Image: source: data:{mime};base64,{data}]"));
            }
            Some("url") => {
                let url = source.get("url").and_then(Value::as_str)?;
                return Some(format!("[Image: source: {url}]"));
            }
            _ => {}
        }
    }
    let url = block
        .get("image_url")
        .or_else(|| block.get("imageUrl"))
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("url").and_then(Value::as_str))
        })?;
    Some(format!("[Image: source: {url}]"))
}

fn warn_content_block(
    block: &Value,
    active: &ActiveEntry<'_>,
    path: &Path,
    parse_warning_count: &mut u32,
) {
    let kind = block
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<missing>");
    log::warn!(
        "skipping unsupported Command Code content block '{kind}' at line {} in '{}'",
        active.stored.line_no,
        path.display()
    );
    *parse_warning_count = parse_warning_count.saturating_add(1);
}

fn push_custom_message(
    entry: &CustomMessageEntry,
    active: &ActiveEntry<'_>,
    messages: &mut Vec<Message>,
    path: &Path,
    parse_warning_count: &mut u32,
) {
    let text = render_visible_content(
        &entry.content,
        active.stored.line_no,
        path,
        parse_warning_count,
    );
    if !text.trim().is_empty() {
        push_system(
            messages,
            format!("[{}]\n{text}", entry.custom_type),
            active.timestamp.clone(),
            None,
        );
    }
}

fn push_system(
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
