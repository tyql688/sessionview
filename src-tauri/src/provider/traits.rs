use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::{Provider, SessionMeta, TrashMeta};
use crate::pricing::PricingCatalog;

use super::{
    default_compute_token_stats_from_messages, infer_restore_action, DeletionPlan, LoadedSession,
    ParsedSession, ProviderError, RestoreAction, ScanOutcome, SourceState, TokenStatRow,
};

/// How the frontend should watch for changes from this provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchStrategy {
    Fs,
    Poll,
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
