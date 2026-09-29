use serde::Deserialize;

use crate::models::TokenUsage;

/// The on-disk shape of a single `messages.jsonl` line.
#[derive(Debug, Deserialize)]
pub(crate) struct WireLine {
    pub message_id: String,
    pub message: WireMessage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireMessage {
    pub role: String,
    #[serde(default)]
    pub content: Vec<WireContent>,
    pub model: Option<String>,
    pub usage: Option<WireUsage>,
    pub timestamp: Option<i64>,
    pub canonical_text_range: Option<CanonicalTextRange>,
    // toolResult-only:
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub is_error: Option<bool>,
    /// Typed payload on `task` (and similar) results. Carries
    /// `sub_session_id` for the hidden child Agent.
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

/// Character offsets into the concatenated text parts of a user turn.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CanonicalTextRange {
    pub start_offset: usize,
    pub end_offset: usize,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum WireContent {
    Known(KnownWireContent),
    Unknown(serde_json::Value),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum KnownWireContent {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    ToolCall {
        id: String,
        name: String,
        /// Tool arguments arrive as either an object (newer sessions) or a
        /// raw JSON-encoded string (legacy / partial-stream chunks). Accept
        /// both without forcing the parser to pick.
        arguments: serde_json::Value,
    },
    Image {
        #[serde(default)]
        data: Option<String>,
        #[serde(default, rename = "mimeType")]
        mime_type: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WireUsage {
    #[serde(default)]
    pub input: u32,
    #[serde(default)]
    pub output: u32,
    #[serde(default, rename = "cacheRead")]
    pub cache_read: u32,
    #[serde(default, rename = "cacheWrite")]
    pub cache_write: u32,
    #[serde(default)]
    pub cost: Option<WireCost>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WireCost {
    #[serde(default)]
    pub total: f64,
}

impl WireUsage {
    pub(super) fn into_token_usage(self) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input,
            output_tokens: self.output,
            cache_creation_input_tokens: self.cache_write,
            cache_read_input_tokens: self.cache_read,
        }
    }
}
