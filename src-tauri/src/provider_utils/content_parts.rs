use serde_json::Value;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ContentPartsRender {
    Empty,
    Rendered(String),
    Unsupported,
}

/// A provider-rendered tool result body plus the verdicts that feed
/// `ToolResultFacts`: whether the tool reported an error and whether the
/// body is an uninterpreted raw payload (`ToolResultMode::Raw`).
pub(crate) struct RenderedToolOutput {
    pub text: String,
    pub is_error: Option<bool>,
    pub is_raw: bool,
}

impl RenderedToolOutput {
    pub(crate) fn rendered(text: String) -> Self {
        Self {
            text,
            is_error: None,
            is_raw: false,
        }
    }

    pub(crate) fn raw(text: String) -> Self {
        Self {
            text,
            is_error: None,
            is_raw: true,
        }
    }
}

fn tagged_media(text: &str) -> Option<(&'static str, &str)> {
    let text = text.trim();
    for (prefix, label) in [
        ("<image path=\"", "Image"),
        ("<audio path=\"", "Audio"),
        ("<video path=\"", "Video"),
    ] {
        if let Some(path) = text
            .strip_prefix(prefix)
            .and_then(|value| value.strip_suffix("\">"))
            .filter(|value| !value.is_empty())
        {
            return Some((label, path));
        }
    }
    None
}

fn media_url<'a>(part: &'a Value, camel_key: &str, snake_key: &str) -> Option<&'a str> {
    let value = part.get(camel_key).or_else(|| part.get(snake_key))?;
    value
        .as_str()
        .or_else(|| value.get("url").and_then(Value::as_str))
        .filter(|url| !url.is_empty())
}

/// Render the common text/media content-part envelope used by Kimi, Codex,
/// and MCP tools. Unsupported parts stay distinguishable from a valid empty
/// result so callers can preserve unknown JSON without displaying `[]` for
/// known empty content.
pub(crate) fn render_content_parts(parts: &[Value]) -> ContentPartsRender {
    let mut chunks = Vec::new();
    let mut pending_images = 0usize;
    let mut pending_audio = 0usize;
    let mut pending_video = 0usize;

    for part in parts {
        let part_type = part
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| part.get("text").and_then(Value::as_str).map(|_| "text"))
            .or_else(|| {
                (part.get("imageUrl").is_some() || part.get("image_url").is_some())
                    .then_some("input_image")
            })
            .or_else(|| {
                (part.get("audioUrl").is_some() || part.get("audio_url").is_some())
                    .then_some("input_audio")
            })
            .or_else(|| {
                (part.get("videoUrl").is_some() || part.get("video_url").is_some())
                    .then_some("video_url")
            })
            .unwrap_or("");
        match part_type {
            "text" | "input_text" | "output_text" => {
                let Some(text) = part
                    .get("text")
                    .or_else(|| part.get("input_text"))
                    .or_else(|| part.get("output_text"))
                    .and_then(Value::as_str)
                else {
                    return ContentPartsRender::Unsupported;
                };
                if let Some((kind, path)) = tagged_media(text) {
                    chunks.push(format!("[{kind}: source: {path}]"));
                    match kind {
                        "Image" => pending_images += 1,
                        "Audio" => pending_audio += 1,
                        "Video" => pending_video += 1,
                        _ => {}
                    }
                } else if !matches!(text.trim(), "</image>" | "</audio>" | "</video>")
                    && !text.is_empty()
                {
                    chunks.push(text.to_string());
                }
            }
            "image_url" | "input_image" => {
                if pending_images > 0 {
                    pending_images -= 1;
                } else if let Some(url) = media_url(part, "imageUrl", "image_url") {
                    chunks.push(format!("[Image: source: {url}]"));
                } else {
                    return ContentPartsRender::Unsupported;
                }
            }
            "audio_url" | "input_audio" => {
                if pending_audio > 0 {
                    pending_audio -= 1;
                } else if let Some(url) = media_url(part, "audioUrl", "audio_url") {
                    chunks.push(format!("[Audio: source: {url}]"));
                } else {
                    return ContentPartsRender::Unsupported;
                }
            }
            "video_url" => {
                if pending_video > 0 {
                    pending_video -= 1;
                } else if let Some(url) = media_url(part, "videoUrl", "video_url") {
                    chunks.push(format!("[Video: source: {url}]"));
                } else {
                    return ContentPartsRender::Unsupported;
                }
            }
            _ => return ContentPartsRender::Unsupported,
        }
    }

    if chunks.is_empty() {
        ContentPartsRender::Empty
    } else {
        ContentPartsRender::Rendered(chunks.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ContentPartsRender, render_content_parts};

    #[test]
    fn content_parts_prefer_local_media_path_over_blob_url() {
        let parts = json!([
            {"type": "text", "text": "<image path=\"/tmp/screenshot.png\">"},
            {"type": "image_url", "imageUrl": {"url": "blobref:image/png;abc"}},
            {"type": "text", "text": "</image>"}
        ]);

        assert_eq!(
            render_content_parts(parts.as_array().unwrap()),
            ContentPartsRender::Rendered("[Image: source: /tmp/screenshot.png]".to_string())
        );
    }

    #[test]
    fn content_parts_reject_unknown_parts_without_dropping_them() {
        let parts = json!([{"type": "future_media", "payload": "keep me"}]);
        assert_eq!(
            render_content_parts(parts.as_array().unwrap()),
            ContentPartsRender::Unsupported
        );
    }

    #[test]
    fn content_parts_distinguish_known_empty_content() {
        let parts = json!([{"type": "text", "text": ""}]);
        assert_eq!(
            render_content_parts(parts.as_array().unwrap()),
            ContentPartsRender::Empty
        );
    }

    #[test]
    fn content_parts_reject_malformed_known_parts() {
        let parts = json!([{"type": "input_text", "text": 42}]);
        assert_eq!(
            render_content_parts(parts.as_array().unwrap()),
            ContentPartsRender::Unsupported
        );
    }

    #[test]
    fn content_parts_render_audio_outputs() {
        let parts = json!([{"type": "input_audio", "audio_url": "/tmp/result.wav"}]);
        assert_eq!(
            render_content_parts(parts.as_array().unwrap()),
            ContentPartsRender::Rendered("[Audio: source: /tmp/result.wav]".to_string())
        );
    }

    #[test]
    fn content_parts_render_legacy_untyped_image_outputs() {
        let parts = json!([{"image_url": "/tmp/legacy.png", "detail": "original"}]);
        assert_eq!(
            render_content_parts(parts.as_array().unwrap()),
            ContentPartsRender::Rendered("[Image: source: /tmp/legacy.png]".to_string())
        );
    }

    #[test]
    fn tagged_media_only_suppresses_its_matching_blob_part() {
        let parts = json!([
            {"type": "text", "text": "<image path=\"/tmp/local.png\">"},
            {"type": "image_url", "imageUrl": {"url": "blobref:image/png;abc"}},
            {"type": "text", "text": "</image>"},
            {"type": "image_url", "imageUrl": {"url": "https://example.com/remote.png"}}
        ]);
        assert_eq!(
            render_content_parts(parts.as_array().unwrap()),
            ContentPartsRender::Rendered(
                "[Image: source: /tmp/local.png]\n[Image: source: https://example.com/remote.png]"
                    .to_string()
            )
        );
    }
}
