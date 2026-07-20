pub mod parser;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::models::Provider;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
    partition_files_by_freshness,
};

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
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
        Ok(LoadedSession::from_parsed(parsed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderDescriptor;

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
}
