use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::models::{
    token_totals_from_messages, Message, Provider, SessionMeta, TokenTotals, TrashMeta,
};

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

// ---------------------------------------------------------------------------
// Deletion plan execution — shared by trash_session, delete_session, batch
// ---------------------------------------------------------------------------

/// Execute a trash operation: move files to trash dir, return metadata records.
pub fn execute_trash(
    plan: &DeletionPlan,
    meta: &SessionMeta,
    provider_key: &str,
    trash_dir: &Path,
    ts: i64,
) -> Result<Vec<TrashMeta>, String> {
    let mut records = Vec::new();

    // Main session
    let trash_file = match plan.file_action {
        FileAction::Remove => {
            let src = Path::new(&meta.source_path);
            if src.exists() {
                match move_to_trash(src, trash_dir, ts) {
                    Ok(TrashResult::Moved { trash_file }) => trash_file,
                    Err(e) => return Err(format!("failed to move parent to trash: {e}")),
                }
            } else {
                String::new()
            }
        }
        FileAction::Shared | FileAction::Skip => String::new(),
    };
    records.push(TrashMeta {
        id: meta.id.clone(),
        provider: provider_key.to_string(),
        title: meta.title.clone(),
        original_path: meta.source_path.clone(),
        trashed_at: ts,
        trash_file,
        project_name: meta.project_name.clone(),
        variant_name: meta.variant_name.clone(),
        parent_id: None,
    });

    // Children
    for child in &plan.child_plans {
        let child_trash_file = match child.file_action {
            FileAction::Remove => {
                let src = Path::new(&child.source_path);
                if src.exists() {
                    match move_to_trash(src, trash_dir, ts) {
                        Ok(TrashResult::Moved { trash_file }) => trash_file,
                        Err(e) => {
                            return Err(format!("failed to move child {} to trash: {e}", child.id))
                        }
                    }
                } else {
                    String::new()
                }
            }
            FileAction::Shared | FileAction::Skip => String::new(),
        };
        records.push(TrashMeta {
            id: child.id.clone(),
            provider: provider_key.to_string(),
            title: child.title.clone(),
            original_path: child.source_path.clone(),
            trashed_at: ts,
            trash_file: child_trash_file,
            project_name: meta.project_name.clone(),
            variant_name: meta.variant_name.clone(),
            parent_id: Some(meta.id.clone()),
        });
    }

    // Cleanup directories
    for dir in &plan.cleanup_dirs {
        if dir.is_dir() {
            std::fs::remove_dir_all(dir).map_err(|e| {
                format!("failed to remove cleanup directory {}: {e}", dir.display())
            })?;
        }
    }

    Ok(records)
}

/// Execute a permanent delete: remove files or purge from shared source.
pub fn execute_purge(
    plan: &DeletionPlan,
    provider: &dyn SessionProvider,
    meta: &SessionMeta,
) -> Result<(), String> {
    match plan.file_action {
        FileAction::Remove => {
            let src = Path::new(&meta.source_path);
            if src.exists() {
                std::fs::remove_file(src).map_err(|e| {
                    format!("failed to remove parent source file {}: {e}", src.display())
                })?;
            }
        }
        FileAction::Shared => {
            provider
                .purge_from_source(&meta.source_path, &meta.id)
                .map_err(|e| format!("failed to purge parent from shared source: {e}"))?;
        }
        FileAction::Skip => {}
    }

    for child in &plan.child_plans {
        match child.file_action {
            FileAction::Remove => {
                let src = Path::new(&child.source_path);
                if src.exists() {
                    std::fs::remove_file(src).map_err(|e| {
                        format!("failed to remove child source file {}: {e}", src.display())
                    })?;
                }
                // Also try .meta.json (Claude subagents)
                let meta_path = src.with_extension("meta.json");
                if meta_path.exists() {
                    std::fs::remove_file(&meta_path).map_err(|e| {
                        format!(
                            "failed to remove child metadata file {}: {e}",
                            meta_path.display()
                        )
                    })?;
                }
            }
            FileAction::Shared => {
                provider
                    .purge_from_source(&child.source_path, &child.id)
                    .map_err(|e| {
                        format!("failed to purge child {} from shared source: {e}", child.id)
                    })?;
            }
            FileAction::Skip => {}
        }
    }

    for dir in &plan.cleanup_dirs {
        if dir.is_dir() {
            std::fs::remove_dir_all(dir).map_err(|e| {
                format!("failed to remove cleanup directory {}: {e}", dir.display())
            })?;
        }
    }
    Ok(())
}

/// Execute a restore: move file back or undo shared deletion.
/// Returns `true` if a source sync is needed after restore.
pub fn execute_restore(
    action: &RestoreAction,
    entry: &TrashMeta,
    trash_dir: &Path,
    all_entries: &[TrashMeta],
) -> Result<bool, String> {
    match action {
        RestoreAction::MoveBack => {
            if entry.trash_file.is_empty() {
                return Ok(false);
            }
            let src = trash_dir.join(&entry.trash_file);
            let dest = Path::new(&entry.original_path);

            if !src.exists() {
                // Already restored or deleted externally
                return Ok(true);
            }

            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create parent directory: {e}"))?;
            }

            // Check if other trash entries reference the same trash file
            let others_use_same_file = all_entries
                .iter()
                .any(|e| e.id != entry.id && e.trash_file == entry.trash_file);

            if others_use_same_file {
                if !dest.exists() {
                    std::fs::copy(&src, dest)
                        .map_err(|e| format!("failed to copy file back: {e}"))?;
                }
            } else if dest.exists() {
                let _ = std::fs::remove_file(&src);
            } else {
                std::fs::rename(&src, dest)
                    .or_else(|_| std::fs::copy(&src, dest).and_then(|_| std::fs::remove_file(&src)))
                    .map_err(|e| format!("failed to restore file: {e}"))?;
            }

            Ok(true)
        }
        RestoreAction::UndoSharedDeletion => {
            // Caller handles remove_shared_deletion + sync_source
            Ok(true)
        }
        RestoreAction::Noop => Ok(false),
    }
}

/// Shared deletion plan for JSONL providers with subagent directories
/// (Claude, CC-Mirror, and similar).
/// - Parent: Remove file + Remove children + cleanup session dir
/// - Child: Remove own file only
pub fn jsonl_subagents_deletion_plan(meta: &SessionMeta, children: &[SessionMeta]) -> DeletionPlan {
    if meta.parent_id.is_some() {
        return DeletionPlan {
            file_action: FileAction::Remove,
            child_plans: Vec::new(),
            cleanup_dirs: Vec::new(),
        };
    }

    let child_plans = children
        .iter()
        .map(|c| ChildPlan {
            id: c.id.clone(),
            source_path: c.source_path.clone(),
            title: c.title.clone(),
            file_action: FileAction::Remove,
        })
        .collect();

    // Session dir: /path/to/{session_id}/ (may contain subagents/, context.jsonl, state.json)
    let source = PathBuf::from(&meta.source_path);
    let session_dir = source.with_extension("");
    let mut cleanup_dirs = Vec::new();
    if session_dir.is_dir() {
        cleanup_dirs.push(session_dir);
    }

    DeletionPlan {
        file_action: FileAction::Remove,
        child_plans,
        cleanup_dirs,
    }
}

pub fn jsonl_subagent_related_paths(source: &Path) -> Vec<PathBuf> {
    let is_child = source
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("subagents");

    let (parent_file, subagents_dir) = if is_child {
        let Some(subagents_dir) = source.parent() else {
            return existing_path(source);
        };
        let Some(session_dir) = subagents_dir.parent() else {
            return existing_path(source);
        };
        (
            session_dir.with_extension("jsonl"),
            subagents_dir.to_path_buf(),
        )
    } else {
        (
            source.to_path_buf(),
            source.with_extension("").join("subagents"),
        )
    };

    let mut paths = Vec::new();
    if parent_file.is_file() {
        paths.push(parent_file);
    }

    if subagents_dir.is_dir() {
        let mut children = match std::fs::read_dir(&subagents_dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
                .collect::<Vec<_>>(),
            Err(error) => {
                log::warn!(
                    "failed to read subagent dir '{}': {error}",
                    subagents_dir.display()
                );
                Vec::new()
            }
        };
        children.sort();
        paths.extend(children);
    }

    paths
}

fn existing_path(source: &Path) -> Vec<PathBuf> {
    if source.is_file() {
        vec![source.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// Result of trashing a session (internal, used by move_to_trash).
enum TrashResult {
    Moved { trash_file: String },
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

/// A single Codex `event_msg.token_count` event, captured during parse so
/// the indexer's `compute_codex_token_stats` doesn't have to re-open the
/// file. Populated only by the Codex parser; empty for every other
/// provider. Kept here (rather than in `providers::codex::parser`) so
/// `ParsedSession` doesn't depend on a provider-leaf module.
#[derive(Clone, Debug)]
pub struct CodexUsageEvent {
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
    /// Codex `event_msg.token_count` events captured during the single
    /// parse pass. Lets `compute_codex_token_stats` aggregate per-date
    /// token totals without re-opening the source file. Populated only by
    /// the Codex parser.
    pub codex_usage_events: Vec<CodexUsageEvent>,
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

struct ProviderCatalogEntry {
    kind: Provider,
    key: &'static str,
    label: &'static str,
    descriptor: &'static dyn ProviderDescriptor,
    build_runtime: fn() -> Option<Box<dyn SessionProvider>>,
}

fn build_claude_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::claude::ClaudeProvider::new().map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_codex_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::codex::CodexProvider::new().map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_antigravity_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::antigravity::AntigravityProvider::new()
        .map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_opencode_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::opencode::OpenCodeProvider::new()
        .map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_kimi_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::kimi::KimiProvider::new().map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn build_cc_mirror_runtime() -> Option<Box<dyn SessionProvider>> {
    crate::providers::cc_mirror::CcMirrorProvider::new()
        .map(|p| Box::new(p) as Box<dyn SessionProvider>)
}

fn provider_catalog() -> &'static [ProviderCatalogEntry] {
    &PROVIDER_CATALOG
}

fn provider_entry(provider: &Provider) -> &'static ProviderCatalogEntry {
    // Exhaustive match — adding a new Provider variant forces this to be updated
    // at compile time, replacing the previous runtime .expect() panic risk.
    // Indices must stay in lock-step with PROVIDER_CATALOG; enforced by
    // `provider_entry_indices_match_catalog` below.
    match provider {
        Provider::Claude => &PROVIDER_CATALOG[0],
        Provider::Codex => &PROVIDER_CATALOG[1],
        Provider::Antigravity => &PROVIDER_CATALOG[2],
        Provider::OpenCode => &PROVIDER_CATALOG[3],
        Provider::Kimi => &PROVIDER_CATALOG[4],
        Provider::CcMirror => &PROVIDER_CATALOG[5],
    }
}

fn provider_entry_for_key(key: &str) -> Option<&'static ProviderCatalogEntry> {
    provider_catalog().iter().find(|entry| entry.key == key)
}

static PROVIDER_KINDS: [Provider; 6] = [
    Provider::Claude,
    Provider::Codex,
    Provider::Antigravity,
    Provider::OpenCode,
    Provider::Kimi,
    Provider::CcMirror,
];

static PROVIDER_CATALOG: [ProviderCatalogEntry; 6] = [
    ProviderCatalogEntry {
        kind: Provider::Claude,
        key: "claude",
        label: "Claude Code",
        descriptor: &crate::providers::claude::Descriptor,
        build_runtime: build_claude_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Codex,
        key: "codex",
        label: "Codex",
        descriptor: &crate::providers::codex::Descriptor,
        build_runtime: build_codex_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Antigravity,
        key: "antigravity",
        label: "Antigravity",
        descriptor: &crate::providers::antigravity::Descriptor,
        build_runtime: build_antigravity_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::OpenCode,
        key: "opencode",
        label: "OpenCode",
        descriptor: &crate::providers::opencode::Descriptor,
        build_runtime: build_opencode_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::Kimi,
        key: "kimi",
        label: "Kimi CLI",
        descriptor: &crate::providers::kimi::Descriptor,
        build_runtime: build_kimi_runtime,
    },
    ProviderCatalogEntry {
        kind: Provider::CcMirror,
        key: "cc-mirror",
        label: "CC-Mirror",
        descriptor: &crate::providers::cc_mirror::Descriptor,
        build_runtime: build_cc_mirror_runtime,
    },
];

impl Provider {
    pub fn label(&self) -> &'static str {
        provider_entry(self).label
    }

    pub fn key(&self) -> &'static str {
        provider_entry(self).key
    }

    pub fn parse(s: &str) -> Option<Provider> {
        provider_entry_for_key(s).map(|entry| entry.kind.clone())
    }

    pub fn parse_strict(s: &str) -> Result<Provider, String> {
        Self::parse(s).ok_or_else(|| format!("unknown provider: '{s}'"))
    }

    pub fn all() -> &'static [Provider] {
        &PROVIDER_KINDS
    }

    /// Get the descriptor for this provider (static metadata).
    pub fn descriptor(&self) -> &'static dyn ProviderDescriptor {
        provider_entry(self).descriptor
    }

    pub fn build_runtime(&self) -> Option<Box<dyn SessionProvider>> {
        (provider_entry(self).build_runtime)()
    }

    pub fn require_runtime(&self) -> Result<Box<dyn SessionProvider>, String> {
        self.build_runtime()
            .ok_or_else(|| format!("provider unavailable: {}", self.key()))
    }

    /// Identify which provider owns a source path.
    pub fn from_source_path(source_path: &str) -> Option<Provider> {
        Provider::all()
            .iter()
            .find(|p| p.descriptor().owns_source_path(source_path))
            .cloned()
    }

    /// Parse a display key (as produced by `descriptor().display_key()`) back to a provider and label.
    /// Handles cc-mirror variants like "cc-mirror:cczai" → (CcMirror, "cczai").
    pub fn parse_display_key(display_key: &str) -> Option<(Provider, String)> {
        // Direct match: covers most providers
        if let Some(p) = Provider::parse(display_key) {
            let label = p.label().to_string();
            return Some((p, label));
        }
        // Custom formats: e.g. "cc-mirror:variant"
        for p in Provider::all() {
            if let Some(label) = p.descriptor().try_parse_display_key(display_key) {
                return Some((p.clone(), label));
            }
        }
        None
    }
}

/// Generate a trash-safe filename by sanitizing and inserting a timestamp.
fn trash_file_name(source_path: &Path, timestamp: i64) -> String {
    let base_name = source_path.file_name().map_or_else(
        || "session".to_string(),
        |f| f.to_string_lossy().to_string(),
    );
    let base_name = base_name.replace(['/', '\\'], "_");
    match base_name.rfind('.') {
        Some(dot_pos) => {
            let (name, ext) = base_name.split_at(dot_pos);
            format!("{name}_{timestamp}{ext}")
        }
        None => format!("{base_name}_{timestamp}"),
    }
}

/// Move a source file to the trash directory. Shared helper for `trash_session` implementations.
fn move_to_trash(
    source_path: &Path,
    trash_dir: &Path,
    timestamp: i64,
) -> Result<TrashResult, ProviderError> {
    let file_name = trash_file_name(source_path, timestamp);
    let dest = trash_dir.join(&file_name);
    std::fs::rename(source_path, &dest)
        .or_else(|_| {
            std::fs::copy(source_path, &dest).and_then(|_| std::fs::remove_file(source_path))
        })
        .map_err(ProviderError::Io)?;
    Ok(TrashResult::Moved {
        trash_file: file_name,
    })
}

/// Create a provider instance by enum variant. Returns None if HOME is unavailable.
pub fn make_provider(provider: &Provider) -> Option<Box<dyn SessionProvider>> {
    provider.build_runtime()
}

/// Create all provider instances, silently skipping any that cannot resolve HOME.
pub fn all_providers() -> Vec<Box<dyn SessionProvider>> {
    Provider::all().iter().filter_map(make_provider).collect()
}

pub fn all_runtimes() -> Vec<Box<dyn SessionProvider>> {
    all_providers()
}

pub trait SessionProvider: Send + Sync {
    fn provider(&self) -> Provider;
    fn watch_paths(&self) -> Vec<PathBuf>;
    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError>;
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

fn is_shared_source_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.ends_with(".db") || normalized.ends_with("/logs.json")
}

pub fn infer_restore_action(entry: &TrashMeta) -> RestoreAction {
    if !entry.trash_file.is_empty() {
        RestoreAction::MoveBack
    } else if is_shared_source_path(&entry.original_path) {
        RestoreAction::UndoSharedDeletion
    } else {
        RestoreAction::Noop
    }
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
                "/home/user/.kimi/sessions/hash/uuid/wire.jsonl",
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

    #[test]
    fn provider_entry_indices_match_catalog() {
        // Guards against reordering `PROVIDER_CATALOG` without updating the
        // exhaustive match in `provider_entry` (and vice versa).
        for kind in Provider::all() {
            let entry = provider_entry(kind);
            assert_eq!(
                &entry.kind, kind,
                "provider_entry({kind:?}) returned entry with kind {:?}",
                entry.kind
            );
        }
    }
}
