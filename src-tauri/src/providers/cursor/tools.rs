//! Cursor CLI transcript helpers.
//!
//! The `agent` CLI writes one JSONL record per turn (role + structured
//! content array). The user side decorates prompts with XML-ish wrappers
//! the model uses for grounding (`<user_query>`, `<user_info>`,
//! `<image_files>`, `<timestamp>`, etc.). This module hosts the pure
//! string/value helpers — extraction, redaction, image markers, tool
//! arg remapping — so parser.rs stays focused on the per-line walk.

use serde_json::Value;

use crate::provider_utils::is_system_content;

// ---------------------------------------------------------------------------
// Content extraction
// ---------------------------------------------------------------------------

/// Pull text out of a `message.content` value. Cursor uses either a JSON
/// array of parts or, occasionally, a raw string. We accept both and
/// collapse the parts into a single newline-separated string.
pub fn extract_text_from_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => {
            // Some legacy records store the array as a JSON-encoded string.
            if s.trim_start().starts_with('[') {
                if let Ok(arr) = serde_json::from_str::<Vec<Value>>(s) {
                    return extract_text_from_parts(&arr);
                }
            }
            s.clone()
        }
        Some(Value::Array(arr)) => extract_text_from_parts(arr),
        _ => String::new(),
    }
}

/// Iterate the `content[]` parts and collect text. `[REDACTED]`
/// placeholders are dropped (Cursor emits them when reasoning is
/// stripped server-side); non-text parts (tool_use, etc.) are skipped
/// here — callers handle them separately.
pub fn extract_text_from_parts(arr: &[Value]) -> String {
    let mut chunks = Vec::new();
    for item in arr {
        if item.get("type").and_then(|v| v.as_str()) != Some("text") {
            continue;
        }
        let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
        if text.trim() == "[REDACTED]" {
            continue;
        }
        if !text.is_empty() {
            chunks.push(text.to_string());
        }
    }
    chunks.join("\n")
}

/// Return the part array from a Cursor `message.content` field,
/// handling the string-encoded edge case for callers that need to
/// inspect non-text parts (tool_use).
pub fn parse_content_array(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::Array(arr)) => arr.clone(),
        Some(Value::String(s)) if s.trim_start().starts_with('[') => {
            serde_json::from_str::<Vec<Value>>(s).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// User text normalisation
// ---------------------------------------------------------------------------

/// Strip the `<user_query>` wrapper Cursor adds around the actual prompt
/// and rewrite `<image_files>` blocks into `[Image: source: <path>]`
/// markers so the frontend renders them like other providers. Filters
/// out pure-system text that isn't meant for display.
///
/// Returns the empty string when nothing user-facing survives — callers
/// drop the message entirely in that case.
pub fn normalise_user_text(text: &str) -> String {
    let with_image_markers = rewrite_image_files_block(text);
    let prompt = extract_tag_content(&with_image_markers, "user_query")
        .map(str::to_string)
        .unwrap_or(with_image_markers);
    let trimmed = prompt.trim();
    if trimmed.is_empty() || is_system_content(trimmed) {
        return String::new();
    }
    if trimmed.starts_with("<user_info>") || trimmed.starts_with("<agent_transcripts>") {
        return String::new();
    }
    trimmed.to_string()
}

/// Rewrite Cursor's `<image_files>` block into `[Image: source: <path>]`
/// markers — one per listed file. The original block has the shape:
///
/// ```text
/// [Image]
/// <image_files>
/// The following images were provided by the user and saved to the
/// workspace for future use:
/// 1. /abs/path/to/img1.png
/// 2. /abs/path/to/img2.png
/// </image_files>
/// ```
///
/// We strip the wrapper, drop the duplicate `[Image]` markers Cursor
/// inserts before it, and emit one canonical line per image so the
/// frontend's existing `[Image: source: ...]` renderer picks them up.
pub fn rewrite_image_files_block(text: &str) -> String {
    let Some(inner) = extract_tag_content(text, "image_files") else {
        return text.to_string();
    };
    let mut markers = Vec::new();
    for raw_line in inner.lines() {
        let line = raw_line.trim();
        // List entries look like `1. /abs/path/file.png`.
        let path = line
            .split_once('.')
            .map(|(prefix, rest)| (prefix.trim(), rest.trim()))
            .filter(|(prefix, _)| !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()))
            .map(|(_, rest)| rest);
        if let Some(p) = path {
            if !p.is_empty() {
                markers.push(format!("[Image: source: {p}]"));
            }
        }
    }
    // Drop the original `<image_files>` block (and the `[Image]` stub
    // Cursor inserts before it) from the surrounding text.
    let before = text
        .split("<image_files>")
        .next()
        .unwrap_or("")
        .replace("[Image]", "")
        .trim_end()
        .to_string();
    let after = text
        .split("</image_files>")
        .nth(1)
        .unwrap_or("")
        .to_string();
    let mut out = String::new();
    if !before.is_empty() {
        out.push_str(&before);
        out.push('\n');
    }
    out.push_str(&markers.join("\n"));
    let after_trimmed = after.trim_start();
    if !after_trimmed.is_empty() {
        out.push('\n');
        out.push_str(after_trimmed);
    }
    out
}

/// Extract the substring between `<tag>` and `</tag>` from `text`.
pub fn extract_tag_content<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)?;
    let after = &text[start + open.len()..];
    let end = after.find(&close)?;
    Some(&after[..end])
}

/// Find the workspace path embedded in `<user_info>` (one line of the
/// form `Workspace Path: /abs/path`). Returns None when missing.
pub fn extract_workspace_path(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Workspace Path:") {
            let path = rest.trim();
            if !path.is_empty() {
                return Some(path.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Assistant text: <think> blocks + redaction
// ---------------------------------------------------------------------------

/// Pull the first `<think>…</think>` block out of an assistant message.
/// Cursor models emit explicit reasoning in this wrapper; the frontend
/// renders it under `MessageRole::System` with a `[thinking]` prefix.
pub fn extract_think_content(text: &str) -> Option<String> {
    let start = text.find("<think>")?;
    let after = &text[start + "<think>".len()..];
    let end = after.find("</think>").unwrap_or(after.len());
    let thinking = after[..end].trim();
    if thinking.is_empty() {
        None
    } else {
        Some(thinking.to_string())
    }
}

/// Remove every `<think>…</think>` block from `text`. Unclosed
/// `<think>` runs (streaming interrupted before `</think>`) discard
/// everything from the opening tag onward — partial reasoning is
/// noise we don't want to surface as part of the assistant reply.
pub fn strip_think_tags(text: &str) -> String {
    if !text.contains("<think>") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = remaining.find("<think>") {
        out.push_str(&remaining[..start]);
        remaining = &remaining[start + "<think>".len()..];
        match remaining.find("</think>") {
            Some(end) => remaining = &remaining[end + "</think>".len()..],
            None => {
                // Unclosed — drop the trailing fragment entirely.
                remaining = "";
                break;
            }
        }
    }
    out.push_str(remaining);
    out.trim().to_string()
}

/// Drop `[REDACTED]` placeholders Cursor leaves where reasoning was
/// scrubbed server-side. Also collapses the blank lines they leave
/// behind so the bubble doesn't render gaps.
pub fn strip_redacted(text: &str) -> String {
    let cleaned = text.replace("[REDACTED]", "");
    cleaned
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Tool args remapping
// ---------------------------------------------------------------------------

/// Translate Cursor's tool args into the canonical shape the frontend
/// already renders for Claude / Codex / Kimi (file_path / old_string /
/// new_string / command / etc.). Unknown tools pass through.
pub fn remap_tool_args(tool_name: &str, args: &Value) -> Option<String> {
    // ApplyPatch ships raw patch text as a string, not an object.
    if let Value::String(s) = args {
        return Some(remap_patch_string(s));
    }
    let obj = args.as_object()?;
    match tool_name {
        "Bash" => {
            let cmd = obj
                .get("command")
                .or_else(|| obj.get("input"))
                .and_then(|c| c.as_str())?;
            let mut out = serde_json::json!({ "command": cmd });
            if let Some(desc) = obj.get("description").and_then(|v| v.as_str()) {
                out["description"] = serde_json::json!(desc);
            }
            if let Some(cwd) = obj
                .get("working_directory")
                .or_else(|| obj.get("cwd"))
                .and_then(|v| v.as_str())
            {
                out["cwd"] = serde_json::json!(cwd);
            }
            Some(out.to_string())
        }
        "Read" => {
            let path = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .and_then(|p| p.as_str())?;
            let mut out = serde_json::json!({ "file_path": path });
            if let Some(limit) = obj.get("limit") {
                out["limit"] = limit.clone();
            }
            if let Some(offset) = obj.get("offset") {
                out["offset"] = offset.clone();
            }
            Some(out.to_string())
        }
        "Write" => {
            let path = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .and_then(|p| p.as_str())?;
            let mut out = serde_json::json!({ "file_path": path });
            if let Some(contents) = obj
                .get("contents")
                .or_else(|| obj.get("content"))
                .and_then(|v| v.as_str())
            {
                out["content"] = serde_json::json!(contents);
            }
            Some(out.to_string())
        }
        "Edit" => {
            let path = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let old = obj
                .get("old_str")
                .or_else(|| obj.get("old_string"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let new = obj
                .get("new_str")
                .or_else(|| obj.get("new_string"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            Some(
                serde_json::json!({
                    "file_path": path,
                    "old_string": old,
                    "new_string": new,
                })
                .to_string(),
            )
        }
        "Glob" => {
            let pattern = obj
                .get("glob_pattern")
                .or_else(|| obj.get("pattern"))
                .and_then(|p| p.as_str())?;
            let mut out = serde_json::json!({ "pattern": pattern });
            if let Some(target) = obj
                .get("target_directory")
                .or_else(|| obj.get("path"))
                .and_then(|p| p.as_str())
            {
                out["path"] = serde_json::json!(target);
            }
            Some(out.to_string())
        }
        "Grep" => {
            let pattern = obj.get("pattern").and_then(|p| p.as_str())?;
            let mut out = serde_json::json!({ "pattern": pattern });
            if let Some(p) = obj.get("path").and_then(|p| p.as_str()) {
                out["path"] = serde_json::json!(p);
            }
            if let Some(glob) = obj.get("glob").and_then(|v| v.as_str()) {
                out["glob"] = serde_json::json!(glob);
            }
            if let Some(output_mode) = obj.get("output_mode").and_then(|v| v.as_str()) {
                out["output_mode"] = serde_json::json!(output_mode);
            }
            if let Some(head_limit) = obj.get("head_limit") {
                out["head_limit"] = head_limit.clone();
            }
            if let Some(ignore_case) = obj.get("-i") {
                out["-i"] = ignore_case.clone();
            }
            if let Some(line_numbers) = obj.get("-n") {
                out["-n"] = line_numbers.clone();
            }
            Some(out.to_string())
        }
        "Agent" => {
            // Cursor's Task tool ships {description, prompt, subagent_type}.
            // Keep all three so the frontend agent bubble has identity + body.
            let mut out = serde_json::Map::new();
            for key in ["description", "prompt", "subagent_type"] {
                if let Some(v) = obj.get(key).and_then(|v| v.as_str()) {
                    out.insert(key.to_string(), serde_json::json!(v));
                }
            }
            if out.is_empty() {
                Some(args.to_string())
            } else {
                Some(serde_json::Value::Object(out).to_string())
            }
        }
        _ => Some(args.to_string()),
    }
}

/// Lift the file path out of an `ApplyPatch` payload so the bubble
/// summary has something to display, while still carrying the raw
/// patch text for the diff view.
fn remap_patch_string(patch: &str) -> String {
    let mut file_path = "";
    for line in patch.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("*** Update File: ")
            .or_else(|| trimmed.strip_prefix("*** Add File: "))
            .or_else(|| trimmed.strip_prefix("*** Delete File: "))
        {
            file_path = rest.trim();
            break;
        }
    }
    serde_json::json!({ "file_path": file_path, "patch": patch }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_text_from_string_or_array() {
        assert_eq!(extract_text_from_content(Some(&json!("hi"))), "hi");
        let arr = json!([
            {"type":"text","text":"a"},
            {"type":"tool_use","name":"Read"},
            {"type":"text","text":"b"},
        ]);
        assert_eq!(extract_text_from_content(Some(&arr)), "a\nb");
    }

    #[test]
    fn redacted_text_is_filtered() {
        let arr = json!([
            {"type":"text","text":"[REDACTED]"},
            {"type":"text","text":"actual response"}
        ]);
        assert_eq!(extract_text_from_content(Some(&arr)), "actual response");
    }

    #[test]
    fn user_query_is_unwrapped() {
        let raw = "<timestamp>x</timestamp>\n<user_query>\nbuild the thing\n</user_query>";
        assert_eq!(normalise_user_text(raw), "build the thing");
    }

    #[test]
    fn image_files_block_becomes_marker_lines() {
        let raw = "[Image]\n<image_files>\nThe following images were provided:\n1. /tmp/a.png\n2. /tmp/b.png\n</image_files>\n<user_query>describe them</user_query>";
        let out = normalise_user_text(raw);
        // user_query wins as the inner text — image markers stay in the
        // pre-block region; we keep only the final user_query.
        assert_eq!(out, "describe them");
        // The rewrite helper itself preserves the markers:
        let rewritten = rewrite_image_files_block(raw);
        assert!(rewritten.contains("[Image: source: /tmp/a.png]"));
        assert!(rewritten.contains("[Image: source: /tmp/b.png]"));
        assert!(!rewritten.contains("<image_files>"));
    }

    #[test]
    fn strip_think_handles_unclosed_tag() {
        let text = "before<think>unfinished";
        assert_eq!(strip_think_tags(text), "before");
        assert_eq!(extract_think_content(text), Some("unfinished".to_string()));
    }

    #[test]
    fn remap_edit_args_canonicalises_keys() {
        let args = json!({"path":"/a.txt","old_str":"x","new_str":"y"});
        let out = remap_tool_args("Edit", &args).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["file_path"], "/a.txt");
        assert_eq!(parsed["old_string"], "x");
        assert_eq!(parsed["new_string"], "y");
    }

    #[test]
    fn remap_glob_uses_glob_pattern() {
        let args = json!({"glob_pattern":"**/*.rs","target_directory":"/src"});
        let out = remap_tool_args("Glob", &args).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["pattern"], "**/*.rs");
        assert_eq!(parsed["path"], "/src");
    }

    #[test]
    fn write_preserves_contents() {
        let args = json!({"path": "/a.txt", "contents": "hello\nworld"});
        let out = remap_tool_args("Write", &args).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["file_path"], "/a.txt");
        assert_eq!(parsed["content"], "hello\nworld");
    }

    #[test]
    fn read_preserves_limit_offset() {
        let args = json!({"path": "/a.txt", "limit": 50, "offset": 200});
        let out = remap_tool_args("Read", &args).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["file_path"], "/a.txt");
        assert_eq!(parsed["limit"], 50);
        assert_eq!(parsed["offset"], 200);
    }

    #[test]
    fn grep_preserves_all_flags() {
        let args = json!({
            "pattern": "needle",
            "path": "/src",
            "glob": "*.rs",
            "head_limit": 20,
            "output_mode": "content",
            "-i": true,
            "-n": true,
        });
        let out = remap_tool_args("Grep", &args).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["pattern"], "needle");
        assert_eq!(parsed["path"], "/src");
        assert_eq!(parsed["glob"], "*.rs");
        assert_eq!(parsed["head_limit"], 20);
        assert_eq!(parsed["output_mode"], "content");
        assert_eq!(parsed["-i"], true);
        assert_eq!(parsed["-n"], true);
    }

    #[test]
    fn bash_preserves_description_and_cwd() {
        let args = json!({
            "command": "ls -la",
            "description": "list files",
            "working_directory": "/tmp",
        });
        let out = remap_tool_args("Bash", &args).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["command"], "ls -la");
        assert_eq!(parsed["description"], "list files");
        assert_eq!(parsed["cwd"], "/tmp");
    }

    #[test]
    fn agent_task_canonicalises_to_description_prompt_subagent() {
        let args = json!({
            "description": "audit parser",
            "prompt": "look at parser.rs and report issues",
            "subagent_type": "general-purpose",
            "extra_noise": "ignored",
        });
        let out = remap_tool_args("Agent", &args).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["description"], "audit parser");
        assert_eq!(parsed["prompt"], "look at parser.rs and report issues");
        assert_eq!(parsed["subagent_type"], "general-purpose");
        assert!(parsed.get("extra_noise").is_none());
    }

    #[test]
    fn workspace_path_pulled_from_user_info() {
        let text = "<user_info>\nWorkspace Path: /home/u/proj\nOS: macOS\n</user_info>";
        assert_eq!(
            extract_workspace_path(text),
            Some("/home/u/proj".to_string())
        );
    }
}
