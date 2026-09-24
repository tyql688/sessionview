pub mod parser;
pub mod types;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rayon::prelude::*;
use serde_json::Value;
use walkdir::WalkDir;

use crate::models::Provider;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
    partition_files_by_freshness,
};

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
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
    fn parser_revision(&self) -> Option<&'static str> {
        Some("2")
    }
}

pub struct PiProvider {
    home_dir: PathBuf,
}

impl PiProvider {
    pub(crate) fn new() -> Option<Self> {
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

        let mut files = Vec::new();
        for entry in WalkDir::new(&sessions_dir) {
            match entry {
                Ok(entry)
                    if entry.file_type().is_file()
                        && entry.path().extension().is_some_and(|ext| ext == "jsonl") =>
                {
                    files.push(entry.into_path());
                }
                Ok(_) => {}
                Err(error) => {
                    log::warn!("failed to scan Pi sessions: {error}");
                }
            }
        }

        // A session file copied into another project directory keeps its
        // header `id`, so one session can have several files. The index holds
        // one row per id: scan only the most recently modified copy (the one
        // Pi continues), or incremental scans alternate between the copies
        // and re-parse the other one on every pass.
        let mut newest: HashMap<String, (SystemTime, PathBuf)> = HashMap::new();
        let mut kept = Vec::with_capacity(files.len());
        for path in files {
            let Some((id, modified)) = session_identity(&path) else {
                // No readable session header: the parser decides and reports.
                kept.push(path);
                continue;
            };
            match newest.entry(id) {
                Entry::Vacant(slot) => {
                    slot.insert((modified, path));
                }
                Entry::Occupied(mut slot) => {
                    let older = if modified > slot.get().0 {
                        slot.insert((modified, path)).1
                    } else {
                        path
                    };
                    log::debug!(
                        "Skipping older copy of Pi session {}: {}",
                        slot.key(),
                        older.display()
                    );
                }
            }
        }
        kept.extend(newest.into_values().map(|(_, path)| path));
        kept.sort();
        kept
    }
}

/// The header `id` and modification time of a Pi session file.
fn session_identity(path: &Path) -> Option<(String, SystemTime)> {
    let file = std::fs::File::open(path).ok()?;
    let modified = file.metadata().ok()?.modified().ok()?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line).ok()?;
    let header: Value = serde_json::from_str(&line).ok()?;
    if header.get("type")?.as_str()? != "session" {
        return None;
    }
    Some((header.get("id")?.as_str()?.to_string(), modified))
}

impl SessionProvider for PiProvider {
    fn provider(&self) -> Provider {
        Provider::Pi
    }

    fn source_roots(&self) -> Vec<PathBuf> {
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
    fn collect_jsonl_files_includes_nested_sessions() {
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join(".pi/agent/sessions/project");
        let direct = sessions.join("direct.jsonl");
        let nested = sessions.join("parent/agent/run/session.jsonl");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&direct, "{}").unwrap();
        std::fs::write(&nested, "{}").unwrap();
        std::fs::write(sessions.join("ignored.txt"), "{}").unwrap();

        let files = PiProvider::with_home(home.path().to_path_buf()).collect_jsonl_files();

        assert_eq!(files, vec![direct, nested]);
    }

    #[test]
    fn collect_jsonl_files_keeps_only_the_newest_copy_of_a_session() {
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join(".pi/agent/sessions");
        let copied = r#"{"type":"session","version":3,"id":"11111111-1111-4111-a111-111111111111","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/a"}"#;
        let other = r#"{"type":"session","version":3,"id":"22222222-2222-4222-a222-222222222222","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/a"}"#;
        let older = sessions.join("--tmp-a--/session.jsonl");
        let newer = sessions.join("--tmp-b--/session.jsonl");
        let unrelated = sessions.join("--tmp-a--/other.jsonl");
        for (path, header) in [(&older, copied), (&newer, copied), (&unrelated, other)] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, header).unwrap();
        }
        std::fs::File::options()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000))
            .unwrap();

        let files = PiProvider::with_home(home.path().to_path_buf()).collect_jsonl_files();

        assert_eq!(files, vec![unrelated, newer]);
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
