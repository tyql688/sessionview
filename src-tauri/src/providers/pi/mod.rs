pub mod parser;
pub mod types;

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
        p.contains("/.pi/agent/sessions/")
    }
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        Some(format!("pi --session {}", session_id))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "pi".into()
    }
    fn sort_order(&self) -> u32 {
        10
    }
    fn color(&self) -> &'static str {
        "#000000"
    }
    fn cli_command(&self) -> &'static str {
        "pi"
    }
    fn avatar_svg(&self) -> &'static str {
        r##"<svg width="24" height="24" viewBox="0 0 800 800" xmlns="http://www.w3.org/2000/svg"><path fill="currentColor" fill-rule="evenodd" d="M165.29 165.29H517.36V400H400V517.36H282.65V634.72H165.29ZM282.65 282.65V400H400V282.65Z"/><path fill="currentColor" d="M517.36 400H634.72V634.72H517.36Z"/></svg>"##
    }
}

pub struct PiProvider {
    home_dir: PathBuf,
}

impl PiProvider {
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

    fn sessions_dir(&self) -> PathBuf {
        self.home_dir.join(".pi").join("agent").join("sessions")
    }

    fn collect_jsonl_files(&self) -> Vec<PathBuf> {
        let sessions_dir = self.sessions_dir();
        if !sessions_dir.exists() {
            return Vec::new();
        }

        let mut all_files: Vec<PathBuf> = Vec::new();
        let project_dirs = match fs::read_dir(&sessions_dir) {
            Ok(d) => d,
            Err(e) => {
                log::warn!(
                    "cannot read Pi sessions dir '{}': {e}",
                    sessions_dir.display()
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
                let path = file_entry.path();
                if path.extension().is_some_and(|ext| ext == "jsonl") {
                    all_files.push(path);
                }
            }
        }

        all_files
    }
}

impl SessionProvider for PiProvider {
    fn provider(&self) -> Provider {
        Provider::Pi
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        let sessions_dir = self.sessions_dir();
        if sessions_dir.exists() {
            vec![sessions_dir]
        } else {
            Vec::new()
        }
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let files = self.collect_jsonl_files();
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
        let files = self.collect_jsonl_files();
        let (fresh, stale) = partition_files_by_freshness(files, known);

        let parsed: Vec<ParsedSession> = fresh
            .par_iter()
            .filter_map(|path| parser::parse_session_file(path))
            .collect();

        let unchanged_source_paths: Vec<String> = stale;

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
        match parser::parse_session_file(&path) {
            Some(session) => Ok(vec![session]),
            None => Ok(Vec::new()),
        }
    }

    fn load_messages(
        &self,
        _session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        let path = PathBuf::from(source_path);
        if !path.exists() {
            return Err(ProviderError::Parse(format!(
                "Session file not found: {}",
                source_path
            )));
        }
        parser::load_messages(&path).ok_or_else(|| {
            ProviderError::Parse(format!("Failed to load Pi session: {}", source_path))
        })
    }

    fn deletion_plan(&self, _meta: &SessionMeta, _children: &[SessionMeta]) -> DeletionPlan {
        DeletionPlan {
            file_action: crate::provider::FileAction::Remove,
            child_plans: Vec::new(),
            cleanup_dirs: Vec::new(),
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
            "/Users/test/.pi/agent/sessions/--Users-test-project--/2024-12-03_abc123.jsonl"
        ));
        assert!(!descriptor.owns_source_path("/Users/test/.claude/projects/project/session.jsonl"));
    }

    #[test]
    fn descriptor_resume_command() {
        let descriptor = Descriptor;
        assert_eq!(
            descriptor.resume_command("abc123", None),
            Some("pi --session abc123".to_string())
        );
    }

    #[test]
    fn descriptor_display_key() {
        let descriptor = Descriptor;
        assert_eq!(descriptor.display_key(None), "pi");
    }

    #[test]
    fn descriptor_sort_order() {
        let descriptor = Descriptor;
        assert_eq!(descriptor.sort_order(), 10);
    }

    #[test]
    fn descriptor_color() {
        let descriptor = Descriptor;
        assert_eq!(descriptor.color(), "#000000");
    }

    #[test]
    #[ignore = "requires local Pi session data"]
    fn scan_real_local_sessions() {
        // Use real local Pi session data if available
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => return,
        };
        let sessions_dir = home.join(".pi").join("agent").join("sessions");
        if !sessions_dir.exists() {
            return;
        }

        let provider = PiProvider::new();
        let provider = match provider {
            Some(p) => p,
            None => return,
        };

        // Test scan_all
        let result = provider.scan_all();
        assert!(result.is_ok(), "scan_all should succeed");

        let sessions = result.unwrap();
        if sessions.is_empty() {
            // No sessions found, that's okay
            return;
        }

        println!("Found {} Pi sessions", sessions.len());
        for session in &sessions {
            assert_eq!(session.meta.provider, Provider::Pi);
            assert!(!session.meta.id.is_empty());
            assert!(!session.meta.title.is_empty());
            assert!(!session.meta.source_path.is_empty());
            println!(
                "  - {}: {} ({} messages)",
                session.meta.id, session.meta.title, session.meta.message_count
            );
        }
    }
}
