use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::models::ToolMetadata;
use crate::provider_utils::shorten_home_path;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn truncate_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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

fn render_call_metadata(structured: &serde_json::Map<String, Value>) -> String {
    let mut html = String::new();
    if let Some(description) = string_field(structured, &["callDescription"]) {
        html.push_str(&render_field("description", description));
    }

    let Some(display) = structured
        .get("callDisplay")
        .and_then(|value| value.as_object())
    else {
        return html;
    };
    for key in [
        "kind",
        "operation",
        "path",
        "cwd",
        "language",
        "command",
        "agent_name",
    ] {
        if let Some(value) = string_field(display, &[key]) {
            html.push_str(&render_field(key, value));
        }
    }
    html
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

fn render_structured_patch_diff(structured_patch: &Value) -> String {
    let Some(hunks) = structured_patch.as_array() else {
        return String::new();
    };
    if hunks.is_empty() {
        return String::new();
    }

    let mut html = String::from(r#"<div class="tool-line-diff">"#);
    let mut rendered_lines = 0usize;

    for hunk in hunks {
        let Some(lines) = hunk.get("lines").and_then(|value| value.as_array()) else {
            continue;
        };
        let old_start = hunk.get("oldStart").and_then(|value| value.as_i64());
        let old_lines = hunk.get("oldLines").and_then(|value| value.as_i64());
        let new_start = hunk.get("newStart").and_then(|value| value.as_i64());
        let new_lines = hunk.get("newLines").and_then(|value| value.as_i64());
        if let (Some(old_start), Some(old_lines), Some(new_start), Some(new_lines)) =
            (old_start, old_lines, new_start, new_lines)
        {
            html.push_str(&render_diff_line(
                "skip",
                &format!("@@ -{old_start},{old_lines} +{new_start},{new_lines} @@"),
            ));
        } else {
            html.push_str(&render_diff_line("skip", "@@"));
        }

        for raw_line in lines.iter().filter_map(|line| line.as_str()) {
            if let Some(rest) = raw_line.strip_prefix('+') {
                html.push_str(&render_diff_line("add", rest));
            } else if let Some(rest) = raw_line.strip_prefix('-') {
                html.push_str(&render_diff_line("remove", rest));
            } else if let Some(rest) = raw_line.strip_prefix(' ') {
                html.push_str(&render_diff_line("context", rest));
            } else {
                html.push_str(&render_diff_line("skip", raw_line));
            }
            rendered_lines += 1;
        }
    }

    html.push_str("</div>");
    if rendered_lines == 0 {
        String::new()
    } else {
        html
    }
}

pub(crate) fn tool_icon(name: &str, metadata: Option<&ToolMetadata>) -> &'static str {
    if metadata.is_some_and(|m| m.category == "mcp") || name.starts_with("mcp__") {
        return "🔌";
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
        "ToolSearch" => "🧰",
        "Skill" => "⚡",
        "AskUserQuestion" => "❓",
        "CronCreate" | "CronList" | "CronDelete" | "ScheduleWakeup" => "⏰",
        "CreateGoal" | "GetGoal" | "SetGoalBudget" | "UpdateGoal" => "🎯",
        _ => "⚙",
    }
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
            .map(|s| {
                if s.len() > 60 {
                    format!("{}...", truncate_char_boundary(s, 57))
                } else {
                    s.to_string()
                }
            })
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
                .map(|s| {
                    if s.len() > 60 {
                        format!("{}...", truncate_char_boundary(s, 57))
                    } else {
                        s.to_string()
                    }
                })
                .unwrap_or_default();
            join_non_empty([cron, prompt])
        }
        "CronDelete" => string_field(&obj, &["id"]).unwrap_or_default().to_string(),
        "ReadMediaFile" => string_field(&obj, &["path"])
            .map(shorten_home_path)
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
            .map(|s| {
                if s.len() > 60 {
                    format!("{}...", truncate_char_boundary(s, 57))
                } else {
                    s.to_string()
                }
            })
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
            for (k, v) in obj
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k, s)))
                .take(3)
            {
                html.push_str(&render_field(k, v));
            }
        }
    }

    html
}

fn structured_record(metadata: &ToolMetadata) -> Option<&serde_json::Map<String, Value>> {
    metadata.structured.as_ref()?.as_object()
}

pub(crate) fn render_tool_result_detail(metadata: Option<&ToolMetadata>) -> String {
    let Some(metadata) = metadata else {
        return String::new();
    };
    let Some(structured) = structured_record(metadata) else {
        return String::new();
    };

    let mut html = String::new();
    if let Some(status) = metadata.status.as_deref() {
        html.push_str(&render_field("status", status));
    }
    html.push_str(&render_call_metadata(structured));

    match metadata.canonical_name.as_str() {
        "Bash" => {
            if let Some(stdout) = string_field(structured, &["stdout", "output"]) {
                if !stdout.is_empty() {
                    html.push_str(&render_pre_field("stdout", stdout));
                }
            }
            if let Some(stderr) = string_field(structured, &["stderr"]) {
                if !stderr.is_empty() {
                    html.push_str(&render_pre_field("stderr", stderr));
                }
            }
        }
        "Edit" | "Write" => {
            if let Some(file) = string_field(structured, &["filePath", "file_path"]) {
                html.push_str(&render_field("file", file));
            }
            let structured_patch_html = structured
                .get("structuredPatch")
                .map(render_structured_patch_diff)
                .unwrap_or_default();
            let has_structured_patch = !structured_patch_html.is_empty();
            html.push_str(&structured_patch_html);
            let old = string_field(structured, &["oldString", "old_string"]);
            let new = string_field(structured, &["newString", "new_string"]);
            if !has_structured_patch && (old.is_some() || new.is_some()) {
                html.push_str(&render_line_diff(old.unwrap_or(""), new.unwrap_or("")));
            } else if !has_structured_patch
                && structured.get("type").and_then(|value| value.as_str()) == Some("create")
            {
                if let Some(content) = string_field(structured, &["content"]) {
                    if !content.is_empty() {
                        html.push_str(&render_line_diff("", content));
                    }
                }
            }
        }
        "Agent" => {
            for (label, key) in [
                ("agent", "agentId"),
                ("type", "agentType"),
                ("tokens", "totalTokens"),
                ("tools", "totalToolUseCount"),
            ] {
                if let Some(value) = structured.get(key) {
                    html.push_str(&render_field(label, &value_to_short_string(value)));
                }
            }
        }
        "ToolSearch" => {
            if let Some(query) = string_field(structured, &["query"]) {
                html.push_str(&render_field("query", query));
            }
            if let Some(matches) = structured.get("matches").and_then(|v| v.as_array()) {
                html.push_str(&render_field("matches", &matches.len().to_string()));
            }
        }
        "WebFetch" => {
            for key in ["url", "code", "codeText", "durationMs"] {
                if let Some(value) = structured.get(key) {
                    html.push_str(&render_field(key, &value_to_short_string(value)));
                }
            }
        }
        "ImageGeneration" => {
            if let Some(path) = string_field(structured, &["savedPath", "saved_path"]) {
                html.push_str(&render_field("savedPath", path));
            }
            if let Some(prompt) = string_field(structured, &["revisedPrompt", "revised_prompt"]) {
                html.push_str(&render_field("revisedPrompt", prompt));
            }
        }
        "DynamicTool" => {
            for key in ["tool", "name", "success", "duration", "content"] {
                if let Some(value) = structured.get(key) {
                    html.push_str(&render_field(key, &value_to_short_string(value)));
                }
            }
        }
        _ if metadata.category == "task" => {
            for key in ["taskId", "task_id", "statusChange", "message"] {
                if let Some(value) = structured.get(key) {
                    html.push_str(&render_field(key, &value_to_short_string(value)));
                }
            }
        }
        _ if metadata.category == "mcp" => {
            if let Some(mcp) = &metadata.mcp {
                html.push_str(&render_field("server", &mcp.server));
                html.push_str(&render_field("tool", &mcp.tool));
            }
        }
        _ => {}
    }

    html
}

fn value_to_short_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(values) => values
            .iter()
            .map(value_to_short_string)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(obj) => {
            let from = obj.get("from");
            let to = obj.get("to");
            if let (Some(from), Some(to)) = (from, to) {
                return format!(
                    "{} → {}",
                    value_to_short_string(from),
                    value_to_short_string(to)
                );
            }
            obj.iter()
                .map(|(key, value)| format!("{key}: {}", value_to_short_string(value)))
                .collect::<Vec<_>>()
                .join(", ")
        }
        Value::Null => String::new(),
    }
}

pub(crate) fn suppress_raw_output(metadata: Option<&ToolMetadata>, result_has_diff: bool) -> bool {
    match metadata.and_then(|m| m.result_kind.as_deref()) {
        Some("terminal_output") => true,
        Some("file_patch") => result_has_diff,
        _ => false,
    }
}

pub(crate) fn should_skip_tool(name: &str, metadata: Option<&ToolMetadata>) -> bool {
    name.starts_with("toolu_") && metadata.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ToolMetadata;
    use serde_json::json;

    fn metadata(canonical: &str, category: &str) -> ToolMetadata {
        ToolMetadata {
            raw_name: canonical.to_string(),
            canonical_name: canonical.to_string(),
            display_name: canonical.to_string(),
            category: category.to_string(),
            summary: None,
            status: None,
            ids: Default::default(),
            mcp: None,
            result_kind: None,
            structured: None,
        }
    }

    #[test]
    fn html_escape_replaces_all_reserved_characters() {
        assert_eq!(
            html_escape(r#"<a href="x">&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn truncate_char_boundary_does_not_split_multibyte() {
        // "café" is 5 bytes (é is 2). Truncating at 4 must back off to 3
        // so we never slice mid-codepoint.
        assert_eq!(truncate_char_boundary("café", 4), "caf");
        assert_eq!(truncate_char_boundary("ab", 10), "ab");
    }

    #[test]
    fn is_path_label_matches_path_suffixes() {
        assert!(is_path_label("file"));
        assert!(is_path_label("path"));
        assert!(is_path_label("filePath"));
        assert!(is_path_label("FILE"));
        assert!(!is_path_label("command"));
    }

    #[test]
    fn render_field_escapes_label_and_value() {
        let html = render_field("name", "a<b>&c");
        assert!(html.contains(r#"<span class="tool-field-label">name</span>"#));
        assert!(html.contains("a&lt;b&gt;&amp;c"));
    }

    #[test]
    fn render_line_diff_marks_added_and_removed_lines() {
        let html = render_line_diff("one\ntwo\n", "one\nTWO\n");
        // The unchanged line is context, the changed line shows as remove+add.
        assert!(html.contains(r#"<div class="tool-line-diff">"#));
        assert!(html.contains(r#"tool-diff-line context"#));
        assert!(html.contains(r#"tool-diff-line remove"#));
        assert!(html.contains(r#"tool-diff-line add"#));
        assert!(html.contains(">two<"));
        assert!(html.contains(">TWO<"));
    }

    #[test]
    fn render_patch_diff_classifies_marker_lines() {
        let patch = "*** Begin Patch\n*** Update File: a.txt\n@@ ctx @@\n+added line\n-removed line\n unchanged\n*** End Patch";
        let html = render_patch_diff(patch);
        // Begin/End patch sentinels are dropped; +/-/space prefixes map to
        // add/remove/context, and @@/*** headers become "skip".
        assert!(!html.contains("Begin Patch"));
        assert!(!html.contains("End Patch"));
        assert!(html.contains(r#"tool-diff-line skip"#)); // the @@ / *** Update header
        assert!(html.contains("added line"));
        assert!(html.contains("removed line"));
        assert!(html.contains("unchanged"));
    }

    #[test]
    fn tool_icon_uses_canonical_name_then_falls_back() {
        assert_eq!(tool_icon("Read", None), "📄");
        assert_eq!(tool_icon("Bash", None), "💻");
        assert_eq!(tool_icon("ReadMediaFile", None), "🖼️");
        assert_eq!(tool_icon("TaskOutput", None), "📋");
        assert_eq!(tool_icon("CronList", None), "⏰");
        assert_eq!(tool_icon("SetGoalBudget", None), "🎯");
        // Unknown tool → default gear.
        assert_eq!(tool_icon("Frobnicate", None), "⚙");
        // mcp category / mcp__ prefix → plug.
        assert_eq!(tool_icon("mcp__server__do", None), "🔌");
        assert_eq!(tool_icon("anything", Some(&metadata("Read", "mcp"))), "🔌");
        // metadata canonical name wins over the raw name.
        assert_eq!(tool_icon("raw", Some(&metadata("Grep", "search"))), "🔎");
    }

    #[test]
    fn tool_display_name_prefers_metadata() {
        assert_eq!(tool_display_name("raw_tool", None), "raw_tool");
        let mut md = metadata("Read", "file");
        md.display_name = "Read File".into();
        assert_eq!(tool_display_name("raw_tool", Some(&md)), "Read File");
    }

    #[test]
    fn tool_summary_prefers_metadata_summary() {
        let mut md = metadata("Bash", "shell");
        md.summary = Some("run the build".into());
        assert_eq!(
            tool_summary("Bash", r#"{"command":"make"}"#, Some(&md)),
            "run the build"
        );
    }

    #[test]
    fn tool_summary_extracts_file_path_for_read() {
        // /srv/... is outside any home dir, so shorten_home_path leaves it
        // intact and we can assert the raw extracted file path.
        let summary = tool_summary("Read", r#"{"file_path":"/srv/proj/a.rs"}"#, None);
        assert_eq!(summary, "/srv/proj/a.rs");
    }

    #[test]
    fn tool_summary_truncates_long_bash_command() {
        let long = "x".repeat(100);
        let input = json!({ "command": long }).to_string();
        let summary = tool_summary("Bash", &input, None);
        assert!(summary.ends_with("..."));
        // 57 chars kept + "..." == 60.
        assert_eq!(summary.chars().count(), 60);
    }

    #[test]
    fn tool_summary_reads_apply_patch_file_line() {
        let input = "*** Begin Patch\n*** Update File: /srv/proj/x.rs\n+new\n*** End Patch";
        assert_eq!(tool_summary("Apply_patch", input, None), "/srv/proj/x.rs");
    }

    #[test]
    fn tool_summary_handles_kimi_specific_tools() {
        assert_eq!(
            tool_summary("TaskOutput", r#"{"task_id":"task-123","block":true}"#, None),
            "task-123 · wait"
        );
        assert_eq!(
            tool_summary("SetGoalBudget", r#"{"value":3,"unit":"turns"}"#, None),
            "3 · turns"
        );
        assert_eq!(
            tool_summary(
                "ReadMediaFile",
                r#"{"path":"/Users/alice/image.png"}"#,
                None
            ),
            "~/image.png"
        );
    }

    #[test]
    fn render_tool_input_detail_renders_bash_command_block() {
        let html = render_tool_input_detail("Bash", r#"{"command":"echo hi"}"#);
        assert!(html.contains(r#"<pre class="tool-cmd">echo hi</pre>"#));
        assert!(html.contains(r#"<span class="tool-field-label">$</span>"#));
    }

    #[test]
    fn render_tool_input_detail_renders_edit_line_diff() {
        let input = json!({
            "file_path": "/srv/proj/a.rs",
            "old_string": "let x = 1;",
            "new_string": "let x = 2;",
        })
        .to_string();
        let html = render_tool_input_detail("Edit", &input);
        assert!(html.contains("/srv/proj/a.rs"));
        assert!(html.contains(r#"<div class="tool-line-diff">"#));
        assert!(html.contains("let x = 1;"));
        assert!(html.contains("let x = 2;"));
    }

    #[test]
    fn render_tool_input_detail_falls_back_to_raw_for_non_object() {
        let html = render_tool_input_detail("Unknown", "not json at all");
        assert_eq!(html, r#"<pre class="tool-raw">not json at all</pre>"#);
    }

    #[test]
    fn render_tool_result_detail_renders_bash_stdout_and_status() {
        let mut md = metadata("Bash", "shell");
        md.status = Some("completed".into());
        md.structured = Some(json!({ "output": "hello world", "stderr": "" }));
        let html = render_tool_result_detail(Some(&md));
        assert!(html.contains(r#"<span class="tool-field-value">completed</span>"#));
        assert!(html.contains(r#"<pre class="tool-cmd">hello world</pre>"#));
        // empty stderr is suppressed.
        assert!(!html.contains("stderr"));
    }

    #[test]
    fn render_tool_result_detail_keeps_kimi_call_display_metadata() {
        let mut md = metadata("Bash", "shell");
        md.status = Some("completed".into());
        md.structured = Some(json!({
            "callDescription": "Run pwd",
            "callDisplay": {
                "kind": "bash",
                "cwd": "/Users/alice/project",
                "command": "pwd"
            },
            "output": "hello world"
        }));
        let html = render_tool_result_detail(Some(&md));
        assert!(html.contains("Run pwd"));
        assert!(html.contains("/Users/alice/project"));
        assert!(html.contains(r#"<pre class="tool-cmd">hello world</pre>"#));
    }

    #[test]
    fn render_tool_result_detail_empty_without_structured() {
        assert_eq!(render_tool_result_detail(None), "");
        assert_eq!(
            render_tool_result_detail(Some(&metadata("Bash", "shell"))),
            ""
        );
    }

    #[test]
    fn value_to_short_string_formats_from_to_transition() {
        let value = json!({ "from": "pending", "to": "done" });
        assert_eq!(value_to_short_string(&value), "pending → done");
        assert_eq!(value_to_short_string(&json!(42)), "42");
        assert_eq!(value_to_short_string(&json!(["a", "", "b"])), "a, b");
    }

    #[test]
    fn suppress_raw_output_honours_result_kind() {
        let mut md = metadata("Bash", "shell");
        md.result_kind = Some("terminal_output".into());
        assert!(suppress_raw_output(Some(&md), false));

        md.result_kind = Some("file_patch".into());
        assert!(suppress_raw_output(Some(&md), true));
        assert!(!suppress_raw_output(Some(&md), false));

        assert!(!suppress_raw_output(None, true));
    }

    #[test]
    fn should_skip_tool_only_skips_unresolved_toolu_ids() {
        assert!(should_skip_tool("toolu_01ABC", None));
        assert!(!should_skip_tool(
            "toolu_01ABC",
            Some(&metadata("Read", "file"))
        ));
        assert!(!should_skip_tool("Read", None));
    }
}
