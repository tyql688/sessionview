use serde_json::{json, Map, Value};

use super::names::canonical_tool_name;
use super::summary::compact_string;
use super::ToolResultFacts;
use crate::models::Provider;
use crate::provider_utils::shorten_home_path;

pub(super) fn compact_json_value(value: &Value, depth: usize) -> Value {
    if depth > 3 {
        return match value {
            Value::String(s) => Value::String(compact_string(s, 4_000)),
            Value::Number(_) | Value::Bool(_) | Value::Null => value.clone(),
            _ => json!("<nested>"),
        };
    }
    match value {
        Value::String(s) => Value::String(compact_string(s, 4_000)),
        Value::Array(arr) => Value::Array(
            arr.iter()
                .take(25)
                .map(|item| compact_json_value(item, depth + 1))
                .collect(),
        ),
        Value::Object(obj) => {
            let mut next = Map::new();
            for (key, value) in obj {
                if should_omit_large_field(key, value) {
                    next.insert(key.clone(), json!("<omitted>"));
                    continue;
                }
                if key == "structuredPatch" {
                    next.insert(key.clone(), compact_structured_patch(value));
                    continue;
                }
                if key == "filePath" || key == "file_path" || key == "path" {
                    if let Some(path) = value.as_str() {
                        next.insert(key.clone(), json!(shorten_home_path(path)));
                        continue;
                    }
                }
                next.insert(key.clone(), compact_json_value(value, depth + 1));
            }
            Value::Object(next)
        }
        _ => value.clone(),
    }
}

fn should_omit_large_field(key: &str, value: &Value) -> bool {
    match key {
        "originalFile" | "base64" | "b64_json" => true,
        "data" | "image" => value.as_str().is_some_and(is_inline_base64_image),
        _ => false,
    }
}

fn is_inline_base64_image(value: &str) -> bool {
    let value = value.trim_start();
    value.starts_with("data:image/") && value.contains(";base64,")
}

fn compact_structured_patch(value: &Value) -> Value {
    let Some(hunks) = value.as_array() else {
        return compact_json_value(value, 0);
    };

    Value::Array(
        hunks
            .iter()
            .take(25)
            .map(|hunk| {
                let Some(obj) = hunk.as_object() else {
                    return compact_json_value(hunk, 0);
                };
                let mut next = Map::new();
                for (key, value) in obj {
                    if key == "lines" {
                        let lines = value
                            .as_array()
                            .map(|lines| {
                                lines
                                    .iter()
                                    .take(250)
                                    .map(|line| {
                                        line.as_str()
                                            .map(|line| json!(compact_string(line, 4_000)))
                                            .unwrap_or_else(|| compact_json_value(line, 0))
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        next.insert(key.clone(), Value::Array(lines));
                    } else {
                        next.insert(key.clone(), compact_json_value(value, 0));
                    }
                }
                Value::Object(next)
            })
            .collect(),
    )
}

pub(super) fn result_kind_for_tool(raw_name: &str, result: Option<&Value>) -> Option<String> {
    if raw_name.starts_with("mcp__") {
        return Some("mcp".to_string());
    }
    if canonical_tool_name(Provider::Codex, raw_name) == "ImageGeneration" {
        return Some("image".to_string());
    }
    let result = result?;
    if result_output_path(result).is_some() {
        return Some("persisted_output".to_string());
    }

    let canonical_name = canonical_tool_name(Provider::Claude, raw_name);
    if has_patch_result(result)
        || (result.get("oldString").is_some() && result.get("newString").is_some())
        || (result.get("old_string").is_some() && result.get("new_string").is_some())
        || (canonical_name == "Edit" && result.get("output").is_some())
        || (canonical_name == "Edit" && result.get("message").is_some())
    {
        return Some("file_patch".to_string());
    }
    if result.get("stdout").is_some()
        || result.get("stderr").is_some()
        || result.get("exitCode").is_some()
        || (canonical_name == "Bash" && result.get("output").is_some())
    {
        return Some("terminal_output".to_string());
    }
    if result.get("agentId").is_some() || result.get("agent_id").is_some() {
        return Some("agent_summary".to_string());
    }
    if result.get("task").is_some()
        || result.get("taskId").is_some()
        || result.get("task_id").is_some()
    {
        return Some("task_status".to_string());
    }
    None
}

fn result_output_path(result: &Value) -> Option<&str> {
    result
        .get("persistedOutputPath")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("outputPath").and_then(|v| v.as_str()))
        .or_else(|| {
            result
                .get("metadata")
                .and_then(|v| v.as_object())
                .and_then(|obj| obj.get("outputPath"))
                .and_then(|v| v.as_str())
        })
}

fn has_patch_result(result: &Value) -> bool {
    result.get("structuredPatch").is_some()
        || result.get("patch").is_some()
        || result.get("patches").is_some()
        || result.get("fileDiff").is_some()
        || result.get("diff").is_some()
        || result.get("filediff").is_some()
        || result.get("metadata").is_some_and(|metadata| {
            metadata.get("diff").is_some() || metadata.get("filediff").is_some()
        })
        || result
            .get("resultDisplay")
            .is_some_and(|display| display.get("fileDiff").is_some())
        || result
            .get("display")
            .and_then(|v| v.as_array())
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.get("type").and_then(|v| v.as_str()) == Some("diff"))
            })
        || result
            .get("detailedContent")
            .and_then(|v| v.as_str())
            .is_some_and(|content| {
                content.contains("diff --git")
                    || (content.contains("\n--- ") && content.contains("\n+++ "))
            })
}

pub(super) fn normalized_status(result: ToolResultFacts<'_>) -> Option<String> {
    if result.is_error.unwrap_or(false) {
        return Some("error".to_string());
    }
    if let Some(status) = result.status {
        return Some(status.to_string());
    }
    if let Some(result) = result.raw_result {
        if let Some(status) = result.get("status").and_then(|v| v.as_str()) {
            return Some(status.to_string());
        }
        if result
            .get("interrupted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return Some("interrupted".to_string());
        }
        if let Some(success) = result.get("success").and_then(|v| v.as_bool()) {
            return Some(if success { "success" } else { "error" }.to_string());
        }
    }
    Some("success".to_string())
}
