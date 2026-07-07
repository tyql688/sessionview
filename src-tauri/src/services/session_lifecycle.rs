use std::path::Path;
use std::sync::Mutex;

use crate::db::Database;
use crate::models::TrashMeta;
use crate::provider::{FileAction, RestoreAction, SessionProvider};
use crate::services::error::{ServiceError, ServiceResult};
use crate::services::image_cache::{image_cache_data_dir, ImageCacheService};
use crate::services::{resolve_session_deletion, SourceSyncService};
use crate::trash_state::{
    add_shared_deletion, atomic_write_json, read_trash_meta, remove_shared_deletion,
    shared_deletions_path, trash_dir, trash_meta_path,
};

/// Serialize all trash metadata read-modify-write operations.
static TRASH_META_LOCK: Mutex<()> = Mutex::new(());

struct RestoreEntries {
    entry: TrashMeta,
    child_entries: Vec<TrashMeta>,
    remaining: Vec<TrashMeta>,
}

pub(crate) struct SessionLifecycleService<'a> {
    db: &'a Database,
    source_sync: SourceSyncService<'a>,
}

impl<'a> SessionLifecycleService<'a> {
    pub(crate) fn new(db: &'a Database) -> Self {
        Self {
            db,
            source_sync: SourceSyncService::new(db),
        }
    }

    pub(crate) fn trash_session(&self, session_id: &str) -> ServiceResult<()> {
        let trash_dir = trash_dir()?;
        let deletion = resolve_session_deletion(self.db, session_id)?;

        let now_ts = chrono::Utc::now().timestamp();
        let meta_path = trash_meta_path(&trash_dir);
        let _lock = TRASH_META_LOCK
            .lock()
            .map_err(|_| ServiceError::TrashMetaLockPoisoned)?;

        let provider_key = deletion.meta.provider.key();
        let mut entries = read_trash_meta(&meta_path);
        let records = crate::provider::execute_trash(
            &deletion.plan,
            &deletion.meta,
            provider_key,
            &trash_dir,
            now_ts,
        )?;
        entries.extend(records);

        let shared_deletions_path = shared_deletions_path(&trash_dir);
        if deletion.plan.file_action == FileAction::Shared {
            add_shared_deletion(
                &shared_deletions_path,
                &deletion.meta.id,
                provider_key,
                &deletion.meta.source_path,
            )?;
        }

        atomic_write_json(&meta_path, &entries)?;
        self.db
            .delete_session(session_id)
            .map_err(|e| ServiceError::DbDelete(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn purge_session(&self, session_id: &str) -> ServiceResult<()> {
        let deletion = resolve_session_deletion(self.db, session_id)?;

        // Clean cached images before deleting session data
        if let Ok(loaded) = deletion
            .provider
            .load_messages(session_id, &deletion.meta.source_path)
        {
            if let Some(data_dir) = image_cache_data_dir() {
                ImageCacheService::new(&data_dir).cleanup_images(&loaded.messages);
            }
        }

        crate::provider::execute_purge(&deletion.plan, deletion.provider.as_ref(), &deletion.meta)?;
        if deletion.plan.file_action == FileAction::Remove {
            cleanup_session_dir(&deletion.meta.source_path);
        }
        deletion
            .provider
            .cleanup_on_permanent_delete(&deletion.meta.id);
        self.db
            .delete_session(session_id)
            .map_err(|e| ServiceError::DbDelete(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn trash_sessions(&self, session_ids: &[String]) -> crate::models::BatchResult {
        let mut succeeded = 0u32;
        let mut failed = 0u32;
        let mut errors = Vec::new();
        for session_id in session_ids {
            match self.trash_session(session_id) {
                Ok(()) => succeeded += 1,
                Err(e) => {
                    log::warn!("batch trash failed for {session_id}: {e}");
                    errors.push(format!("{session_id}: {e}"));
                    failed += 1;
                }
            }
        }
        crate::models::BatchResult {
            succeeded,
            failed,
            errors,
        }
    }

    pub(crate) fn restore_sessions(&self, trash_ids: &[String]) -> crate::models::BatchResult {
        let mut succeeded = 0u32;
        let mut failed = 0u32;
        let mut errors = Vec::new();
        for trash_id in trash_ids {
            match self.restore_session(trash_id) {
                Ok(()) => succeeded += 1,
                Err(e) => {
                    log::warn!("batch restore failed for {trash_id}: {e}");
                    errors.push(format!("{trash_id}: {e}"));
                    failed += 1;
                }
            }
        }
        crate::models::BatchResult {
            succeeded,
            failed,
            errors,
        }
    }

    pub(crate) fn permanent_delete_trash_batch(
        &self,
        trash_ids: &[String],
    ) -> crate::models::BatchResult {
        let mut succeeded = 0u32;
        let mut failed = 0u32;
        let mut errors = Vec::new();
        for trash_id in trash_ids {
            match self.permanent_delete_trash(trash_id) {
                Ok(()) => succeeded += 1,
                Err(e) => {
                    log::warn!("batch permanent delete failed for {trash_id}: {e}");
                    errors.push(format!("{trash_id}: {e}"));
                    failed += 1;
                }
            }
        }
        crate::models::BatchResult {
            succeeded,
            failed,
            errors,
        }
    }

    pub(crate) fn list_trash() -> ServiceResult<Vec<TrashMeta>> {
        let trash_dir = trash_dir()?;
        let meta_path = trash_meta_path(&trash_dir);
        let shared_deletions_path = shared_deletions_path(&trash_dir);
        let _lock = TRASH_META_LOCK
            .lock()
            .map_err(|_| ServiceError::TrashMetaLockPoisoned)?;
        prune_unsupported_trash_entries(
            read_trash_meta(&meta_path),
            &meta_path,
            &trash_dir,
            &shared_deletions_path,
        )
    }

    pub(crate) fn restore_session(&self, trash_id: &str) -> ServiceResult<()> {
        let trash_dir = trash_dir()?;
        let meta_path = trash_meta_path(&trash_dir);
        let shared_deletions_path = shared_deletions_path(&trash_dir);
        if !meta_path.exists() {
            return Err(ServiceError::NoTrashMetadata);
        }

        let lock = TRASH_META_LOCK
            .lock()
            .map_err(|_| ServiceError::TrashMetaLockPoisoned)?;

        let entries = prune_unsupported_trash_entries(
            read_trash_meta(&meta_path),
            &meta_path,
            &trash_dir,
            &shared_deletions_path,
        )?;
        let Some(restore_entries) = collect_restore_entries(entries, trash_id) else {
            drop(lock);
            return Ok(());
        };

        let provider = runtime_for_trash_entry(&restore_entries.entry);
        let action = provider
            .as_ref()
            .map(|runtime| runtime.restore_action(&restore_entries.entry))
            .unwrap_or_else(|| crate::provider::infer_restore_action(&restore_entries.entry));

        let needs_sync = crate::provider::execute_restore(
            &action,
            &restore_entries.entry,
            &trash_dir,
            &restore_entries.remaining,
        )?;

        for child in &restore_entries.child_entries {
            let child_action = provider
                .as_ref()
                .map(|runtime| runtime.restore_action(child))
                .unwrap_or_else(|| crate::provider::infer_restore_action(child));
            let _ = crate::provider::execute_restore(
                &child_action,
                child,
                &trash_dir,
                &restore_entries.remaining,
            );
        }

        if action == RestoreAction::UndoSharedDeletion {
            remove_shared_deletion(
                &shared_deletions_path,
                &restore_entries.entry.id,
                &restore_entries.entry.original_path,
            )?;
        }

        atomic_write_json(&meta_path, &restore_entries.remaining)?;
        drop(lock);

        if needs_sync {
            self.source_sync.sync_provider_key(
                &restore_entries.entry.provider,
                &restore_entries.entry.original_path,
            )?;
        }

        Ok(())
    }

    pub(crate) fn empty_trash() -> ServiceResult<()> {
        let trash_dir = trash_dir()?;
        let meta_path = trash_meta_path(&trash_dir);
        let shared_deletions_path = shared_deletions_path(&trash_dir);

        if meta_path.exists() {
            let _lock = TRASH_META_LOCK
                .lock()
                .map_err(|_| ServiceError::TrashMetaLockPoisoned)?;
            let entries = prune_unsupported_trash_entries(
                read_trash_meta(&meta_path),
                &meta_path,
                &trash_dir,
                &shared_deletions_path,
            )?;

            for entry in &entries {
                cleanup_cached_images_for_trash(entry, &trash_dir);
                remove_trash_entry(entry, &trash_dir, &shared_deletions_path, &entries, true)?;
            }

            let empty: Vec<TrashMeta> = Vec::new();
            atomic_write_json(&meta_path, &empty)?;
        }

        Ok(())
    }

    pub(crate) fn permanent_delete_trash(&self, trash_id: &str) -> ServiceResult<()> {
        let trash_dir = trash_dir()?;
        let meta_path = trash_meta_path(&trash_dir);
        let shared_deletions_path = shared_deletions_path(&trash_dir);
        if !meta_path.exists() {
            return Err(ServiceError::NoTrashMetadata);
        }

        let _lock = TRASH_META_LOCK
            .lock()
            .map_err(|_| ServiceError::TrashMetaLockPoisoned)?;
        let entries = prune_unsupported_trash_entries(
            read_trash_meta(&meta_path),
            &meta_path,
            &trash_dir,
            &shared_deletions_path,
        )?;

        if let Some(entry) = entries.iter().find(|entry| entry.id == trash_id) {
            cleanup_cached_images_for_trash(entry, &trash_dir);
            remove_trash_entry(entry, &trash_dir, &shared_deletions_path, &entries, false)?;
        }

        let remaining: Vec<TrashMeta> = entries
            .into_iter()
            .filter(|entry| entry.id != trash_id)
            .collect();
        atomic_write_json(&meta_path, &remaining)?;
        Ok(())
    }
}

fn prune_unsupported_trash_entries(
    entries: Vec<TrashMeta>,
    meta_path: &Path,
    trash_dir: &Path,
    shared_deletions_path: &Path,
) -> ServiceResult<Vec<TrashMeta>> {
    let mut retained = Vec::with_capacity(entries.len());
    let mut removed = false;

    for entry in entries {
        if crate::models::Provider::parse(&entry.provider).is_some() {
            retained.push(entry);
            continue;
        }

        log::warn!(
            "removing trash entry {} with unsupported provider '{}'",
            entry.id,
            entry.provider
        );
        remove_unsupported_trash_entry(&entry, trash_dir, shared_deletions_path)?;
        removed = true;
    }

    if removed {
        atomic_write_json(meta_path, &retained)?;
    }

    Ok(retained)
}

fn remove_unsupported_trash_entry(
    entry: &TrashMeta,
    trash_dir: &Path,
    shared_deletions_path: &Path,
) -> ServiceResult<()> {
    if !entry.trash_file.is_empty() {
        let trash_file = trash_dir.join(&entry.trash_file);
        if trash_file.exists() {
            let metadata = trash_file
                .metadata()
                .map_err(|e| ServiceError::InspectUnsupportedTrashFile(e.to_string()))?;
            if metadata.is_dir() {
                std::fs::remove_dir_all(&trash_file)
                    .map_err(|e| ServiceError::RemoveUnsupportedTrashDir(e.to_string()))?;
            } else {
                std::fs::remove_file(&trash_file)
                    .map_err(|e| ServiceError::RemoveUnsupportedTrashFile(e.to_string()))?;
            }
        } else {
            log::warn!(
                "unsupported trash entry {} references missing trash file: {}",
                entry.id,
                entry.trash_file
            );
        }
    }

    if !entry.original_path.is_empty() {
        remove_shared_deletion(shared_deletions_path, &entry.id, &entry.original_path)?;
    }

    Ok(())
}

fn collect_restore_entries(entries: Vec<TrashMeta>, trash_id: &str) -> Option<RestoreEntries> {
    let entry = entries.iter().find(|entry| entry.id == trash_id)?.clone();
    let mut child_entries = Vec::new();

    let remaining = entries
        .into_iter()
        .filter(|candidate| {
            if candidate.id == trash_id {
                return false;
            }
            if candidate.parent_id.as_deref() == Some(trash_id) {
                if candidate.trash_file.is_empty() {
                    return false;
                }
                child_entries.push(candidate.clone());
                return false;
            }
            if candidate.trash_file.is_empty()
                && !entry.trash_file.is_empty()
                && candidate.original_path == entry.original_path
                && candidate.provider == entry.provider
                && candidate.parent_id.is_none()
            {
                log::debug!(
                    "restore: legacy child match for session {} (provider={}, path={})",
                    candidate.id,
                    candidate.provider,
                    candidate.original_path
                );
                return false;
            }
            true
        })
        .collect();

    Some(RestoreEntries {
        entry,
        child_entries,
        remaining,
    })
}

fn remove_trash_entry(
    entry: &TrashMeta,
    trash_dir: &Path,
    shared_deletions_path: &Path,
    all_entries: &[TrashMeta],
    delete_all: bool,
) -> ServiceResult<()> {
    if entry.trash_file.is_empty() && !entry.original_path.is_empty() {
        purge_shared_trash_entry(entry, shared_deletions_path)?;
        if delete_all {
            return Ok(());
        }
    }

    if !entry.trash_file.is_empty() {
        let file = trash_dir.join(&entry.trash_file);
        let others_use_file = if delete_all {
            false
        } else {
            all_entries
                .iter()
                .filter(|other| other.id != entry.id)
                .any(|other| other.trash_file == entry.trash_file)
        };

        if !others_use_file && file.exists() {
            let _ = std::fs::remove_file(&file);
        }
    }

    if !entry.original_path.is_empty() {
        cleanup_session_dir(&entry.original_path);
    }
    cleanup_provider_entry(entry);
    Ok(())
}

/// Remove session directory from original location.
/// Tries both patterns to cover all providers:
/// - `<file>.jsonl` → `<file>/` (Claude, Codex, CC-Mirror)
/// - `parent()` of file (Kimi — session UUID dir contains subagents/, state.json)
///
/// Safety: only `remove_dir_all` on directories that look session-specific
/// (contain subagents/, state.json, wire.jsonl, or context.jsonl).
/// Shared directories like Gemini's/Antigravity's parent roots are NOT removed.
fn cleanup_session_dir(original_path: &str) {
    let original = Path::new(original_path);
    for candidate in [
        original.with_extension(""),
        original.parent().unwrap_or(original).to_path_buf(),
    ] {
        if !candidate.is_dir() {
            continue;
        }
        if is_session_dir(&candidate) {
            let _ = std::fs::remove_dir_all(&candidate);
        } else {
            let _ = std::fs::remove_dir(&candidate);
        }
    }
}

fn is_session_dir(dir: &Path) -> bool {
    dir.join("subagents").is_dir()
        || dir.join("state.json").is_file()
        || dir.join("wire.jsonl").is_file()
        || dir.join("context.jsonl").is_file()
}

fn runtime_for_trash_entry(entry: &TrashMeta) -> Option<Box<dyn SessionProvider>> {
    crate::models::Provider::parse(&entry.provider).and_then(|provider| provider.build_runtime())
}

/// Clean cached images for a trash entry, trying original path first then trash copy.
fn cleanup_cached_images_for_trash(entry: &TrashMeta, trash_dir: &std::path::Path) {
    let Some(runtime) = runtime_for_trash_entry(entry) else {
        return;
    };
    let loaded = runtime
        .load_messages(&entry.id, &entry.original_path)
        .or_else(|_| {
            let trash_path = trash_dir.join(&entry.trash_file);
            runtime.load_messages(&entry.id, trash_path.to_str().unwrap_or_default())
        });
    if let Ok(loaded) = loaded {
        if let Some(data_dir) = image_cache_data_dir() {
            ImageCacheService::new(&data_dir).cleanup_images(&loaded.messages);
        }
    }
}

fn purge_shared_trash_entry(entry: &TrashMeta, shared_deletions_path: &Path) -> ServiceResult<()> {
    if let Some(provider) = runtime_for_trash_entry(entry) {
        if let Err(error) = provider.purge_from_source(&entry.original_path, &entry.id) {
            log::warn!("failed to purge session {} from source: {error}", entry.id);
        }
    }

    add_shared_deletion(
        shared_deletions_path,
        &entry.id,
        &entry.provider,
        &entry.original_path,
    )?;
    Ok(())
}

fn cleanup_provider_entry(entry: &TrashMeta) {
    if let Some(provider) = runtime_for_trash_entry(entry) {
        provider.cleanup_on_permanent_delete(&entry.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trash_state::SharedDeletion;
    use std::fs;
    use tempfile::TempDir;

    fn trash_entry(id: &str, provider: &str, original_path: &str, trash_file: &str) -> TrashMeta {
        TrashMeta {
            id: id.to_string(),
            provider: provider.to_string(),
            title: id.to_string(),
            original_path: original_path.to_string(),
            trashed_at: 1,
            trash_file: trash_file.to_string(),
            project_name: String::new(),
            variant_name: None,
            parent_id: None,
        }
    }

    #[test]
    fn prunes_unsupported_trash_entries_without_touching_original_path() {
        let dir = TempDir::new().expect("temp dir");
        let trash_dir = dir.path().join("trash");
        fs::create_dir_all(&trash_dir).expect("create trash dir");
        let meta_path = trash_dir.join("trash_meta.json");
        let shared_deletions_path = trash_dir.join("shared_deletions.json");
        let original_path = dir.path().join("source").join("legacy.jsonl");
        fs::create_dir_all(original_path.parent().unwrap()).expect("create source dir");
        fs::write(&original_path, "source").expect("write source");
        let original_path_str = original_path.to_string_lossy().to_string();
        fs::write(trash_dir.join("legacy.jsonl"), "trash").expect("write trash file");
        atomic_write_json(
            &shared_deletions_path,
            &vec![SharedDeletion {
                id: "old".to_string(),
                provider: "legacy-provider".to_string(),
                original_path: original_path_str.clone(),
            }],
        )
        .expect("write shared deletions");

        let retained = prune_unsupported_trash_entries(
            vec![
                trash_entry("old", "legacy-provider", &original_path_str, "legacy.jsonl"),
                trash_entry("kept", "claude", "", ""),
            ],
            &meta_path,
            &trash_dir,
            &shared_deletions_path,
        )
        .expect("prune unsupported entries");

        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].id, "kept");
        assert!(!trash_dir.join("legacy.jsonl").exists());
        assert!(original_path.exists());

        let persisted = read_trash_meta(&meta_path);
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].id, "kept");

        let shared: Vec<SharedDeletion> =
            crate::trash_state::read_shared_deletions(&shared_deletions_path);
        assert!(shared.is_empty());
    }
}
