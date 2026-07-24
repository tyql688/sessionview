//! Serde types for grok's on-disk JSON plus content-block flattening.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(super) struct GrokSummary {
    pub(super) info: GrokSummaryInfo,
    #[serde(default)]
    pub(super) session_summary: Option<String>,
    #[serde(default)]
    pub(super) generated_title: Option<String>,
    #[serde(default)]
    pub(super) created_at: Option<String>,
    #[serde(default)]
    pub(super) updated_at: Option<String>,
    #[serde(default)]
    pub(super) current_model_id: Option<String>,
    /// `"subagent"` or `"subagent_fork"` on child sessions.
    #[serde(default)]
    pub(super) session_kind: Option<String>,
    /// Git branch recorded when the session was created / last refreshed.
    #[serde(default)]
    pub(super) head_branch: Option<String>,
    /// Active agent / persona name (e.g. `general-purpose`, `grok-build-plan`).
    #[serde(default)]
    pub(super) agent_name: Option<String>,
    /// Direct parent id for forks (also recoverable via subagents/*/meta.json).
    #[serde(default)]
    pub(super) parent_session_id: Option<String>,
}

/// Parent-side subagent link: `<parent-dir>/subagents/<child-id>/meta.json`.
#[derive(Debug, Deserialize)]
pub(super) struct GrokSubagentMeta {
    #[serde(default)]
    pub(super) parent_session_id: Option<String>,
    #[serde(default)]
    pub(super) child_session_id: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GrokSummaryInfo {
    pub(super) id: String,
    #[serde(default)]
    pub(super) cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum GrokChatEntry {
    System {},
    User {
        content: GrokContent,
        #[serde(default)]
        prompt_index: Option<Value>,
        #[serde(default)]
        synthetic_reason: Option<String>,
    },
    Reasoning {
        #[serde(default)]
        summary: Vec<GrokReasoningSummary>,
    },
    Assistant {
        #[serde(default)]
        content: String,
        #[serde(default)]
        model_id: Option<String>,
        #[serde(default)]
        tool_calls: Vec<GrokToolCall>,
    },
    ToolResult {
        tool_call_id: String,
        #[serde(default)]
        content: String,
    },
    /// Built-in server-side tools (web_search / x_search) that do not go
    /// through the regular `assistant.tool_calls` + `tool_result` pair.
    BackendToolCall {
        kind: GrokBackendToolKind,
    },
    #[serde(other)]
    Unknown,
}

/// Payload of a `backend_tool_call` chat entry.
#[derive(Debug, Deserialize)]
pub(super) struct GrokBackendToolKind {
    #[serde(default)]
    pub(super) tool_type: Option<String>,
    #[serde(default)]
    pub(super) call_id: Option<String>,
    /// JSON-encoded argument string, same shape as assistant tool_calls.
    #[serde(default)]
    pub(super) input: Option<String>,
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Stable id that matches `updates.jsonl` `toolCallId`.
    #[serde(default)]
    pub(super) id: Option<String>,
    /// Web-search embeds the completed action (query + sources) on the
    /// chat_history entry itself — not only in updates.jsonl.
    #[serde(default)]
    pub(super) action: Option<Value>,
    #[serde(default)]
    pub(super) status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GrokReasoningSummary {
    #[serde(default)]
    pub(super) text: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct GrokToolCall {
    pub(super) id: String,
    pub(super) name: String,
    /// JSON-encoded string, e.g. `"{\"command\":\"ls\"}"`.
    #[serde(default)]
    pub(super) arguments: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum GrokContent {
    Text(String),
    Blocks(Vec<GrokContentBlock>),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum GrokContentBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    Image {},
    #[serde(other)]
    Unknown,
}

pub(super) fn prompt_index_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Concatenated text blocks without any unwrapping — used to classify an
/// entry (e.g. detect the `<user_query>` wrapper) before display formatting.
pub(super) fn content_text_raw(content: &GrokContent) -> String {
    match content {
        GrokContent::Text(text) => text.clone(),
        GrokContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                GrokContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Flatten a user prompt's blocks to display text. Image prompts arrive as
/// an `<image_files>` list of saved asset paths plus base64 `image` blocks:
/// pair each image with its path and emit `[Image: source: <path>]`
/// (unpairable images degrade to `[Image]`); the list text itself is noise.
pub(super) fn user_content_to_text(content: &GrokContent) -> String {
    let blocks = match content {
        GrokContent::Text(text) => return strip_user_query_wrapper(text),
        GrokContent::Blocks(blocks) => blocks,
    };

    let mut image_paths = blocks
        .iter()
        .filter_map(|block| match block {
            GrokContentBlock::Text { text } if text.trim_start().starts_with("<image_files>") => {
                Some(extract_image_file_paths(text))
            }
            _ => None,
        })
        .flatten();

    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        match block {
            GrokContentBlock::Text { text } => {
                if text.trim_start().starts_with("<image_files>") || text.is_empty() {
                    continue;
                }
                parts.push(strip_user_query_wrapper(text));
            }
            GrokContentBlock::Image {} => match image_paths.next() {
                Some(path) => parts.push(format!("[Image: source: {path}]")),
                None => parts.push("[Image]".to_string()),
            },
            GrokContentBlock::Unknown => {}
        }
    }
    parts.join("\n")
}

/// Pull the saved-asset paths out of an `<image_files>` block: numbered
/// list lines like `1. /path/to/assets/image-<uuid>.png`.
pub(super) fn extract_image_file_paths(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let (index, rest) = trimmed.split_once(". ")?;
            if !index.chars().all(|c| c.is_ascii_digit()) || index.is_empty() {
                return None;
            }
            let path = rest.trim();
            path.starts_with('/').then(|| path.to_string())
        })
        .collect()
}

pub(super) fn strip_user_query_wrapper(text: &str) -> String {
    let trimmed = text.trim();
    trimmed
        .strip_prefix("<user_query>")
        .and_then(|rest| rest.strip_suffix("</user_query>"))
        .map(str::trim)
        .unwrap_or(trimmed)
        .to_string()
}
