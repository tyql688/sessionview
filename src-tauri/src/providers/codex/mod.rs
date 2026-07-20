pub mod parser;
mod tools;

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::WalkDir;

use std::collections::{HashMap, HashSet};

use crate::models::Provider;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
    partition_files_by_freshness,
};

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
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

    fn session_index_path(&self) -> PathBuf {
        self.home_dir.join(".codex").join("session_index.jsonl")
    }

    /// Load `~/.codex/session_index.jsonl`, the sidecar where Codex
    /// persists thread names (auto-generated on session start, rewritten
    /// on rename). Append-only JSONL of
    /// `{"id", "thread_name", "updated_at"}` — later lines win. A missing
    /// file is normal (older Codex versions never wrote one); malformed
    /// lines are skipped with a warning, never guessed at.
    pub(crate) fn load_session_index(&self) -> HashMap<String, String> {
        let path = self.session_index_path();
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
            Err(error) => {
                log::warn!(
                    "failed to read Codex session index '{}': {error}",
                    path.display()
                );
                return HashMap::new();
            }
        };
        let mut titles = HashMap::new();
        let mut malformed = 0usize;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
                malformed += 1;
                continue;
            };
            let id = entry.get("id").and_then(|v| v.as_str());
            let name = entry
                .get("thread_name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|name| !name.is_empty());
            match (id, name) {
                (Some(id), Some(name)) => {
                    titles.insert(id.to_string(), name.to_string());
                }
                _ => malformed += 1,
            }
        }
        if malformed > 0 {
            log::warn!(
                "skipped {malformed} malformed line(s) in Codex session index '{}'",
                path.display()
            );
        }
        titles
    }

    fn archived_sessions_dir(&self) -> PathBuf {
        self.home_dir.join(".codex").join("archived_sessions")
    }

    fn collect_jsonl_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut seen_relative_paths = HashSet::new();
        for dir in [self.sessions_dir(), self.archived_sessions_dir()] {
            if !dir.exists() {
                continue;
            }
            let mut dir_files: Vec<_> = WalkDir::new(&dir)
                .into_iter()
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.into_path())
                .filter(|path| {
                    path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
                })
                .collect();
            dir_files.sort();
            for path in dir_files {
                let Ok(relative) = path.strip_prefix(&dir) else {
                    log::warn!(
                        "skipping Codex session outside scan root: '{}'",
                        path.display()
                    );
                    continue;
                };
                if seen_relative_paths.insert(relative.to_path_buf()) {
                    files.push(path);
                }
            }
        }
        files
    }
}

/// Extract the session uuid embedded at the end of a rollout filename
/// (`rollout-<timestamp>-<uuid>.jsonl`), used to match files against
/// `session_index.jsonl` entries without opening them. Returns `None`
/// when the name doesn't end in a well-formed uuid.
fn session_uuid_from_filename(path: &str) -> Option<String> {
    let stem = Path::new(path).file_stem()?.to_str()?;
    let uuid = stem.get(stem.len().checked_sub(36)?..)?;
    let valid = uuid.char_indices().all(|(i, c)| match i {
        8 | 13 | 18 | 23 => c == '-',
        _ => c.is_ascii_hexdigit(),
    });
    valid.then(|| uuid.to_ascii_lowercase())
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

        let index_titles = self.load_session_index();
        let sessions: Vec<ParsedSession> = files
            .par_iter()
            .filter_map(|path| self.parse_session_file_with_index(path, &index_titles))
            .collect();

        Ok(sessions)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        let files = self.collect_jsonl_files();
        let index_titles = self.load_session_index();
        let (mut to_parse, mut unchanged_source_paths) = partition_files_by_freshness(files, known);
        // A rename only rewrites `session_index.jsonl` — the rollout file
        // itself keeps its (size, mtime). Promote unchanged files whose
        // stored provider-derived title disagrees with the index so the
        // new name lands; user-customized titles carry `title: None` and
        // are never promoted (upsert preserves them anyway).
        unchanged_source_paths.retain(|path_str| {
            let stale = session_uuid_from_filename(path_str)
                .and_then(|id| index_titles.get(&id))
                .zip(known.get(path_str).and_then(|state| state.title.as_ref()))
                .is_some_and(|(index_title, stored_title)| index_title != stored_title);
            if stale {
                to_parse.push(PathBuf::from(path_str.as_str()));
            }
            !stale
        });
        let parsed: Vec<ParsedSession> = to_parse
            .par_iter()
            .filter_map(|path| self.parse_session_file_with_index(path, &index_titles))
            .collect();
        Ok(ScanOutcome {
            parsed,
            unchanged_source_paths,
        })
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

        Ok(LoadedSession::from_parsed(parsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_rollout(dir: &std::path::Path, uuid: &str, first_user: &str) -> PathBuf {
        let sessions = dir.join(".codex").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join(format!("rollout-2026-04-10T10-00-00-{uuid}.jsonl"));
        std::fs::write(
            &file,
            format!(
                concat!(
                    "{{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{uuid}\",\"cwd\":\"/tmp/project\"}}}}\n",
                    "{{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"{first_user}\"}}]}}}}\n"
                ),
                uuid = uuid,
                first_user = first_user
            ),
        )
        .unwrap();
        file
    }

    #[test]
    fn collect_jsonl_files_prefers_active_copy() {
        let dir = tempfile::tempdir().unwrap();
        let relative = Path::new("2026/07/rollout.jsonl");
        for root in ["sessions", "archived_sessions"] {
            let path = dir.path().join(".codex").join(root).join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "{}").unwrap();
        }
        let provider = CodexProvider {
            home_dir: dir.path().to_path_buf(),
        };

        let files = provider.collect_jsonl_files();
        assert_eq!(files.len(), 1);
        assert!(files[0].starts_with(provider.sessions_dir()));
    }

    #[test]
    fn session_uuid_from_filename_extracts_trailing_uuid() {
        assert_eq!(
            session_uuid_from_filename(
                "/x/rollout-2026-07-13T21-21-26-019F5BA3-c290-79d2-9a89-370b905ec017.jsonl"
            )
            .as_deref(),
            Some("019f5ba3-c290-79d2-9a89-370b905ec017")
        );
        assert_eq!(session_uuid_from_filename("/x/short.jsonl"), None);
        assert_eq!(
            session_uuid_from_filename(
                "/x/rollout-2026-07-13T21-21-26-zzzz5ba3-c290-79d2-9a89-370b905ec017.jsonl"
            ),
            None
        );
    }

    #[test]
    fn load_session_index_last_wins_and_skips_malformed() {
        use tempfile::TempDir;

        let home = TempDir::new().unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::fs::write(
            home.path().join(".codex").join("session_index.jsonl"),
            concat!(
                "{\"id\":\"aaa\",\"thread_name\":\"Old name\",\"updated_at\":\"2026-04-10T10:00:00Z\"}\n",
                "not json\n",
                "{\"id\":\"bbb\",\"thread_name\":\"  \",\"updated_at\":\"2026-04-10T10:00:00Z\"}\n",
                "{\"id\":\"aaa\",\"thread_name\":\"New name\",\"updated_at\":\"2026-04-11T10:00:00Z\"}\n"
            ),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: home.path().to_path_buf(),
        };
        let titles = provider.load_session_index();
        assert_eq!(titles.get("aaa").map(String::as_str), Some("New name"));
        assert!(!titles.contains_key("bbb"), "blank names must be skipped");
        assert_eq!(titles.len(), 1);
    }

    #[test]
    fn parse_session_prefers_index_title_over_first_user_message() {
        use tempfile::TempDir;

        let home = TempDir::new().unwrap();
        let uuid = "019f0000-0000-7000-8000-000000000001";
        let file = write_rollout(home.path(), uuid, "please do the thing");
        std::fs::write(
            home.path().join(".codex").join("session_index.jsonl"),
            format!("{{\"id\":\"{uuid}\",\"thread_name\":\"Indexed title\",\"updated_at\":\"2026-04-10T10:00:02Z\"}}\n"),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: home.path().to_path_buf(),
        };
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        assert_eq!(parsed.meta.title, "Indexed title");

        // Without an index entry the first user message keeps winning.
        std::fs::remove_file(home.path().join(".codex").join("session_index.jsonl")).unwrap();
        let parsed = provider.parse_session_file(&file).expect("parsed session");
        assert_eq!(parsed.meta.title, "please do the thing");
    }

    #[test]
    fn scan_incremental_reparses_unchanged_file_when_index_title_differs() {
        use tempfile::TempDir;

        let home = TempDir::new().unwrap();
        let uuid = "019f0000-0000-7000-8000-000000000002";
        let file = write_rollout(home.path(), uuid, "hello there");
        std::fs::write(
            home.path().join(".codex").join("session_index.jsonl"),
            format!("{{\"id\":\"{uuid}\",\"thread_name\":\"Renamed thread\",\"updated_at\":\"2026-04-10T10:00:02Z\"}}\n"),
        )
        .unwrap();

        let provider = CodexProvider {
            home_dir: home.path().to_path_buf(),
        };
        let path_str = file.to_string_lossy().to_string();
        let metadata = std::fs::metadata(&file).unwrap();
        let mtime = crate::provider::system_time_to_epoch_seconds(metadata.modified().unwrap())
            .expect("mtime after epoch");
        let fresh_state = |title: Option<&str>| SourceState {
            size: metadata.len(),
            mtime,
            title: title.map(str::to_string),
        };

        // Stored title still the first-user-message fallback → promoted.
        let known = HashMap::from([(path_str.clone(), fresh_state(Some("hello there")))]);
        let outcome = provider.scan_incremental(&known).expect("scan");
        assert_eq!(outcome.parsed.len(), 1);
        assert_eq!(outcome.parsed[0].meta.title, "Renamed thread");
        assert!(outcome.unchanged_source_paths.is_empty());

        // Stored title already matches the index → skipped, converged.
        let known = HashMap::from([(path_str.clone(), fresh_state(Some("Renamed thread")))]);
        let outcome = provider.scan_incremental(&known).expect("scan");
        assert!(outcome.parsed.is_empty());
        assert_eq!(outcome.unchanged_source_paths, vec![path_str.clone()]);

        // User-customized title (None) must never be promoted.
        let known = HashMap::from([(path_str.clone(), fresh_state(None))]);
        let outcome = provider.scan_incremental(&known).expect("scan");
        assert!(outcome.parsed.is_empty());
        assert_eq!(outcome.unchanged_source_paths, vec![path_str]);
    }

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
        // The source input count includes cache; normalized rows keep it disjoint.
        assert_eq!(r.input_tokens, 400, "input must exclude cached tokens");
        assert_eq!(r.cache_read_tokens, 600);
        assert_eq!(
            r.input_tokens + r.cache_read_tokens,
            1000,
            "input + cache_read must not double-count cached tokens"
        );
        assert_eq!(r.output_tokens, 50);
    }
}
