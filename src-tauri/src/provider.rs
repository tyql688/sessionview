use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::models::{
    token_totals_from_messages, Message, Provider, SessionMeta, TokenTotals, TrashMeta,
};
use crate::pricing::{self, PricingCatalog};

mod catalog;
mod trash;

pub use catalog::*;
pub use trash::*;

// ---------------------------------------------------------------------------
// Deletion plan types — provider returns a plan, command layer executes it
// ---------------------------------------------------------------------------

/// How the frontend should watch for changes from this provider.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchStrategy {
    Fs,
    Poll,
}

/// What to do with a session's source file during deletion.
#[derive(Debug, Clone, PartialEq)]
pub enum FileAction {
    /// Move/delete the source file (dedicated file per session).
    Remove,
    /// Shared source — don't touch the file.
    /// On permanent delete, call `purge_from_source()`.
    Shared,
    /// Don't touch the file and no purge needed
    /// (e.g. child session embedded in parent's file).
    Skip,
}

/// How to restore a trashed session.
#[derive(Debug, Clone, PartialEq)]
pub enum RestoreAction {
    /// Move the trash file back to original_path.
    MoveBack,
    /// Remove from shared_deletions tracking, then re-sync source.
    UndoSharedDeletion,
    /// Nothing to restore (embedded child — parent restore handles it).
    Noop,
}

/// Plan for deleting a child session.
#[derive(Debug, Clone)]
pub struct ChildPlan {
    pub id: String,
    pub source_path: String,
    pub title: String,
    pub file_action: FileAction,
}

/// Complete deletion plan returned by provider.
/// Command layer executes this mechanically — zero provider logic.
#[derive(Debug, Clone)]
pub struct DeletionPlan {
    pub file_action: FileAction,
    pub child_plans: Vec<ChildPlan>,
    /// Extra directories to remove after file operations.
    pub cleanup_dirs: Vec<PathBuf>,
}

#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// File-level snapshot the indexer uses to decide whether a session
/// needs to be re-parsed on the next scan. Mirrors what the `sessions`
/// table stores per-row in `(file_size_bytes, source_mtime)`.
///
/// `mtime` is epoch seconds (i64) — matches `SystemTime → UNIX_EPOCH`
/// duration and survives JSON / SQLite roundtrips without precision
/// loss for the resolution we care about (sub-second changes always
/// also bump file size on append-only JSONL).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceState {
    pub size: u64,
    pub mtime: i64,
}

/// Returned by `SessionProvider::scan_all`. `parsed` carries the
/// sessions the provider actually re-parsed; `unchanged_source_paths`
/// carries the file paths whose `(size, mtime)` matched the `known`
/// snapshot passed in and were therefore skipped. The indexer combines
/// both lists when deciding which DB rows are still live (so an
/// unchanged session isn't accidentally pruned just because it doesn't
/// appear in `parsed`).
#[derive(Default)]
pub struct ScanOutcome {
    pub parsed: Vec<ParsedSession>,
    pub unchanged_source_paths: Vec<String>,
}

/// Convert a `SystemTime` into the integer epoch-seconds form we store
/// in `sessions.source_mtime`. Returns `None` for clocks before the
/// UNIX epoch (effectively never on real systems).
pub fn system_time_to_epoch_seconds(t: std::time::SystemTime) -> Option<i64> {
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
}

/// Split a flat list of JSONL paths into "must reparse" vs "unchanged".
/// JSONL providers' `scan_incremental` calls this before kicking off the
/// rayon parse pipeline so only stale files actually hit the parser.
/// The unchanged list is returned as the path strings the DB stored,
/// matching what `Database::source_states_for_provider` keys on.
pub fn partition_files_by_freshness(
    files: Vec<PathBuf>,
    known: &HashMap<String, SourceState>,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut to_parse = Vec::with_capacity(files.len());
    let mut unchanged = Vec::new();
    for file in files {
        let path_str = file.to_string_lossy().to_string();
        match known.get(&path_str) {
            Some(state) if source_state_matches(&file, state) => unchanged.push(path_str),
            _ => to_parse.push(file),
        }
    }
    (to_parse, unchanged)
}

/// Stat `path` and decide whether the on-disk file still matches the
/// `(size, mtime)` we recorded in the DB. Used by JSONL providers'
/// `scan_all` override to skip parsing unchanged files.
pub fn source_state_matches(path: &Path, known: &SourceState) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() != known.size {
        return false;
    }
    let Some(mtime) = meta.modified().ok().and_then(system_time_to_epoch_seconds) else {
        return false;
    };
    mtime == known.mtime
}

/// A single per-(date, model) token-usage row, written to
/// `session_token_stats` by the indexer. Defined here so the provider
/// trait can produce them without depending on `db::sync`.
#[derive(Clone, Debug)]
pub struct TokenStatRow {
    pub date: String,
    pub model: String,
    pub turn_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

/// A single out-of-band token-usage event captured during parse.
///
/// Codex emits per-turn token counts as `event_msg.token_count` lines
/// that aren't attached to any single message — the indexer's per-date
/// aggregation reads from this Vec instead of re-opening the file.
/// Populated only by the Codex parser today; the shape is generic
/// enough that future providers with similar out-of-band usage streams
/// can reuse the slot.
#[derive(Clone, Debug)]
pub struct UsageEvent {
    pub timestamp: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
}

#[derive(Clone)]
pub struct ParsedSession {
    pub meta: SessionMeta,
    pub messages: Vec<Message>,
    pub content_text: String,
    /// Number of per-line / per-record parse warnings (malformed JSONL lines,
    /// JSON fields that couldn't be decoded, etc.) encountered while parsing
    /// this session. File-level failures (can't open, file-wide JSON damage)
    /// are surfaced as `Err` instead. Zero when the parser hasn't been wired
    /// for per-record counting yet.
    pub parse_warning_count: u32,
    /// Conversation IDs of subagents this session explicitly invoked.
    ///
    /// Populated only by providers that surface structured parent→child links
    /// in the transcript itself (today: Antigravity's `INVOKE_SUBAGENT` step
    /// type). `db/sync.rs::upsert_session_on` uses this to back-fill
    /// `parent_id` / `is_sidechain` / inherited project metadata on already-
    /// indexed child rows. Empty for every other provider.
    pub child_session_ids: Vec<String>,
    /// Out-of-band token-usage events emitted by the provider's transcript
    /// (e.g. Codex `event_msg.token_count`). Consumed by the provider's
    /// own `compute_token_stats` override; empty for providers whose token
    /// counts are attached to messages directly.
    pub usage_events: Vec<UsageEvent>,
    /// Last-modified epoch seconds of the source file at parse time.
    /// 0 means "unknown" — the indexer treats that as "always reparse".
    /// Used together with `meta.file_size_bytes` to short-circuit
    /// unchanged files in `scan_incremental`. Not exposed to the
    /// frontend; never read outside the indexer / sync layer.
    pub source_mtime: i64,
}

/// Materialized session payload returned by `SessionProvider::load_messages`
/// at display time. Carries the parser's per-record warning count so the UI
/// can show a ⚠ badge when the session contains malformed lines the parser
/// had to skip.
///
/// `Deref<Target = Vec<Message>>` is implemented so tests and callers that
/// only care about the messages (e.g. `.iter()`, `.len()`, indexing) can treat
/// a `LoadedSession` as if it were a `Vec<Message>`. Call sites that need the
/// warning count access the field directly.
#[derive(Debug, Clone)]
pub struct LoadedSession {
    pub messages: Vec<Message>,
    pub parse_warning_count: u32,
    pub token_totals: TokenTotals,
}

impl LoadedSession {
    pub fn new(messages: Vec<Message>) -> Self {
        Self::from_messages(messages, 0)
    }

    pub fn from_messages(messages: Vec<Message>, parse_warning_count: u32) -> Self {
        let token_totals = token_totals_from_messages(&messages);
        Self {
            messages,
            parse_warning_count,
            token_totals,
        }
    }

    pub fn from_parsed(parsed: ParsedSession) -> Self {
        Self::from_messages(parsed.messages, parsed.parse_warning_count)
    }
}

impl std::ops::Deref for LoadedSession {
    type Target = Vec<Message>;

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}

/// Static metadata for a provider. Implemented by zero-sized descriptor structs
/// in each provider module. Accessed via `Provider::descriptor()`.
pub trait ProviderDescriptor: Send + Sync {
    /// Check if a source file path belongs to this provider.
    fn owns_source_path(&self, source_path: &str) -> bool;

    /// Build the CLI resume command for a session.
    fn resume_command(&self, session_id: &str, variant_name: Option<&str>) -> Option<String>;

    /// Key used to group sessions in the tree.
    fn display_key(&self, variant_name: Option<&str>) -> String;

    /// Try to parse a display key as belonging to this provider.
    /// Returns the display label if the key matches a custom format.
    /// Default: None (handled by Provider::parse fallback).
    fn try_parse_display_key(&self, _display_key: &str) -> Option<String> {
        None
    }

    /// Sort order for provider groups in the tree.
    fn sort_order(&self) -> u32;

    /// Provider brand color (hex).
    fn color(&self) -> &'static str;

    /// CLI command name for the security whitelist (e.g. "claude", "agent").
    /// Empty string if dynamic (e.g. cc-mirror variants).
    fn cli_command(&self) -> &'static str;

    /// SVG icon for HTML export. Returns a complete `<svg>` element or empty string.
    fn avatar_svg(&self) -> &'static str {
        ""
    }

    /// How the frontend should watch for session changes from this provider.
    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Fs
    }
}

pub trait SessionProvider: Send + Sync {
    fn provider(&self) -> Provider;
    fn watch_paths(&self) -> Vec<PathBuf>;
    /// Directories the provider wants watched non-recursively — the
    /// watcher fires on entries created/removed at the dir's top level
    /// but doesn't follow subdirs. Use this for parent dirs whose
    /// children mutate rapidly under concurrent external processes
    /// (e.g. SQLite WAL/SHM churn), where a recursive watch would race
    /// the OS file-watcher's internal fd tracking. Default empty.
    fn watch_paths_shallow(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError>;

    /// Incremental scan: parse only the source files whose
    /// `(size, mtime)` differs from what's stored in `known`, and return
    /// the rest as `unchanged_source_paths` so the indexer can preserve
    /// their DB rows without re-upserting.
    ///
    /// Default implementation parses everything (matches `scan_all`) —
    /// providers whose data lives in per-session files override this to
    /// take advantage of the snapshot. Providers backed by a single
    /// database file (OpenCode) inherit the default since per-file
    /// mtime is meaningless for them.
    fn scan_incremental(
        &self,
        _known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        Ok(ScanOutcome {
            parsed: self.scan_all()?,
            unchanged_source_paths: Vec::new(),
        })
    }

    fn scan_source(&self, source_path: &str) -> Result<Vec<ParsedSession>, ProviderError> {
        Ok(self
            .scan_all()?
            .into_iter()
            .filter(|session| session.meta.source_path == source_path)
            .collect())
    }
    fn load_messages(
        &self,
        session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError>;

    /// Aggregate per-(date, model) token-usage rows for the indexer.
    ///
    /// Default implementation walks `parsed.messages[].token_usage` and
    /// dedups via `seen_hashes` against `Message.usage_hash`. Providers
    /// whose token counts arrive out-of-band (e.g. Codex's
    /// `event_msg.token_count` lines) should override and aggregate from
    /// `parsed.usage_events` instead.
    fn compute_token_stats(
        &self,
        parsed: &ParsedSession,
        pricing_catalog: Option<&PricingCatalog>,
        seen_hashes: Option<&mut HashSet<String>>,
    ) -> Vec<TokenStatRow> {
        default_compute_token_stats_from_messages(parsed, pricing_catalog, seen_hashes)
    }

    /// Return a deletion plan for this session.
    /// Provider decides all file actions; command layer executes mechanically.
    fn deletion_plan(&self, meta: &SessionMeta, children: &[SessionMeta]) -> DeletionPlan;

    /// Determine how to restore a trashed session.
    /// Default: MoveBack for dedicated files, UndoSharedDeletion for shared.
    /// If neither applies (e.g. failed move before metadata write), do no-op.
    fn restore_action(&self, entry: &TrashMeta) -> RestoreAction {
        infer_restore_action(entry)
    }

    /// Permanently remove session data from a shared source (DB/file).
    /// Called by `execute_purge` when `FileAction::Shared`.
    /// Default: no-op (dedicated-file providers don't need this).
    fn purge_from_source(
        &self,
        _source_path: &str,
        _session_id: &str,
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    /// Additional cleanup when a session is permanently deleted (empty trash / permanent delete).
    /// Called after the main file and directory cleanup.
    /// Default: no-op. Override to clean up provider-specific external data.
    fn cleanup_on_permanent_delete(&self, _session_id: &str) {}
}

/// Default token-stats aggregation: walk per-message `token_usage`,
/// dedup by `Message.usage_hash`, apply pricing. Used by all providers
/// except Codex (which overrides with usage-event aggregation).
pub fn default_compute_token_stats_from_messages(
    parsed: &ParsedSession,
    pricing_catalog: Option<&PricingCatalog>,
    mut seen_hashes: Option<&mut HashSet<String>>,
) -> Vec<TokenStatRow> {
    let mut stats_map: HashMap<(String, String), TokenStatRow> = HashMap::with_capacity(32);
    for msg in &parsed.messages {
        let Some(usage) = &msg.token_usage else {
            continue;
        };
        // Dedup: skip if this usage entry was already counted (cross-file).
        if let Some(ref mut seen) = seen_hashes {
            if let Some(ref hash) = msg.usage_hash {
                if !seen.insert(hash.clone()) {
                    continue;
                }
            }
        }

        let Some(timestamp) = msg.timestamp.as_deref() else {
            log::warn!(
                "skipping token usage without message timestamp in session {}",
                parsed.meta.id
            );
            continue;
        };
        let Some(date) = timestamp_to_local_date(timestamp) else {
            log::warn!(
                "skipping token usage with invalid timestamp '{}' in session {}",
                timestamp,
                parsed.meta.id
            );
            continue;
        };
        let Some(model) = msg.model.as_deref().filter(|model| !model.is_empty()) else {
            log::warn!(
                "skipping token usage without message model in session {}",
                parsed.meta.id
            );
            continue;
        };
        // Claude emits `<synthetic>` as the model name for internal
        // placeholder entries (continuation stubs, retry shells, etc.)
        // that don't represent a real API call. Exclude them from token
        // aggregates so daily totals reflect actual usage.
        if model == "<synthetic>" {
            continue;
        }
        let model = model.to_string();
        let entry = stats_map
            .entry((date.clone(), model.clone()))
            .or_insert_with(|| TokenStatRow {
                date,
                model,
                turn_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.0,
            });
        entry.turn_count += 1;
        entry.input_tokens += usage.input_tokens as u64;
        entry.output_tokens += usage.output_tokens as u64;
        entry.cache_read_tokens += usage.cache_read_input_tokens as u64;
        entry.cache_write_tokens += usage.cache_creation_input_tokens as u64;
        entry.cost_usd += pricing::estimate_cost_with_catalog(
            pricing_catalog,
            &entry.model,
            usage.input_tokens as u64,
            usage.output_tokens as u64,
            usage.cache_read_input_tokens as u64,
            usage.cache_creation_input_tokens as u64,
        );
    }

    stats_map.into_values().collect()
}

/// Convert an RFC 3339 timestamp string to a `YYYY-MM-DD` date in the
/// user's local timezone. Falls back to the first 10 chars when parsing
/// fails, which covers the legacy date-only timestamps some providers
/// still produce.
pub fn timestamp_to_local_date(timestamp: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .or_else(|| timestamp.get(..10).map(ToString::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    struct DummyProvider;

    impl SessionProvider for DummyProvider {
        fn provider(&self) -> Provider {
            Provider::Claude
        }
        fn watch_paths(&self) -> Vec<PathBuf> {
            Vec::new()
        }
        fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
            Ok(Vec::new())
        }
        fn load_messages(
            &self,
            _session_id: &str,
            _source_path: &str,
        ) -> Result<LoadedSession, ProviderError> {
            Ok(LoadedSession::new(Vec::new()))
        }
        fn deletion_plan(&self, _meta: &SessionMeta, _children: &[SessionMeta]) -> DeletionPlan {
            DeletionPlan {
                file_action: FileAction::Skip,
                child_plans: Vec::new(),
                cleanup_dirs: Vec::new(),
            }
        }
        fn purge_from_source(
            &self,
            _source_path: &str,
            _session_id: &str,
        ) -> Result<(), ProviderError> {
            Err(ProviderError::Parse("boom".to_string()))
        }
    }

    #[test]
    fn test_from_source_path() {
        let cases = [
            (
                "/home/user/.claude/projects/foo/abc.jsonl",
                Some(Provider::Claude),
            ),
            (
                "/home/user/.codex/sessions/xyz.jsonl",
                Some(Provider::Codex),
            ),
            (
                "/home/user/.gemini/antigravity-cli/brain/abc/.system_generated/logs/transcript.jsonl",
                Some(Provider::Antigravity),
            ),
            (
                "/home/user/.local/share/opencode/opencode.db",
                Some(Provider::OpenCode),
            ),
            (
                "/home/user/.kimi-code/sessions/wd_proj_abc/session_uuid/agents/main/wire.jsonl",
                Some(Provider::Kimi),
            ),
            (
                "/home/user/.cc-mirror/variant/config/projects/foo/abc.jsonl",
                Some(Provider::CcMirror),
            ),
            ("/home/user/random/file.txt", None),
            // cc-mirror path should NOT match claude
            (
                "/home/user/.cc-mirror/cczai/config/projects/foo/abc.jsonl",
                Some(Provider::CcMirror),
            ),
        ];
        for (path, expected) in &cases {
            assert_eq!(
                Provider::from_source_path(path).as_ref(),
                expected.as_ref(),
                "from_source_path({path})"
            );
        }
    }

    #[test]
    fn test_parse_display_key() {
        // Regular providers
        assert_eq!(
            Provider::parse_display_key("claude"),
            Some((Provider::Claude, "Claude Code".to_string()))
        );
        assert_eq!(
            Provider::parse_display_key("codex"),
            Some((Provider::Codex, "Codex".to_string()))
        );
        // CC-Mirror variants
        assert_eq!(
            Provider::parse_display_key("cc-mirror:cczai"),
            Some((Provider::CcMirror, "cczai".to_string()))
        );
        // Unknown
        assert_eq!(Provider::parse_display_key("unknown"), None);
    }

    #[test]
    fn jsonl_subagent_related_paths_returns_parent_and_children() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("project");
        let session_dir = project.join("parent");
        let subagents_dir = session_dir.join("subagents");
        std::fs::create_dir_all(&subagents_dir).unwrap();
        let parent = project.join("parent.jsonl");
        let child_a = subagents_dir.join("agent-a.jsonl");
        let child_b = subagents_dir.join("agent-b.jsonl");
        std::fs::write(&parent, "").unwrap();
        std::fs::write(&child_b, "").unwrap();
        std::fs::write(&child_a, "").unwrap();

        assert_eq!(
            jsonl_subagent_related_paths(&child_a),
            vec![parent.clone(), child_a.clone(), child_b.clone()]
        );
        assert_eq!(
            jsonl_subagent_related_paths(&parent),
            vec![parent.clone(), child_a.clone(), child_b.clone()]
        );

        std::fs::remove_file(&child_a).unwrap();
        assert_eq!(
            jsonl_subagent_related_paths(&child_a),
            vec![parent, child_b]
        );
    }

    #[test]
    fn test_display_key_roundtrip() {
        // Regular providers roundtrip through parse_display_key
        for p in Provider::all() {
            if *p == Provider::CcMirror {
                continue;
            }
            let key = p.descriptor().display_key(None);
            let parsed = Provider::parse_display_key(&key);
            assert!(parsed.is_some(), "display_key roundtrip failed for {:?}", p);
            assert_eq!(parsed.unwrap().0, *p);
        }
        let key = Provider::CcMirror.descriptor().display_key(Some("cczai"));
        let parsed = Provider::parse_display_key(&key);
        assert_eq!(parsed, Some((Provider::CcMirror, "cczai".to_string())));
    }

    #[test]
    fn test_descriptor_sort_order_unique() {
        let mut orders: Vec<u32> = Provider::all()
            .iter()
            .map(|p| p.descriptor().sort_order())
            .collect();
        orders.sort();
        orders.dedup();
        assert_eq!(
            orders.len(),
            Provider::all().len(),
            "sort_order values must be unique"
        );
    }

    #[test]
    fn test_default_restore_action_noop_for_empty_trash_file_on_dedicated_source() {
        let provider = DummyProvider;
        let entry = TrashMeta {
            id: "s1".to_string(),
            provider: "claude".to_string(),
            title: "t".to_string(),
            original_path: "/tmp/session.jsonl".to_string(),
            trashed_at: 0,
            trash_file: String::new(),
            project_name: String::new(),
            variant_name: None,
            parent_id: None,
        };
        assert_eq!(provider.restore_action(&entry), RestoreAction::Noop);
    }

    #[test]
    fn test_default_restore_action_shared_for_db_source() {
        let provider = DummyProvider;
        let entry = TrashMeta {
            id: "s1".to_string(),
            provider: "opencode".to_string(),
            title: "t".to_string(),
            original_path: "/tmp/store.db".to_string(),
            trashed_at: 0,
            trash_file: String::new(),
            project_name: String::new(),
            variant_name: None,
            parent_id: None,
        };
        assert_eq!(
            provider.restore_action(&entry),
            RestoreAction::UndoSharedDeletion
        );
    }

    #[test]
    fn test_execute_purge_propagates_shared_purge_errors() {
        let provider = DummyProvider;
        let plan = DeletionPlan {
            file_action: FileAction::Shared,
            child_plans: Vec::new(),
            cleanup_dirs: Vec::new(),
        };
        let meta = SessionMeta {
            id: "s1".to_string(),
            provider: Provider::Claude,
            title: "t".to_string(),
            project_path: String::new(),
            project_name: String::new(),
            created_at: 0,
            updated_at: 0,
            message_count: 0,
            file_size_bytes: 0,
            source_path: "/tmp/store.db".to_string(),
            is_sidechain: false,
            variant_name: None,
            model: None,
            cc_version: None,
            git_branch: None,
            parent_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };

        let err = execute_purge(&plan, &provider, &meta).expect_err("should propagate purge error");
        assert!(err.contains("boom"));
    }

    #[test]
    fn test_infer_restore_action_moveback_when_trash_file_exists() {
        let entry = TrashMeta {
            id: "s1".to_string(),
            provider: "legacy-provider".to_string(),
            title: "t".to_string(),
            original_path: "/tmp/agent-transcripts/s1/s1.jsonl".to_string(),
            trashed_at: 0,
            trash_file: "1710000000__s1.jsonl".to_string(),
            project_name: String::new(),
            variant_name: None,
            parent_id: None,
        };
        assert_eq!(infer_restore_action(&entry), RestoreAction::MoveBack);
    }
}
