pub mod parser;
mod tools;

use std::path::PathBuf;

use rayon::prelude::*;
use walkdir::WalkDir;

use std::collections::{HashMap, HashSet};

use crate::models::{Provider, SessionMeta, TokenTotals};
use crate::pricing::{self, PricingCatalog};
use crate::provider::{
    jsonl_subagents_deletion_plan, partition_files_by_freshness, timestamp_to_local_date,
    DeletionPlan, LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider,
    SourceState, TokenStatRow,
};

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    fn owns_source_path(&self, source_path: &str) -> bool {
        source_path.replace('\\', "/").contains("/.codex/sessions/")
    }
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        Some(format!("codex resume {session_id}"))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "codex".into()
    }
    fn sort_order(&self) -> u32 {
        2
    }
    fn color(&self) -> &'static str {
        "#10b981"
    }
    fn cli_command(&self) -> &'static str {
        "codex"
    }
}

pub struct CodexProvider {
    home_dir: PathBuf,
}

impl CodexProvider {
    pub fn new() -> Option<Self> {
        let home_dir = dirs::home_dir()?;
        Some(Self { home_dir })
    }

    fn sessions_dir(&self) -> PathBuf {
        self.home_dir.join(".codex").join("sessions")
    }

    fn archived_sessions_dir(&self) -> PathBuf {
        self.home_dir.join(".codex").join("archived_sessions")
    }

    fn collect_jsonl_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for dir in [self.sessions_dir(), self.archived_sessions_dir()] {
            if !dir.exists() {
                continue;
            }
            for entry in WalkDir::new(&dir)
                .into_iter()
                .filter_map(std::result::Result::ok)
            {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    files.push(path.to_path_buf());
                }
            }
        }
        files
    }
}

impl SessionProvider for CodexProvider {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        vec![self.sessions_dir(), self.archived_sessions_dir()]
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let files = self.collect_jsonl_files();
        if files.is_empty() {
            return Ok(Vec::new());
        }

        let sessions: Vec<ParsedSession> = files
            .par_iter()
            .filter_map(|path| self.parse_session_file(path))
            .collect();

        Ok(sessions)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        let files = self.collect_jsonl_files();
        let (to_parse, unchanged_source_paths) = partition_files_by_freshness(files, known);
        let parsed: Vec<ParsedSession> = to_parse
            .par_iter()
            .filter_map(|path| self.parse_session_file(path))
            .collect();
        Ok(ScanOutcome {
            parsed,
            unchanged_source_paths,
        })
    }

    fn scan_source(&self, source_path: &str) -> Result<Vec<ParsedSession>, ProviderError> {
        let path = PathBuf::from(source_path);
        Ok(self.parse_session_file(&path).into_iter().collect())
    }

    fn deletion_plan(&self, meta: &SessionMeta, children: &[SessionMeta]) -> DeletionPlan {
        jsonl_subagents_deletion_plan(meta, children)
    }

    fn load_messages(
        &self,
        _session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        let path = PathBuf::from(source_path);

        let parsed = self.parse_session_file(&path).ok_or_else(|| {
            ProviderError::Parse(format!(
                "failed to parse Codex session file '{}'",
                path.display()
            ))
        })?;

        let token_totals =
            parsed
                .usage_events
                .iter()
                .fold(TokenTotals::default(), |mut totals, event| {
                    // event.input_tokens includes cached tokens; store only the
                    // non-cached part so input/cache_read stay disjoint (no
                    // double-count), consistent with compute_token_stats.
                    totals.input_tokens += event
                        .input_tokens
                        .saturating_sub(event.cache_read_input_tokens);
                    totals.output_tokens += event.output_tokens;
                    totals.cache_read_tokens += event.cache_read_input_tokens;
                    totals
                });
        let mut loaded = LoadedSession::from_parsed(parsed);
        loaded.token_totals = token_totals;
        Ok(loaded)
    }

    /// Codex emits per-turn token counts as `event_msg.token_count` lines
    /// that aren't tied to any single message. Aggregate from
    /// `parsed.usage_events` (captured during the parse pass) instead of
    /// walking `messages[].token_usage` like the default impl. Dedup is a
    /// no-op here because Codex usage events don't carry a hash and don't
    /// duplicate across files.
    fn compute_token_stats(
        &self,
        parsed: &ParsedSession,
        pricing_catalog: Option<&PricingCatalog>,
        _seen_hashes: Option<&mut HashSet<String>>,
    ) -> Vec<TokenStatRow> {
        let mut stats_map: HashMap<(String, String), TokenStatRow> = HashMap::with_capacity(16);
        for event in &parsed.usage_events {
            let Some(date) = timestamp_to_local_date(&event.timestamp) else {
                continue;
            };
            let entry = stats_map
                .entry((date.clone(), event.model.clone()))
                .or_insert_with(|| TokenStatRow {
                    date,
                    model: event.model.clone(),
                    turn_count: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    cost_usd: 0.0,
                });
            entry.turn_count += 1;
            // Codex's event.input_tokens INCLUDES the cached tokens, so store
            // only the non-cached part — keeping input_tokens and
            // cache_read_tokens disjoint, like Claude. Otherwise every token
            // aggregate that sums input+cache_read double-counts the cached
            // portion (≈2x inflation for cache-heavy Codex sessions). Cost is
            // unaffected: it was already computed from non_cached_input.
            let non_cached_input = event
                .input_tokens
                .saturating_sub(event.cache_read_input_tokens);
            entry.input_tokens += non_cached_input;
            entry.output_tokens += event.output_tokens;
            entry.cache_read_tokens += event.cache_read_input_tokens;
            entry.cost_usd += pricing::estimate_cost_with_catalog(
                pricing_catalog,
                &entry.model,
                non_cached_input,
                event.output_tokens,
                event.cache_read_input_tokens,
                0,
            );
        }

        stats_map.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_token_stats_stores_non_cached_input_no_double_count() {
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let file = dir.path().join("codex.jsonl");
        fs::write(
            &file,
            concat!(
                "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n",
                "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        let rows = provider.compute_token_stats(&parsed, None, None);

        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        // event.input_tokens=1000 INCLUDES the 600 cached tokens; the stored
        // input must be the non-cached 400 so input + cache_read = 1000 (the true
        // context), not 1600 — otherwise every token aggregate double-counts.
        assert_eq!(r.input_tokens, 400, "input must exclude cached tokens");
        assert_eq!(r.cache_read_tokens, 600);
        assert_eq!(
            r.input_tokens + r.cache_read_tokens,
            1000,
            "input + cache_read must not double-count cached tokens"
        );
        assert_eq!(r.output_tokens, 50);
    }

    #[test]
    fn codex_parent_deletion_plan_includes_children() {
        let provider = CodexProvider {
            home_dir: PathBuf::from("/tmp"),
        };

        let parent = SessionMeta {
            id: "parent".to_string(),
            provider: Provider::Codex,
            title: "parent".to_string(),
            project_path: String::new(),
            project_name: String::new(),
            created_at: 0,
            updated_at: 0,
            message_count: 0,
            file_size_bytes: 0,
            source_path: "/tmp/parent.jsonl".to_string(),
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

        let child = SessionMeta {
            id: "child".to_string(),
            provider: Provider::Codex,
            title: "child".to_string(),
            project_path: String::new(),
            project_name: String::new(),
            created_at: 0,
            updated_at: 0,
            message_count: 0,
            file_size_bytes: 0,
            source_path: "/tmp/child.jsonl".to_string(),
            is_sidechain: true,
            variant_name: None,
            model: None,
            cc_version: None,
            git_branch: None,
            parent_id: Some("parent".to_string()),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };

        let plan = provider.deletion_plan(&parent, &[child]);
        assert_eq!(plan.child_plans.len(), 1);
        assert_eq!(plan.child_plans[0].id, "child");
    }
}
