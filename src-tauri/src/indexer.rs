use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;

use crate::db::Database;
use crate::models::{Provider, SessionMeta, TreeNode, TreeNodeType};
use crate::pricing::{self, PRICING_CATALOG_JSON_KEY};
use crate::provider::{ParsedSession, SessionProvider, TokenStatRow};
use crate::services::image_cache::{image_cache_provider_for, ImageCacheService};

#[derive(Clone)]
pub struct Indexer {
    db: Arc<Database>,
    providers: Arc<Vec<Box<dyn SessionProvider>>>,
    data_dir: PathBuf,
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

    pub fn reindex(&self) -> Result<usize, String> {
        self.reindex_filtered(None, true)
    }

    pub fn reindex_providers(
        &self,
        filter: Option<&[Provider]>,
        aggressive: bool,
    ) -> Result<usize, String> {
        self.reindex_filtered(filter, aggressive)
    }

    fn reindex_filtered(
        &self,
        filter: Option<&[Provider]>,
        aggressive: bool,
    ) -> Result<usize, String> {
        let start = Instant::now();
        let mut total = 0usize;
        let pricing_catalog = match self.db.get_meta(PRICING_CATALOG_JSON_KEY) {
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
        };

        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .expect("system clock is before the UNIX epoch");

        let excluded = crate::trash_state::shared_deleted_ids();

        // Phase 1 (parallel, CPU/IO): scan each provider's files and compute
        // its token-stats batch. seen_hashes dedup is per-provider already
        // (the parent-before-child ordering only matters within one provider's
        // scan), so providers don't share state and can run in parallel.
        struct ProviderWork {
            provider_kind: Provider,
            sessions: Vec<ParsedSession>,
            stats_batch: Vec<(String, Vec<TokenStatRow>)>,
        }

        let provider_refs: Vec<&Box<dyn SessionProvider>> = self
            .providers
            .iter()
            .filter(|p| match filter {
                Some(allowed) => allowed.contains(&p.provider()),
                None => true,
            })
            .collect();

        let works: Result<Vec<ProviderWork>, String> = provider_refs
            .par_iter()
            .map(|provider| -> Result<ProviderWork, String> {
                let provider_kind = provider.provider();
                let mut sessions = provider.scan_all().map_err(|e| {
                    format!("failed to scan {} provider: {}", provider_kind.key(), e)
                })?;

                if !excluded.is_empty() {
                    sessions.retain(|s| !excluded.contains(&s.meta.id));
                }

                // Parent-before-child ordering so cross-file dedup attributes
                // overlapping usage entries to the parent.
                let (parents, children): (Vec<&ParsedSession>, Vec<&ParsedSession>) = sessions
                    .iter()
                    .partition(|parsed| parsed.meta.parent_id.is_none());

                let mut seen_hashes: HashSet<String> = HashSet::new();
                let mut stats_batch: Vec<(String, Vec<TokenStatRow>)> =
                    Vec::with_capacity(sessions.len());
                for parsed in parents.iter().chain(children.iter()) {
                    let stat_rows = provider.compute_token_stats(
                        parsed,
                        pricing_catalog.as_ref(),
                        Some(&mut seen_hashes),
                    );
                    stats_batch.push((parsed.meta.id.clone(), stat_rows));
                }

                Ok(ProviderWork {
                    provider_kind,
                    sessions,
                    stats_batch,
                })
            })
            .collect();
        let works = works?;

        // Phase 2 (sequential, DB writer): commit each provider's snapshot.
        // SQLite has a single writer mutex; serializing here avoids contention
        // and keeps each provider's transaction atomic.
        for ProviderWork {
            provider_kind,
            sessions,
            stats_batch,
        } in &works
        {
            let count = sessions.len();
            self.db
                .sync_provider_snapshot(provider_kind, sessions, aggressive)
                .map_err(|e| format!("failed to sync {} provider: {}", provider_kind.key(), e))?;

            let batch_refs: Vec<(&str, &[TokenStatRow])> = stats_batch
                .iter()
                .map(|(id, rows)| (id.as_str(), rows.as_slice()))
                .collect();
            if let Err(e) = self.db.replace_token_stats_batch(&batch_refs) {
                log::warn!(
                    "failed to write token stats batch for {}: {e}",
                    provider_kind.key()
                );
            }

            if let Some(cache_provider) = image_cache_provider_for(provider_kind) {
                let image_service = ImageCacheService::new(&self.data_dir);
                for parsed in sessions {
                    image_service.cache_images(cache_provider.as_ref(), &parsed.messages);
                }
            }

            total += count;
        }

        if filter.is_none() {
            self.db
                .set_meta("last_index_time", &now_millis.to_string())
                .map_err(|e| format!("failed to store last_index_time: {e}"))?;
        }
        self.db
            .set_meta("usage_last_refreshed_at", &chrono::Utc::now().to_rfc3339())
            .map_err(|e| format!("failed to store usage_last_refreshed_at: {e}"))?;

        let elapsed = start.elapsed();
        log::info!(
            "Reindex complete: {} sessions indexed in {:.2}s",
            total,
            elapsed.as_secs_f64(),
        );

        Ok(total)
    }

    pub fn build_tree(&self) -> Result<Vec<TreeNode>, String> {
        let mut sessions = self
            .db
            .list_sessions()
            .map_err(|e| format!("failed to list sessions: {e}"))?;
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

                let (top_sessions, subagents): (Vec<_>, Vec<_>) =
                    sessions.iter().partition(|s| s.parent_id.is_none());

                let top_ids: std::collections::HashSet<&str> =
                    top_sessions.iter().map(|s| s.id.as_str()).collect();

                let mut session_nodes: Vec<TreeNode> = top_sessions
                    .iter()
                    .map(|s| {
                        let mut children: Vec<_> = sessions
                            .iter()
                            .filter(|c| c.parent_id.as_deref() == Some(&s.id))
                            .collect();
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
mod tests {
    use crate::models::{Message, MessageRole, Provider, SessionMeta, TokenUsage};
    use crate::pricing::PricingCatalog;
    use crate::provider::{default_compute_token_stats_from_messages, ParsedSession, TokenStatRow};
    use std::collections::HashSet;

    /// Drives the default per-message aggregation path that all
    /// non-Codex providers use through the trait. Tests stay focused on
    /// the dedup/timestamp/model logic without dragging in a real
    /// provider runtime.
    fn compute_token_stats(parsed: &ParsedSession) -> Vec<TokenStatRow> {
        default_compute_token_stats_from_messages(parsed, None, None)
    }

    fn compute_token_stats_with_catalog_dedup(
        parsed: &ParsedSession,
        pricing_catalog: Option<&PricingCatalog>,
        seen_hashes: &mut HashSet<String>,
    ) -> Vec<TokenStatRow> {
        default_compute_token_stats_from_messages(parsed, pricing_catalog, Some(seen_hashes))
    }

    fn make_session(meta_model: Option<&str>, messages: Vec<Message>) -> ParsedSession {
        ParsedSession {
            meta: SessionMeta {
                id: "session-1".into(),
                provider: Provider::Claude,
                title: "Test".into(),
                project_path: "/tmp/project".into(),
                project_name: "project".into(),
                created_at: 1_775_635_200,
                updated_at: 1_775_635_200,
                message_count: messages.len() as u32,
                file_size_bytes: 0,
                source_path: "/tmp/source.jsonl".into(),
                is_sidechain: false,
                variant_name: None,
                model: meta_model.map(str::to_string),
                cc_version: None,
                git_branch: None,
                parent_id: None,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            messages,
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
        }
    }

    fn token_usage(input: u32, output: u32) -> Option<TokenUsage> {
        Some(TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        })
    }

    #[test]
    fn compute_token_stats_skips_usage_without_message_model() {
        let parsed = make_session(
            Some("claude-opus-4-6"),
            vec![Message {
                role: MessageRole::Assistant,
                content: String::new(),
                timestamp: Some("2026-04-09T12:00:00Z".into()),
                tool_name: None,
                tool_input: None,
                token_usage: token_usage(100, 50),
                model: None,
                usage_hash: None,
                tool_metadata: None,
            }],
        );

        let rows = compute_token_stats(&parsed);
        assert!(rows.is_empty());
    }

    #[test]
    fn compute_token_stats_skips_usage_without_message_timestamp() {
        let parsed = make_session(
            Some("gpt-5.4"),
            vec![Message {
                role: MessageRole::Assistant,
                content: String::new(),
                timestamp: None,
                tool_name: None,
                tool_input: None,
                token_usage: token_usage(25, 10),
                model: None,
                usage_hash: None,
                tool_metadata: None,
            }],
        );

        let rows = compute_token_stats(&parsed);
        assert!(rows.is_empty());
    }

    #[test]
    fn compute_token_stats_skips_tool_usage_without_explicit_message_model() {
        let parsed = make_session(
            Some("claude-haiku-4-5-20251001"),
            vec![
                Message {
                    role: MessageRole::Assistant,
                    content: String::new(),
                    timestamp: Some("2026-04-09T12:00:00Z".into()),
                    tool_name: None,
                    tool_input: None,
                    token_usage: None,
                    model: Some("claude-opus-4-6".into()),
                    usage_hash: None,
                    tool_metadata: None,
                },
                Message {
                    role: MessageRole::Tool,
                    content: String::new(),
                    timestamp: Some("2026-04-09T12:00:01Z".into()),
                    tool_name: Some("Bash".into()),
                    tool_input: None,
                    token_usage: token_usage(100, 50),
                    model: None,
                    usage_hash: None,
                    tool_metadata: None,
                },
            ],
        );

        let rows = compute_token_stats(&parsed);
        assert!(rows.is_empty());
    }

    #[test]
    fn compute_token_stats_groups_dates_in_local_timezone() {
        let ts = "2026-04-08T16:30:00Z";
        let expected_date = chrono::DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();

        let parsed = make_session(
            Some("claude-opus-4-6"),
            vec![Message {
                role: MessageRole::Assistant,
                content: String::new(),
                timestamp: Some(ts.into()),
                tool_name: None,
                tool_input: None,
                token_usage: token_usage(10, 5),
                model: Some("claude-opus-4-6".into()),
                usage_hash: None,
                tool_metadata: None,
            }],
        );

        let rows = compute_token_stats(&parsed);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, expected_date);
    }

    #[test]
    fn compute_token_stats_dedups_same_usage_hash_across_sessions() {
        let make_message = || Message {
            role: MessageRole::Assistant,
            content: String::new(),
            timestamp: Some("2026-04-09T12:00:00Z".into()),
            tool_name: None,
            tool_input: None,
            token_usage: token_usage(100, 50),
            model: Some("claude-opus-4-6".into()),
            usage_hash: Some("msg-1:req-1".into()),
            tool_metadata: None,
        };

        let first = make_session(Some("claude-opus-4-6"), vec![make_message()]);
        let second = make_session(Some("claude-opus-4-6"), vec![make_message()]);
        let mut seen_hashes = HashSet::new();

        let first_rows = compute_token_stats_with_catalog_dedup(&first, None, &mut seen_hashes);
        let second_rows = compute_token_stats_with_catalog_dedup(&second, None, &mut seen_hashes);

        assert_eq!(first_rows.len(), 1);
        assert!(second_rows.is_empty());
    }

    #[test]
    fn compute_token_stats_skips_synthetic_model() {
        // Claude emits usage entries with model="<synthetic>" as internal
        // placeholders. They should be excluded from usage aggregates to
        // match ccusage's behavior.
        let parsed = make_session(
            Some("<synthetic>"),
            vec![Message {
                role: MessageRole::Assistant,
                content: String::new(),
                timestamp: Some("2026-04-09T12:00:00Z".into()),
                tool_name: None,
                tool_input: None,
                token_usage: token_usage(500, 200),
                model: Some("<synthetic>".into()),
                usage_hash: Some("msg-x:req-x".into()),
                tool_metadata: None,
            }],
        );

        let rows = compute_token_stats(&parsed);
        assert!(
            rows.is_empty(),
            "<synthetic> entries must not contribute to token stats"
        );
    }
}
