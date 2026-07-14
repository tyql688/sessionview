pub mod parser;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::models::{Provider, SessionMeta, TokenTotals};
use crate::pricing::{self, PricingCatalog};
use crate::provider::{
    partition_files_by_freshness, timestamp_to_local_date, ChildPlan, DeletionPlan, FileAction,
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
    TokenStatRow,
};

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    fn owns_source_path(&self, source_path: &str) -> bool {
        source_path.replace('\\', "/").contains("/.grok/sessions/")
    }
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        Some(format!("grok --resume {session_id}"))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "grok".into()
    }
    fn sort_order(&self) -> u32 {
        11
    }
    fn color(&self) -> &'static str {
        "#27272a"
    }
    fn cli_command(&self) -> &'static str {
        "grok"
    }
}

pub struct GrokProvider {
    grok_dir: PathBuf,
}

impl GrokProvider {
    pub fn new() -> Option<Self> {
        let home_dir = dirs::home_dir()?;
        Some(Self {
            grok_dir: home_dir.join(".grok"),
        })
    }

    /// Build a provider rooted at an arbitrary directory instead of
    /// `~/.grok`. Used by tests to point at fixture trees.
    pub fn with_root(grok_dir: PathBuf) -> Self {
        Self { grok_dir }
    }

    fn sessions_dir(&self) -> PathBuf {
        self.grok_dir.join("sessions")
    }

    /// Collect `<sessions_dir>/<url-encoded-cwd>/<session-uuid>/chat_history.jsonl`
    /// by walking exactly two directory levels.
    fn collect_chat_files(&self) -> Vec<PathBuf> {
        let sessions_dir = self.sessions_dir();
        let cwd_dirs = match std::fs::read_dir(&sessions_dir) {
            Ok(dirs) => dirs,
            Err(error) => {
                if sessions_dir.exists() {
                    log::warn!(
                        "cannot read Grok sessions dir '{}': {error}",
                        sessions_dir.display()
                    );
                }
                return Vec::new();
            }
        };

        let mut files = Vec::new();
        for cwd_entry in cwd_dirs.filter_map(Result::ok) {
            let cwd_dir = cwd_entry.path();
            if !cwd_dir.is_dir() {
                continue;
            }
            let session_dirs = match std::fs::read_dir(&cwd_dir) {
                Ok(dirs) => dirs,
                Err(_) => continue,
            };
            for session_entry in session_dirs.filter_map(Result::ok) {
                let chat_path = session_entry.path().join("chat_history.jsonl");
                if chat_path.is_file() {
                    files.push(chat_path);
                }
            }
        }
        files
    }
}

impl SessionProvider for GrokProvider {
    fn provider(&self) -> Provider {
        Provider::Grok
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        vec![self.sessions_dir()]
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let files = self.collect_chat_files();
        let sessions: Vec<ParsedSession> = files
            .par_iter()
            .filter_map(|path| parser::parse_session_file(path))
            .collect();
        Ok(sessions)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        let files = self.collect_chat_files();
        let (mut to_parse, mut unchanged_source_paths) = partition_files_by_freshness(files, known);
        // A rename rewrites only summary.json; promote unchanged chat files
        // whose stored title disagrees (user-customized titles are None and
        // never promoted) — same pattern as Codex's session_index check.
        unchanged_source_paths.retain(|path_str| {
            let stale = Path::new(path_str)
                .parent()
                .and_then(parser::derive_title_of)
                .zip(known.get(path_str).and_then(|state| state.title.as_ref()))
                .is_some_and(|(summary_title, stored_title)| &summary_title != stored_title);
            if stale {
                to_parse.push(PathBuf::from(path_str.as_str()));
            }
            !stale
        });
        let parsed: Vec<ParsedSession> = to_parse
            .par_iter()
            .filter_map(|path| parser::parse_session_file(path))
            .collect();
        Ok(ScanOutcome {
            parsed,
            unchanged_source_paths,
        })
    }

    fn scan_source(&self, source_path: &str) -> Result<Vec<ParsedSession>, ProviderError> {
        let path = PathBuf::from(source_path);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let session = parser::parse_session_file(&path).ok_or_else(|| {
            ProviderError::Parse(format!(
                "failed to parse Grok session file '{}'",
                path.display()
            ))
        })?;
        Ok(vec![session])
    }

    fn load_messages(
        &self,
        session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        let path = PathBuf::from(source_path);
        // Live sessions rewrite summary.json / chat_history.jsonl in place;
        // a failed read is usually that race — retry before erroring.
        let mut parsed = parser::parse_session_file(&path);
        for _ in 0..2 {
            if parsed.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(60));
            parsed = parser::parse_session_file(&path);
        }
        let parsed = parsed.ok_or_else(|| {
            ProviderError::Parse(format!(
                "failed to parse Grok session {session_id} from {source_path}"
            ))
        })?;
        // Totals from usage_events; input excludes the cached portion.
        let token_totals =
            parsed
                .usage_events
                .iter()
                .fold(TokenTotals::default(), |mut totals, event| {
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

    /// Aggregate from per-turn `usage_events` (Codex pattern); store only the
    /// non-cached input so input/cache_read stay disjoint.
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

    fn deletion_plan(&self, meta: &SessionMeta, children: &[SessionMeta]) -> DeletionPlan {
        // Trash the chat file (restorable), sweep the whole session dir.
        // Subagent children are sibling dirs and get the same treatment.
        let session_dir_of = |source_path: &str| {
            Path::new(source_path)
                .parent()
                .filter(|dir| dir.is_dir())
                .map(Path::to_path_buf)
        };
        let mut cleanup_dirs: Vec<_> = session_dir_of(&meta.source_path).into_iter().collect();
        let child_plans: Vec<ChildPlan> = children
            .iter()
            .map(|child| {
                cleanup_dirs.extend(session_dir_of(&child.source_path));
                ChildPlan {
                    id: child.id.clone(),
                    source_path: child.source_path.clone(),
                    title: child.title.clone(),
                    file_action: FileAction::Remove,
                }
            })
            .collect();
        DeletionPlan {
            file_action: FileAction::Remove,
            child_plans,
            cleanup_dirs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderDescriptor;

    #[test]
    fn descriptor_owns_source_path() {
        let descriptor = Descriptor;
        assert!(descriptor.owns_source_path(
            "/Users/test/.grok/sessions/%2Ftmp%2Fproj/01900000-aaaa-bbbb-cccc-000000000000/chat_history.jsonl"
        ));
        assert!(!descriptor.owns_source_path("/Users/test/.claude/projects/proj/session.jsonl"));
    }

    #[test]
    fn descriptor_resume_command() {
        let descriptor = Descriptor;
        assert_eq!(
            descriptor.resume_command("abc123", None),
            Some("grok --resume abc123".to_string())
        );
    }

    #[test]
    fn descriptor_display_key() {
        assert_eq!(Descriptor.display_key(None), "grok");
    }

    #[test]
    fn scan_source_returns_empty_when_file_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = GrokProvider::with_root(tmp.path().to_path_buf());
        let missing = tmp.path().join("missing").join("chat_history.jsonl");
        let sessions = provider.scan_source(missing.to_str().unwrap()).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn deletion_plan_removes_whole_session_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("sessions").join("%2Ftmp%2Fp").join("s-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let source = session_dir.join("chat_history.jsonl");
        std::fs::write(&source, "").unwrap();

        let provider = GrokProvider::with_root(tmp.path().to_path_buf());
        let meta = SessionMeta {
            id: "s-1".to_string(),
            provider: Provider::Grok,
            title: "t".to_string(),
            project_path: "/tmp/p".to_string(),
            project_name: "p".to_string(),
            created_at: 1,
            updated_at: 2,
            message_count: 1,
            file_size_bytes: 0,
            source_path: source.to_string_lossy().to_string(),
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

        let plan = provider.deletion_plan(&meta, &[]);
        assert_eq!(plan.file_action, FileAction::Remove);
        assert!(plan.child_plans.is_empty());
        assert_eq!(plan.cleanup_dirs, vec![session_dir]);
    }
}
