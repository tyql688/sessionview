//! Render kimi-code tool results into the plain-text form the frontend
//! shows under the tool bubble.
//!
//! Two shapes feed in:
//!
//! * **Format A** (migrated wire.jsonl): the tool message is a role=tool
//!   `context.append_message` whose `content` is an array of
//!   `{type:"text", text:"…"}` parts. The legacy kimi-cli decorated
//!   results with `<system>Command executed successfully.</system>`
//!   wrappers; we drop those so they don't clutter the bubble unless
//!   they're the only content.
//!
//! * **Format B** (native kimi-code 0.1.1+): the result lives inside a
//!   `context.append_loop_event` of type `tool.result`. The `result`
//!   object is small — typically `{ "output": "..." }` — and may also
//!   carry `is_error` / `message` / `display[]` depending on tool kind.

use serde_json::Value;

use crate::provider::util::{ContentPartsRender, render_content_parts};

use crate::provider::util::RenderedToolOutput;

/// Strip the `<system>…</system>` wrappers legacy kimi-cli used to inject
/// around tool results. Returns the original string when nothing matches.
fn strip_system_wrapper(text: &str) -> &str {
    let trimmed = text.trim();
    let stripped = trimmed
        .strip_prefix("<system>")
        .and_then(|s| s.strip_suffix("</system>"));
    stripped.unwrap_or(text)
}

fn is_system_wrapped(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("<system>") && trimmed.ends_with("</system>")
}

fn part_text(part: &Value) -> Option<&str> {
    part.get("text")
        .or_else(|| part.get("input_text"))
        .or_else(|| part.get("output_text"))
        .and_then(Value::as_str)
}

/// Render a Format A tool message's content into a single rendered
/// string. Drops `<system>` decorations when there's also real content
/// alongside them. Format A doesn't carry an explicit error flag.
pub(crate) fn render_format_a_tool_output(parts: &[Value]) -> RenderedToolOutput {
    if render_content_parts(parts) == ContentPartsRender::Unsupported {
        return RenderedToolOutput::raw(Value::Array(parts.to_vec()).to_string());
    }

    // `<system>` decorations only render when nothing else would.
    let has_real_content = parts.iter().any(|part| match part_text(part) {
        Some(text) => !text.trim().is_empty() && !is_system_wrapped(text),
        None => true,
    });
    let normalized = parts
        .iter()
        .filter_map(|part| {
            let Some(text) = part.get("text").and_then(Value::as_str) else {
                return Some(part.clone());
            };
            if !is_system_wrapped(text) {
                return Some(part.clone());
            }
            if has_real_content {
                return None;
            }
            let mut part = part.clone();
            part["text"] = Value::String(strip_system_wrapper(text).to_string());
            Some(part)
        })
        .collect::<Vec<_>>();
    match render_content_parts(&normalized) {
        ContentPartsRender::Rendered(text) => RenderedToolOutput::rendered(text),
        ContentPartsRender::Empty => RenderedToolOutput::rendered(String::new()),
        ContentPartsRender::Unsupported => unreachable!("content parts were validated above"),
    }
}

/// Render a Format B `tool.result.result` payload. Looks for `output`
/// first (the common case in kimi-code 0.1.1), then falls back to
/// `message`, then serialises the whole result if nothing readable was
/// found. Unknown shapes are preserved as the sole raw result; `is_error`
/// mirrors the `is_error`/`isError`/`success` flags when available.
pub(crate) fn render_format_b_tool_output(result: Option<&Value>) -> RenderedToolOutput {
    let Some(result) = result else {
        return RenderedToolOutput {
            text: String::new(),
            is_error: None,
            is_raw: false,
        };
    };
    let is_error = result
        .get("is_error")
        .and_then(|v| v.as_bool())
        .or_else(|| result.get("isError").and_then(|v| v.as_bool()))
        .or_else(|| {
            result
                .get("success")
                .and_then(|v| v.as_bool())
                .map(|ok| !ok)
        });

    let mut known_empty_output = false;
    if let Some(output) = result.get("output") {
        if let Some(output) = output.as_str() {
            if !output.is_empty() {
                return RenderedToolOutput {
                    text: output.to_string(),
                    is_error,
                    is_raw: false,
                };
            }
            known_empty_output = true;
        }
        if let Some(parts) = output.as_array() {
            match render_content_parts(parts) {
                ContentPartsRender::Rendered(rendered) => {
                    return RenderedToolOutput {
                        text: rendered,
                        is_error,
                        is_raw: false,
                    };
                }
                ContentPartsRender::Empty => known_empty_output = true,
                ContentPartsRender::Unsupported => {
                    if let Ok(raw) = serde_json::to_string(output) {
                        return RenderedToolOutput {
                            text: raw,
                            is_error,
                            is_raw: true,
                        };
                    }
                }
            }
        } else if output.is_null() {
            known_empty_output = true;
        } else if !output.is_string() {
            return RenderedToolOutput {
                text: output.to_string(),
                is_error,
                is_raw: true,
            };
        }
    }
    if let Some(message) = result.get("message").and_then(|v| v.as_str())
        && !message.is_empty()
    {
        return RenderedToolOutput {
            text: message.to_string(),
            is_error,
            is_raw: false,
        };
    }
    if known_empty_output {
        return RenderedToolOutput {
            text: String::new(),
            is_error,
            is_raw: false,
        };
    }
    let text = serde_json::to_string(result).unwrap_or_default();
    RenderedToolOutput {
        is_raw: !text.is_empty(),
        text,
        is_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_a_drops_system_wrapper_when_real_text_exists() {
        let parts = vec![
            json!({"type": "text", "text": "<system>Command executed successfully.</system>"}),
            json!({"type": "text", "text": "file1\nfile2"}),
        ];
        let rendered = render_format_a_tool_output(&parts);
        assert_eq!(rendered.text, "file1\nfile2");
        assert_eq!(rendered.is_error, None);
        assert!(!rendered.is_raw);
    }

    #[test]
    fn format_a_keeps_system_when_alone() {
        let parts = vec![
            json!({"type": "text", "text": "<system>Command executed successfully.</system>"}),
        ];
        let rendered = render_format_a_tool_output(&parts);
        assert_eq!(rendered.text, "Command executed successfully.");
        assert!(!rendered.is_raw);
    }

    #[test]
    fn format_a_preserves_unknown_parts_as_raw_json() {
        let parts = vec![json!({"type": "future_result", "payload": {"value": 1}})];
        let rendered = render_format_a_tool_output(&parts);
        assert_eq!(
            rendered.text,
            r#"[{"payload":{"value":1},"type":"future_result"}]"#
        );
        assert!(rendered.is_raw);
    }

    #[test]
    fn format_a_renders_known_media_parts_as_output() {
        let parts = vec![json!({"type": "input_audio", "audio_url": "/tmp/result.wav"})];
        let rendered = render_format_a_tool_output(&parts);
        assert_eq!(rendered.text, "[Audio: source: /tmp/result.wav]");
        assert!(!rendered.is_raw);
    }

    #[test]
    fn format_b_prefers_output_over_message() {
        let result = json!({"output": "hello", "message": "ok"});
        let rendered = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered.text, "hello");
        assert_eq!(rendered.is_error, None);
        assert!(!rendered.is_raw);
    }

    #[test]
    fn format_b_renders_content_part_arrays_without_raw_json() {
        let result = json!({
            "output": [
                {"type": "text", "text": "<image path=\"/tmp/screenshot.png\">"},
                {"type": "image_url", "imageUrl": {"url": "blobref:image/png;abc"}},
                {"type": "text", "text": "</image>"}
            ]
        });
        let rendered = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered.text, "[Image: source: /tmp/screenshot.png]");
        assert_eq!(rendered.is_error, None);
        assert!(!rendered.is_raw);
    }

    #[test]
    fn format_b_keeps_unknown_output_arrays_as_json() {
        let result = json!({"output": [{"type": "future_media", "payload": "keep"}]});
        let rendered = render_format_b_tool_output(Some(&result));
        assert_eq!(
            rendered.text,
            r#"[{"payload":"keep","type":"future_media"}]"#
        );
        assert!(rendered.is_raw);
    }

    #[test]
    fn format_b_keeps_unknown_output_objects_as_raw_json() {
        let result = json!({"output": {"future": 1}, "message": "do not hide the payload"});
        let rendered = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered.text, r#"{"future":1}"#);
        assert!(rendered.is_raw);
    }

    #[test]
    fn format_b_falls_back_to_message_when_output_empty() {
        let result = json!({"output": "", "message": "File written."});
        let rendered = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered.text, "File written.");
        assert!(!rendered.is_raw);
    }

    #[test]
    fn format_b_does_not_render_known_empty_content_parts_as_raw_json() {
        let result = json!({"output": [{"type": "text", "text": ""}]});
        let rendered = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered.text, "");
        assert!(!rendered.is_raw);
    }

    #[test]
    fn format_b_surfaces_error_flag() {
        let result = json!({"is_error": true, "output": "boom"});
        let rendered = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered.is_error, Some(true));
    }

    #[test]
    fn format_b_surfaces_camelcase_error_flag() {
        let result = json!({"isError": true, "output": "boom"});
        let rendered = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered.is_error, Some(true));
    }

    #[test]
    fn format_b_translates_success_false_to_error_true() {
        let result = json!({"success": false, "output": "boom"});
        let rendered = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered.is_error, Some(true));
    }
}
