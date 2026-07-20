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

/// Strip the `<system>…</system>` wrappers legacy kimi-cli used to inject
/// around tool results. Returns the original string when nothing matches.
fn strip_system_wrapper(text: &str) -> &str {
    let trimmed = text.trim();
    let stripped = trimmed
        .strip_prefix("<system>")
        .and_then(|s| s.strip_suffix("</system>"));
    stripped.unwrap_or(text)
}

/// Render a Format A tool message's content into a single rendered
/// string. Drops `<system>` decorations when there's also real content
/// alongside them. Returns `(rendered, is_error)` — Format A doesn't
/// carry an explicit error flag so the bool is always `None`.
pub(crate) fn render_format_a_tool_output(parts: &[Value]) -> (String, Option<bool>) {
    let mut texts: Vec<String> = Vec::new();
    let mut system_only: Vec<String> = Vec::new();
    for part in parts {
        if part.get("type").and_then(|v| v.as_str()) != Some("text") {
            continue;
        }
        let raw = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
        if raw.is_empty() {
            continue;
        }
        let trimmed = raw.trim();
        if trimmed.starts_with("<system>") && trimmed.ends_with("</system>") {
            system_only.push(strip_system_wrapper(raw).to_string());
        } else {
            texts.push(raw.to_string());
        }
    }
    let rendered = if texts.is_empty() {
        system_only.join("\n")
    } else {
        texts.join("\n")
    };
    (rendered, None)
}

/// Render a Format B `tool.result.result` payload. Looks for `output`
/// first (the common case in kimi-code 0.1.1), then falls back to
/// `message`, then serialises the whole result if nothing readable was
/// found. Returns `(rendered, is_error)` where `is_error` mirrors the
/// `is_error`/`isError`/`success` flags when the tool surfaces them.
pub(crate) fn render_format_b_tool_output(result: Option<&Value>) -> (String, Option<bool>) {
    let Some(result) = result else {
        return (String::new(), None);
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

    if let Some(output) = result.get("output").and_then(|v| v.as_str())
        && !output.is_empty()
    {
        return (output.to_string(), is_error);
    }
    if let Some(message) = result.get("message").and_then(|v| v.as_str())
        && !message.is_empty()
    {
        return (message.to_string(), is_error);
    }
    // Last resort: serialise the result so we don't drop information.
    match serde_json::to_string(result) {
        Ok(s) => (s, is_error),
        Err(_) => (String::new(), is_error),
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
        let (rendered, err) = render_format_a_tool_output(&parts);
        assert_eq!(rendered, "file1\nfile2");
        assert_eq!(err, None);
    }

    #[test]
    fn format_a_keeps_system_when_alone() {
        let parts = vec![
            json!({"type": "text", "text": "<system>Command executed successfully.</system>"}),
        ];
        let (rendered, _) = render_format_a_tool_output(&parts);
        assert_eq!(rendered, "Command executed successfully.");
    }

    #[test]
    fn format_b_prefers_output_over_message() {
        let result = json!({"output": "hello", "message": "ok"});
        let (rendered, err) = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered, "hello");
        assert_eq!(err, None);
    }

    #[test]
    fn format_b_falls_back_to_message_when_output_empty() {
        let result = json!({"output": "", "message": "File written."});
        let (rendered, _) = render_format_b_tool_output(Some(&result));
        assert_eq!(rendered, "File written.");
    }

    #[test]
    fn format_b_surfaces_error_flag() {
        let result = json!({"is_error": true, "output": "boom"});
        let (_, err) = render_format_b_tool_output(Some(&result));
        assert_eq!(err, Some(true));
    }

    #[test]
    fn format_b_surfaces_camelcase_error_flag() {
        let result = json!({"isError": true, "output": "boom"});
        let (_, err) = render_format_b_tool_output(Some(&result));
        assert_eq!(err, Some(true));
    }

    #[test]
    fn format_b_translates_success_false_to_error_true() {
        let result = json!({"success": false, "output": "boom"});
        let (_, err) = render_format_b_tool_output(Some(&result));
        assert_eq!(err, Some(true));
    }
}
