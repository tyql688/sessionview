pub(crate) mod images;
pub mod parser;

use std::fs;
use std::path::PathBuf;

use rayon::prelude::*;

use std::collections::HashMap;

use crate::models::{Provider, SessionMeta};
use crate::provider::{
    partition_files_by_freshness, DeletionPlan, LoadedSession, ParsedSession, ProviderError,
    ScanOutcome, SessionProvider, SourceState,
};

pub struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    fn owns_source_path(&self, source_path: &str) -> bool {
        let p = source_path.replace('\\', "/");
        p.contains("/.claude/projects/") && !p.contains("/.cc-mirror/")
    }
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
    fn avatar_svg(&self) -> &'static str {
        r##"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z" fill="#D97757" fill-rule="nonzero"/></svg>"##
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
                        if let Ok(sub_entries) = fs::read_dir(&subagents_dir) {
                            for sub_entry in sub_entries {
                                let sub_entry = match sub_entry {
                                    Ok(e) => e,
                                    Err(_) => continue,
                                };
                                let sub_path = sub_entry.path();
                                if sub_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                                    all_files.push(sub_path);
                                }
                            }
                        }
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

    fn watch_paths(&self) -> Vec<PathBuf> {
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

    fn scan_source(&self, source_path: &str) -> Result<Vec<ParsedSession>, ProviderError> {
        let path = PathBuf::from(source_path);
        let related_paths = crate::provider::jsonl_subagent_related_paths(&path);
        Ok(related_paths
            .par_iter()
            .filter_map(parser::parse_session_file)
            .collect())
    }

    fn deletion_plan(&self, meta: &SessionMeta, children: &[SessionMeta]) -> DeletionPlan {
        crate::provider::jsonl_subagents_deletion_plan(meta, children)
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
