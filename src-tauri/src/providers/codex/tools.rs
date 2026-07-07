use serde_json::Value;

// Provider-agnostic marker helpers — see `services::image_markers`.
pub(crate) use crate::services::image_markers::{
    extract_image_source_segments, is_image_placeholder,
};

pub(crate) fn extract_codex_content(payload: &Value) -> String {
    match payload.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => extract_codex_array_content(arr),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
        None => {
            // Also check for direct "output" field (function_call_output)
            payload
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        }
    }
}

pub(crate) fn extract_codex_array_content(arr: &[Value]) -> String {
    let mut parts = Vec::new();

    for item in arr {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match item_type {
            "input_image" => {
                if let Some(image_url) = item.get("image_url").and_then(|v| v.as_str()) {
                    parts.push(format!("[Image: source: {image_url}]"));
                }
            }
            _ => {
                let Some(text) = extract_codex_text(item) else {
                    continue;
                };

                if is_codex_image_wrapper(text) {
                    continue;
                }

                parts.push(text.to_string());
            }
        }
    }

    parts.join("\n")
}

pub(crate) fn extract_codex_text(item: &Value) -> Option<&str> {
    item.get("text")
        .or_else(|| item.get("output_text"))
        .or_else(|| item.get("input_text"))
        .and_then(|t| t.as_str())
}

pub(crate) fn is_codex_image_wrapper(text: &str) -> bool {
    let trimmed = text.trim();
    (trimmed.starts_with("<image name=") && trimmed.ends_with('>')) || trimmed == "</image>"
}

/// Extract readable text from Codex tool output.
/// Handles: plain text, JSON `{"output":"..."}`, JSON array `[{"type":"text","text":"..."}]`.
pub(crate) fn extract_tool_output(raw: &str) -> String {
    let trimmed = raw.trim();
    // Try JSON object with "output" field (custom_tool_call_output)
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            if let Some(out) = v.get("output").and_then(|o| o.as_str()) {
                return omit_base64_image_sources(out);
            }
        }
    }
    // Try JSON array of text parts (MCP tool output)
    if trimmed.starts_with('[') {
        if let Ok(arr) = serde_json::from_str::<Vec<Value>>(trimmed) {
            let parts: Vec<String> = arr
                .iter()
                .filter_map(|item| {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        return Some(omit_base64_image_sources(text));
                    }
                    if item
                        .get("image_url")
                        .and_then(|v| v.as_str())
                        .is_some_and(is_base64_image_url)
                    {
                        return Some("[Image]".to_string());
                    }
                    None
                })
                .collect();
            if !parts.is_empty() {
                return parts.join("\n");
            }
        }
    }
    omit_base64_image_sources(raw)
}

pub(crate) fn strip_inline_image_sources(text: &str) -> String {
    if !text.contains("[Image: source:") {
        return text.to_string();
    }

    text.lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("[Image: source:") {
                "[Image]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn omit_base64_image_sources(text: &str) -> String {
    if !text.contains(";base64,") {
        return text.to_string();
    }

    let mut result = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("[Image: source: data:image/") {
        result.push_str(&remaining[..start]);
        let image_marker = &remaining[start..];
        let Some(end) = image_marker.find(']') else {
            result.push_str("[Image]");
            return result;
        };
        result.push_str("[Image]");
        remaining = &image_marker[end + 1..];
    }
    result.push_str(remaining);

    if result.contains(";base64,") && is_base64_image_url(result.trim()) {
        "[Image]".to_string()
    } else {
        result
    }
}

fn is_base64_image_url(value: &str) -> bool {
    value.starts_with("data:image/") && value.contains(";base64,")
}

pub(crate) fn build_codex_user_message(
    payload: &Value,
    response_image_segments: &[String],
) -> String {
    let message = payload
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let local_image_segments: Vec<String> = payload
        .get("local_images")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .map(image_source_marker)
        .collect();

    let mut preferred_image_segments = response_image_segments.to_vec();
    if preferred_image_segments.len() < local_image_segments.len() {
        preferred_image_segments.extend(
            local_image_segments
                .iter()
                .skip(preferred_image_segments.len())
                .cloned(),
        );
    }

    let text_elements = payload.get("text_elements").and_then(|v| v.as_array());
    let (mut merged_text, used_inline_images) =
        merge_text_elements_with_image_segments(message, text_elements, &preferred_image_segments);

    let remote_image_segments: Vec<String> = payload
        .get("images")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .map(image_source_marker)
        .collect();

    append_image_segments(&mut merged_text, &remote_image_segments);
    append_image_segments(
        &mut merged_text,
        &preferred_image_segments[used_inline_images..],
    );

    merged_text
}

fn image_source_marker(source: &str) -> String {
    format!("[Image: source: {source}]")
}

fn append_image_segments(content: &mut String, segments: &[String]) {
    for segment in segments {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(segment);
    }
}

fn merge_text_elements_with_image_segments(
    text: &str,
    text_elements: Option<&Vec<Value>>,
    image_segments: &[String],
) -> (String, usize) {
    let Some(text_elements) = text_elements else {
        return (text.to_string(), 0);
    };

    let mut ranges: Vec<(usize, usize, Option<&str>)> = text_elements
        .iter()
        .filter_map(|element| {
            let start = element
                .get("byte_range")
                .and_then(|range| range.get("start"))
                .and_then(|value| value.as_u64())? as usize;
            let end = element
                .get("byte_range")
                .and_then(|range| range.get("end"))
                .and_then(|value| value.as_u64())? as usize;
            let placeholder = element.get("placeholder").and_then(|value| value.as_str());
            Some((start, end, placeholder))
        })
        .collect();

    ranges.sort_by_key(|(start, _, _)| *start);

    let mut merged = String::new();
    let mut last_index = 0usize;
    let mut source_index = 0usize;

    for (start, end, placeholder) in ranges {
        if start > end
            || end > text.len()
            || start < last_index
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            continue;
        }

        merged.push_str(&text[last_index..start]);
        let original = &text[start..end];
        let element_text = placeholder.unwrap_or(original);

        if source_index < image_segments.len() && is_image_placeholder(element_text) {
            merged.push_str(&image_segments[source_index]);
            source_index += 1;
        } else {
            merged.push_str(original);
        }

        last_index = end;
    }

    merged.push_str(&text[last_index..]);
    (merged, source_index)
}
