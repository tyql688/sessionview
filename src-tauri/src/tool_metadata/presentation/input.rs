//! Input-side detail builders: how a tool call's arguments render.

use serde_json::Value;

use crate::models::{ToolDetail, ToolMetadata};

use super::util::*;

pub(super) fn input_detail_for(metadata: &ToolMetadata, value: &Value) -> Option<ToolDetail> {
    let Some(obj) = value.as_object() else {
        let raw = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        if metadata.canonical_name == "Edit" && raw.contains("*** Begin Patch") {
            let mut lines = Vec::new();
            let files = patched_files(&raw);
            if !files.is_empty() {
                lines.push(line("files", files.join("\n")));
            }
            return Some(detail(lines).with_patch_diff(build_patch_line_diff(&raw)));
        }
        return Some(detail(vec![line("raw", value_to_display_string(value))]));
    };

    match metadata.canonical_name.as_str() {
        "Edit" => Some(edit_input_detail(obj)),
        "Write" => Some(write_input_detail(obj)),
        "Read" | "ReadMediaFile" => Some(read_input_detail(obj)),
        "Bash" => Some(detail(vec![line(
            "command",
            first_string(obj, &["command", "cmd", "CommandLine"]).unwrap_or_default(),
        )])),
        "Plan" => Some(plan_input_detail(obj)),
        "Grep" => Some(grep_input_detail(obj)),
        _ => Some(generic_detail(obj)),
    }
}

pub(super) fn edit_input_detail(obj: &serde_json::Map<String, Value>) -> ToolDetail {
    if let Some(patch) = first_string(obj, &["patch"]) {
        let files = patched_files(&patch);
        let lines = if files.is_empty() {
            vec![line(
                "file",
                pick_field(obj, &["file_path", "filePath", "TargetFile"]).unwrap_or_default(),
            )]
        } else {
            vec![line("files", files.join("\n"))]
        };
        return detail(lines).with_patch_diff(build_patch_line_diff(&patch));
    }

    if let Some(chunks) = obj
        .get("ReplacementChunks")
        .and_then(|value| value.as_array())
    {
        let file = pick_field(obj, &["TargetFile", "file_path", "filePath"])
            .unwrap_or_else(|| "(unknown)".to_string());
        let patch = build_patch_from_antigravity_chunks(&file, chunks);
        return detail(vec![line("file", file)]).with_patch_diff(build_patch_line_diff(&patch));
    }

    detail(vec![line(
        "file",
        pick_field(obj, &["file_path", "filePath", "TargetFile"]).unwrap_or_default(),
    )])
    .with_diff(
        first_string(obj, &["old_string", "oldString", "TargetContent"]).unwrap_or_default(),
        first_string(obj, &["new_string", "newString", "ReplacementContent"]).unwrap_or_default(),
    )
}

pub(super) fn write_input_detail(obj: &serde_json::Map<String, Value>) -> ToolDetail {
    detail(vec![
        line(
            "file",
            pick_field(obj, &["file_path", "filePath", "TargetFile"]).unwrap_or_default(),
        ),
        line(
            "content",
            first_string(obj, &["content", "CodeContent", "code_content"]).unwrap_or_default(),
        ),
    ])
}

pub(super) fn read_input_detail(obj: &serde_json::Map<String, Value>) -> ToolDetail {
    let mut lines = vec![line(
        "file",
        pick_field(
            obj,
            &[
                "file_path",
                "filePath",
                "AbsolutePath",
                "path",
                "TargetFile",
            ],
        )
        .unwrap_or_default(),
    )];
    append_present_fields(
        &mut lines,
        obj,
        &[
            ("offset", &["offset"][..]),
            ("limit", &["limit"][..]),
            ("start", &["StartLine"][..]),
            ("end", &["EndLine"][..]),
        ],
    );
    detail(lines)
}

pub(super) fn plan_input_detail(obj: &serde_json::Map<String, Value>) -> ToolDetail {
    let mut lines = Vec::new();
    if let Some(explanation) = first_string(obj, &["explanation"]) {
        lines.push(line("explanation", explanation));
    }
    if let Some(plan) = obj.get("plan").and_then(|value| value.as_array()) {
        let steps = plan
            .iter()
            .filter_map(|step| step.as_object())
            .map(|step| {
                let status = first_string(step, &["status"]).unwrap_or_default();
                let marker = match status.as_str() {
                    "completed" => "done",
                    "in_progress" => "active",
                    _ => "pending",
                };
                format!(
                    "{marker}: {}",
                    first_string(step, &["step"]).unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !steps.is_empty() {
            lines.push(line("plan", steps));
        }
    }
    detail(lines)
}

pub(super) fn grep_input_detail(obj: &serde_json::Map<String, Value>) -> ToolDetail {
    let mut lines = Vec::new();
    if let Some(pattern) = first_string(obj, &["pattern", "query", "Query", "DirectoryPath"]) {
        lines.push(line("pattern", pattern));
    }
    if let Some(path) = first_string(obj, &["path", "SearchPath"]) {
        lines.push(line("path", path));
    }
    if let Some(glob) = first_string(obj, &["glob"]) {
        lines.push(line("glob", glob));
    }
    detail(lines)
}
