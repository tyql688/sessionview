use crate::db::Database;
use crate::models::{Provider, ProviderSnapshot};
use crate::services::error::{ServiceError, ServiceResult};
use std::path::{Path, PathBuf};

pub(crate) struct ProviderSnapshotService<'a> {
    db: &'a Database,
}

impl<'a> ProviderSnapshotService<'a> {
    pub(crate) fn new(db: &'a Database) -> Self {
        Self { db }
    }

    pub(crate) fn list(&self) -> ServiceResult<Vec<ProviderSnapshot>> {
        let counts = self
            .db
            .provider_session_counts()
            .map_err(|e| ServiceError::LoadProviderSessionCounts(e.to_string()))?;

        let mut snapshots = Vec::new();

        for provider in Provider::all() {
            let (path, exists) = provider
                .build_runtime()
                .map(|runtime| snapshot_path_info(&runtime.source_roots()))
                .unwrap_or_default();

            snapshots.push(ProviderSnapshot {
                key: provider.clone(),
                label: provider.label().to_string(),
                color: provider.descriptor().color().to_string(),
                sort_order: provider.descriptor().sort_order(),
                path,
                exists,
                session_count: counts.get(provider.key()).copied().unwrap_or(0),
            });
        }

        snapshots.sort_by_key(|snapshot| snapshot.sort_order);
        Ok(snapshots)
    }
}

fn snapshot_path_info(paths: &[PathBuf]) -> (String, bool) {
    let Some(path) = common_source_root(paths).or_else(|| paths.first().cloned()) else {
        return (String::new(), false);
    };

    (path.to_string_lossy().to_string(), path.exists())
}

fn common_source_root(paths: &[PathBuf]) -> Option<PathBuf> {
    let first = paths.first()?;

    first
        .ancestors()
        .find(|candidate| paths.iter().all(|path| path.starts_with(candidate)))
        .and_then(|candidate| {
            if paths.len() > 1 && candidate.parent().is_none() {
                None
            } else {
                Some(Path::to_path_buf(candidate))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::{common_source_root, snapshot_path_info, ProviderSnapshotService};
    use crate::db::Database;
    use crate::models::Provider;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn common_source_root_returns_shared_ancestor() {
        let paths = vec![
            PathBuf::from("/tmp/.cc-mirror/a/config/projects"),
            PathBuf::from("/tmp/.cc-mirror/b/config/projects"),
        ];

        assert_eq!(
            common_source_root(&paths),
            Some(PathBuf::from("/tmp/.cc-mirror"))
        );
    }

    #[test]
    fn snapshot_path_info_uses_single_path_when_no_common_root_exists() {
        let paths = vec![
            PathBuf::from("/tmp/provider-one"),
            PathBuf::from("/var/provider-two"),
        ];

        assert_eq!(
            snapshot_path_info(&paths),
            ("/tmp/provider-one".to_string(), false)
        );
    }

    #[test]
    fn list_returns_all_providers_in_snapshot_order() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let service = ProviderSnapshotService::new(&db);

        let snapshots = service.list().unwrap();
        let keys: Vec<Provider> = snapshots
            .iter()
            .map(|snapshot| snapshot.key.clone())
            .collect();

        assert_eq!(
            keys,
            vec![
                Provider::Claude,
                Provider::CcMirror,
                Provider::Codex,
                Provider::Antigravity,
                Provider::OpenCode,
                Provider::Kimi,
                Provider::Cursor,
                Provider::Pi,
                Provider::Grok,
            ]
        );
        assert_eq!(snapshots.len(), Provider::all().len());
    }
}
