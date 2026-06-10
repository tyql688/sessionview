use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Deletion plan types — provider returns a plan, command layer executes it
// ---------------------------------------------------------------------------

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
