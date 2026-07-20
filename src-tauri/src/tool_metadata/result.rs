use serde_json::{Map, Value, json};

use super::ToolResultFacts;
use crate::provider_utils::shorten_home_path;

pub(super) fn normalize_json_value(value: &Value) -> Value {
    match value {
        Value::Array(arr) => Value::Array(arr.iter().map(normalize_json_value).collect()),
        Value::Object(obj) => {
            let mut next = Map::new();
            for (key, value) in obj {
                if (key == "filePath" || key == "file_path" || key == "path")
                    && let Some(path) = value.as_str()
                {
                    next.insert(key.clone(), json!(shorten_home_path(path)));
                    continue;
                }
                next.insert(key.clone(), normalize_json_value(value));
            }
            Value::Object(next)
        }
        _ => value.clone(),
    }
}

pub(super) fn result_kind_for_tool(
    canonical_name: &str,
    category: &str,
    raw_name: &str,
    result: Option<&Value>,
) -> Option<String> {
    if category == "mcp" || raw_name.starts_with("mcp__") {
        return Some("mcp".to_string());
    }
    if canonical_name == "ImageGeneration" {
        return Some("image".to_string());
    }
    let result = result?;
    if result_output_path(result).is_some() {
        return Some("persisted_output".to_string());
    }

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
        // TaskOutput results ARE terminal output (a background command /
        // subagent's captured stream). Without this the result detail's
        // "output" line and the raw output section render the same text
        // twice — the detail already carries it, so suppress the raw copy.
        || (canonical_name == "TaskOutput" && result.get("output").is_some())
    {
        return Some("terminal_output".to_string());
    }
    if result.get("agentId").is_some() || result.get("agent_id").is_some() {
        return Some("agent_summary".to_string());
    }
    if result.get("task").is_some()
        || result.get("taskId").is_some()
        || result.get("task_id").is_some()
        || result.get("tasks").is_some()
    {
        return Some("task_status".to_string());
    }
    if canonical_name == "ToolSearch" {
        return Some("search_result".to_string());
    }
    if matches!(canonical_name, "WebSearch" | "WebFetch") {
        return Some("web_result".to_string());
    }
    if matches!(canonical_name, "AskUserQuestion" | "RequestPermissions") {
        return Some("interaction_result".to_string());
    }
    if matches!(
        canonical_name,
        "ScheduleWakeup" | "CronCreate" | "CronList" | "CronDelete"
    ) {
        return Some("schedule_result".to_string());
    }
    if matches!(
        canonical_name,
        "CreateGoal" | "GetGoal" | "SetGoalBudget" | "UpdateGoal"
    ) {
        return Some("goal_status".to_string());
    }
    if matches!(
        canonical_name,
        "DynamicTool"
            | "JavaScript"
            | "ComputerUse"
            | "Workflow"
            | "StructuredOutput"
            | "Skill"
            | "SendMessage"
    ) {
        return Some("tool_output".to_string());
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
