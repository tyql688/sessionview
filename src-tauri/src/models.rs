use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
    Antigravity,
    #[serde(rename = "opencode")]
    OpenCode,
    Kimi,
    Cursor,
    #[serde(rename = "cc-mirror")]
    CcMirror,
    Pi,
    Grok,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub provider: Provider,
    pub title: String,
    pub project_path: String,
    pub project_name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: u32,
    pub file_size_bytes: u64,
    pub source_path: String,
    pub is_sidechain: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    CommandInput,
    CommandOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}

impl TokenUsage {
    pub fn total_tokens(&self) -> u64 {
        u64::from(self.input_tokens)
            + u64::from(self.output_tokens)
            + u64::from(self.cache_creation_input_tokens)
            + u64::from(self.cache_read_input_tokens)
    }
}

impl TokenTotals {
    pub fn add_usage(&mut self, usage: &TokenUsage) {
        self.input_tokens += u64::from(usage.input_tokens);
        self.output_tokens += u64::from(usage.output_tokens);
        self.cache_read_tokens += u64::from(usage.cache_read_input_tokens);
        self.cache_write_tokens += u64::from(usage.cache_creation_input_tokens);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpToolMetadata {
    pub server: String,
    pub tool: String,
    pub display: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RawOutputPolicy {
    #[default]
    Keep,
    SuppressTerminal,
    SuppressPatchWhenDiffPresent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolPresentation {
    pub icon: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_detail: Option<ToolDetail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_detail: Option<ToolDetail>,
    #[serde(default)]
    pub raw_output_policy: RawOutputPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetail {
    pub lines: Vec<ToolLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<ToolInlineDiff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch_diff: Option<Vec<ToolDiffLine>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persisted_output_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolLine {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolInlineDiff {
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ToolDiffLineType {
    Context,
    Add,
    Remove,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolDiffLine {
    #[serde(rename = "type")]
    pub kind: ToolDiffLineType,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolMetadata {
    pub raw_name: String,
    pub canonical_name: String,
    pub display_name: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ids: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpToolMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation: Option<ToolPresentation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_kind: Option<MessageKind>,
    pub content: String,
    pub timestamp: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_metadata: Option<ToolMetadata>,
    pub token_usage: Option<TokenUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `messageId:requestId` hash for cross-file usage deduplication.
    ///
    /// `None` means the source provider does not expose a stable `(messageId, requestId)`
    /// pair — this is the norm for Codex / Antigravity / Kimi, where
    /// sessions are not split across files and usage rows cannot collide. Only Claude and
    /// OpenCode populate `Some(..)`. `None` here is *not* the CLAUDE.md "placeholder when a
    /// real value should be computed" antipattern — it is an explicit "unsupported" marker
    /// and `indexer.rs::compute_token_stats` simply skips dedup for those rows.
    #[serde(skip, default)]
    pub usage_hash: Option<String>,
}

impl Message {
    /// Construct a message with the given role and content; all optional
    /// fields default to `None`. Use the struct-update syntax
    /// (`Message { timestamp: Some(t), ..Message::assistant(content) }`)
    /// to override individual fields.
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            message_kind: None,
            content: content.into(),
            timestamp: None,
            tool_name: None,
            tool_input: None,
            tool_metadata: None,
            token_usage: None,
            model: None,
            usage_hash: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(MessageRole::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(MessageRole::Assistant, content)
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new(MessageRole::System, content)
    }

    pub fn command_input(content: impl Into<String>) -> Self {
        Self {
            message_kind: Some(MessageKind::CommandInput),
            ..Self::user(content)
        }
    }

    pub fn command_output(content: impl Into<String>) -> Self {
        Self {
            message_kind: Some(MessageKind::CommandOutput),
            ..Self::assistant(content)
        }
    }
}

pub fn token_totals_from_messages(messages: &[Message]) -> TokenTotals {
    let mut totals = TokenTotals::default();
    for message in dedup_usage_messages(messages) {
        if !message_counts_for_usage(message) {
            continue;
        }
        if let Some(usage) = &message.token_usage {
            totals.add_usage(usage);
        }
    }
    totals
}

/// Collapse usage-bearing messages that share a `usage_hash`, keeping the
/// entry with the largest token total per hash.
///
/// Claude Code streams cumulative usage: one API call's usage appears on
/// several JSONL lines (same `messageId:requestId`), each carrying the
/// running total, so the largest entry is the call's final total. Keeping
/// the first entry instead would undercount output tokens. Messages
/// without a hash are kept as-is.
pub fn dedup_usage_messages(messages: &[Message]) -> Vec<&Message> {
    let mut best_index: HashMap<&str, usize> = HashMap::new();
    let mut deduped: Vec<&Message> = Vec::new();
    for message in messages {
        let Some(usage) = &message.token_usage else {
            continue;
        };
        let Some(hash) = message.usage_hash.as_deref() else {
            deduped.push(message);
            continue;
        };
        match best_index.get(hash) {
            Some(&index) => {
                let kept_total = deduped[index]
                    .token_usage
                    .as_ref()
                    .map_or(0, TokenUsage::total_tokens);
                if usage.total_tokens() > kept_total {
                    deduped[index] = message;
                }
            }
            None => {
                best_index.insert(hash, deduped.len());
                deduped.push(message);
            }
        }
    }
    deduped
}

fn message_counts_for_usage(message: &Message) -> bool {
    if message.token_usage.is_none() {
        return false;
    }
    if message.timestamp.as_deref().is_none_or(str::is_empty) {
        return false;
    }
    matches!(
        message.model.as_deref(),
        Some(model) if !model.is_empty() && model != "<synthetic>"
    )
}

#[cfg(test)]
mod tests {
    use super::{Message, MessageRole, TokenTotals, TokenUsage, token_totals_from_messages};

    fn message(timestamp: Option<&str>, model: Option<&str>, usage: TokenUsage) -> Message {
        message_with_hash(timestamp, model, usage, None)
    }

    fn message_with_hash(
        timestamp: Option<&str>,
        model: Option<&str>,
        usage: TokenUsage,
        usage_hash: Option<&str>,
    ) -> Message {
        Message {
            role: MessageRole::Assistant,
            message_kind: None,
            content: String::new(),
            timestamp: timestamp.map(str::to_string),
            tool_name: None,
            tool_input: None,
            tool_metadata: None,
            token_usage: Some(usage),
            model: model.map(str::to_string),
            usage_hash: usage_hash.map(str::to_string),
        }
    }

    fn usage(input: u32, output: u32, cache_read: u32, cache_write: u32) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_write,
        }
    }

    #[test]
    fn token_totals_match_usage_indexing_requirements() {
        let totals = token_totals_from_messages(&[
            message(
                Some("2026-04-10T10:00:00Z"),
                Some("claude-opus-4-6"),
                usage(100, 50, 20, 10),
            ),
            message(None, Some("claude-opus-4-6"), usage(1, 1, 1, 1)),
            message(Some("2026-04-10T10:00:00Z"), None, usage(1, 1, 1, 1)),
            message(
                Some("2026-04-10T10:00:00Z"),
                Some("<synthetic>"),
                usage(1, 1, 1, 1),
            ),
            message_with_hash(
                Some("2026-04-10T10:00:01Z"),
                Some("claude-opus-4-6"),
                usage(7, 3, 2, 1),
                Some("msg-1:req-1"),
            ),
            message_with_hash(
                Some("2026-04-10T10:00:02Z"),
                Some("claude-opus-4-6"),
                usage(700, 300, 200, 100),
                Some("msg-1:req-1"),
            ),
        ]);

        assert_eq!(
            totals,
            TokenTotals {
                input_tokens: 800,
                output_tokens: 350,
                cache_read_tokens: 220,
                cache_write_tokens: 110,
            }
        );
    }

    #[test]
    fn token_totals_keep_final_cumulative_usage_per_hash() {
        // Claude Code streams cumulative usage: the lines of one API call
        // share a usage_hash and the output count grows line over line.
        // The largest entry (the call's final total) must win regardless
        // of line order.
        let totals = token_totals_from_messages(&[
            message_with_hash(
                Some("2026-06-07T10:00:00Z"),
                Some("claude-opus-4-8"),
                usage(100, 5, 1000, 50),
                Some("msg-1:req-1"),
            ),
            message_with_hash(
                Some("2026-06-07T10:00:01Z"),
                Some("claude-opus-4-8"),
                usage(100, 480, 1000, 50),
                Some("msg-1:req-1"),
            ),
            message_with_hash(
                Some("2026-06-07T10:00:02Z"),
                Some("claude-opus-4-8"),
                usage(100, 60, 1000, 50),
                Some("msg-1:req-1"),
            ),
        ]);

        assert_eq!(
            totals,
            TokenTotals {
                input_tokens: 100,
                output_tokens: 480,
                cache_read_tokens: 1000,
                cache_write_tokens: 50,
            }
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    pub meta: SessionMeta,
    pub messages: Vec<Message>,
    /// Number of per-line parse warnings surfaced while loading this session
    /// (malformed JSONL lines or JSON fields the provider parser had to skip).
    /// Populated by `commands::sessions::load_detail` from
    /// `LoadedSession.parse_warning_count`; not persisted.
    ///
    /// Zero means either the session parsed cleanly or the provider parser
    /// has not yet wired per-record counting.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub parse_warning_count: u32,
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub label: String,
    pub node_type: TreeNodeType,
    pub children: Vec<TreeNode>,
    pub count: u32,
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_sidechain: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TreeNodeType {
    Provider,
    Project,
    Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub session: SessionMeta,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub session_count: u64,
    pub db_size_bytes: u64,
    pub last_index_time: String,
    pub usage_last_refreshed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingCatalogStatus {
    pub updated_at: Option<String>,
    pub model_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub key: Provider,
    pub label: String,
    pub color: String,
    pub sort_order: u32,
    pub path: String,
    pub exists: bool,
    pub session_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchFilters {
    pub query: String,
    pub provider: Option<String>,
    pub project: Option<String>,
    pub after: Option<i64>,
    pub before: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    pub total_sessions: u64,
    pub total_turns: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_write_tokens: u64,
    pub total_cost: f64,
    pub cache_hit_rate: f64,
    pub daily_usage: Vec<DailyUsage>,
    pub model_costs: Vec<ModelCost>,
    pub project_costs: Vec<ProjectCost>,
    pub recent_sessions: Vec<SessionCostRow>,
    /// Session counts per provider, filtered by the current date range.
    pub provider_session_counts: Vec<ProviderSessionCount>,
    /// Previous period totals for trend comparison (None when range is "All"
    /// or when insufficient historical data exists).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_period: Option<PrevPeriodTotals>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSessionCount {
    pub provider: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrevPeriodTotals {
    pub total_sessions: u64,
    pub total_turns: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyUsage {
    pub date: String,
    pub provider: String,
    pub tokens: u64,
    pub cost: f64,
}

/// GitHub-style activity calendar: per-day aggregates over a date window plus
/// the set of years that have any data (drives the year selector). The grid
/// layout (week alignment, gap-filling, intensity buckets) is computed on the
/// frontend; the backend only aggregates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityCalendar {
    /// Days with at least one record, ascending by date. Days with no activity
    /// are absent — the frontend fills the gaps when laying out the grid.
    pub days: Vec<ActivityDay>,
    /// Distinct calendar years (descending) that have any data for the selected
    /// providers, ignoring the requested date window.
    pub available_years: Vec<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityDay {
    pub date: String,
    /// Distinct sessions active on this day.
    pub sessions: u64,
    pub turns: u64,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCost {
    pub model: String,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectCost {
    pub project: String,
    pub project_path: String,
    /// Every provider (Claude Code, Codex, ...) that contributed usage to this
    /// project, sorted. Usage below is summed across all of them.
    pub providers: Vec<String>,
    /// Per-provider breakdown for this project (sorted by cost desc), so the
    /// merged row can be expanded to show how much each tool contributed.
    pub by_provider: Vec<ProjectProviderUsage>,
    /// Per-model breakdown for this project (sorted by cost desc), used by the
    /// folder analytics detail view.
    pub by_model: Vec<ProjectModelUsage>,
    pub sessions: u64,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectProviderUsage {
    pub provider: String,
    pub sessions: u64,
    pub turns: u64,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectModelUsage {
    pub model: String,
    pub sessions: u64,
    pub turns: u64,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectToolUsageStats {
    pub project_path: String,
    pub sessions_scanned: u64,
    pub tool_calls: u64,
    pub tools: Vec<ProjectToolUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectToolUsage {
    pub key: String,
    pub label: String,
    pub category: String,
    pub count: u64,
    pub sessions: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDailyUsage {
    pub date: String,
    pub provider: String,
    pub model: String,
    pub sessions: u64,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCostRow {
    pub id: String,
    pub project: String,
    pub project_path: String,
    pub provider: String,
    pub model: String,
    pub updated_at: i64,
    pub turns: u64,
    pub tokens: u64,
    pub cost: f64,
}
