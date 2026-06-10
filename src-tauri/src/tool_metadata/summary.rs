use serde_json::Value;

use crate::provider_utils::shorten_home_path;

pub(super) fn compact_string(value: &str, limit: usize) -> String {
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

pub(super) fn input_summary(
    canonical_name: &str,
    raw_name: &str,
    input: Option<&Value>,
) -> Option<String> {
    let input = input?;
    let summary = match canonical_name {
        "Read" | "Write" | "Edit" => string_field(
            input,
            &[
                "file_path",
                "filePath",
                "path",
                // Antigravity uses PascalCase keys.
                "AbsolutePath",
                "TargetFile",
            ],
        )
        .map(shorten_home_path)
        .unwrap_or_default(),
        "Bash" => string_field(
            input,
            &[
                "description",
                "command",
                "cmd",
                // Antigravity: run_command's `CommandLine` field.
                "CommandLine",
            ],
        )
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
        "Grep" => string_field(input, &["pattern", "query", "Query"])
            .map(|pattern| {
                let mut value = format!("/{}/", compact_string(pattern, 60));
                if let Some(path) = string_field(input, &["path", "dir_path", "SearchPath"]) {
                    value.push(' ');
                    value.push_str(&shorten_home_path(path));
                }
                value
            })
            .unwrap_or_default(),
        "Glob" => string_field(input, &["pattern"])
            .or_else(|| string_field(input, &["dir_path", "path", "DirectoryPath"]))
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
        "WebSearch" => string_field(input, &["query", "Query"])
            .unwrap_or_default()
            .to_string(),
        "WebFetch" => string_field(input, &["url", "Url"])
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
