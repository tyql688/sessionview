//! Content/usage extraction: message-content → text conversion, token-usage
//! extraction, dedup-hash helpers, and the `<persisted-output>` resolver.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde_json::Value;

use crate::models::TokenUsage;
use crate::provider_utils::RenderedToolOutput;
use crate::tool_metadata::ToolResultFacts;

use super::super::images::{count_image_markers, normalize_image_source_segments};

pub(super) fn tool_result_facts<'a>(
    result_item: &'a Value,
    top_level_result: Option<&'a Value>,
    raw_output: bool,
) -> ToolResultFacts<'a> {
    // Async-Agent results carry a lifecycle marker
    // (`"async_launched"` / `"completed"` / …). Surface it so the UI
    // can distinguish "kicked off in background" from "finished",
    // instead of always showing the default success badge.
    let status = top_level_result
        .and_then(|v| v.get("status"))
        .and_then(|v| v.as_str());
    ToolResultFacts {
        raw_result: top_level_result,
        is_error: result_item.get("is_error").and_then(|v| v.as_bool()),
        status,
        artifact_path: top_level_result
            .and_then(|v| v.get("persistedOutputPath"))
            .and_then(|v| v.as_str()),
        raw_output: Some(raw_output),
    }
}

pub(super) fn unique_hash_from_entry(entry: &Value) -> Option<String> {
    let message_id = entry
        .get("message")
        .and_then(|message| message.get("id"))
        .and_then(|id| id.as_str())?;
    let request_id = entry.get("requestId").and_then(|id| id.as_str())?;
    Some(format!("{message_id}:{request_id}"))
}

pub(super) fn dedup_hash_from_entry(entry: &Value) -> Option<String> {
    let base = unique_hash_from_entry(entry)?;
    // Hash the content rather than storing its full serialization in the dedup
    // set — keeps `processed_hashes` small for sessions with large messages.
    let content_hash = match entry.get("message").and_then(|m| m.get("content")) {
        // `to_string` on an in-memory Value never fails in practice; if it ever
        // did, skip dedup for this entry (returns None) instead of panicking.
        Some(content) => {
            let serialized = serde_json::to_string(content).ok()?;
            let mut hasher = DefaultHasher::new();
            serialized.hash(&mut hasher);
            hasher.finish()
        }
        None => 0,
    };
    Some(format!("{base}:{content_hash:x}"))
}

/// Extract token usage from a message's `usage` field.
pub(super) fn extract_token_usage(message: &Value) -> Option<TokenUsage> {
    let value = message.get("usage")?;
    let mut usage = crate::provider_utils::token_usage_from(
        value,
        &crate::provider_utils::UsageKeys {
            input: &["input_tokens"],
            output: &["output_tokens"],
            cache_read: &["cache_read_input_tokens"],
            cache_write: &["cache_creation_input_tokens"],
        },
    )?;
    // Some lines report cache writes only in the `cache_creation` breakdown.
    // A larger flat total wins: the breakdown may omit TTL buckets.
    if let Some(cache_creation) = value.get("cache_creation").and_then(Value::as_object) {
        let total = cache_creation
            .get("ephemeral_5m_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(
                cache_creation
                    .get("ephemeral_1h_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            );
        let Ok(total) = u32::try_from(total) else {
            log::warn!("Claude cache creation usage exceeds the supported token range");
            return None;
        };
        usage.cache_creation_input_tokens = usage.cache_creation_input_tokens.max(total);
    }
    if usage.total_tokens() == 0 {
        return None;
    }
    Some(usage)
}

/// Extract text content from a message object.
/// The `content` field can be a string or an array of typed blocks.
/// Handles both "text" and "tool_use" content blocks.
pub(super) fn extract_message_content(message: &Value) -> String {
    let content = message.get("content");
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => {
            let mut parts = Vec::new();
            let mut image_block_count = 0usize;
            for item in arr {
                let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match item_type {
                    "text" => {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            parts.push(normalize_image_source_segments(text));
                        }
                    }
                    "tool_use" => {
                        let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                        let input = item
                            .get("input")
                            .map(std::string::ToString::to_string)
                            .unwrap_or_default();
                        let end = if input.len() > 200 {
                            input.floor_char_boundary(200)
                        } else {
                            input.len()
                        };
                        parts.push(format!("[Tool: {}] {}", name, &input[..end]));
                    }
                    "tool_result" => {
                        if let Some(text) = item.get("content").and_then(|c| c.as_str()) {
                            let end = if text.len() > 200 {
                                text.floor_char_boundary(200)
                            } else {
                                text.len()
                            };
                            parts.push(format!("[Result] {}", &text[..end]));
                        }
                    }
                    "image" => {
                        image_block_count += 1;
                    }
                    other => {
                        log::debug!(
                            "unknown Claude assistant content block type '{other}' — skipped"
                        );
                    }
                }
            }
            let marker_count = parts
                .iter()
                .map(|part| count_image_markers(part))
                .sum::<usize>();
            for _ in marker_count..image_block_count {
                parts.push("[Image]".to_string());
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

/// Check if a "user" message is actually a tool_result turn.
/// In the Anthropic API, tool results are sent as user-role messages
/// with content blocks of type "tool_result".
pub(super) fn is_tool_result_message(message: &Value) -> bool {
    match message.get("content") {
        Some(Value::Array(arr)) if !arr.is_empty() => arr
            .iter()
            .all(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_result")),
        _ => false,
    }
}

/// Resolve `<persisted-output>` tags by reading the referenced external file.
/// Falls back to keeping the original content (with preview) if the file can't be read.
/// Only paths under `~/.claude/` are allowed to prevent arbitrary file reads.
pub fn resolve_persisted_outputs(content: &str) -> String {
    const TAG_START: &str = "<persisted-output>";
    const TAG_END: &str = "</persisted-output>";
    /// Defensive guard against pathological inputs (deeply nested or
    /// malformed tags). Any real Claude session has at most a handful
    /// per message; values above this likely indicate a corrupt file.
    const MAX_TAGS_PER_MESSAGE: usize = 1024;

    if !content.contains(TAG_START) {
        return content.to_string();
    }

    let mut result = String::new();
    let mut remaining = content;
    let mut iterations = 0usize;

    while let Some(start_pos) = remaining.find(TAG_START) {
        iterations += 1;
        if iterations > MAX_TAGS_PER_MESSAGE {
            log::warn!(
                "resolve_persisted_outputs: bailing after {MAX_TAGS_PER_MESSAGE} tags; \
                 returning remaining content unmodified"
            );
            result.push_str(remaining);
            return result;
        }

        // Add everything before the tag
        result.push_str(&remaining[..start_pos]);

        let after_tag_start = &remaining[start_pos + TAG_START.len()..];
        if let Some(end_pos) = after_tag_start.find(TAG_END) {
            let inner = &after_tag_start[..end_pos];

            // Extract file path from "Full output saved to: /path"
            let file_content = inner
                .lines()
                .find_map(|line| {
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix("Full output saved to: ") {
                        Some(rest.trim().to_string())
                    } else if trimmed.contains("saved to: ") {
                        trimmed
                            .split("saved to: ")
                            .nth(1)
                            .map(|p| p.trim().to_string())
                    } else {
                        None
                    }
                })
                .and_then(|path| {
                    let canonical = match std::fs::canonicalize(&path) {
                        Ok(canonical) => canonical,
                        Err(error) => {
                            log::warn!(
                                "failed to canonicalize Claude full-output path '{path}': {error}"
                            );
                            return None;
                        }
                    };
                    let home = dirs::home_dir()?;
                    let allowed = [home.join(".claude"), home.join(".cc-mirror")];
                    if !allowed.iter().any(|base| {
                        std::fs::canonicalize(base)
                            .ok()
                            .is_some_and(|b| canonical.starts_with(&b))
                    }) {
                        return None;
                    }
                    match std::fs::read_to_string(&canonical) {
                        Ok(content) => Some(content),
                        Err(error) => {
                            log::warn!(
                                "failed to read Claude full-output file '{}': {error}",
                                canonical.display()
                            );
                            None
                        }
                    }
                });

            match file_content {
                Some(full) => result.push_str(&full),
                None => {
                    // Keep the original tag content as fallback
                    result.push_str(TAG_START);
                    result.push_str(inner);
                    result.push_str(TAG_END);
                }
            }

            remaining = &after_tag_start[end_pos + TAG_END.len()..];
        } else {
            // No closing tag found, keep everything as-is
            result.push_str(&remaining[start_pos..]);
            remaining = "";
        }
    }

    result.push_str(remaining);
    result
}

/// Extract text content from a single tool_result block.
/// The `content` field can be a string, an array of text blocks, or absent.
pub(super) fn extract_tool_result_content(result: &Value) -> RenderedToolOutput {
    match result.get("content") {
        Some(Value::String(s)) => RenderedToolOutput::rendered(s.clone()),
        Some(Value::Array(arr)) => {
            let mut parts = Vec::new();
            let mut unsupported = false;
            for item in arr {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        let Some(t) = item.get("text").and_then(Value::as_str) else {
                            unsupported = true;
                            continue;
                        };
                        parts.push(t.to_string());
                    }
                    Some("image") => {
                        let source = item.get("source");
                        let source_type = source
                            .and_then(|s| s.get("type"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("base64");
                        match source_type {
                            "base64" => {
                                let data =
                                    source.and_then(|s| s.get("data")).and_then(|d| d.as_str());
                                let media = source
                                    .and_then(|s| s.get("media_type"))
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("image/png");
                                if let Some(b64) = data {
                                    parts.push(format!(
                                        "[Image: source: data:{};base64,{}]",
                                        media, b64
                                    ));
                                } else {
                                    unsupported = true;
                                }
                            }
                            "url" => {
                                if let Some(url) =
                                    source.and_then(|s| s.get("url")).and_then(|u| u.as_str())
                                {
                                    parts.push(format!("[Image: source: {url}]"));
                                } else {
                                    unsupported = true;
                                }
                            }
                            other => {
                                log::debug!(
                                    "unknown Claude tool_result image source.type '{other}'"
                                );
                                unsupported = true;
                            }
                        }
                    }
                    Some("tool_reference") => {
                        if let Some(name) = item.get("tool_name").and_then(Value::as_str) {
                            parts.push(format!("[Tool: {name}]"));
                        } else {
                            unsupported = true;
                        }
                    }
                    Some(other) => {
                        log::debug!(
                            "preserving Claude tool_result with unknown content block type '{other}' as raw"
                        );
                        unsupported = true;
                    }
                    None => unsupported = true,
                }
            }
            if unsupported {
                RenderedToolOutput::raw(serde_json::to_string(arr).unwrap_or_default())
            } else {
                RenderedToolOutput::rendered(parts.join("\n"))
            }
        }
        Some(value) => RenderedToolOutput::raw(serde_json::to_string(value).unwrap_or_default()),
        None => RenderedToolOutput::rendered(String::new()),
    }
}
