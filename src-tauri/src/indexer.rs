use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rayon::prelude::*;

use crate::db::Database;
use crate::models::{Provider, SessionMeta, TreeNode, TreeNodeType};
use crate::pricing::{self, PricingCatalog, PRICING_CATALOG_JSON_KEY};
use crate::provider::{ParsedSession, SessionProvider, TokenStatRow};
use crate::services::error::{ServiceError, ServiceResult};
use crate::services::image_cache::ImageCacheService;

#[derive(Clone)]
pub struct Indexer {
    db: Arc<Database>,
    providers: Arc<Vec<Box<dyn SessionProvider>>>,
    data_dir: PathBuf,
}

struct ProviderWork {
    provider_kind: Provider,
    sessions: Vec<ParsedSession>,
    unchanged_source_paths: Vec<String>,
    stats_batch: Vec<(String, Vec<TokenStatRow>)>,
}

fn epoch_millis(time: SystemTime) -> ServiceResult<i64> {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .map_err(|error| {
            ServiceError::Message(format!("system clock is before the UNIX epoch: {error}"))
        })
}

fn build_token_stats_batch(
    provider: &dyn SessionProvider,
    sessions: &[ParsedSession],
    pricing_catalog: Option<&PricingCatalog>,
) -> Vec<(String, Vec<TokenStatRow>)> {
    // Parent-before-child ordering so cross-file dedup attributes overlapping
    // usage entries to the parent.
    let (parents, children): (Vec<&ParsedSession>, Vec<&ParsedSession>) = sessions
        .iter()
        .partition(|parsed| parsed.meta.parent_id.is_none());

    let mut seen_hashes: HashSet<String> = HashSet::new();
    let mut stats_batch: Vec<(String, Vec<TokenStatRow>)> = Vec::with_capacity(sessions.len());
    for parsed in parents.iter().chain(children.iter()) {
        let stat_rows =
            provider.compute_token_stats(parsed, pricing_catalog, Some(&mut seen_hashes));
        stats_batch.push((parsed.meta.id.clone(), stat_rows));
    }
    stats_batch
}

impl Indexer {
    pub fn new(
        db: Arc<Database>,
        providers: Vec<Box<dyn SessionProvider>>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            db,
            providers: Arc::new(providers),
            data_dir,
        }
    }

    pub fn reindex(&self) -> ServiceResult<usize> {
        self.reindex_filtered(None, true, false)
    }

    /// Full reparse of every source file regardless of the freshness snapshot,
    /// WITHOUT destroying any existing data first. Token stats are swapped
    /// per-session inside each provider's commit, so a failure part-way leaves
    /// the previous stats in place instead of an empty usage panel.
    pub fn refresh_usage(&self) -> ServiceResult<usize> {
        self.reindex_filtered(None, true, true)
    }

    pub fn reindex_providers(
        &self,
        filter: Option<&[Provider]>,
        aggressive: bool,
    ) -> ServiceResult<usize> {
        self.reindex_filtered(filter, aggressive, false)
    }

    fn reindex_filtered(
        &self,
        filter: Option<&[Provider]>,
        aggressive: bool,
        force_parse: bool,
    ) -> ServiceResult<usize> {
        let start = Instant::now();
        let mut total = 0usize;
        let pricing_catalog = self.cached_pricing_catalog();
        let now_millis = epoch_millis(SystemTime::now())?;

        let excluded = crate::trash_state::shared_deleted_ids();
        let provider_refs = self.selected_providers(filter);
        let works = self.collect_provider_work(
            &provider_refs,
            pricing_catalog.as_ref(),
            &excluded,
            force_parse,
        )?;

        // Phase 2 (sequential, DB writer): commit each provider's snapshot.
        // SQLite has a single writer mutex; serializing here avoids contention
        // and keeps each provider's transaction atomic.
        let image_service = ImageCacheService::new(&self.data_dir);
        for work in &works {
            total += self.commit_provider_work(work, aggressive, &image_service)?;
        }

        let full_reindex = filter.is_none();
        let parsed_any_session = total > 0;
        self.store_refresh_timestamps(
            full_reindex,
            full_reindex || parsed_any_session,
            now_millis,
        )?;

        // Fold the sync's WAL growth back into the main file while the app
        // is otherwise idle. Best-effort: a busy reader just leaves the WAL
        // for the next pass.
        if let Err(error) = self.db.checkpoint_truncate() {
            log::warn!("post-reindex WAL checkpoint failed: {error}");
        }

        let elapsed = start.elapsed();
        log::info!(
            "Reindex complete: {total} sessions indexed in {:.2}s",
            elapsed.as_secs_f64()
        );

        Ok(total)
    }

    fn cached_pricing_catalog(&self) -> Option<PricingCatalog> {
        match self.db.get_meta(PRICING_CATALOG_JSON_KEY) {
            Ok(Some(json)) => match pricing::parse_catalog(&json) {
                Ok(catalog) => Some(catalog),
                Err(error) => {
                    log::warn!("failed to parse cached pricing catalog: {error}");
                    None
                }
            },
            Ok(None) => None,
            Err(error) => {
                log::warn!("failed to read cached pricing catalog: {error}");
                None
            }
        }
    }

    fn selected_providers<'a>(
        &'a self,
        filter: Option<&[Provider]>,
    ) -> Vec<&'a dyn SessionProvider> {
        self.providers
            .iter()
            .filter(|provider| match filter {
                Some(allowed) => allowed.contains(&provider.provider()),
                None => true,
            })
            .map(|provider| provider.as_ref())
            .collect()
    }

    fn collect_provider_work(
        &self,
        providers: &[&dyn SessionProvider],
        pricing_catalog: Option<&PricingCatalog>,
        excluded: &HashSet<String>,
        force_parse: bool,
    ) -> ServiceResult<Vec<ProviderWork>> {
        // Phase 1 (parallel, CPU/IO): scan each provider's files and compute
        // its token-stats batch. seen_hashes dedup is per-provider already
        // (the parent-before-child ordering only matters within one provider's
        // scan), so providers don't share state and can run in parallel.
        providers
            .par_iter()
            .map(|provider| {
                self.scan_provider_work(*provider, pricing_catalog, excluded, force_parse)
            })
            .collect()
    }

    fn scan_provider_work(
        &self,
        provider: &dyn SessionProvider,
        pricing_catalog: Option<&PricingCatalog>,
        excluded: &HashSet<String>,
        force_parse: bool,
    ) -> ServiceResult<ProviderWork> {
        let provider_kind = provider.provider();
        // Pre-fetch the per-source `(size, mtime)` snapshot the provider uses
        // to short-circuit unchanged files. Each provider walks its own data
        // layout; this DB query is the single source of truth for "what we
        // already indexed". A forced parse hands the provider an empty
        // snapshot instead: every file reads as changed and gets re-parsed,
        // without the destructive mtime-zeroing the old refresh path used.
        let known = if force_parse {
            HashMap::new()
        } else {
            self.db
                .source_states_for_provider(provider_kind.key())
                .map_err(|e| {
                    ServiceError::LoadProviderSourceSnapshot(
                        provider_kind.key().to_string(),
                        e.to_string(),
                    )
                })?
        };
        let outcome = provider.scan_incremental(&known).map_err(|e| {
            ServiceError::ScanProvider(provider_kind.key().to_string(), e.to_string())
        })?;
        let mut sessions = outcome.parsed;
        let unchanged_source_paths = outcome.unchanged_source_paths;

        if !excluded.is_empty() {
            sessions.retain(|session| !excluded.contains(&session.meta.id));
        }

        let stats_batch = build_token_stats_batch(provider, &sessions, pricing_catalog);

        Ok(ProviderWork {
            provider_kind,
            sessions,
            unchanged_source_paths,
            stats_batch,
        })
    }

    fn commit_provider_work(
        &self,
        work: &ProviderWork,
        aggressive: bool,
        image_service: &ImageCacheService,
    ) -> ServiceResult<usize> {
        self.db
            .sync_provider_snapshot(
                &work.provider_kind,
                &work.sessions,
                aggressive,
                &work.unchanged_source_paths,
            )
            .map_err(|e| {
                ServiceError::SyncProvider(work.provider_kind.key().to_string(), e.to_string())
            })?;

        let batch_refs: Vec<(&str, &[TokenStatRow])> = work
            .stats_batch
            .iter()
            .map(|(id, rows)| (id.as_str(), rows.as_slice()))
            .collect();
        if let Err(e) = self.db.replace_token_stats_batch(&batch_refs) {
            log::warn!(
                "failed to write token stats batch for {}: {e}",
                work.provider_kind.key()
            );
        }

        for parsed in &work.sessions {
            image_service.cache_images(&parsed.messages);
        }

        Ok(work.sessions.len())
    }

    fn store_refresh_timestamps(
        &self,
        store_last_index_time: bool,
        store_usage_refreshed_at: bool,
        now_millis: i64,
    ) -> ServiceResult<()> {
        if store_last_index_time {
            self.db
                .set_meta("last_index_time", &now_millis.to_string())
                .map_err(|e| ServiceError::StoreLastIndexTime(e.to_string()))?;
        }
        if !store_usage_refreshed_at {
            return Ok(());
        }
        self.db
            .set_meta("usage_last_refreshed_at", &chrono::Utc::now().to_rfc3339())
            .map_err(|e| ServiceError::StoreUsageLastRefreshed(e.to_string()))?;
        Ok(())
    }

    pub fn build_tree(&self) -> ServiceResult<Vec<TreeNode>> {
        let mut sessions = self
            .db
            .list_sessions()
            .map_err(|e| ServiceError::ListSessions(e.to_string()))?;
        crate::providers::cc_mirror::hydrate_variant_names(&mut sessions);

        let mut provider_map: BTreeMap<String, BTreeMap<String, Vec<SessionMeta>>> =
            BTreeMap::new();

        for session in sessions {
            let display_key = session
                .provider
                .descriptor()
                .display_key(session.variant_name.as_deref());
            let project_key = if session.project_path.is_empty() {
                String::new()
            } else {
                session.project_path.clone()
            };

            provider_map
                .entry(display_key)
                .or_default()
                .entry(project_key)
                .or_default()
                .push(session);
        }

        let mut tree = Vec::new();

        for (display_key, projects) in &provider_map {
            let (provider_enum, label) = match Provider::parse_display_key(display_key) {
                Some(pair) => pair,
                None => continue,
            };

            let mut sorted_projects: Vec<_> = projects.iter().collect();
            sorted_projects.sort_by(|a, b| {
                let max_a = a.1.iter().map(|s| s.updated_at).max().unwrap_or(0);
                let max_b = b.1.iter().map(|s| s.updated_at).max().unwrap_or(0);
                max_b.cmp(&max_a)
            });

            let mut project_nodes = Vec::new();
            let mut provider_total = 0u32;

            for (project_path, sessions) in &sorted_projects {
                let project_label = sessions
                    .first()
                    .map(|s| {
                        if s.project_name.is_empty() {
                            "(No Project)".to_string()
                        } else {
                            s.project_name.clone()
                        }
                    })
                    .unwrap_or_else(|| "(No Project)".to_string());

                let mut top_sessions = Vec::new();
                let mut subagents = Vec::new();
                let mut children_by_parent: HashMap<&str, Vec<&SessionMeta>> = HashMap::new();

                for session in sessions.iter() {
                    if let Some(parent_id) = session.parent_id.as_deref() {
                        subagents.push(session);
                        children_by_parent
                            .entry(parent_id)
                            .or_default()
                            .push(session);
                    } else {
                        top_sessions.push(session);
                    }
                }

                let top_ids: HashSet<&str> = top_sessions.iter().map(|s| s.id.as_str()).collect();

                let mut session_nodes: Vec<TreeNode> = top_sessions
                    .iter()
                    .map(|s| {
                        let mut children =
                            children_by_parent.remove(s.id.as_str()).unwrap_or_default();
                        children.sort_by_key(|c| c.created_at);
                        let child_nodes: Vec<TreeNode> = children
                            .iter()
                            .map(|c| TreeNode {
                                id: c.id.clone(),
                                label: c.title.clone(),
                                node_type: TreeNodeType::Session,
                                children: Vec::new(),
                                count: 0,
                                provider: Some(provider_enum.clone()),
                                updated_at: Some(c.updated_at),
                                is_sidechain: true,
                                project_path: None,
                            })
                            .collect();

                        TreeNode {
                            id: s.id.clone(),
                            label: s.title.clone(),
                            node_type: TreeNodeType::Session,
                            children: child_nodes,
                            count: 0,
                            provider: Some(provider_enum.clone()),
                            updated_at: Some(s.updated_at),
                            is_sidechain: s.is_sidechain,
                            project_path: None,
                        }
                    })
                    .collect();

                for orphan in &subagents {
                    if let Some(ref pid) = orphan.parent_id {
                        if !top_ids.contains(pid.as_str()) {
                            session_nodes.push(TreeNode {
                                id: orphan.id.clone(),
                                label: orphan.title.clone(),
                                node_type: TreeNodeType::Session,
                                children: Vec::new(),
                                count: 0,
                                provider: Some(provider_enum.clone()),
                                updated_at: Some(orphan.updated_at),
                                is_sidechain: true,
                                project_path: None,
                            });
                        }
                    }
                }

                let count = session_nodes.len() as u32;
                if count == 0 {
                    continue;
                }
                provider_total += count;

                project_nodes.push(TreeNode {
                    id: format!("{display_key}:{project_path}"),
                    label: project_label,
                    node_type: TreeNodeType::Project,
                    children: session_nodes,
                    count,
                    provider: Some(provider_enum.clone()),
                    updated_at: None,
                    is_sidechain: false,
                    project_path: Some(project_path.to_string()),
                });
            }

            tree.push(TreeNode {
                id: display_key.to_string(),
                label,
                node_type: TreeNodeType::Provider,
                children: project_nodes,
                count: provider_total,
                provider: Some(provider_enum),
                updated_at: None,
                is_sidechain: false,
                project_path: None,
            });
        }

        tree.sort_by(|a, b| {
            let order_a = a
                .provider
                .as_ref()
                .map(|p| p.descriptor().sort_order())
                .unwrap_or(99);
            let order_b = b
                .provider
                .as_ref()
                .map(|p| p.descriptor().sort_order())
                .unwrap_or(99);
            order_a.cmp(&order_b).then(a.id.cmp(&b.id))
        });

        Ok(tree)
    }
}

#[cfg(test)]
mod tests;
