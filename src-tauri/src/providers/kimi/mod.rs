pub mod parser;
mod tools;

use std::collections::HashMap;
use std::path::PathBuf;

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::models::Provider;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
    partition_files_by_freshness,
};

pub(crate) use parser::SessionIndex;
pub use parser::session_id_for_path;

pub struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        // Kimi's resume CLI requires the full directory name including the
        // `session_` or `ses_` prefix — bare UUIDs return "Session not found".
        // We store the prefixed name in meta.id for parent sessions; for
        // subagents the id is `<parent-id>:<agent-name>` and kimi has no
        // resume target for them, so strip the suffix and resume the parent.
        let id = match session_id.split_once(':') {
            Some((parent, _agent)) => parent,
            None => session_id,
        };
        Some(format!("kimi --session {id}"))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "kimi".into()
    }
    fn sort_order(&self) -> u32 {
        6
    }
    fn color(&self) -> &'static str {
        "#1783ff"
    }
    fn cli_command(&self) -> &'static str {
        "kimi"
    }
}

pub struct KimiProvider {
    kimi_dir: PathBuf,
}

impl KimiProvider {
    pub fn new() -> Option<Self> {
        let home_dir = dirs::home_dir()?;
        Some(Self {
            kimi_dir: home_dir.join(".kimi-code"),
        })
    }

    /// Build a provider rooted at an arbitrary directory instead of
    /// `~/.kimi-code`. Used by integration tests to point at fixture
    /// trees; not intended for production code paths.
    pub fn with_root(kimi_dir: PathBuf) -> Self {
        Self { kimi_dir }
    }

    fn sessions_dir(&self) -> PathBuf {
        self.kimi_dir.join("sessions")
    }

    fn session_index_path(&self) -> PathBuf {
        self.kimi_dir.join("session_index.jsonl")
    }

    fn load_session_index(&self) -> SessionIndex {
        SessionIndex::load(&self.session_index_path())
    }

    /// Walk `<sessions_dir>/<wd_*>/<session_dir>/agents/<name>/wire.jsonl`.
    /// Each wire.jsonl is one ParsedSession (main agent = parent session,
    /// `agent-N` = subagent linked back via state.json.parentAgentId).
    fn collect_wire_files(&self) -> Vec<PathBuf> {
        let sessions_dir = self.sessions_dir();
        if !sessions_dir.exists() {
            return Vec::new();
        }
        let mut files = Vec::new();
        for entry in WalkDir::new(&sessions_dir)
            .max_depth(5)
            .into_iter()
            .filter_map(std::result::Result::ok)
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.file_name().is_some_and(|n| n == "wire.jsonl")
                && path
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.file_name())
                    == Some(std::ffi::OsStr::new("agents"))
            {
                files.push(path.to_path_buf());
            }
        }
        files
    }
}

impl SessionProvider for KimiProvider {
    fn provider(&self) -> Provider {
        Provider::Kimi
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        vec![self.sessions_dir()]
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        let files = self.collect_wire_files();
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let index = self.load_session_index();
        let parsed: Vec<ParsedSession> = files
            .par_iter()
            .filter_map(|path| parser::parse_session(path, &index))
            .collect();
        Ok(parsed)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        let files = self.collect_wire_files();
        let (to_parse, unchanged_source_paths) = partition_files_by_freshness(files, known);
        let index = self.load_session_index();
        let parsed: Vec<ParsedSession> = to_parse
            .par_iter()
            .filter_map(|path| parser::parse_session(path, &index))
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
        let index = self.load_session_index();
        let parsed = parser::parse_session(&path, &index).ok_or_else(|| {
            ProviderError::Parse(format!("session {session_id} not found in {source_path}"))
        })?;
        Ok(LoadedSession::from_parsed(parsed))
    }
}
