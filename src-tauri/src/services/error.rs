//! Typed error for the service / indexer plumbing.
//!
//! Replaces the bare `Result<_, String>` returns that used to bubble
//! `format!("failed to X: {e}")` strings straight to the Tauri command
//! boundary. Each variant reproduces the original flat message verbatim
//! (the inner cause is captured as a `String`, NOT `#[source]`, so the
//! serialized `{:#}` text the frontend toast sees is byte-identical to
//! the pre-migration behaviour — see `tests` at the bottom of this file
//! and `crate::error::CommandError`).
//!
//! `Message(String)` is the transparent passthrough used for errors that
//! originate in still-`String`-typed helpers (`trash_state`,
//! `provider::trash`, `Provider::require_runtime` / `parse_strict`).
//! Those strings are already fully formatted at their source, so `?`
//! propagating them through `From<String>` preserves the exact text.

use thiserror::Error;

/// Typed error returned by the session-lifecycle / source-sync /
/// resolution / snapshot services and the indexer. Converts into
/// `CommandError` with an identical `{:#}` rendering to the old flat
/// `String` errors.
#[derive(Debug, Error)]
pub enum ServiceError {
    /// Passthrough for already-formatted messages propagated from
    /// helpers that still return `Result<_, String>`. Carries the
    /// source string verbatim so no text is altered.
    #[error("{0}")]
    Message(String),

    // --- session_resolution ---
    #[error("failed to load session {0}: {1}")]
    LoadSession(String, String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("failed to load child sessions for {0}: {1}")]
    LoadChildSessions(String, String),

    // --- session_lifecycle ---
    #[error("trash meta lock poisoned")]
    TrashMetaLockPoisoned,
    #[error("failed to delete from db: {0}")]
    DbDelete(String),
    #[error("No trash metadata found")]
    NoTrashMetadata,
    #[error("failed to inspect unsupported trash file: {0}")]
    InspectUnsupportedTrashFile(String),
    #[error("failed to remove unsupported trash directory: {0}")]
    RemoveUnsupportedTrashDir(String),
    #[error("failed to remove unsupported trash file: {0}")]
    RemoveUnsupportedTrashFile(String),

    // --- source_sync ---
    #[error("failed to scan source: {0}")]
    ScanSource(String),
    #[error("failed to sync source snapshot: {0}")]
    SyncSourceSnapshot(String),
    #[error("failed to store usage_last_refreshed_at: {0}")]
    StoreUsageLastRefreshed(String),

    // --- provider_snapshots ---
    #[error("failed to load provider session counts: {0}")]
    LoadProviderSessionCounts(String),

    // --- indexer ---
    #[error("failed to load {0} source snapshot: {1}")]
    LoadProviderSourceSnapshot(String, String),
    #[error("failed to scan {0} provider: {1}")]
    ScanProvider(String, String),
    #[error("failed to sync {0} provider: {1}")]
    SyncProvider(String, String),
    #[error("failed to store last_index_time: {0}")]
    StoreLastIndexTime(String),
    #[error("failed to list sessions: {0}")]
    ListSessions(String),
}

impl From<String> for ServiceError {
    fn from(s: String) -> Self {
        ServiceError::Message(s)
    }
}

impl From<ServiceError> for crate::error::CommandError {
    fn from(err: ServiceError) -> Self {
        // Capture the full `Display` text as the anyhow message so the
        // command boundary serializes (`format!("{:#}", _)`) to exactly
        // the same string the old flat `String` errors produced.
        crate::error::CommandError(anyhow::Error::msg(err.to_string()))
    }
}

/// Result alias for service plumbing.
pub type ServiceResult<T> = std::result::Result<T, ServiceError>;

#[cfg(test)]
mod tests {
    use super::ServiceError;
    use crate::error::CommandError;

    /// Render a `ServiceError` exactly as the frontend toast sees it:
    /// converted to `CommandError`, then serialized via `{:#}` (the
    /// `CommandError::serialize` path uses the same `format!("{:#}", _)`).
    fn rendered(err: ServiceError) -> String {
        let command: CommandError = err.into();
        format!("{:#}", command.0)
    }

    #[test]
    fn message_passthrough_preserves_text_verbatim() {
        // A string propagated from an out-of-scope `Result<_, String>`
        // helper (e.g. `trash_dir()`), surfaced via `From<String>`.
        let err: ServiceError = "failed to create trash directory: boom".to_string().into();
        assert_eq!(rendered(err), "failed to create trash directory: boom");
    }

    #[test]
    fn load_session_matches_old_flat_string() {
        let err = ServiceError::LoadSession("sess-1".into(), "db locked".into());
        assert_eq!(rendered(err), "failed to load session sess-1: db locked");
    }

    #[test]
    fn session_not_found_matches_old_flat_string() {
        let err = ServiceError::SessionNotFound("sess-1".into());
        assert_eq!(rendered(err), "session not found: sess-1");
    }

    #[test]
    fn load_child_sessions_matches_old_flat_string() {
        let err = ServiceError::LoadChildSessions("sess-1".into(), "db locked".into());
        assert_eq!(
            rendered(err),
            "failed to load child sessions for sess-1: db locked"
        );
    }

    #[test]
    fn trash_meta_lock_poisoned_matches_old_flat_string() {
        assert_eq!(
            rendered(ServiceError::TrashMetaLockPoisoned),
            "trash meta lock poisoned"
        );
    }

    #[test]
    fn db_delete_matches_old_flat_string() {
        let err = ServiceError::DbDelete("UNIQUE constraint".into());
        assert_eq!(rendered(err), "failed to delete from db: UNIQUE constraint");
    }

    #[test]
    fn no_trash_metadata_matches_old_flat_string() {
        assert_eq!(
            rendered(ServiceError::NoTrashMetadata),
            "No trash metadata found"
        );
    }

    #[test]
    fn inspect_unsupported_trash_file_matches_old_flat_string() {
        let err = ServiceError::InspectUnsupportedTrashFile("permission denied".into());
        assert_eq!(
            rendered(err),
            "failed to inspect unsupported trash file: permission denied"
        );
    }

    #[test]
    fn remove_unsupported_trash_dir_matches_old_flat_string() {
        let err = ServiceError::RemoveUnsupportedTrashDir("not empty".into());
        assert_eq!(
            rendered(err),
            "failed to remove unsupported trash directory: not empty"
        );
    }

    #[test]
    fn remove_unsupported_trash_file_matches_old_flat_string() {
        let err = ServiceError::RemoveUnsupportedTrashFile("permission denied".into());
        assert_eq!(
            rendered(err),
            "failed to remove unsupported trash file: permission denied"
        );
    }

    #[test]
    fn scan_source_matches_old_flat_string() {
        let err = ServiceError::ScanSource("io error".into());
        assert_eq!(rendered(err), "failed to scan source: io error");
    }

    #[test]
    fn sync_source_snapshot_matches_old_flat_string() {
        let err = ServiceError::SyncSourceSnapshot("db locked".into());
        assert_eq!(rendered(err), "failed to sync source snapshot: db locked");
    }

    #[test]
    fn store_usage_last_refreshed_matches_old_flat_string() {
        let err = ServiceError::StoreUsageLastRefreshed("db locked".into());
        assert_eq!(
            rendered(err),
            "failed to store usage_last_refreshed_at: db locked"
        );
    }

    #[test]
    fn load_provider_session_counts_matches_old_flat_string() {
        let err = ServiceError::LoadProviderSessionCounts("db locked".into());
        assert_eq!(
            rendered(err),
            "failed to load provider session counts: db locked"
        );
    }

    #[test]
    fn load_provider_source_snapshot_matches_old_flat_string() {
        let err = ServiceError::LoadProviderSourceSnapshot("claude".into(), "db locked".into());
        assert_eq!(
            rendered(err),
            "failed to load claude source snapshot: db locked"
        );
    }

    #[test]
    fn scan_provider_matches_old_flat_string() {
        let err = ServiceError::ScanProvider("codex".into(), "io error".into());
        assert_eq!(rendered(err), "failed to scan codex provider: io error");
    }

    #[test]
    fn sync_provider_matches_old_flat_string() {
        let err = ServiceError::SyncProvider("claude".into(), "db locked".into());
        assert_eq!(rendered(err), "failed to sync claude provider: db locked");
    }

    #[test]
    fn store_last_index_time_matches_old_flat_string() {
        let err = ServiceError::StoreLastIndexTime("db locked".into());
        assert_eq!(rendered(err), "failed to store last_index_time: db locked");
    }

    #[test]
    fn list_sessions_matches_old_flat_string() {
        let err = ServiceError::ListSessions("db locked".into());
        assert_eq!(rendered(err), "failed to list sessions: db locked");
    }

    /// The cancel sentinel travels as a `Message` passthrough (the
    /// command layer constructs it directly today, but if it ever flows
    /// through `ServiceError` the substring must survive untouched).
    #[test]
    fn cancel_sentinel_substring_survives_passthrough() {
        let err: ServiceError = "__cc_session_load_canceled__".to_string().into();
        assert!(rendered(err).contains("__cc_session_load_canceled__"));
    }
}
