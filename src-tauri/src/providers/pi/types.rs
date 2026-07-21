use serde::{Deserialize, Serialize};

/// Pi session entry types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PiEntry {
    #[serde(rename = "session")]
    Session(PiSessionHeader),
    #[serde(rename = "message")]
    Message(PiMessageEntry),
    #[serde(rename = "model_change")]
    ModelChange(PiModelChangeEntry),
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange(PiThinkingLevelChangeEntry),
    #[serde(rename = "compaction")]
    Compaction(PiCompactionEntry),
    #[serde(rename = "branch_summary")]
    BranchSummary(PiBranchSummaryEntry),
    #[serde(rename = "custom")]
    Custom(PiCustomEntry),
    #[serde(rename = "custom_message")]
    CustomMessage(PiCustomMessageEntry),
    #[serde(rename = "label")]
    Label(PiLabelEntry),
    #[serde(rename = "session_info")]
    SessionInfo(PiSessionInfoEntry),
}

/// Session header (first line of JSONL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiSessionHeader {
    #[serde(default = "default_session_version")]
    pub version: u32,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    #[serde(rename = "parentSession")]
    #[serde(default)]
    pub parent_session: Option<String>,
}

fn default_session_version() -> u32 {
    1
}

/// Base for all entries (except header)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiEntryBase {
    pub id: String,
    #[serde(rename = "parentId")]
    #[serde(default)]
    pub parent_id: Option<String>,
    pub timestamp: String,
}

/// Message entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiMessageEntry {
    #[serde(flatten)]
    pub base: PiEntryBase,
    pub message: PiAgentMessage,
}

/// Model change entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiModelChangeEntry {
    #[serde(flatten)]
    pub base: PiEntryBase,
    pub provider: String,
    #[serde(rename = "modelId")]
    pub model_id: String,
}

/// Thinking level change entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiThinkingLevelChangeEntry {
    #[serde(flatten)]
    pub base: PiEntryBase,
    #[serde(rename = "thinkingLevel")]
    pub thinking_level: String,
}

/// Compaction entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiCompactionEntry {
    #[serde(flatten)]
    pub base: PiEntryBase,
    pub summary: String,
    #[serde(rename = "firstKeptEntryId")]
    pub first_kept_entry_id: Option<String>,
    #[serde(rename = "firstKeptEntryIndex")]
    pub first_kept_entry_index: Option<usize>,
    #[serde(rename = "tokensBefore")]
    pub tokens_before: Option<u64>,
}

/// Branch summary entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiBranchSummaryEntry {
    #[serde(flatten)]
    pub base: PiEntryBase,
    pub summary: String,
    #[serde(rename = "fromId")]
    pub from_id: String,
}

/// Custom entry (extension state, not in LLM context)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiCustomEntry {
    #[serde(flatten)]
    pub base: PiEntryBase,
    #[serde(rename = "customType")]
    pub custom_type: String,
    pub data: Option<serde_json::Value>,
}

/// Custom message entry (extension message, in LLM context)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiCustomMessageEntry {
    #[serde(flatten)]
    pub base: PiEntryBase,
    #[serde(rename = "customType")]
    pub custom_type: String,
    pub content: PiContent,
    pub display: bool,
}

/// Label entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiLabelEntry {
    #[serde(flatten)]
    pub base: PiEntryBase,
    #[serde(rename = "targetId")]
    pub target_id: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// Session info entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiSessionInfoEntry {
    #[serde(flatten)]
    pub base: PiEntryBase,
    #[serde(default)]
    pub name: Option<String>,
}

/// Agent message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum PiAgentMessage {
    #[serde(rename = "user")]
    User(PiUserMessage),
    #[serde(rename = "assistant")]
    Assistant(PiAssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(PiToolResultMessage),
    #[serde(rename = "bashExecution")]
    BashExecution(PiBashExecutionMessage),
    #[serde(rename = "custom")]
    Custom(PiCustomMessage),
    #[serde(rename = "branchSummary")]
    BranchSummary(PiBranchSummaryMessage),
    #[serde(rename = "compactionSummary")]
    CompactionSummary(PiCompactionSummaryMessage),
}

/// User message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiUserMessage {
    pub content: PiContent,
    pub timestamp: u64,
}

/// Assistant message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiAssistantMessage {
    pub content: Vec<PiContentBlock>,
    pub api: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub usage: Option<PiUsage>,
    #[serde(rename = "stopReason")]
    pub stop_reason: Option<String>,
    #[serde(rename = "errorMessage")]
    pub error_message: Option<String>,
    pub timestamp: u64,
}

/// Tool result message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiToolResultMessage {
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub content: Vec<PiContentBlock>,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
    #[serde(rename = "isError")]
    pub is_error: bool,
    pub timestamp: u64,
}

/// Bash execution message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiBashExecutionMessage {
    pub command: String,
    pub output: String,
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    #[serde(rename = "fullOutputPath")]
    pub full_output_path: Option<String>,
    #[serde(rename = "excludeFromContext")]
    pub exclude_from_context: Option<bool>,
    pub timestamp: u64,
}

/// Custom message (extension)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiCustomMessage {
    #[serde(rename = "customType")]
    pub custom_type: String,
    pub content: PiContent,
    pub display: bool,
    pub timestamp: u64,
}

/// Branch summary message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiBranchSummaryMessage {
    pub summary: String,
    #[serde(rename = "fromId")]
    pub from_id: String,
    pub timestamp: u64,
}

/// Compaction summary message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiCompactionSummaryMessage {
    pub summary: String,
    #[serde(rename = "tokensBefore")]
    pub tokens_before: u64,
    pub timestamp: u64,
}

/// Content (string or array of content blocks)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PiContent {
    Text(String),
    Blocks(Vec<PiContentBlock>),
}

/// Content block types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PiContentBlock {
    Known(PiKnownContentBlock),
    Unknown(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PiKnownContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType", alias = "mime_type")]
        mime_type: String,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "toolCall")]
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
}

/// Usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiUsage {
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: u64,
    #[serde(rename = "totalTokens")]
    pub total_tokens: u64,
    pub cost: Option<PiCost>,
}

/// Cost breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiCost {
    pub input: f64,
    pub output: f64,
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
    pub total: f64,
}
