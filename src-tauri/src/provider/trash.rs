use std::path::{Path, PathBuf};

use crate::models::{SessionMeta, TrashMeta};

use super::{ChildPlan, DeletionPlan, FileAction, ProviderError, RestoreAction, SessionProvider};

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
    use crate::models::Provider;
    use crate::provider::{LoadedSession, ParsedSession};
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
