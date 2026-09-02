//! GitHub Copilot CLI session provider.
//!
//! Copilot CLI writes one event log per session in the `copilot-agent` wire
//! format:
//!
//! ```text
//! $COPILOT_HOME/session-state/<uuid>/events.jsonl
//!   (~/.copilot by default; COPILOT_HOME replaces the whole path)
//! ```
//!
//! Session directories in `session-state/` without an `events.jsonl`
//! (coding-agent workspaces that persist only checkpoints) carry no
//! transcript and are skipped — there is nothing to render.
//!
//! Freshness keys on each event log's `(size, mtime)` via
//! `partition_files_by_freshness`, like DSH. Legacy
//! `~/.copilot/history-session-state/` pretty-printed JSON is not supported
//! (upstream dropped it too).

pub(crate) mod parser;

use std::collections::HashMap;
use std::path::PathBuf;

use rayon::prelude::*;

use crate::models::Provider;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
    partition_files_by_freshness,
};

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    // Documented resume form (`copilot --help`): `--resume <id>` / `--continue`.
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        Some(format!("copilot --resume {session_id}"))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "copilot".into()
    }
    fn sort_order(&self) -> u32 {
        14
    }
    fn color(&self) -> &'static str {
        "#57666d"
    }
    fn cli_command(&self) -> &'static str {
        "copilot"
    }
}

pub struct CopilotProvider {
    /// `$COPILOT_HOME` (defaults to `~/.copilot`), when resolvable.
    copilot_home: PathBuf,
}

impl CopilotProvider {
    pub fn new() -> Option<Self> {
        let copilot_home = std::env::var_os("COPILOT_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".copilot")))?;
        Some(Self::with_root(copilot_home))
    }

    /// Test constructor: point the provider at an arbitrary `.copilot` root.
    pub fn with_root(copilot_home: PathBuf) -> Self {
        Self { copilot_home }
    }

    fn cli_sessions_root(&self) -> PathBuf {
        self.copilot_home.join("session-state")
    }

    /// Every candidate event-log path. Each file is one session; a resumed
    /// session keeps its id inside the same directory, so ids never collide.
    fn collect_session_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        collect_named_files(&self.cli_sessions_root(), "events.jsonl", &mut files);
        files.sort();
        files
    }
}

/// Collect `<session-dir>/events.jsonl` for every per-session directory
/// directly under `dir`. Session roots are exactly one level deep.
fn collect_named_files(dir: &std::path::Path, name: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join(name);
        if candidate.is_file() {
            out.push(candidate);
        }
    }
}

impl SessionProvider for CopilotProvider {
    fn provider(&self) -> Provider {
        Provider::Copilot
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        let root = self.cli_sessions_root();
        if root.is_dir() {
            vec![root]
        } else {
            Vec::new()
        }
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let files = self.collect_session_files();
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
        let files = self.collect_session_files();
        let (fresh, stale) = partition_files_by_freshness(files, known);

        let parsed: Vec<ParsedSession> = fresh
            .par_iter()
            .filter_map(|path| parser::parse_session_file(path))
            .collect();

        Ok(ScanOutcome {
            parsed,
            unchanged_source_paths: stale,
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
                "Copilot session file not found: {source_path}"
            )));
        }
        let parsed = parser::parse_session_file(&path).ok_or_else(|| {
            ProviderError::Parse(format!(
                "failed to parse Copilot session file '{source_path}'"
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
            Some("copilot --resume abc123".to_string())
        );
    }

    #[test]
    fn descriptor_static_metadata() {
        let descriptor = Descriptor;
        assert_eq!(descriptor.display_key(None), "copilot");
        assert_eq!(descriptor.sort_order(), 14);
        assert_eq!(descriptor.cli_command(), "copilot");
        assert!(descriptor.color().starts_with('#'));
    }

    #[test]
    fn collect_session_files_skips_checkpoint_only_dirs() {
        let home = tempfile::tempdir().unwrap();
        let cli_dir = home.path().join("session-state").join("s1");
        std::fs::create_dir_all(&cli_dir).unwrap();
        std::fs::write(cli_dir.join("events.jsonl"), "").unwrap();
        // A checkpoint-only session dir carries no transcript — skipped.
        let empty_dir = home.path().join("session-state").join("s2");
        std::fs::create_dir_all(&empty_dir).unwrap();
        std::fs::write(empty_dir.join("workspace.yaml"), "id: s2\n").unwrap();

        let provider = CopilotProvider::with_root(home.path().to_path_buf());
        let files = provider.collect_session_files();
        assert_eq!(files.len(), 1, "events.jsonl only: {files:?}");
    }

    /// End-to-end smoke test against real Copilot data on this machine.
    /// Point `COPILOT_HOME` at the `.copilot` tree if needed, then run:
    ///   cargo test --lib copilot::tests::smoke_against_real_data -- --ignored --nocapture
    #[test]
    #[ignore = "hits the real Copilot trees; run with --ignored"]
    fn smoke_against_real_data() {
        let Some(provider) = CopilotProvider::new() else {
            eprintln!("no resolvable Copilot roots; skipping");
            return;
        };
        eprintln!("roots: {:?}", provider.source_roots());
        let files = provider.collect_session_files();
        eprintln!("candidate event logs: {}", files.len());

        let sessions = provider.scan_all().expect("scan_all");
        assert!(
            !sessions.is_empty(),
            "expected sessions from the real Copilot tree"
        );
        let with_usage = sessions
            .iter()
            .filter(|s| !s.usage_events.is_empty())
            .count();
        eprintln!(
            "scanned {} Copilot sessions ({} with usage events)",
            sessions.len(),
            with_usage
        );
        for parsed in &sessions {
            eprintln!(
                "  {:<38} project={:?} title={:?} msgs={} model={:?} ver={:?} branch={:?}",
                parsed.meta.id,
                parsed.meta.project_name,
                parsed.meta.title.chars().take(40).collect::<String>(),
                parsed.meta.message_count,
                parsed.meta.model,
                parsed.meta.cc_version,
                parsed.meta.git_branch,
            );
        }
        if let Some(first) = sessions.iter().find(|s| s.meta.message_count > 2) {
            let fresh = CopilotProvider::new().unwrap();
            let loaded = fresh
                .load_messages(&first.meta.id, &first.meta.source_path)
                .expect("load_messages");
            assert_eq!(loaded.messages.len(), first.meta.message_count as usize);
            eprintln!(
                "reloaded {} -> {} messages, {} warnings",
                first.meta.id,
                loaded.messages.len(),
                loaded.parse_warning_count
            );
        }
    }
}
