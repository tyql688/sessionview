pub(crate) mod images;
pub mod parser;

use std::fs;
use std::path::PathBuf;

use rayon::prelude::*;

use std::collections::HashMap;

use crate::models::Provider;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
    partition_files_by_freshness,
};

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        Some(format!("claude --resume {session_id}"))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "claude".into()
    }
    fn sort_order(&self) -> u32 {
        0
    }
    fn color(&self) -> &'static str {
        "#d97757"
    }
    fn cli_command(&self) -> &'static str {
        "claude"
    }
}

pub struct ClaudeProvider {
    home_dir: PathBuf,
}

impl ClaudeProvider {
    pub fn new() -> Option<Self> {
        let home_dir = dirs::home_dir()?;
        Some(Self { home_dir })
    }

    /// Test constructor: point the provider at a fake home directory.
    pub fn with_home(home_dir: PathBuf) -> Self {
        Self { home_dir }
    }

    /// Thin wrapper for tests — delegates to the free function in parser module.
    pub fn parse_session(&self, path: &std::path::Path) -> Option<crate::provider::ParsedSession> {
        let buf = path.to_path_buf();
        parser::parse_session_file(&buf)
    }

    fn projects_dir(&self) -> PathBuf {
        self.home_dir.join(".claude").join("projects")
    }

    fn collect_jsonl_files(&self) -> Vec<PathBuf> {
        let projects_dir = self.projects_dir();
        if !projects_dir.exists() {
            return Vec::new();
        }
        let mut all_files: Vec<PathBuf> = Vec::new();
        let project_dirs = match fs::read_dir(&projects_dir) {
            Ok(d) => d,
            Err(e) => {
                log::warn!(
                    "cannot read Claude projects dir '{}': {e}",
                    projects_dir.display()
                );
                return Vec::new();
            }
        };
        for entry in project_dirs {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let project_dir = entry.path();
            if !project_dir.is_dir() {
                continue;
            }
            let files = match fs::read_dir(&project_dir) {
                Ok(f) => f,
                Err(_) => continue,
            };
            for file_entry in files {
                let file_entry = match file_entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let file_path = file_entry.path();
                if file_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    all_files.push(file_path);
                } else if file_path.is_dir() {
                    let subagents_dir = file_path.join("subagents");
                    if subagents_dir.is_dir() {
                        all_files.extend(crate::provider_utils::collect_subagent_jsonl_files(
                            &subagents_dir,
                        ));
                    }
                }
            }
        }
        all_files
    }
}

impl SessionProvider for ClaudeProvider {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        vec![self.projects_dir()]
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let all_files = self.collect_jsonl_files();

        let sessions: Vec<ParsedSession> = all_files
            .par_iter()
            .filter_map(parser::parse_session_file)
            .collect();

        Ok(sessions)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        let all_files = self.collect_jsonl_files();
        let (to_parse, unchanged_source_paths) = partition_files_by_freshness(all_files, known);
        let parsed: Vec<ParsedSession> = to_parse
            .par_iter()
            .filter_map(parser::parse_session_file)
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

        let parsed = parser::parse_session_file(&path).ok_or_else(|| {
            ProviderError::Parse(format!(
                "failed to parse Claude session file '{}'",
                path.display()
            ))
        })?;

        // <persisted-output> tags are kept as-is on the message stream.
        // The frontend resolves the referenced file lazily via the
        // `resolve_persisted_output` command when the user actually views
        // the relevant tool result, avoiding O(N·M) string scans + N
        // synchronous fs reads at session-open time on huge sessions.
        Ok(LoadedSession::from_parsed(parsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const PARENT_ID: &str = "11111111-1111-4111-a111-111111111111";

    fn user_line(text: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}},"timestamp":"2026-06-07T10:00:00.000Z","cwd":"/home/user/my-project","sessionId":"{PARENT_ID}","uuid":"u1"}}"#
        )
    }

    /// Fake `~/.claude/projects` tree with a parent session, a plain
    /// subagent, and a Workflow agent nested under subagents/workflows/.
    fn fake_home_with_workflow_agent() -> TempDir {
        let home = TempDir::new().expect("temp home must be created");
        let project_dir = home.path().join(".claude/projects/-home-user-my-project");
        let workflows_dir = project_dir.join(format!("{PARENT_ID}/subagents/workflows/wf_1"));
        let subagents_dir = project_dir.join(format!("{PARENT_ID}/subagents"));
        fs::create_dir_all(&workflows_dir).expect("workflow dir must be created");

        fs::write(
            project_dir.join(format!("{PARENT_ID}.jsonl")),
            user_line("parent session"),
        )
        .expect("parent jsonl must be written");
        fs::write(
            subagents_dir.join("agent-aaaaaaaaaaaaaaaaa.jsonl"),
            user_line("plain subagent"),
        )
        .expect("plain subagent jsonl must be written");
        fs::write(
            workflows_dir.join("agent-bbbbbbbbbbbbbbbbb.jsonl"),
            user_line("workflow subagent"),
        )
        .expect("workflow subagent jsonl must be written");
        home
    }

    #[test]
    fn scan_all_includes_workflow_nested_subagents() {
        let home = fake_home_with_workflow_agent();
        let provider = ClaudeProvider::with_home(home.path().to_path_buf());

        let sessions = provider.scan_all().expect("scan must succeed");
        let mut ids: Vec<&str> = sessions.iter().map(|s| s.meta.id.as_str()).collect();
        ids.sort_unstable();

        assert_eq!(
            ids,
            vec![
                PARENT_ID,
                "agent-aaaaaaaaaaaaaaaaa",
                "agent-bbbbbbbbbbbbbbbbb",
            ]
        );
    }

    #[test]
    fn workflow_agent_links_parent_and_inherits_project_path() {
        let home = fake_home_with_workflow_agent();
        let provider = ClaudeProvider::with_home(home.path().to_path_buf());

        let sessions = provider.scan_all().expect("scan must succeed");
        let workflow_agent = sessions
            .iter()
            .find(|s| s.meta.id == "agent-bbbbbbbbbbbbbbbbb")
            .expect("workflow agent must be scanned");

        assert_eq!(workflow_agent.meta.parent_id.as_deref(), Some(PARENT_ID));
        assert!(workflow_agent.meta.is_sidechain);
        assert_eq!(workflow_agent.meta.project_path, "/home/user/my-project");
    }
}
