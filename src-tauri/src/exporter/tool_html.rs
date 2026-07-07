use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::models::{RawOutputPolicy, ToolDetail, ToolDiffLine, ToolDiffLineType, ToolMetadata};
use crate::provider_utils::shorten_home_path;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn is_path_label(label: &str) -> bool {
    let label = label.to_ascii_lowercase();
    label == "file" || label == "path" || label.ends_with("path")
}

fn string_field<'a>(obj: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| obj.get(*key).and_then(|v| v.as_str()))
}

fn join_non_empty(parts: impl IntoIterator<Item = String>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn render_field(label: &str, value: &str) -> String {
    let display_value = if is_path_label(label) {
        shorten_home_path(value)
    } else {
        value.to_string()
    };
    format!(
        r#"<div class="tool-field"><span class="tool-field-label">{}</span><span class="tool-field-value">{}</span></div>"#,
        html_escape(label),
        html_escape(&display_value)
    )
}

fn render_pre_field(label: &str, value: &str) -> String {
    format!(
        r#"<div class="tool-field"><span class="tool-field-label">{}</span><pre class="tool-cmd">{}</pre></div>"#,
        html_escape(label),
        html_escape(value)
    )
}

fn render_diff_line(kind: &str, text: &str) -> String {
    let marker = match kind {
        "add" => "+",
        "remove" => "-",
        "skip" => "⋯",
        _ => " ",
    };
    format!(
        r#"<div class="tool-diff-line {kind}"><span class="tool-diff-gutter"></span><span class="tool-diff-gutter"></span><span class="tool-diff-marker">{marker}</span><span class="tool-diff-code">{}</span></div>"#,
        html_escape(if text.is_empty() { " " } else { text })
    )
}

fn is_pre_label(label: &str, value: &str) -> bool {
    value.contains('\n')
        || matches!(
            label,
            "$" | "command"
                | "content"
                | "output"
                | "stdout"
                | "stderr"
                | "error"
                | "result"
                | "raw"
        )
}

fn render_presentation_diff_line(line: &ToolDiffLine) -> String {
    let kind = match line.kind {
        ToolDiffLineType::Add => "add",
        ToolDiffLineType::Remove => "remove",
        ToolDiffLineType::Skip => "skip",
        ToolDiffLineType::Context => "context",
    };
    render_diff_line(kind, &line.text)
}

fn render_patch_diff_lines(lines: &[ToolDiffLine]) -> String {
    if lines.is_empty() {
        return String::new();
    }

    let mut html = String::from(r#"<div class="tool-line-diff">"#);
    for line in lines {
        html.push_str(&render_presentation_diff_line(line));
    }
    html.push_str("</div>");
    html
}

fn render_tool_presentation_detail(detail: &ToolDetail) -> String {
    let mut html = String::new();
    for line in &detail.lines {
        if is_pre_label(&line.label, &line.value) {
            html.push_str(&render_pre_field(&line.label, &line.value));
        } else {
            html.push_str(&render_field(&line.label, &line.value));
        }
    }
    if let Some(diff) = &detail.diff {
        html.push_str(&render_line_diff(&diff.old, &diff.new));
    }
    if let Some(patch_diff) = &detail.patch_diff {
        html.push_str(&render_patch_diff_lines(patch_diff));
    }
    html
}

pub(crate) fn render_line_diff(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut html = String::from(r#"<div class="tool-line-diff">"#);

    for change in diff.iter_all_changes() {
        let kind = match change.tag() {
            ChangeTag::Delete => "remove",
            ChangeTag::Insert => "add",
            ChangeTag::Equal => "context",
        };
        html.push_str(&render_diff_line(
            kind,
            change.value().trim_end_matches('\n'),
        ));
    }

    html.push_str("</div>");
    html
}

pub(crate) fn render_patch_diff(patch: &str) -> String {
    let mut html = String::from(r#"<div class="tool-line-diff">"#);

    for line in patch.lines() {
        if line == "*** Begin Patch" || line == "*** End Patch" || line.is_empty() {
            continue;
        }
        if line.starts_with("*** ") || line.starts_with("@@") {
            html.push_str(&render_diff_line("skip", &shorten_home_path(line)));
        } else if let Some(rest) = line.strip_prefix('+') {
            html.push_str(&render_diff_line("add", rest));
        } else if let Some(rest) = line.strip_prefix('-') {
            html.push_str(&render_diff_line("remove", rest));
        } else if let Some(rest) = line.strip_prefix(' ') {
            html.push_str(&render_diff_line("context", rest));
        } else {
            html.push_str(&render_diff_line("skip", line));
        }
    }

    html.push_str("</div>");
    html
}

pub(crate) fn tool_icon(name: &str, metadata: Option<&ToolMetadata>) -> String {
    if let Some(icon) = metadata
        .and_then(|metadata| metadata.presentation.as_ref())
        .map(|presentation| presentation.icon.clone())
    {
        return icon;
    }
    if metadata.is_some_and(|m| m.category == "mcp") || name.starts_with("mcp__") {
        return "🔌".to_string();
    }
    match metadata.map(|m| m.canonical_name.as_str()).unwrap_or(name) {
        "Read" => "📄",
        "Edit" | "Apply_patch" => "✏️",
        "Write" => "📝",
        "Bash" => "💻",
        "Glob" => "🔍",
        "Grep" => "🔎",
        "Agent" => "🤖",
        "Plan" | "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskOutput" => "📋",
        "TaskStop" => "🛑",
        "WebSearch" | "WebFetch" => "🌐",
        "ImageGeneration" | "ReadMediaFile" => "🖼️",
        "DynamicTool" => "🧩",
        "JavaScript" => "🟨",
        "ComputerUse" => "🖱️",
        "Workflow" => "🔁",
        "StructuredOutput" => "📊",
        "ToolSearch" => "🧰",
        "Skill" => "⚡",
        "AskUserQuestion" => "❓",
        "CronCreate" | "CronList" | "CronDelete" | "ScheduleWakeup" => "⏰",
        "CreateGoal" | "GetGoal" | "SetGoalBudget" | "UpdateGoal" => "🎯",
        "SendMessage" => "✉️",
        "RequestPermissions" => "🔐",
        "ListMcpResourcesTool" => "🔌",
        _ => "⚙",
    }
    .to_string()
}

pub(crate) fn tool_display_name<'a>(name: &'a str, metadata: Option<&'a ToolMetadata>) -> &'a str {
    metadata.map(|m| m.display_name.as_str()).unwrap_or(name)
}

pub(crate) fn tool_summary(name: &str, input: &str, metadata: Option<&ToolMetadata>) -> String {
    if let Some(summary) = metadata.and_then(|m| m.summary.as_deref()) {
        return summary.to_string();
    }
    let trimmed_input = input.trim_start();
    if (name == "Apply_patch" || name == "Edit")
        && !trimmed_input.starts_with('{')
        && input.contains("*** Begin Patch")
    {
        return input
            .lines()
            .find(|l| {
                l.starts_with("*** Add File:")
                    || l.starts_with("*** Update File:")
                    || l.starts_with("*** Delete File:")
            })
            .and_then(|l| l.split(':').nth(1))
            .map(|s| shorten_home_path(s.trim()))
            .unwrap_or_default();
    }

    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(input) else {
        return String::new();
    };

    match name {
        "Read" | "Edit" | "Write" => string_field(&obj, &["file_path", "filePath", "path"])
            .map(shorten_home_path)
            .unwrap_or_default(),
        "Bash" => string_field(&obj, &["description", "command", "cmd"])
            .map(str::to_owned)
            .unwrap_or_default(),
        "Grep" | "Glob" => string_field(&obj, &["pattern", "query"])
            .unwrap_or_default()
            .to_string(),
        "Plan" => string_field(&obj, &["explanation"])
            .unwrap_or_default()
            .to_string(),
        "TaskList" => {
            let active = obj
                .get("active_only")
                .and_then(|v| v.as_bool())
                .map(|value| if value { "active" } else { "all" }.to_string())
                .unwrap_or_default();
            let limit = obj
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|value| format!("limit {value}"))
                .unwrap_or_default();
            join_non_empty([active, limit])
        }
        "TaskOutput" => {
            let task = string_field(&obj, &["task_id", "taskId"])
                .map(|s| s.to_string())
                .unwrap_or_default();
            let mode = obj
                .get("block")
                .and_then(|v| v.as_bool())
                .filter(|value| *value)
                .map(|_| "wait".to_string())
                .unwrap_or_default();
            join_non_empty([task, mode])
        }
        "TaskStop" => {
            let task = string_field(&obj, &["task_id", "taskId"])
                .map(|s| s.to_string())
                .unwrap_or_default();
            let reason = string_field(&obj, &["reason"])
                .map(|s| s.to_string())
                .unwrap_or_default();
            join_non_empty([task, reason])
        }
        "CronCreate" => {
            let cron = string_field(&obj, &["cron"])
                .map(|s| s.to_string())
                .unwrap_or_default();
            let prompt = string_field(&obj, &["prompt"])
                .map(str::to_owned)
                .unwrap_or_default();
            join_non_empty([cron, prompt])
        }
        "CronDelete" => string_field(&obj, &["id"]).unwrap_or_default().to_string(),
        "Skill" => string_field(&obj, &["skill"])
            .unwrap_or_default()
            .to_string(),
        "ToolSearch" | "WebSearch" => string_field(&obj, &["query", "Query"])
            .unwrap_or_default()
            .to_string(),
        "WebFetch" => string_field(&obj, &["url", "Url"])
            .unwrap_or_default()
            .to_string(),
        "ReadMediaFile" => string_field(&obj, &["path"])
            .map(shorten_home_path)
            .unwrap_or_default(),
        "Workflow" => string_field(&obj, &["name", "description", "script"])
            .map(str::to_owned)
            .unwrap_or_default(),
        "StructuredOutput" => string_field(
            &obj,
            &[
                "finding_id",
                "title",
                "analysis",
                "summary",
                "corrected_root_cause",
                "minimal_fix",
            ],
        )
        .map(str::to_owned)
        .unwrap_or_default(),
        "JavaScript" => string_field(&obj, &["title", "code"])
            .map(str::to_owned)
            .unwrap_or_default(),
        "ComputerUse" => {
            let app = string_field(&obj, &["app"])
                .map(|s| s.to_string())
                .unwrap_or_default();
            let action = string_field(&obj, &["key", "direction", "element_index", "action"])
                .map(|s| s.to_string())
                .unwrap_or_default();
            join_non_empty([app, action])
        }
        "SendMessage" => string_field(&obj, &["description", "prompt", "message", "content"])
            .map(str::to_owned)
            .unwrap_or_default(),
        "AskUserQuestion" => {
            let questions = obj
                .get("questions")
                .and_then(|v| v.as_array())
                .map(|questions| format!("{} question(s)", questions.len()))
                .unwrap_or_default();
            let background = obj
                .get("background")
                .and_then(|v| v.as_bool())
                .filter(|value| *value)
                .map(|_| "background".to_string())
                .unwrap_or_default();
            join_non_empty([questions, background])
        }
        "CreateGoal" => string_field(&obj, &["objective"])
            .map(str::to_owned)
            .unwrap_or_default(),
        "SetGoalBudget" => {
            let value = obj
                .get("value")
                .and_then(|v| v.as_u64())
                .map(|value| value.to_string())
                .unwrap_or_default();
            let unit = string_field(&obj, &["unit"])
                .map(|s| s.to_string())
                .unwrap_or_default();
            join_non_empty([value, unit])
        }
        "UpdateGoal" => string_field(&obj, &["status"])
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

pub(crate) fn render_tool_input_detail(tool_name: &str, tool_input: &str) -> String {
    let trimmed_tool_input = tool_input.trim_start();
    if (tool_name == "Apply_patch" || tool_name == "Edit")
        && !trimmed_tool_input.starts_with('{')
        && tool_input.contains("*** Begin Patch")
    {
        let file_line = tool_input
            .lines()
            .find(|l| {
                l.starts_with("*** Add File:")
                    || l.starts_with("*** Update File:")
                    || l.starts_with("*** Delete File:")
            })
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim());
        let mut html = String::new();
        if let Some(fp) = file_line {
            html.push_str(&render_field("file", fp));
        }
        html.push_str(&render_patch_diff(tool_input));
        return html;
    }

    let parsed: Result<Value, _> = serde_json::from_str(tool_input);
    let obj = match parsed {
        Ok(Value::Object(m)) => m,
        _ => {
            return format!(r#"<pre class="tool-raw">{}</pre>"#, html_escape(tool_input));
        }
    };

    let mut html = String::new();
    match tool_name {
        "Edit" => {
            if let Some(fp) = string_field(&obj, &["file_path", "filePath", "TargetFile"]) {
                html.push_str(&render_field("file", fp));
            }
            if let Some(patch) = string_field(&obj, &["patch"]) {
                html.push_str(&render_patch_diff(patch));
                return html;
            }
            // Antigravity `multi_replace_file_content`: each chunk is its
            // own old/new pair. Render them in order.
            if let Some(chunks) = obj.get("ReplacementChunks").and_then(|v| v.as_array()) {
                for chunk in chunks {
                    let old = chunk
                        .get("TargetContent")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let new_text = chunk
                        .get("ReplacementContent")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !old.is_empty() || !new_text.is_empty() {
                        html.push_str(&render_line_diff(old, new_text));
                    }
                }
                return html;
            }
            let old = string_field(&obj, &["old_string", "oldString", "TargetContent"]);
            let new = string_field(&obj, &["new_string", "newString", "ReplacementContent"]);
            if old.is_some() || new.is_some() {
                html.push_str(&render_line_diff(old.unwrap_or(""), new.unwrap_or("")));
            }
        }
        "Bash" => {
            if let Some(cmd) = string_field(&obj, &["command", "cmd", "CommandLine"]) {
                html.push_str(&render_pre_field("$", cmd));
            }
        }
        "Plan" => {
            if let Some(explanation) = string_field(&obj, &["explanation"]) {
                html.push_str(&render_field("explanation", explanation));
            }
            if let Some(plan) = obj.get("plan").and_then(|v| v.as_array()) {
                html.push_str(r#"<div class="tool-plan">"#);
                for step in plan {
                    let text = step.get("step").and_then(|s| s.as_str()).unwrap_or("");
                    let status = step.get("status").and_then(|s| s.as_str()).unwrap_or("");
                    let icon = match status {
                        "completed" => "✓",
                        "in_progress" => "▸",
                        _ => "○",
                    };
                    let cls = match status {
                        "completed" => "plan-done",
                        "in_progress" => "plan-active",
                        _ => "plan-pending",
                    };
                    html.push_str(&format!(
                        r#"<div class="plan-step {cls}"><span class="plan-icon">{icon}</span> {}</div>"#,
                        html_escape(text)
                    ));
                }
                html.push_str("</div>");
            }
        }
        "Read" | "Write" | "ReadMediaFile" => {
            if let Some(fp) = string_field(
                &obj,
                &[
                    "file_path",
                    "filePath",
                    "path",
                    "AbsolutePath",
                    "TargetFile",
                ],
            ) {
                html.push_str(&render_field("file", fp));
            }
        }
        "Grep" | "Glob" => {
            if let Some(p) = string_field(&obj, &["pattern", "query", "Query", "DirectoryPath"]) {
                html.push_str(&render_field("pattern", p));
            }
            if let Some(path) = string_field(&obj, &["path"]) {
                html.push_str(&render_field("path", path));
            }
        }
        _ => {
            for (k, v) in obj.iter().filter_map(|(k, v)| v.as_str().map(|s| (k, s))) {
                html.push_str(&render_field(k, v));
            }
        }
    }

    html
}

pub(crate) fn render_tool_input_detail_for_message(
    metadata: Option<&ToolMetadata>,
    tool_name: &str,
    tool_input: &str,
) -> String {
    if let Some(detail) = metadata
        .and_then(|metadata| metadata.presentation.as_ref())
        .and_then(|presentation| presentation.input_detail.as_ref())
    {
        return render_tool_presentation_detail(detail);
    }
    render_tool_input_detail(tool_name, tool_input)
}

pub(crate) fn render_tool_result_detail(metadata: Option<&ToolMetadata>) -> String {
    let Some(metadata) = metadata else {
        return String::new();
    };
    if let Some(detail) = metadata
        .presentation
        .as_ref()
        .and_then(|presentation| presentation.result_detail.as_ref())
    {
        return render_tool_presentation_detail(detail);
    }
    String::new()
}

pub(crate) fn suppress_raw_output(metadata: Option<&ToolMetadata>, result_has_diff: bool) -> bool {
    if let Some(policy) = metadata
        .and_then(|metadata| metadata.presentation.as_ref())
        .map(|presentation| &presentation.raw_output_policy)
    {
        return match policy {
            RawOutputPolicy::SuppressTerminal => true,
            RawOutputPolicy::SuppressPatchWhenDiffPresent => result_has_diff,
            RawOutputPolicy::Keep => false,
        };
    }

    false
}

pub(crate) fn should_skip_tool(name: &str, metadata: Option<&ToolMetadata>) -> bool {
    name.starts_with("toolu_") && metadata.is_none()
}

#[cfg(test)]
mod tests;
