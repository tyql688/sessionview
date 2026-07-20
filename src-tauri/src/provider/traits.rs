use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::models::Provider;
use crate::pricing::PricingCatalog;

use super::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SourceState, TokenStatRow,
    compute_token_stats_from_usage_events, default_compute_token_stats_from_messages,
};

/// Static metadata for a provider. Implemented by zero-sized descriptor structs
/// in each provider module. Accessed via `Provider::descriptor()`.
pub trait ProviderDescriptor: Send + Sync {
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
}

pub trait SessionProvider: Send + Sync {
    fn provider(&self) -> Provider;
    fn source_roots(&self) -> Vec<PathBuf>;
    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError>;

    /// Incremental scan: parse only the source files whose
    /// `(size, mtime)` differs from what's stored in `known`, and return
    /// the rest as `unchanged_source_paths` so the indexer can preserve
    /// their DB rows without re-upserting.
    ///
    /// Default implementation parses everything (matches `scan_all`) —
    /// providers whose data lives in per-session files override this to
    /// take advantage of the snapshot. OpenCode (single SQLite file) also
    /// overrides it, comparing a combined main-db + non-empty-WAL
    /// `(size, mtime)` so idle polls short-circuit without reparsing.
    fn scan_incremental(
        &self,
        _known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        Ok(ScanOutcome {
            parsed: self.scan_all()?,
            unchanged_source_paths: Vec::new(),
        })
    }

    fn load_messages(
        &self,
        session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError>;

    /// Aggregate per-(bucket, model) token-usage rows for the indexer.
    ///
    /// Out-of-band usage events are authoritative when present; otherwise
    /// aggregate usage attached to messages.
    fn compute_token_stats(
        &self,
        parsed: &ParsedSession,
        pricing_catalog: Option<&PricingCatalog>,
        seen_hashes: Option<&mut HashSet<String>>,
    ) -> Vec<TokenStatRow> {
        if parsed.usage_events.is_empty() {
            default_compute_token_stats_from_messages(parsed, pricing_catalog, seen_hashes)
        } else {
            compute_token_stats_from_usage_events(parsed, pricing_catalog, seen_hashes)
        }
    }
}
