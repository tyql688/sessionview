//! Pure serde_json `Value`-builder helpers shared by the Codex parser's
//! per-line dispatch. These functions normalize Codex's raw payload shapes
//! into the canonical structured-result shape consumed by `tool_metadata`,
//! pair tool calls with their outputs, and surface system events. They hold
//! no cross-line state — every input is a borrowed `Value` (or accumulator
//! slice) and every output is a freshly constructed `Value`/`Message`.

use serde_json::{json, Map, Value};

use crate::models::{Message, MessageRole};
use crate::tool_metadata::{enrich_tool_metadata, ToolResultFacts};

pub(super) fn parse_json_str(value: Option<&str>) -> Option<Value> {
    serde_json::from_str(value?).ok()
}

pub(super) fn codex_tool_input_value(
    raw_name: &str,
    raw_input: Option<&str>,
    tool_input: Option<&str>,
) -> Option<Value> {
    if raw_name == "apply_patch" {
        return tool_input.map(|patch| json!({ "patch": patch }));
    }

    parse_json_str(tool_input).or_else(|| parse_json_str(raw_input))
}

pub(super) fn codex_tool_result_value(raw_output: &str, output: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(raw_output.trim()) {
        if let Some(obj) = value.as_object() {
            if let Some(metadata) = obj.get("metadata").and_then(|v| v.as_object()) {
                let mut result = serde_json::Map::new();
                result.insert("stdout".to_string(), json!(output));
                if let Some(exit_code) = metadata.get("exit_code") {
                    result.insert("exitCode".to_string(), exit_code.clone());
                }
                if let Some(duration) = metadata.get("duration_seconds") {
                    result.insert("durationSeconds".to_string(), duration.clone());
                }
                return Some(Value::Object(result));
            }
        }
        return Some(value);
    }

    if output.trim().is_empty() {
        None
    } else {
        Some(json!({ "stdout": output }))
    }
}

pub(super) fn codex_duration_seconds(value: Option<&Value>) -> Option<f64> {
    let duration = value?.as_object()?;
    let secs = duration.get("secs").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let nanos = duration
        .get("nanos")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    Some(secs + nanos / 1_000_000_000.0)
}

pub(super) fn codex_exec_command_event_result(payload: &Value, fallback_output: &str) -> Value {
    let mut result = Map::new();
    if let Some(command) = payload.get("command") {
        result.insert("command".to_string(), command.clone());
    }
    if let Some(cwd) = payload.get("cwd") {
        result.insert("cwd".to_string(), cwd.clone());
    }
    if let Some(parsed_cmd) = payload.get("parsed_cmd") {
        result.insert("parsedCmd".to_string(), parsed_cmd.clone());
    }
    if let Some(source) = payload.get("source") {
        result.insert("source".to_string(), source.clone());
    }
    if let Some(status) = payload.get("status") {
        result.insert("status".to_string(), status.clone());
    }
    if let Some(process_id) = payload.get("process_id") {
        result.insert("processId".to_string(), process_id.clone());
    }
    if let Some(interaction_input) = payload.get("interaction_input") {
        result.insert("interactionInput".to_string(), interaction_input.clone());
    }
    if let Some(exit_code) = payload.get("exit_code") {
        result.insert("exitCode".to_string(), exit_code.clone());
    }
    if let Some(duration) = codex_duration_seconds(payload.get("duration")) {
        result.insert("durationSeconds".to_string(), json!(duration));
    }

    let stdout = payload
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or(fallback_output);
    if !stdout.is_empty() {
        result.insert("stdout".to_string(), json!(stdout));
    }
    if let Some(stderr) = payload.get("stderr").and_then(|v| v.as_str()) {
        if !stderr.is_empty() {
            result.insert("stderr".to_string(), json!(stderr));
        }
    }
    if let Some(aggregated_output) = payload.get("aggregated_output").and_then(|v| v.as_str()) {
        if !aggregated_output.is_empty() {
            result.insert("aggregatedOutput".to_string(), json!(aggregated_output));
        }
    }
    if let Some(formatted_output) = payload.get("formatted_output").and_then(|v| v.as_str()) {
        if !formatted_output.is_empty() {
            result.insert("formattedOutput".to_string(), json!(formatted_output));
        }
    }

    Value::Object(result)
}

pub(super) fn codex_patch_event_patch(path: &str, change: &Value) -> Option<Value> {
    let change_type = change.get("type").and_then(|v| v.as_str())?;
    let mut patch = Map::new();
    patch.insert("files".to_string(), json!([path]));
    patch.insert("changeType".to_string(), json!(change_type));
    if let Some(move_path) = change.get("move_path").and_then(|v| v.as_str()) {
        patch.insert("movePath".to_string(), json!(move_path));
    }
    if let Some(unified_diff) = change.get("unified_diff").and_then(|v| v.as_str()) {
        patch.insert("diff".to_string(), json!(unified_diff));
    }
    Some(Value::Object(patch))
}

pub(super) fn codex_patch_event_result(payload: &Value) -> Value {
    let mut result = Map::new();
    if let Some(stdout) = payload.get("stdout") {
        result.insert("stdout".to_string(), stdout.clone());
    }
    if let Some(stderr) = payload.get("stderr") {
        result.insert("stderr".to_string(), stderr.clone());
    }
    if let Some(success) = payload.get("success") {
        result.insert("success".to_string(), success.clone());
    }
    if let Some(status) = payload.get("status") {
        result.insert("status".to_string(), status.clone());
    }

    let mut combined = Vec::new();
    let mut patches = Vec::new();
    if let Some(changes) = payload.get("changes") {
        result.insert("changes".to_string(), changes.clone());
        if let Some(change_map) = changes.as_object() {
            for (path, change) in change_map {
                if let Some(patch) = codex_patch_event_patch(path, change) {
                    patches.push(patch);
                }
                let header = match change.get("type").and_then(|v| v.as_str()) {
                    Some("add") => format!("*** Add File: {path}"),
                    Some("delete") => format!("*** Delete File: {path}"),
                    _ => format!("*** Update File: {path}"),
                };
                combined.push(header);
                if let Some(move_path) = change.get("move_path").and_then(|v| v.as_str()) {
                    combined.push(format!("*** Move to: {move_path}"));
                }
                if let Some(unified_diff) = change.get("unified_diff").and_then(|v| v.as_str()) {
                    combined.push(unified_diff.to_string());
                }
            }
        }
    }

    if !patches.is_empty() {
        result.insert("patches".to_string(), Value::Array(patches));
    }
    if !combined.is_empty() {
        result.insert("diff".to_string(), json!(combined.join("\n")));
    }

    Value::Object(result)
}

pub(super) fn codex_mcp_tool_call_event_result(payload: &Value) -> Value {
    let mut result = Map::new();
    if let Some(invocation) = payload.get("invocation") {
        result.insert("invocation".to_string(), invocation.clone());
    }
    if let Some(raw_result) = payload.get("result") {
        result.insert("result".to_string(), raw_result.clone());
        let success = raw_result.get("Err").is_none()
            && !raw_result
                .get("Ok")
                .and_then(|ok| ok.get("is_error"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        result.insert("success".to_string(), json!(success));
    }
    if let Some(duration) = codex_duration_seconds(payload.get("duration")) {
        result.insert("durationSeconds".to_string(), json!(duration));
    }
    Value::Object(result)
}

pub(super) fn merge_tool_result(existing: Option<&Value>, update: &Value) -> Value {
    match (existing.and_then(Value::as_object), update.as_object()) {
        (Some(existing), Some(update)) => {
            let mut merged = existing.clone();
            for (key, value) in update {
                merged.insert(key.clone(), value.clone());
            }
            Value::Object(merged)
        }
        _ => update.clone(),
    }
}

pub(super) fn enrich_existing_tool_message(
    message: &mut Message,
    raw_result: Value,
    is_error: Option<bool>,
    status: Option<&str>,
) {
    let Some(metadata) = message.tool_metadata.as_mut() else {
        return;
    };
    let merged = merge_tool_result(metadata.structured.as_ref(), &raw_result);
    enrich_tool_metadata(
        metadata,
        ToolResultFacts {
            raw_result: Some(&merged),
            is_error,
            status,
            artifact_path: None,
        },
    );
}

pub(super) fn codex_call_id(payload: &Value) -> Option<&str> {
    payload
        .get("call_id")
        .or_else(|| payload.get("callId"))
        .or_else(|| payload.get("id"))
        .and_then(|v| v.as_str())
}

pub(super) fn codex_content_items_text(payload: &Value) -> String {
    let items = payload
        .get("content_items")
        .or_else(|| payload.get("content"))
        .and_then(|v| v.as_array());
    let Some(items) = items else {
        return payload
            .get("message")
            .or_else(|| payload.get("output"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    };

    items
        .iter()
        .filter_map(|item| {
            item.get("text")
                .or_else(|| item.get("output_text"))
                .or_else(|| item.get("input_text"))
                .and_then(|v| v.as_str())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn push_system_event(
    messages: &mut Vec<Message>,
    timestamp: Option<String>,
    content: String,
) {
    messages.push(Message {
        role: MessageRole::System,
        content,
        timestamp,
        tool_name: None,
        tool_input: None,
        tool_metadata: None,
        token_usage: None,
        model: None,
        usage_hash: None,
    });
}

pub(super) fn codex_image_generation_input(payload: &Value) -> Value {
    let mut input = Map::new();
    if let Some(prompt) = payload.get("revised_prompt").and_then(|v| v.as_str()) {
        input.insert("revised_prompt".to_string(), json!(prompt));
    }
    if let Some(status) = payload.get("status").and_then(|v| v.as_str()) {
        input.insert("status".to_string(), json!(status));
    }
    Value::Object(input)
}

pub(super) fn codex_image_generation_result(payload: &Value) -> Value {
    let mut result = Map::new();
    if let Some(status) = payload.get("status").and_then(|v| v.as_str()) {
        result.insert("status".to_string(), json!(status));
    }
    if let Some(prompt) = payload.get("revised_prompt").and_then(|v| v.as_str()) {
        result.insert("revisedPrompt".to_string(), json!(prompt));
    }
    if let Some(saved_path) = payload.get("saved_path").and_then(|v| v.as_str()) {
        result.insert("savedPath".to_string(), json!(saved_path));
    }
    Value::Object(result)
}

pub(super) fn dynamic_tool_input(payload: &Value) -> Value {
    let mut input = Map::new();
    if let Some(tool) = payload.get("tool").and_then(|v| v.as_str()) {
        input.insert("tool".to_string(), json!(tool));
    }
    if let Some(namespace) = payload.get("namespace") {
        input.insert("namespace".to_string(), namespace.clone());
    }
    if let Some(arguments) = payload.get("arguments") {
        input.insert("arguments".to_string(), arguments.clone());
    }
    Value::Object(input)
}

pub(super) fn dynamic_tool_result(payload: &Value) -> Value {
    let mut result = Map::new();
    for key in [
        "tool",
        "namespace",
        "arguments",
        "success",
        "error",
        "duration",
    ] {
        if let Some(value) = payload.get(key) {
            result.insert(key.to_string(), value.clone());
        }
    }
    let content = codex_content_items_text(payload);
    if !content.is_empty() {
        result.insert("content".to_string(), json!(content));
    }
    Value::Object(result)
}
