use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::models::{McpToolMetadata, Provider, ToolMetadata};
use crate::provider_utils::shorten_home_path;

pub struct ToolCallFacts<'a> {
    pub provider: Provider,
    pub raw_name: &'a str,
    pub input: Option<&'a Value>,
    pub call_id: Option<&'a str>,
    pub assistant_id: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub struct ToolResultFacts<'a> {
    pub raw_result: Option<&'a Value>,
    pub is_error: Option<bool>,
    pub status: Option<&'a str>,
    pub artifact_path: Option<&'a str>,
}

pub fn parse_mcp_tool_name(name: &str) -> Option<McpToolMetadata> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    Some(McpToolMetadata {
        server: server.to_string(),
        tool: tool.to_string(),
        display: tool.replace('_', " "),
    })
}

pub fn canonical_tool_name(provider: Provider, name: &str) -> String {
    if provider == Provider::Gemini && (name.contains("Agent") || name.contains("agent")) {
        return "Agent".to_string();
    }

    match name {
        "Shell" | "shell" | "bash" | "exec_command" | "shell_command" | "run_shell_command"
        | "run_in_terminal" | "write_stdin" | "Monitor" | "LocalShellCall" => "Bash",
        "Read" | "read" | "ReadFile" | "read_file" | "view" => "Read",
        "read_mcp_resource" => "ListMcpResourcesTool",
        "Write" | "write" | "WriteFile" | "write_file" | "create" => "Write",
        "Edit" | "edit" | "edit_file" | "replace" | "StrReplace" | "str_replace"
        | "StrReplaceFile" | "ApplyPatch" | "Apply_patch" | "MultiEdit" | "str_replace_editor"
        | "apply_patch" | "EditNotebook" => "Edit",
        "Delete" | "delete" => "Delete",
        "Grep"
        | "grep"
        | "rg"
        | "Search"
        | "SemanticSearch"
        | "grep_search"
        | "search_file_content" => "Grep",
        "Glob" | "glob" | "file_search" | "ReadFolder" | "list_directory" | "list" => "Glob",
        "Task" | "task" | "Subagent" | "agent" | "read_agent" | "spawn_agent" | "wait_agent"
        | "send_input" | "close_agent" => "Agent",
        "send_message" => "SendMessage",
        "followup_task" => "FollowupTask",
        "list_agents" => "ListAgents",
        "update_plan" | "TodoWrite" | "todo" | "todowrite" | "Enter Plan Mode"
        | "EnterPlanMode" | "ExitPlanMode" | "enter_plan_mode" | "exit_plan_mode" => "Plan",
        "request_user_input" | "ask_user" | "question" => "AskUserQuestion",
        "request_permissions" => "RequestPermissions",
        "ScheduleWakeup" => "ScheduleWakeup",
        "ReadLints" => "Lint",
        "web_fetch" | "webfetch" => "WebFetch",
        "web_search" | "web_search_call" | "websearch" => "WebSearch",
        "image_generation_call" | "image_generation_end" => "ImageGeneration",
        "dynamic_tool_call"
        | "dynamic_tool_call_request"
        | "dynamic_tool_call_response"
        | "load_workspace_dependencies"
        | "install_workspace_dependencies" => "DynamicTool",
        "codesearch" => "ToolSearch",
        "list_mcp_resources" | "list_mcp_resource_templates" => "ListMcpResourcesTool",
        "skill" => "Skill",
        "sql" | "SQL" => "SQL",
        other => other,
    }
    .to_string()
}

fn tool_category(canonical_name: &str, raw_name: &str) -> String {
    if raw_name.starts_with("mcp__") {
        return "mcp".to_string();
    }

    match canonical_name {
        "Bash" => "shell",
        "Read" | "Write" | "Edit" | "Delete" => "file",
        "Grep" | "Glob" | "Search" | "ToolSearch" | "ListMcpResourcesTool" => "search",
        "Agent" | "SendMessage" | "ListAgents" => "agent",
        "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskStop" => "task",
        "FollowupTask" => "task",
        "WebSearch" | "WebFetch" => "web",
        "ImageGeneration" => "media",
        "DynamicTool" => "tool",
        "Skill" => "skill",
        "CronCreate" | "CronDelete" | "ScheduleWakeup" => "cron",
        "EnterPlanMode" | "ExitPlanMode" | "Plan" => "plan",
        "AskUserQuestion" | "RequestPermissions" => "interaction",
        "SQL" => "database",
        _ => "unknown",
    }
    .to_string()
}

fn display_tool_name(raw_name: &str, canonical_name: &str) -> String {
    if let Some(mcp) = parse_mcp_tool_name(raw_name) {
        return mcp.display;
    }
    match raw_name {
        "write_stdin" => "write stdin".to_string(),
        "Monitor" => "monitor".to_string(),
        "ScheduleWakeup" => "schedule wakeup".to_string(),
        "update_plan" => "update plan".to_string(),
        "request_user_input" => "request user input".to_string(),
        "request_permissions" => "request permissions".to_string(),
        "apply_patch" => "apply patch".to_string(),
        "spawn_agent" => "spawn agent".to_string(),
        "wait_agent" => "wait agent".to_string(),
        "send_input" => "send input".to_string(),
        "close_agent" => "close agent".to_string(),
        "send_message" => "send message".to_string(),
        "followup_task" => "followup task".to_string(),
        "list_agents" => "list agents".to_string(),
        "list_mcp_resources" => "list mcp resources".to_string(),
        "list_mcp_resource_templates" => "list mcp resource templates".to_string(),
        "read_mcp_resource" => "read mcp resource".to_string(),
        "todowrite" => "todo write".to_string(),
        "question" => "question".to_string(),
        "webfetch" => "web fetch".to_string(),
        "websearch" => "web search".to_string(),
        "image_generation_call" | "image_generation_end" => "image generation".to_string(),
        "dynamic_tool_call" | "dynamic_tool_call_request" | "dynamic_tool_call_response" => {
            "dynamic tool".to_string()
        }
        "load_workspace_dependencies" => "load workspace dependencies".to_string(),
        "install_workspace_dependencies" => "install workspace dependencies".to_string(),
        "codesearch" => "code search".to_string(),
        "skill" => "skill".to_string(),
        "list" => "list".to_string(),
        _ => canonical_name.to_string(),
    }
}

fn compact_string(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let end = value.floor_char_boundary(limit);
    format!("{}…", &value[..end])
}

fn string_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
}

fn input_summary(canonical_name: &str, raw_name: &str, input: Option<&Value>) -> Option<String> {
    let input = input?;
    let summary = match canonical_name {
        "Read" | "Write" | "Edit" => string_field(input, &["file_path", "filePath", "path"])
            .map(shorten_home_path)
            .unwrap_or_default(),
        "Bash" => string_field(input, &["description", "command", "cmd"])
            .map(|s| compact_string(s, 80))
            .unwrap_or_default(),
        "ScheduleWakeup" => {
            let delay = input
                .get("delaySeconds")
                .or_else(|| input.get("delay_seconds"))
                .and_then(|v| v.as_u64())
                .map(|seconds| format!("{seconds}s"));
            let reason = string_field(input, &["reason"])
                .map(|s| compact_string(s, 80))
                .unwrap_or_default();
            [delay.unwrap_or_default(), reason]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        }
        "Grep" => string_field(input, &["pattern", "query"])
            .map(|pattern| {
                let mut value = format!("/{}/", compact_string(pattern, 60));
                if let Some(path) = string_field(input, &["path", "dir_path"]) {
                    value.push(' ');
                    value.push_str(&shorten_home_path(path));
                }
                value
            })
            .unwrap_or_default(),
        "Glob" => string_field(input, &["pattern"])
            .or_else(|| string_field(input, &["dir_path", "path"]))
            .unwrap_or_default()
            .to_string(),
        "Agent" => {
            if raw_name == "wait_agent" {
                input
                    .get("targets")
                    .and_then(|v| v.as_array())
                    .map(|arr| match arr.len() {
                        0 => String::new(),
                        1 => arr[0]
                            .as_str()
                            .map(|s| compact_string(s, 40))
                            .unwrap_or_default(),
                        n => format!("{n} agents"),
                    })
                    .unwrap_or_default()
            } else if matches!(raw_name, "send_input" | "close_agent" | "read_agent") {
                string_field(input, &["target", "agent_id"])
                    .map(|s| compact_string(s, 40))
                    .unwrap_or_default()
            } else {
                // spawn_agent / Task / Subagent / agent / read_agent
                string_field(input, &["description", "prompt", "message"])
                    .map(|s| compact_string(s, 80))
                    .unwrap_or_default()
            }
        }
        "SendMessage" | "FollowupTask" => {
            string_field(input, &["description", "prompt", "message"])
                .map(|s| compact_string(s, 80))
                .unwrap_or_default()
        }
        "TaskCreate" => string_field(input, &["subject", "description"])
            .map(|s| compact_string(s, 80))
            .unwrap_or_default(),
        "TaskUpdate" => {
            let id = string_field(input, &["taskId", "task_id"]).unwrap_or_default();
            let status = string_field(input, &["status"]).unwrap_or_default();
            [id, status]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" → ")
        }
        "TaskStop" => string_field(input, &["task_id", "taskId"])
            .unwrap_or_default()
            .to_string(),
        "Skill" => string_field(input, &["skill"])
            .unwrap_or_default()
            .to_string(),
        "ToolSearch" => string_field(input, &["query"])
            .unwrap_or_default()
            .to_string(),
        "WebSearch" => string_field(input, &["query"])
            .unwrap_or_default()
            .to_string(),
        "WebFetch" => string_field(input, &["url"])
            .unwrap_or_default()
            .to_string(),
        "ImageGeneration" => string_field(input, &["revised_prompt", "prompt"])
            .map(|s| compact_string(s, 80))
            .unwrap_or_default(),
        "DynamicTool" => string_field(input, &["tool", "name"])
            .or_else(|| Some(raw_name).filter(|name| *name != "dynamic_tool_call"))
            .map(|s| compact_string(s, 80))
            .unwrap_or_default(),
        "ListMcpResourcesTool" => {
            let server = string_field(input, &["server"]).unwrap_or_default();
            let uri = string_field(input, &["uri"]).unwrap_or_default();
            [server, uri]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }
        "AskUserQuestion" => input
            .get("questions")
            .and_then(|v| v.as_array())
            .map(|questions| format!("{} question(s)", questions.len()))
            .unwrap_or_default(),
        "Plan" => {
            if let Some(explanation) = string_field(input, &["explanation"]) {
                compact_string(explanation, 80)
            } else if let Some(todos) = input.get("todos").and_then(|v| v.as_array()) {
                format!("{} todo(s)", todos.len())
            } else {
                input
                    .get("plan")
                    .and_then(|v| v.as_array())
                    .map(|steps| format!("{} step(s)", steps.len()))
                    .unwrap_or_default()
            }
        }
        _ if raw_name == "write_stdin" => {
            if let Some(session_id) = input.get("session_id").and_then(|v| v.as_u64()) {
                format!("session {session_id}")
            } else {
                string_field(input, &["chars"])
                    .map(|s| compact_string(s, 80))
                    .unwrap_or_default()
            }
        }
        _ if raw_name.starts_with("mcp__") => {
            string_field(input, &["element", "url", "filter", "level"])
                .map(|s| compact_string(s, 80))
                .unwrap_or_default()
        }
        _ => input
            .as_object()
            .and_then(|obj| {
                obj.values()
                    .find_map(|v| v.as_str().filter(|s| !s.is_empty()))
            })
            .map(|s| compact_string(s, 80))
            .unwrap_or_default(),
    };

    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

fn compact_json_value(value: &Value, depth: usize) -> Value {
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

fn result_kind_for_tool(raw_name: &str, result: Option<&Value>) -> Option<String> {
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

fn normalized_status(result: ToolResultFacts<'_>) -> Option<String> {
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

pub fn build_tool_metadata(call: ToolCallFacts<'_>) -> ToolMetadata {
    let canonical_name = canonical_tool_name(call.provider, call.raw_name);
    let display_name = display_tool_name(call.raw_name, &canonical_name);
    let mut ids = BTreeMap::new();
    if let Some(id) = call.call_id {
        ids.insert("tool_use_id".to_string(), id.to_string());
    }
    if let Some(id) = call.assistant_id {
        ids.insert("assistant_id".to_string(), id.to_string());
    }

    ToolMetadata {
        raw_name: call.raw_name.to_string(),
        canonical_name: canonical_name.clone(),
        display_name,
        category: tool_category(&canonical_name, call.raw_name),
        summary: input_summary(&canonical_name, call.raw_name, call.input),
        status: None,
        ids,
        mcp: parse_mcp_tool_name(call.raw_name),
        result_kind: None,
        structured: None,
    }
}

pub fn enrich_tool_metadata(metadata: &mut ToolMetadata, result: ToolResultFacts<'_>) {
    metadata.status = normalized_status(ToolResultFacts { ..result });
    metadata.result_kind = result_kind_for_tool(&metadata.raw_name, result.raw_result)
        .or_else(|| metadata.result_kind.clone());
    metadata.structured = result
        .raw_result
        .map(|value| {
            let mut compact = compact_json_value(value, 0);
            normalize_structured_result(&mut compact);
            compact
        })
        .or_else(|| metadata.structured.clone());
    if let Some(path) = result.artifact_path {
        let mut structured = metadata
            .structured
            .take()
            .unwrap_or_else(|| Value::Object(Map::new()));
        if !structured.is_object() {
            structured = Value::Object(Map::new());
        }
        if let Value::Object(obj) = &mut structured {
            obj.insert(
                "persistedOutputPath".to_string(),
                Value::String(path.to_string()),
            );
        }
        metadata.structured = Some(structured);
    }
}

fn normalize_structured_result(value: &mut Value) {
    let Value::Object(obj) = value else {
        return;
    };

    promote_string_alias(obj, "agent_id", "agentId");
    // Codex v2 dropped `agent_id` from spawn_agent results (upstream #17005);
    // fall back to the `new_thread_id` carried by `collab_agent_spawn_end`.
    promote_string_alias(obj, "new_thread_id", "agentId");
    promote_string_alias(obj, "task_id", "taskId");

    if obj.contains_key("persistedOutputPath") {
        return;
    }
    let path = obj
        .get("outputPath")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            obj.get("metadata")
                .and_then(|v| v.as_object())
                .and_then(|metadata| metadata.get("outputPath"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    if let Some(path) = path {
        obj.insert("persistedOutputPath".to_string(), Value::String(path));
    }
}

fn promote_string_alias(obj: &mut Map<String, Value>, from: &str, to: &str) {
    if obj.contains_key(to) {
        return;
    }
    let Some(value) = obj.get(from).cloned() else {
        return;
    };
    obj.insert(to.to_string(), value);
}

#[cfg(test)]
mod tests {
    use super::{
        build_tool_metadata, enrich_tool_metadata, parse_mcp_tool_name, ToolCallFacts,
        ToolResultFacts,
    };
    use crate::models::Provider;
    use serde_json::json;

    #[test]
    fn maps_common_tool_aliases_to_canonical_names() {
        for (raw, canonical) in [
            ("Shell", "Bash"),
            ("exec_command", "Bash"),
            ("ReadFile", "Read"),
            ("apply_patch", "Edit"),
            ("ApplyPatch", "Edit"),
            ("EditNotebook", "Edit"),
            ("delete", "Delete"),
            ("update_plan", "Plan"),
            ("ExitPlanMode", "Plan"),
            ("ScheduleWakeup", "ScheduleWakeup"),
            ("Monitor", "Bash"),
            ("image_generation_call", "ImageGeneration"),
            ("dynamic_tool_call", "DynamicTool"),
            ("load_workspace_dependencies", "DynamicTool"),
            ("write_stdin", "Bash"),
            ("request_user_input", "AskUserQuestion"),
            ("question", "AskUserQuestion"),
            ("SemanticSearch", "Grep"),
            ("read_mcp_resource", "ListMcpResourcesTool"),
            ("list_mcp_resources", "ListMcpResourcesTool"),
            ("list_mcp_resource_templates", "ListMcpResourcesTool"),
            ("Subagent", "Agent"),
            ("spawn_agent", "Agent"),
            ("send_message", "SendMessage"),
            ("followup_task", "FollowupTask"),
            ("list_agents", "ListAgents"),
            ("request_permissions", "RequestPermissions"),
            ("todowrite", "Plan"),
            ("webfetch", "WebFetch"),
            ("websearch", "WebSearch"),
            ("codesearch", "ToolSearch"),
            ("skill", "Skill"),
            ("list", "Glob"),
            ("sql", "SQL"),
        ] {
            let metadata = build_tool_metadata(ToolCallFacts {
                provider: Provider::Claude,
                raw_name: raw,
                input: None,
                call_id: None,
                assistant_id: None,
            });
            assert_eq!(metadata.canonical_name, canonical);
        }
    }

    #[test]
    fn sql_tool_uses_database_category() {
        let metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Claude,
            raw_name: "sql",
            input: None,
            call_id: None,
            assistant_id: None,
        });
        assert_eq!(metadata.canonical_name, "SQL");
        assert_eq!(metadata.category, "database");
    }

    #[test]
    fn promotes_snake_case_ids_and_nested_output_path() {
        let mut metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Codex,
            raw_name: "spawn_agent",
            input: None,
            call_id: Some("call_1"),
            assistant_id: None,
        });
        enrich_tool_metadata(
            &mut metadata,
            ToolResultFacts {
                raw_result: Some(&json!({
                    "agent_id": "agent-123",
                    "task_id": "task-456",
                    "metadata": {
                        "outputPath": "/tmp/tool-output.txt"
                    }
                })),
                is_error: Some(false),
                status: None,
                artifact_path: None,
            },
        );

        assert_eq!(metadata.result_kind.as_deref(), Some("persisted_output"));
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("agentId"))
                .and_then(|value| value.as_str()),
            Some("agent-123")
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("taskId"))
                .and_then(|value| value.as_str()),
            Some("task-456")
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("persistedOutputPath"))
                .and_then(|value| value.as_str()),
            Some("/tmp/tool-output.txt")
        );
    }

    #[test]
    fn promotes_new_thread_id_to_agent_id_alias() {
        let mut metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Codex,
            raw_name: "spawn_agent",
            input: None,
            call_id: Some("call_x"),
            assistant_id: None,
        });
        enrich_tool_metadata(
            &mut metadata,
            ToolResultFacts {
                raw_result: Some(&json!({
                    "new_thread_id": "019dae0e-8a30-76f2-92cc-e81cfcf0d125",
                    "new_agent_nickname": "Hume",
                })),
                is_error: Some(false),
                status: Some("success"),
                artifact_path: None,
            },
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("agentId"))
                .and_then(|value| value.as_str()),
            Some("019dae0e-8a30-76f2-92cc-e81cfcf0d125"),
            "new_thread_id must populate agentId when agent_id is absent (codex >=0.123)"
        );
    }

    #[test]
    fn agent_input_summary_covers_codex_spawn_wait_send_close() {
        fn summary(raw: &str, input: serde_json::Value) -> Option<String> {
            let metadata = build_tool_metadata(ToolCallFacts {
                provider: Provider::Codex,
                raw_name: raw,
                input: Some(&input),
                call_id: None,
                assistant_id: None,
            });
            metadata.summary
        }

        // spawn_agent: falls back to `message` when description/prompt absent
        assert_eq!(
            summary(
                "spawn_agent",
                json!({ "message": "你负责实现 1211 角色配置", "task_name": "worker" })
            )
            .as_deref(),
            Some("你负责实现 1211 角色配置")
        );
        // wait_agent: 0 targets → no summary
        assert_eq!(
            summary("wait_agent", json!({ "targets": [], "timeout_ms": 10000 })),
            None
        );
        // wait_agent: 1 target → compact target id
        assert_eq!(
            summary(
                "wait_agent",
                json!({ "targets": ["019dae0e-8a30-76f2-92cc-e81cfcf0d125"] })
            )
            .as_deref(),
            Some("019dae0e-8a30-76f2-92cc-e81cfcf0d125")
        );
        // wait_agent: multiple targets → "N agents"
        assert_eq!(
            summary(
                "wait_agent",
                json!({ "targets": ["a", "b", "c"], "timeout_ms": 10000 })
            )
            .as_deref(),
            Some("3 agents")
        );
        // send_input / close_agent / read_agent: target id
        for raw in ["send_input", "close_agent", "read_agent"] {
            assert_eq!(
                summary(raw, json!({ "target": "019dae0e-8a30" })).as_deref(),
                Some("019dae0e-8a30"),
                "{raw} summary must come from input.target"
            );
        }
    }

    #[test]
    fn parses_mcp_tool_names() {
        let mcp = parse_mcp_tool_name("mcp__plugin_playwright__browser_snapshot").unwrap();
        assert_eq!(mcp.server, "plugin_playwright");
        assert_eq!(mcp.tool, "browser_snapshot");
        assert_eq!(mcp.display, "browser snapshot");
    }

    #[test]
    fn summarizes_new_tool_aliases_and_omits_large_media_fields() {
        let wakeup = build_tool_metadata(ToolCallFacts {
            provider: Provider::Claude,
            raw_name: "ScheduleWakeup",
            input: Some(&json!({
                "delaySeconds": 60,
                "reason": "wait for service startup"
            })),
            call_id: None,
            assistant_id: None,
        });
        assert_eq!(wakeup.category, "cron");
        assert_eq!(
            wakeup.summary.as_deref(),
            Some("60s · wait for service startup")
        );

        let image = build_tool_metadata(ToolCallFacts {
            provider: Provider::Codex,
            raw_name: "image_generation_call",
            input: Some(&json!({ "revised_prompt": "make an icon" })),
            call_id: Some("ig_1"),
            assistant_id: None,
        });
        assert_eq!(image.category, "media");
        assert_eq!(image.summary.as_deref(), Some("make an icon"));

        let mut dynamic = build_tool_metadata(ToolCallFacts {
            provider: Provider::Codex,
            raw_name: "load_workspace_dependencies",
            input: Some(&json!({ "tool": "load_workspace_dependencies" })),
            call_id: Some("call_1"),
            assistant_id: None,
        });
        assert_eq!(dynamic.category, "tool");
        assert_eq!(
            dynamic.summary.as_deref(),
            Some("load_workspace_dependencies")
        );
        enrich_tool_metadata(
            &mut dynamic,
            ToolResultFacts {
                raw_result: Some(&json!({
                    "success": true,
                    "base64": "long image payload",
                    "data": { "message": "keep structured data" },
                    "image": "data:image/png;base64,long image payload",
                    "content": "ok"
                })),
                is_error: Some(false),
                status: None,
                artifact_path: None,
            },
        );
        assert_eq!(
            dynamic
                .structured
                .as_ref()
                .and_then(|value| value.get("base64"))
                .and_then(|value| value.as_str()),
            Some("<omitted>")
        );
        assert_eq!(
            dynamic
                .structured
                .as_ref()
                .and_then(|value| value.get("data"))
                .and_then(|value| value.get("message"))
                .and_then(|value| value.as_str()),
            Some("keep structured data")
        );
        assert_eq!(
            dynamic
                .structured
                .as_ref()
                .and_then(|value| value.get("image"))
                .and_then(|value| value.as_str()),
            Some("<omitted>")
        );
    }

    #[test]
    fn compacts_large_structured_results() {
        let mut metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Claude,
            raw_name: "Edit",
            input: None,
            call_id: Some("toolu_1"),
            assistant_id: Some("assistant-1"),
        });
        enrich_tool_metadata(
            &mut metadata,
            ToolResultFacts {
                raw_result: Some(&json!({
                    "filePath": "/repo/src/main.rs",
                    "originalFile": "very large",
                    "oldString": "old",
                    "newString": "new",
                    "structuredPatch": [{
                        "oldStart": 1,
                        "oldLines": 1,
                        "newStart": 1,
                        "newLines": 1,
                        "lines": ["-old", "+new"]
                    }]
                })),
                is_error: Some(false),
                status: None,
                artifact_path: None,
            },
        );

        assert_eq!(metadata.category, "file");
        assert_eq!(metadata.result_kind.as_deref(), Some("file_patch"));
        assert_eq!(
            metadata.ids.get("tool_use_id").map(String::as_str),
            Some("toolu_1")
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("originalFile"))
                .and_then(|value| value.as_str()),
            Some("<omitted>")
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("structuredPatch"))
                .and_then(|value| value.get(0))
                .and_then(|value| value.get("lines"))
                .and_then(|value| value.get(0))
                .and_then(|value| value.as_str()),
            Some("-old")
        );
    }
}
