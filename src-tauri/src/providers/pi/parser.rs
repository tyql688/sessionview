use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::models::{Message, MessageRole, Provider, SessionMeta, TokenUsage};
use crate::provider::{LoadedSession, ParsedSession};
use crate::tool_metadata::{
    build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

use super::types::*;

/// Parse a Pi session JSONL file
pub fn parse_session_file(path: &Path) -> Option<ParsedSession> {
    let (header, entries, parse_warning_count) = parse_entries(path)?;
    if header.version < 2 {
        log::warn!(
            "Skipping Pi session v{}: {}",
            header.version,
            path.display()
        );
        return None;
    }

    let active_branch = build_active_branch(&entries);
    let messages = extract_messages(&entries, &active_branch);
    let title = extract_title(&entries, &active_branch, &header);
    let model = extract_model(&entries, &active_branch);
    let (input_tokens, output_tokens, cache_read_tokens, cache_write_tokens) =
        extract_token_totals(&entries, &active_branch);

    let created_at = match parse_timestamp(&header.timestamp) {
        Some(ts) => ts,
        None => {
            log::warn!(
                "Skipping Pi session with malformed header timestamp '{}': {}",
                header.timestamp,
                path.display()
            );
            return None;
        }
    };
    let updated_at = extract_updated_at(&entries, &active_branch).unwrap_or(created_at);

    let meta = SessionMeta {
        id: header.id.clone(),
        provider: Provider::Pi,
        title,
        project_path: header.cwd.clone(),
        project_name: extract_project_name(&header.cwd),
        created_at,
        updated_at,
        message_count: messages.len() as u32,
        file_size_bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        source_path: path.to_string_lossy().to_string(),
        is_sidechain: false,
        variant_name: None,
        model,
        cc_version: None,
        git_branch: None,
        parent_id: header.parent_session.clone(),
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
    };

    let content_text = crate::provider_utils::truncate_to_bytes(
        &messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        crate::provider_utils::FTS_CONTENT_LIMIT,
    );
    Some(ParsedSession {
        meta,
        messages,
        content_text,
        parse_warning_count,
        child_session_ids: Vec::new(),
        usage_events: Vec::new(),
        source_mtime: std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(crate::provider::system_time_to_epoch_seconds)
            .unwrap_or(0),
    })
}

/// Load messages from a Pi session file (for detail view)
pub fn load_messages(path: &Path) -> Option<LoadedSession> {
    let (_header, entries, parse_warning_count) = parse_entries(path)?;
    let active_branch = build_active_branch(&entries);
    let messages = extract_messages(&entries, &active_branch);

    Some(LoadedSession::from_messages(messages, parse_warning_count))
}

fn parse_entries(path: &Path) -> Option<(PiSessionHeader, Vec<PiEntry>, u32)> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            log::warn!("Failed to read Pi session '{}': {error}", path.display());
            return None;
        }
    };
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        return None;
    }

    let header: PiSessionHeader = match serde_json::from_str(lines.first()?) {
        Ok(header) => header,
        Err(error) => {
            log::warn!(
                "Failed to parse Pi session header '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let mut entries: Vec<PiEntry> = Vec::new();
    let mut parse_warning_count = 0u32;
    for (i, line) in lines.iter().enumerate().skip(1) {
        match serde_json::from_str::<PiEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(error) => {
                parse_warning_count = parse_warning_count.saturating_add(1);
                log::warn!(
                    "Failed to parse Pi session entry at line {} in '{}': {error}",
                    i + 1,
                    path.display()
                );
            }
        }
    }

    Some((header, entries, parse_warning_count))
}

/// Build the active branch by walking from the last entry to root
fn build_active_branch(entries: &[PiEntry]) -> Vec<String> {
    if entries.is_empty() {
        return Vec::new();
    }

    // Find the last entry (leaf)
    let last_entry = match entries.last() {
        Some(e) => e,
        None => return Vec::new(),
    };
    let leaf_id = get_entry_id(last_entry);

    // Build parent map
    let mut parent_map: HashMap<String, String> = HashMap::new();
    for entry in entries {
        if let (Some(id), Some(parent_id)) = (get_entry_id(entry), get_entry_parent_id(entry)) {
            parent_map.insert(id, parent_id);
        }
    }

    // Walk from leaf to root
    let mut branch = Vec::new();
    let mut current = leaf_id;
    while let Some(id) = current {
        branch.push(id.clone());
        current = parent_map.get(&id).cloned();
    }

    branch.reverse();
    branch
}

/// Extract messages from entries on the active branch
fn extract_messages(entries: &[PiEntry], branch: &[String]) -> Vec<Message> {
    let branch_set: HashSet<&str> = branch.iter().map(String::as_str).collect();
    let mut messages = Vec::new();
    let mut call_id_to_idx: HashMap<String, usize> = HashMap::new();

    for entry in entries {
        let entry_id = match get_entry_id(entry) {
            Some(id) => id,
            None => continue,
        };

        if !branch_set.contains(entry_id.as_str()) {
            continue;
        }

        match entry {
            PiEntry::Message(msg_entry) => {
                push_agent_messages(&msg_entry.message, &mut messages, &mut call_id_to_idx);
            }
            PiEntry::Compaction(compaction) => {
                push_system_message(
                    &mut messages,
                    format!("[Compaction] {}", compaction.summary),
                    parse_timestamp(&compaction.base.timestamp).map(format_timestamp),
                    None,
                );
            }
            PiEntry::BranchSummary(summary) => {
                push_system_message(
                    &mut messages,
                    format!("[Branch Summary] {}", summary.summary),
                    parse_timestamp(&summary.base.timestamp).map(format_timestamp),
                    None,
                );
            }
            PiEntry::CustomMessage(custom) => {
                if custom.display {
                    let content = extract_content_text(&custom.content);
                    push_system_message(
                        &mut messages,
                        format!("[{}] {}", custom.custom_type, content),
                        parse_timestamp(&custom.base.timestamp).map(format_timestamp),
                        None,
                    );
                }
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
                    timestamp: Some(format_timestamp(user.timestamp as i64)),
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
                    Some(format_timestamp(custom.timestamp as i64)),
                    None,
                );
            }
        }
        PiAgentMessage::BranchSummary(summary) => {
            push_system_message(
                messages,
                format!("[Branch Summary] {}", summary.summary),
                Some(format_timestamp(summary.timestamp as i64)),
                None,
            );
        }
        PiAgentMessage::CompactionSummary(compaction) => {
            push_system_message(
                messages,
                format!("[Compaction] {}", compaction.summary),
                Some(format_timestamp(compaction.timestamp as i64)),
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
    let timestamp = Some(format_timestamp(assistant.timestamp as i64));
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
        timestamp: Some(format_timestamp(result.timestamp as i64)),
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
        timestamp: Some(format_timestamp(bash.timestamp as i64)),
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
fn extract_content_text(content: &PiContent) -> String {
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

/// Extract title from session
fn extract_title(entries: &[PiEntry], branch: &[String], header: &PiSessionHeader) -> String {
    let branch_set: HashSet<&str> = branch.iter().map(String::as_str).collect();

    // Look for session_info entry
    for entry in entries.iter().rev() {
        if let PiEntry::SessionInfo(info) = entry {
            if branch_set.contains(info.base.id.as_str()) {
                return info.name.clone();
            }
        }
    }

    // Fall back to first user message
    for entry in entries {
        if let PiEntry::Message(msg_entry) = entry {
            if branch_set.contains(msg_entry.base.id.as_str()) {
                if let PiAgentMessage::User(user) = &msg_entry.message {
                    let text = extract_content_text(&user.content);
                    let title = text.chars().take(100).collect::<String>();
                    if !title.is_empty() {
                        return title;
                    }
                }
            }
        }
    }

    format!("Session {}", header.id)
}

/// Extract model from entries
fn extract_model(entries: &[PiEntry], branch: &[String]) -> Option<String> {
    let branch_set: HashSet<&str> = branch.iter().map(String::as_str).collect();

    // Look for model_change entry
    for entry in entries.iter().rev() {
        if let PiEntry::ModelChange(model_change) = entry {
            if branch_set.contains(model_change.base.id.as_str()) {
                return Some(format!(
                    "{}/{}",
                    model_change.provider, model_change.model_id
                ));
            }
        }
    }

    // Fall back to assistant message model
    for entry in entries.iter().rev() {
        if let PiEntry::Message(msg_entry) = entry {
            if branch_set.contains(msg_entry.base.id.as_str()) {
                if let PiAgentMessage::Assistant(assistant) = &msg_entry.message {
                    if let Some(model) = &assistant.model {
                        let provider = assistant.provider.as_deref().unwrap_or("unknown");
                        return Some(format!("{}/{}", provider, model));
                    }
                }
            }
        }
    }

    None
}

/// Extract token totals from entries
fn extract_token_totals(entries: &[PiEntry], branch: &[String]) -> (u64, u64, u64, u64) {
    let branch_set: HashSet<&str> = branch.iter().map(String::as_str).collect();
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut cache_read_tokens = 0u64;
    let mut cache_write_tokens = 0u64;

    for entry in entries {
        if let PiEntry::Message(msg_entry) = entry {
            if branch_set.contains(msg_entry.base.id.as_str()) {
                if let PiAgentMessage::Assistant(assistant) = &msg_entry.message {
                    if let Some(usage) = &assistant.usage {
                        input_tokens += usage.input;
                        output_tokens += usage.output;
                        cache_read_tokens += usage.cache_read;
                        cache_write_tokens += usage.cache_write;
                    }
                }
            }
        }
    }

    (
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
    )
}

/// Extract updated_at timestamp
fn extract_updated_at(entries: &[PiEntry], branch: &[String]) -> Option<i64> {
    let branch_set: HashSet<&str> = branch.iter().map(String::as_str).collect();

    for entry in entries.iter().rev() {
        let timestamp = match entry {
            PiEntry::Message(msg) => {
                if branch_set.contains(msg.base.id.as_str()) {
                    Some(&msg.base.timestamp)
                } else {
                    None
                }
            }
            PiEntry::ModelChange(mc) => {
                if branch_set.contains(mc.base.id.as_str()) {
                    Some(&mc.base.timestamp)
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(ts) = timestamp {
            return parse_timestamp(ts);
        }
    }

    None
}

/// Get entry ID
fn get_entry_id(entry: &PiEntry) -> Option<String> {
    match entry {
        PiEntry::Session(_) => None,
        PiEntry::Message(e) => Some(e.base.id.clone()),
        PiEntry::ModelChange(e) => Some(e.base.id.clone()),
        PiEntry::ThinkingLevelChange(e) => Some(e.base.id.clone()),
        PiEntry::Compaction(e) => Some(e.base.id.clone()),
        PiEntry::BranchSummary(e) => Some(e.base.id.clone()),
        PiEntry::Custom(e) => Some(e.base.id.clone()),
        PiEntry::CustomMessage(e) => Some(e.base.id.clone()),
        PiEntry::Label(e) => Some(e.base.id.clone()),
        PiEntry::SessionInfo(e) => Some(e.base.id.clone()),
    }
}

/// Get entry parent ID
fn get_entry_parent_id(entry: &PiEntry) -> Option<String> {
    match entry {
        PiEntry::Session(_) => None,
        PiEntry::Message(e) => e.base.parent_id.clone(),
        PiEntry::ModelChange(e) => e.base.parent_id.clone(),
        PiEntry::ThinkingLevelChange(e) => e.base.parent_id.clone(),
        PiEntry::Compaction(e) => e.base.parent_id.clone(),
        PiEntry::BranchSummary(e) => e.base.parent_id.clone(),
        PiEntry::Custom(e) => e.base.parent_id.clone(),
        PiEntry::CustomMessage(e) => e.base.parent_id.clone(),
        PiEntry::Label(e) => e.base.parent_id.clone(),
        PiEntry::SessionInfo(e) => e.base.parent_id.clone(),
    }
}

/// Parse ISO timestamp to Unix milliseconds
fn parse_timestamp(ts: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Format Unix milliseconds to ISO string
fn format_timestamp(ts: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ts)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

/// Extract project name from path
fn extract_project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_header() {
        let json = r#"{"type":"session","version":3,"id":"test-uuid","timestamp":"2024-12-03T14:00:00.000Z","cwd":"/path/to/project"}"#;
        let entry: PiEntry = serde_json::from_str(json).unwrap();
        match entry {
            PiEntry::Session(header) => {
                assert_eq!(header.version, 3);
                assert_eq!(header.id, "test-uuid");
                assert_eq!(header.cwd, "/path/to/project");
            }
            _ => panic!("Expected session entry"),
        }
    }

    #[test]
    fn parse_user_message() {
        let json = r#"{"type":"message","id":"a1b2c3d4","parentId":null,"timestamp":"2024-12-03T14:00:01.000Z","message":{"role":"user","content":"Hello","timestamp":1733236801000}}"#;
        let entry: PiEntry = serde_json::from_str(json).unwrap();
        match entry {
            PiEntry::Message(msg) => {
                assert_eq!(msg.base.id, "a1b2c3d4");
                match msg.message {
                    PiAgentMessage::User(user) => match user.content {
                        PiContent::Text(text) => assert_eq!(text, "Hello"),
                        _ => panic!("Expected text content"),
                    },
                    _ => panic!("Expected user message"),
                }
            }
            _ => panic!("Expected message entry"),
        }
    }

    #[test]
    fn parse_assistant_message_with_usage() {
        let json = r#"{"type":"message","id":"b2c3d4e5","parentId":"a1b2c3d4","timestamp":"2024-12-03T14:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Hi!"}],"provider":"anthropic","model":"claude-sonnet-4-5","usage":{"input":100,"output":50,"cacheRead":0,"cacheWrite":0,"totalTokens":150},"stopReason":"stop","timestamp":1733236802000}}"#;
        let entry: PiEntry = serde_json::from_str(json).unwrap();
        match entry {
            PiEntry::Message(msg) => match msg.message {
                PiAgentMessage::Assistant(assistant) => {
                    assert_eq!(assistant.provider, Some("anthropic".to_string()));
                    assert_eq!(assistant.model, Some("claude-sonnet-4-5".to_string()));
                    let usage = assistant.usage.unwrap();
                    assert_eq!(usage.input, 100);
                    assert_eq!(usage.output, 50);
                }
                _ => panic!("Expected assistant message"),
            },
            _ => panic!("Expected message entry"),
        }
    }

    #[test]
    fn extract_messages_splits_thinking_and_merges_multiple_tool_results() {
        let entries: Vec<PiEntry> = [
            r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:00:00.000Z","message":{"role":"user","content":"Inspect files","timestamp":1781074800000}}"#,
            r#"{"type":"message","id":"assistant-1","parentId":"user-1","timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Need to read files","thinkingSignature":"reasoning_content"},{"type":"text","text":"I will inspect these files."},{"type":"toolCall","id":"call-read","name":"read","arguments":{"path":"README.md"}},{"type":"toolCall","id":"call-bash","name":"bash","arguments":{"command":"pwd"}}],"provider":"pi-test","model":"mimo-test","usage":{"input":10,"output":5,"cacheRead":2,"cacheWrite":1,"totalTokens":18},"stopReason":"toolUse","timestamp":1781074801000}}"#,
            r#"{"type":"message","id":"result-1","parentId":"assistant-1","timestamp":"2026-06-10T07:00:02.000Z","message":{"role":"toolResult","toolCallId":"call-read","toolName":"read","content":[{"type":"text","text":"file body"}],"details":{},"isError":false,"timestamp":1781074802000}}"#,
            r#"{"type":"message","id":"result-2","parentId":"result-1","timestamp":"2026-06-10T07:00:03.000Z","message":{"role":"toolResult","toolCallId":"call-bash","toolName":"bash","content":[{"type":"text","text":"/tmp/project"}],"details":{"truncation":{"fullOutputPath":"/tmp/pi-output.log"}},"isError":false,"timestamp":1781074803000}}"#,
        ]
        .into_iter()
        .map(|json| serde_json::from_str(json).unwrap())
        .collect();

        let branch = build_active_branch(&entries);
        let messages = extract_messages(&entries, &branch);

        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.starts_with("[thinking]\n"));
        assert_eq!(messages[2].role, MessageRole::Assistant);
        assert_eq!(messages[2].content, "I will inspect these files.");
        assert!(!messages[2].content.contains("[thinking]"));
        assert_eq!(messages[2].token_usage.as_ref().unwrap().input_tokens, 10);

        assert_eq!(messages[3].role, MessageRole::Tool);
        assert_eq!(messages[3].tool_name.as_deref(), Some("Read"));
        assert_eq!(messages[3].content, "file body");
        assert_eq!(
            messages[3]
                .tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.ids.get("tool_use_id"))
                .map(String::as_str),
            Some("call-read")
        );
        assert_eq!(
            messages[3]
                .tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.status.as_deref()),
            Some("success")
        );

        assert_eq!(messages[4].role, MessageRole::Tool);
        assert_eq!(messages[4].tool_name.as_deref(), Some("Bash"));
        assert_eq!(messages[4].content, "/tmp/project");
        assert_eq!(
            messages[4]
                .tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.result_kind.as_deref()),
            Some("persisted_output")
        );
    }

    #[test]
    fn extract_messages_keeps_tool_only_turn_as_tool_with_error_status() {
        let entries: Vec<PiEntry> = [
            r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:00:00.000Z","message":{"role":"user","content":"Edit file","timestamp":1781074800000}}"#,
            r#"{"type":"message","id":"assistant-1","parentId":"user-1","timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Need to edit"},{"type":"toolCall","id":"call-edit","name":"edit","arguments":{"path":"src/main.rs","oldText":"old","newText":"new"}}],"provider":"pi-test","model":"mimo-test","usage":{"input":4,"output":3,"cacheRead":0,"cacheWrite":0,"totalTokens":7},"stopReason":"toolUse","timestamp":1781074801000}}"#,
            r#"{"type":"message","id":"result-1","parentId":"assistant-1","timestamp":"2026-06-10T07:00:02.000Z","message":{"role":"toolResult","toolCallId":"call-edit","toolName":"edit","content":[{"type":"text","text":"replacement failed"}],"details":{"diff":"--- a\n+++ b"},"isError":true,"timestamp":1781074802000}}"#,
        ]
        .into_iter()
        .map(|json| serde_json::from_str(json).unwrap())
        .collect();

        let branch = build_active_branch(&entries);
        let messages = extract_messages(&entries, &branch);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].role, MessageRole::System);
        assert_eq!(messages[2].role, MessageRole::Tool);
        assert_eq!(messages[2].tool_name.as_deref(), Some("Edit"));
        assert_eq!(messages[2].content, "replacement failed");
        assert!(messages[2].token_usage.is_some());
        assert_eq!(
            messages[2]
                .tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.status.as_deref()),
            Some("error")
        );
    }

    #[test]
    fn extract_project_name_test() {
        assert_eq!(extract_project_name("/path/to/project"), "project");
        assert_eq!(extract_project_name("/home/user/code"), "code");
        assert_eq!(extract_project_name("/"), "/");
    }

    #[test]
    #[ignore = "requires local Pi session data"]
    fn parse_real_local_session() {
        // Use real local Pi session data if available
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => return,
        };
        let sessions_dir = home.join(".pi").join("agent").join("sessions");
        if !sessions_dir.exists() {
            return;
        }

        // Find first JSONL file
        let mut session_file = None;
        for entry in std::fs::read_dir(&sessions_dir).into_iter().flatten() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            for file in std::fs::read_dir(&path).into_iter().flatten() {
                let file = match file {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let file_path = file.path();
                if file_path.extension().is_some_and(|ext| ext == "jsonl") {
                    session_file = Some(file_path);
                    break;
                }
            }
            if session_file.is_some() {
                break;
            }
        }

        let file_path = match session_file {
            Some(f) => f,
            None => return,
        };

        // Parse the session
        let result = parse_session_file(&file_path);
        assert!(
            result.is_some(),
            "Failed to parse real Pi session: {}",
            file_path.display()
        );

        let session = result.unwrap();

        // Verify basic structure
        assert_eq!(session.meta.provider, Provider::Pi);
        assert!(
            !session.meta.id.is_empty(),
            "Session ID should not be empty"
        );
        assert!(
            !session.meta.title.is_empty(),
            "Session title should not be empty"
        );
        assert!(
            !session.meta.project_path.is_empty(),
            "Project path should not be empty"
        );
        assert!(
            !session.meta.project_name.is_empty(),
            "Project name should not be empty"
        );
        assert!(
            session.meta.created_at > 0,
            "Created timestamp should be positive"
        );
        assert!(
            session.meta.updated_at > 0,
            "Updated timestamp should be positive"
        );
        assert!(
            session.meta.message_count > 0,
            "Message count should be positive"
        );
        assert!(
            session.meta.file_size_bytes > 0,
            "File size should be positive"
        );

        // Verify messages
        assert!(!session.messages.is_empty(), "Messages should not be empty");
        for msg in &session.messages {
            assert!(
                !msg.content.is_empty() || msg.tool_name.is_some(),
                "Message should have content or tool name"
            );
        }

        // Verify source path
        assert_eq!(session.meta.source_path, file_path.to_string_lossy());

        println!(
            "Parsed Pi session: id={}, title={}, messages={}, tokens={}",
            session.meta.id,
            session.meta.title,
            session.meta.message_count,
            session.meta.input_tokens + session.meta.output_tokens
        );
    }
}
