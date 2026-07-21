//! Result-side detail builders: how a tool's outcome renders.

use serde_json::Value;

use crate::models::{ToolDetail, ToolLine, ToolMetadata};

use super::util::*;

pub(super) fn result_detail_for(metadata: &ToolMetadata) -> Option<ToolDetail> {
    let structured = metadata
        .structured
        .as_ref()
        .and_then(|value| value.as_object());
    let lines = Vec::new();
    let persisted_output_path = structured.and_then(persisted_output_path);

    let mut detail = match (metadata.canonical_name.as_str(), structured) {
        ("Bash", Some(structured)) => bash_result_detail(lines, structured),
        ("Edit" | "Write", Some(structured)) => edit_result_detail(lines, structured),
        ("Agent", Some(structured)) => agent_result_detail(lines, structured),
        (
            "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskOutput" | "TaskStop",
            Some(structured),
        ) => task_result_detail(lines, metadata, structured),
        ("ToolSearch", Some(structured)) => tool_search_result_detail(lines, structured),
        ("WebSearch", Some(structured)) => web_search_result_detail(lines, structured),
        ("WebFetch", Some(structured)) => web_fetch_result_detail(lines, structured),
        ("ImageGeneration", Some(structured)) => image_result_detail(lines, structured),
        ("DynamicTool", Some(structured)) => dynamic_result_detail(lines, structured),
        (
            "JavaScript" | "ComputerUse" | "StructuredOutput" | "SendMessage" | "ReadMediaFile",
            Some(structured),
        ) => output_result_detail(lines, structured),
        ("AskUserQuestion" | "RequestPermissions", Some(structured)) => {
            question_result_detail(lines, structured)
        }
        ("ScheduleWakeup" | "CronCreate" | "CronList" | "CronDelete", Some(structured)) => {
            schedule_result_detail(lines, structured)
        }
        ("Skill", Some(structured)) => skill_result_detail(lines, structured),
        ("Workflow", Some(structured)) => workflow_result_detail(lines, structured),
        ("CreateGoal" | "GetGoal" | "SetGoalBudget" | "UpdateGoal", Some(structured)) => {
            goal_result_detail(lines, structured)
        }
        (_, Some(_)) if metadata.category == "mcp" => detail(lines),
        (_, Some(structured)) => default_result_detail(lines, metadata, structured),
        (_, None) => detail(lines),
    };

    detail.persisted_output_path = persisted_output_path.map(str::to_string);
    detail.media = structured.map(structured_media).unwrap_or_default();
    if detail.lines.is_empty()
        && detail.diff.is_none()
        && detail.patch_diff.is_none()
        && detail.persisted_output_path.is_none()
        && detail.media.is_empty()
    {
        None
    } else {
        Some(detail)
    }
}

pub(super) fn bash_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("cwd", &["cwd"][..]),
            ("source", &["source"][..]),
            ("exit", &["exitCode", "exit_code"][..]),
            ("duration", &["durationSeconds", "duration_seconds"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn edit_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    if let Some(file) = pick_field(structured, &["filePath", "file_path", "path"]) {
        lines.push(line("file", file));
    }
    let files = patch_files(structured);
    if !files.is_empty() {
        lines.push(line("files", files.join("\n")));
    }

    let metadata = nested_record(structured.get("metadata"));
    let file_diff = metadata.and_then(|record| nested_record(record.get("filediff")));
    let patch_text = first_string(structured, &["diff", "patch"])
        .or_else(|| metadata.and_then(|record| first_string(record, &["diff"])))
        .or_else(|| file_diff.and_then(|record| first_string(record, &["patch"])));
    if let Some(patch) = patch_text {
        return detail(lines).with_patch_diff(build_patch_line_diff(&patch));
    }

    let structured_patch = structured
        .get("structuredPatch")
        .map(build_structured_patch_line_diff)
        .unwrap_or_default();
    if !structured_patch.is_empty() {
        return detail(lines).with_patch_diff(structured_patch);
    }

    let old_text = first_string(structured, &["oldString", "old_string"]).unwrap_or_default();
    let new_text = first_string(structured, &["newString", "new_string"]).unwrap_or_default();
    if !old_text.is_empty() || !new_text.is_empty() {
        return detail(lines).with_diff(old_text, new_text);
    }

    if structured.get("type").and_then(Value::as_str) == Some("create")
        && let Some(content) = first_string(structured, &["content"])
        && !content.is_empty()
    {
        return detail(lines).with_diff("", content);
    }

    detail(lines)
}

pub(super) fn agent_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("agent", &["agentId"][..]),
            ("type", &["agentType"][..]),
            ("tokens", &["totalTokens"][..]),
            ("tools", &["totalToolUseCount"][..]),
            (
                "nickname",
                &["nickname", "new_agent_nickname", "receiver_agent_nickname"][..],
            ),
            ("role", &["new_agent_role", "receiver_agent_role"][..]),
            ("model", &["model"][..]),
            ("reasoning", &["reasoning_effort"][..]),
            ("sender", &["sender_thread_id"][..]),
            ("newThread", &["new_thread_id"][..]),
            ("receiver", &["receiver_thread_id"][..]),
        ],
    );
    if structured.get("timed_out").and_then(Value::as_bool) == Some(true) {
        lines.push(line("timedOut", "true"));
    }
    if let Some(status) = nested_status_text(structured.get("status"))
        .or_else(|| nested_status_text(structured.get("previous_status")))
    {
        lines.push(line("statusText", status));
    }
    if let Some(agent_statuses) = structured.get("agent_statuses").and_then(Value::as_array) {
        if !agent_statuses.is_empty() {
            lines.push(line("agentStatuses", agent_statuses.len().to_string()));
        }
    } else if let Some(statuses) = structured.get("statuses").and_then(Value::as_object) {
        lines.push(line("agentStatuses", statuses.len().to_string()));
    }
    detail(lines)
}

pub(super) fn task_result_detail(
    mut lines: Vec<ToolLine>,
    metadata: &ToolMetadata,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    let task = structured.get("task").and_then(Value::as_object);
    if metadata.canonical_name == "TaskCreate" {
        if let Some(id) = task
            .and_then(|record| first_string(record, &["id", "taskId", "task_id"]))
            .or_else(|| first_string(structured, &["id", "taskId", "task_id"]))
        {
            lines.push(line("task", id));
        }
        if let Some(subject) = task
            .and_then(|record| first_string(record, &["subject", "description"]))
            .or_else(|| first_string(structured, &["subject", "description"]))
        {
            lines.push(line("subject", subject));
        }
        return detail(lines);
    }

    if metadata.canonical_name == "TaskList" {
        if let Some(tasks) = structured.get("tasks").and_then(Value::as_array) {
            lines.push(line("tasks", tasks.len().to_string()));
        }
        return detail(lines);
    }

    if metadata.canonical_name == "TaskOutput" {
        for (label_name, keys) in [
            ("retrieval", &["retrieval_status"][..]),
            ("task", &["task_id"][..]),
            ("status", &["status"][..]),
            ("type", &["task_type"][..]),
            ("description", &["description"][..]),
        ] {
            let value = task
                .and_then(|record| first_string(record, keys))
                .or_else(|| first_string(structured, keys));
            if let Some(value) = value {
                lines.push(line(label_name, value));
            }
        }
        return detail(lines);
    }

    append_present_fields(
        &mut lines,
        structured,
        &[
            ("task", &["taskId", "task_id"][..]),
            ("type", &["task_type"][..]),
            ("status", &["status"][..]),
            ("statusChange", &["statusChange"][..]),
            ("updatedFields", &["updatedFields"][..]),
            ("command", &["command"][..]),
            ("success", &["success"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn tool_search_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("query", &["query"][..]),
            ("matches", &["total_deferred_tools"][..]),
        ],
    );
    if let Some(matches) = structured.get("matches").and_then(Value::as_array) {
        lines.push(line("matches", matches.len().to_string()));
    }
    detail(lines)
}

pub(super) fn web_search_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("query", &["query"][..]),
            ("searches", &["searchCount"][..]),
            ("duration", &["durationSeconds"][..]),
        ],
    );
    if let Some(results) = structured.get("results").and_then(Value::as_array) {
        lines.push(line("results", results.len().to_string()));
    }
    detail(lines)
}

pub(super) fn web_fetch_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("url", &["url"][..]),
            ("code", &["code"][..]),
            ("codeText", &["codeText"][..]),
            ("bytes", &["bytes"][..]),
            ("durationMs", &["durationMs"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn image_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("savedPath", &["savedPath", "saved_path"][..]),
            ("revisedPrompt", &["revisedPrompt", "revised_prompt"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn dynamic_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("tool", &["tool", "name"][..]),
            ("success", &["success"][..]),
            ("duration", &["duration"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn output_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("success", &["success"][..]),
            ("duration", &["durationSeconds", "duration_seconds"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn question_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    if let Some(questions) = structured.get("questions").and_then(Value::as_array) {
        lines.push(line("questions", questions.len().to_string()));
    }
    append_present_fields(&mut lines, structured, &[("answers", &["answers"][..])]);
    detail(lines)
}

pub(super) fn schedule_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("scheduledFor", &["scheduledFor"][..]),
            ("clampedDelaySeconds", &["clampedDelaySeconds"][..]),
            ("wasClamped", &["wasClamped"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn skill_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("command", &["commandName", "skill"][..]),
            ("success", &["success"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn workflow_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("workflowName", &["workflowName"][..]),
            ("status", &["status"][..]),
            ("runId", &["runId"][..]),
            ("taskId", &["taskId"][..]),
            ("taskType", &["taskType"][..]),
            ("scriptPath", &["scriptPath"][..]),
            ("transcriptDir", &["transcriptDir"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn goal_result_detail(
    mut lines: Vec<ToolLine>,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    append_present_fields(
        &mut lines,
        structured,
        &[
            ("status", &["status"][..]),
            ("objective", &["objective"][..]),
            ("remainingTokens", &["remainingTokens"][..]),
            ("token_budget", &["token_budget"][..]),
            ("completionBudgetReport", &["completionBudgetReport"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn default_result_detail(
    mut lines: Vec<ToolLine>,
    metadata: &ToolMetadata,
    structured: &serde_json::Map<String, Value>,
) -> ToolDetail {
    if metadata.category == "task" {
        if let Some(task) = structured.get("task").and_then(Value::as_object) {
            append_present_fields(
                &mut lines,
                task,
                &[
                    ("id", &["id"][..]),
                    ("subject", &["subject"][..]),
                    ("task_id", &["task_id"][..]),
                    ("status", &["status"][..]),
                    ("task_type", &["task_type"][..]),
                ],
            );
        }
        if let Some(tasks) = structured.get("tasks").and_then(Value::as_array) {
            lines.push(line("tasks", tasks.len().to_string()));
        }
    }
    detail(lines)
}
