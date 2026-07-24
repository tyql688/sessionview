//! Shared low-level helpers for building tool presentation details.

use std::collections::BTreeSet;

use crate::provider::util::shorten_home_path;
use serde_json::Value;

use crate::models::{ToolDetail, ToolDiffLine, ToolDiffLineType, ToolInlineDiff, ToolLine};

pub(super) fn generic_detail(obj: &serde_json::Map<String, Value>) -> ToolDetail {
    let mut lines = Vec::new();
    append_generic_lines(&mut lines, obj);
    detail(lines)
}

pub(super) fn append_generic_lines(
    lines: &mut Vec<ToolLine>,
    obj: &serde_json::Map<String, Value>,
) {
    for (key, value) in obj {
        if matches!(
            key.as_str(),
            "callDescription" | "callDisplay" | "persistedOutputPath" | "structuredPatch"
        ) {
            continue;
        }
        let value = value_to_display_string(value);
        if !value.is_empty() {
            lines.push(line(key, value));
        }
    }
}

pub(super) fn append_present_fields(
    lines: &mut Vec<ToolLine>,
    obj: &serde_json::Map<String, Value>,
    fields: &[(&str, &[&str])],
) {
    for (label, keys) in fields {
        if let Some(value) = pick_value(obj, keys) {
            let display = value_to_display_string(value);
            if !display.is_empty() {
                lines.push(line(*label, display));
            }
        }
    }
}

pub(super) fn pick_value<'a>(
    obj: &'a serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| obj.get(*key).filter(|value| !value.is_null()))
}

pub(super) fn first_string(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| obj.get(*key))
        .find_map(|value| {
            value
                .as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        })
}

pub(super) fn pick_field(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| obj.get(*key))
        .filter(|value| !value.is_null())
        .map(value_to_display_string)
}

pub(super) fn nested_record(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    value.and_then(Value::as_object)
}

pub(super) fn persisted_output_path(structured: &serde_json::Map<String, Value>) -> Option<&str> {
    structured
        .get("persistedOutputPath")
        .and_then(Value::as_str)
        .or_else(|| structured.get("outputPath").and_then(Value::as_str))
        .or_else(|| {
            structured
                .get("metadata")
                .and_then(Value::as_object)
                .and_then(|metadata| metadata.get("outputPath"))
                .and_then(Value::as_str)
        })
}

pub(super) fn nested_status_text(value: Option<&Value>) -> Option<String> {
    let record = value.and_then(Value::as_object)?;
    for key in ["completed", "failed", "running", "pending", "interrupted"] {
        if let Some(text) = first_string(record, &[key]) {
            return Some(text);
        }
    }
    None
}

pub(super) fn patch_files(structured: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut files = BTreeSet::new();
    if let Some(patch) = structured.get("patch").and_then(Value::as_object) {
        push_file_array(&mut files, patch.get("files"));
    }
    if let Some(patches) = structured.get("patches").and_then(Value::as_array) {
        for patch in patches.iter().filter_map(Value::as_object) {
            push_file_array(&mut files, patch.get("files"));
        }
    }
    files.into_iter().collect()
}

pub(super) fn push_file_array(files: &mut BTreeSet<String>, value: Option<&Value>) {
    let Some(values) = value.and_then(Value::as_array) else {
        return;
    };
    for file in values.iter().filter_map(Value::as_str) {
        if !file.is_empty() {
            files.insert(shorten_home_path(file));
        }
    }
}

pub(super) fn patched_files(patch_text: &str) -> Vec<String> {
    let files = patch_text
        .lines()
        .filter_map(|line| {
            line.strip_prefix("*** Update File: ")
                .or_else(|| line.strip_prefix("*** Add File: "))
                .or_else(|| line.strip_prefix("*** Delete File: "))
                .or_else(|| line.strip_prefix("*** Move to: "))
                .map(str::trim)
        })
        .filter(|file| !file.is_empty())
        .map(shorten_home_path)
        .collect::<BTreeSet<_>>();
    files.into_iter().collect()
}

pub(super) fn build_patch_from_antigravity_chunks(file: &str, chunks: &[Value]) -> String {
    let mut patch = format!("*** Begin Patch\n*** Update File: {file}\n");
    for chunk in chunks.iter().filter_map(Value::as_object) {
        let old_text = first_string(chunk, &["TargetContent"]).unwrap_or_default();
        let new_text = first_string(chunk, &["ReplacementContent"]).unwrap_or_default();
        let old_lines = split_patch_payload_lines(&old_text);
        let new_lines = split_patch_payload_lines(&new_text);
        let start_line = chunk
            .get("StartLine")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1);
        let old_count = old_lines.len().max(1);
        let new_count = new_lines.len().max(1);
        patch.push_str(&format!(
            "@@ -{start_line},{old_count} +{start_line},{new_count} @@\n"
        ));
        for line in old_lines {
            patch.push('-');
            patch.push_str(line);
            patch.push('\n');
        }
        for line in new_lines {
            patch.push('+');
            patch.push_str(line);
            patch.push('\n');
        }
    }
    patch.push_str("*** End Patch\n");
    patch
}

pub(super) fn split_patch_payload_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect()
    }
}

#[derive(Clone, Copy)]
struct PatchLineCursor {
    old_line: Option<u32>,
    new_line: Option<u32>,
}

fn parse_hunk_start(token: &str) -> Option<u32> {
    token.split(',').next()?.parse().ok()
}

fn parse_unified_hunk_header(line: &str) -> Option<PatchLineCursor> {
    let rest = line.strip_prefix("@@")?.trim_start();
    let rest = rest.strip_prefix('-')?;
    let (old_part, rest) = rest.split_once(char::is_whitespace)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('+')?;
    let new_part = rest.split_whitespace().next()?;
    Some(PatchLineCursor {
        old_line: parse_hunk_start(old_part),
        new_line: parse_hunk_start(new_part),
    })
}

pub(super) fn build_patch_line_diff(patch_text: &str) -> Vec<ToolDiffLine> {
    let mut lines = Vec::new();
    let mut cursor = PatchLineCursor {
        old_line: None,
        new_line: None,
    };
    for raw_line in patch_text.lines() {
        if raw_line == "*** Begin Patch" || raw_line == "*** End Patch" || raw_line.is_empty() {
            continue;
        }

        if raw_line.starts_with("*** ") || raw_line.starts_with("@@") {
            cursor = if raw_line.starts_with("@@") {
                parse_unified_hunk_header(raw_line).unwrap_or(PatchLineCursor {
                    old_line: None,
                    new_line: None,
                })
            } else {
                PatchLineCursor {
                    old_line: None,
                    new_line: None,
                }
            };
            push_diff_line(
                &mut lines,
                ToolDiffLineType::Skip,
                &shorten_home_path(raw_line),
                None,
                None,
            );
        } else if let Some(rest) = raw_line.strip_prefix('+') {
            push_diff_line(
                &mut lines,
                ToolDiffLineType::Add,
                rest,
                None,
                cursor.new_line,
            );
            increment_line(&mut cursor.new_line);
        } else if let Some(rest) = raw_line.strip_prefix('-') {
            push_diff_line(
                &mut lines,
                ToolDiffLineType::Remove,
                rest,
                cursor.old_line,
                None,
            );
            increment_line(&mut cursor.old_line);
        } else if let Some(rest) = raw_line.strip_prefix(' ') {
            push_diff_line(
                &mut lines,
                ToolDiffLineType::Context,
                rest,
                cursor.old_line,
                cursor.new_line,
            );
            increment_line(&mut cursor.old_line);
            increment_line(&mut cursor.new_line);
        } else {
            push_diff_line(&mut lines, ToolDiffLineType::Skip, raw_line, None, None);
        }
    }
    lines
}

pub(super) fn build_structured_patch_line_diff(structured_patch: &Value) -> Vec<ToolDiffLine> {
    let Some(hunks) = structured_patch.as_array() else {
        return Vec::new();
    };
    let mut lines = Vec::new();

    for hunk in hunks.iter().filter_map(Value::as_object) {
        let Some(raw_lines) = hunk.get("lines").and_then(Value::as_array) else {
            continue;
        };
        let old_start = u32_field(hunk, "oldStart");
        let old_lines = u32_field(hunk, "oldLines").unwrap_or(0);
        let new_start = u32_field(hunk, "newStart");
        let new_lines = u32_field(hunk, "newLines").unwrap_or(0);

        let header = match (old_start, new_start) {
            (Some(old_start), Some(new_start)) => {
                format!("@@ -{old_start},{old_lines} +{new_start},{new_lines} @@")
            }
            _ => "@@".to_string(),
        };
        push_diff_line(&mut lines, ToolDiffLineType::Skip, &header, None, None);

        let mut old_line = old_start;
        let mut new_line = new_start;
        for raw in raw_lines.iter().filter_map(Value::as_str) {
            if let Some(rest) = raw.strip_prefix('+') {
                push_diff_line(&mut lines, ToolDiffLineType::Add, rest, None, new_line);
                increment_line(&mut new_line);
            } else if let Some(rest) = raw.strip_prefix('-') {
                push_diff_line(&mut lines, ToolDiffLineType::Remove, rest, old_line, None);
                increment_line(&mut old_line);
            } else if let Some(rest) = raw.strip_prefix(' ') {
                push_diff_line(
                    &mut lines,
                    ToolDiffLineType::Context,
                    rest,
                    old_line,
                    new_line,
                );
                increment_line(&mut old_line);
                increment_line(&mut new_line);
            } else {
                push_diff_line(&mut lines, ToolDiffLineType::Skip, raw, None, None);
            }
        }
    }

    lines
}

pub(super) fn u32_field(obj: &serde_json::Map<String, Value>, key: &str) -> Option<u32> {
    obj.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

pub(super) fn increment_line(line: &mut Option<u32>) {
    if let Some(value) = line {
        *value += 1;
    }
}

pub(super) fn push_diff_line(
    lines: &mut Vec<ToolDiffLine>,
    kind: ToolDiffLineType,
    text: &str,
    old_line: Option<u32>,
    new_line: Option<u32>,
) {
    lines.push(ToolDiffLine {
        kind,
        old_line,
        new_line,
        text: text.trim_end_matches('\n').to_string(),
    });
}

pub(super) fn line(label: impl Into<String>, value: impl Into<String>) -> ToolLine {
    let label = label.into();
    let value = value.into();
    ToolLine {
        value: if is_path_label(&label) {
            shorten_home_path(&value)
        } else {
            value
        },
        label,
    }
}

pub(super) fn is_path_label(label: &str) -> bool {
    let normalized = label.to_ascii_lowercase();
    normalized == "file" || normalized == "path" || normalized.ends_with("path")
}

pub(super) fn detail(lines: Vec<ToolLine>) -> ToolDetail {
    ToolDetail {
        lines,
        diff: None,
        patch_diff: None,
        persisted_output_path: None,
        media: Vec::new(),
    }
}

pub(super) trait ToolDetailExt {
    fn with_diff(self, old: impl Into<String>, new: impl Into<String>) -> ToolDetail;
    fn with_patch_diff(self, patch_diff: Vec<ToolDiffLine>) -> ToolDetail;
}

impl ToolDetailExt for ToolDetail {
    fn with_diff(mut self, old: impl Into<String>, new: impl Into<String>) -> ToolDetail {
        self.diff = Some(ToolInlineDiff {
            old: old.into(),
            new: new.into(),
        });
        self
    }

    fn with_patch_diff(mut self, patch_diff: Vec<ToolDiffLine>) -> ToolDetail {
        self.patch_diff = Some(patch_diff);
        self
    }
}

pub(super) fn value_to_display_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Array(values) => values
            .iter()
            .map(value_to_display_string)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(obj) => {
            if let (Some(from), Some(to)) = (obj.get("from"), obj.get("to")) {
                return format!(
                    "{} → {}",
                    value_to_display_string(from),
                    value_to_display_string(to)
                );
            }
            obj.iter()
                .map(|(key, value)| format!("{key}: {}", value_to_display_string(value)))
                .collect::<Vec<_>>()
                .join(", ")
        }
        Value::Null => String::new(),
    }
}

/// Extract image sources from the structured result's content-part envelope.
/// Covers the shapes providers store today: `output` / `content` /
/// `experimental_content` part arrays (or JSON-encoded arrays), plus MCP's
/// `result.content` and `result.Ok.content`. This is the single owner of
/// that wire knowledge — the frontend renders `ToolDetail::media` verbatim.
pub(super) fn structured_media(structured: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut candidates: Vec<&Value> = Vec::new();
    for key in ["output", "content", "experimental_content"] {
        if let Some(value) = structured.get(key) {
            candidates.push(value);
        }
    }
    match structured.get("result") {
        Some(result @ (Value::Array(_) | Value::String(_))) => candidates.push(result),
        Some(Value::Object(result)) => {
            if let Some(content) = result.get("content") {
                candidates.push(content);
            }
            if let Some(content) = result.get("Ok").and_then(|ok| ok.get("content")) {
                candidates.push(content);
            }
        }
        _ => {}
    }

    for candidate in candidates {
        let parsed;
        let parts = match candidate {
            Value::Array(parts) => parts.as_slice(),
            Value::String(text) if text.trim_start().starts_with('[') => {
                match serde_json::from_str::<Value>(text) {
                    Ok(Value::Array(owned)) => {
                        parsed = owned;
                        parsed.as_slice()
                    }
                    _ => continue,
                }
            }
            _ => continue,
        };
        let sources: Vec<String> = parts.iter().filter_map(image_source).collect();
        if !sources.is_empty() {
            return sources;
        }
    }
    // Grok image_gen (and similar) store a bare filesystem path on the
    // result object rather than an image content block.
    for key in ["path", "savedPath", "saved_path"] {
        if let Some(path) = structured.get(key).and_then(Value::as_str) {
            let trimmed = path.trim();
            if !trimmed.is_empty() && looks_like_image_path(trimmed) {
                return vec![trimmed.to_string()];
            }
        }
    }
    Vec::new()
}

fn looks_like_image_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
        || lower.ends_with(".bmp")
        || lower.ends_with(".svg")
}

fn image_source(part: &Value) -> Option<String> {
    let obj = part.as_object()?;
    let part_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
    let has_url_key = obj.contains_key("image_url") || obj.contains_key("imageUrl");
    if matches!(part_type, "input_image" | "image_url") || (part_type.is_empty() && has_url_key) {
        return part_media_url(obj);
    }
    if part_type != "image" {
        return None;
    }
    if let Some(url) = part_media_url(obj) {
        return Some(url);
    }
    let data = obj.get("data").and_then(Value::as_str)?.trim();
    let mime = obj
        .get("mimeType")
        .or_else(|| obj.get("mime_type"))
        .and_then(Value::as_str)?;
    if data.is_empty() || !mime.starts_with("image/") {
        return None;
    }
    Some(if data.starts_with("data:image/") {
        data.to_string()
    } else {
        format!("data:{mime};base64,{data}")
    })
}

fn part_media_url(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let value = obj.get("image_url").or_else(|| obj.get("imageUrl"))?;
    let url = value
        .as_str()
        .or_else(|| value.get("url").and_then(Value::as_str))?
        .trim();
    (!url.is_empty()).then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_patch_line_diff_derives_line_numbers_from_unified_hunks() {
        let lines = build_patch_line_diff(
            r#"*** Begin Patch
*** Update File: src/app.ts
@@ -12,2 +12,3 @@
 const same = true;
-const oldValue = 1;
+const newValue = 2;
+const extra = true;
*** End Patch
"#,
        );

        assert_eq!(lines[2].kind, ToolDiffLineType::Context);
        assert_eq!(lines[2].old_line, Some(12));
        assert_eq!(lines[2].new_line, Some(12));
        assert_eq!(lines[3].kind, ToolDiffLineType::Remove);
        assert_eq!(lines[3].old_line, Some(13));
        assert_eq!(lines[3].new_line, None);
        assert_eq!(lines[4].kind, ToolDiffLineType::Add);
        assert_eq!(lines[4].old_line, None);
        assert_eq!(lines[4].new_line, Some(13));
        assert_eq!(lines[5].kind, ToolDiffLineType::Add);
        assert_eq!(lines[5].new_line, Some(14));
    }
}
