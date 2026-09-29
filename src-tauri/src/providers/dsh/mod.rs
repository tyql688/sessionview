pub(crate) mod parser;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::models::Provider;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
    partition_files_by_freshness,
};

/// Session artifacts live under `$DSH_HOME/sessions/<project-key>/<session-id>/`.
/// `DSH_HOME` defaults to `~/.dsh` (see `dsh-home-paths`).
const COMPRESSED_LOG_NAME: &str = "session.jsonl.zstd";
const PLAIN_LOG_NAME: &str = "session.jsonl";

/// Rank a session artifact by `(format version, compressed)`: newer DSH
/// releases persist versioned artifacts (`session.v2.jsonl.zstd`,
/// `session.v3.jsonl.zstd`, …) alongside — or in place of — the unversioned
/// names, and a directory mid-migration can hold several generations at
/// once (highest version wins; the compressed encoding wins ties, mirroring
/// the old plain-vs-zstd rule). Returns `None` for non-artifacts
/// (`session.lock`, `*.Zone.Identifier`, …).
fn artifact_rank(file_name: &str) -> Option<(u64, bool)> {
    if file_name == PLAIN_LOG_NAME {
        return Some((0, false));
    }
    if file_name == COMPRESSED_LOG_NAME {
        return Some((0, true));
    }
    let rest = file_name.strip_prefix("session.v")?;
    let dot = rest.find('.')?;
    let version: u64 = rest[..dot].parse().ok()?;
    match &rest[dot..] {
        ".jsonl" => Some((version, false)),
        ".jsonl.zstd" => Some((version, true)),
        _ => None,
    }
}

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    // DSH resumes sessions through its terminal profile: `--resume <id>` is
    // the documented launcher form (the web profile has no resume flag).
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        Some(format!("dsh --profile tui --resume {session_id}"))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "dsh".into()
    }
    fn sort_order(&self) -> u32 {
        12
    }
    fn color(&self) -> &'static str {
        "#4d6bfe"
    }
    fn cli_command(&self) -> &'static str {
        "dsh"
    }
    /// v2/v3/v4 artifacts are newly discovered, and legacy files containing
    /// `model/selection`, `compaction/prune` or `activity/status` rows now
    /// yield fewer parse warnings (pruned transcripts change too).
    fn parser_revision(&self) -> Option<&'static str> {
        Some("1")
    }
}

pub struct DshProvider {
    home_dir: PathBuf,
}

impl DshProvider {
    pub fn new() -> Option<Self> {
        let home_dir = std::env::var_os("DSH_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".dsh")))?;
        Some(Self { home_dir })
    }

    /// Test constructor: point the provider at a fake DSH home directory.
    pub fn with_home(home_dir: PathBuf) -> Self {
        Self { home_dir }
    }

    /// Thin wrapper for tests — delegates to the free function in parser module.
    pub fn parse_session(&self, path: &std::path::Path) -> Option<ParsedSession> {
        parser::parse_session_file(path)
    }

    fn sessions_dir(&self) -> PathBuf {
        self.home_dir.join("sessions")
    }

    /// One artifact per session directory: DSH compresses `session.jsonl`
    /// into `session.jsonl.zstd`, and both can coexist transiently around
    /// that switch. Newer releases persist versioned artifacts
    /// (`session.v2/v3/v4.jsonl.zstd`); the highest version wins, so a
    /// directory mid-migration indexes its newest log, and the compressed
    /// encoding breaks ties within a generation (keeping one session id from
    /// parsing twice out of two source paths).
    fn collect_session_files(&self) -> Vec<PathBuf> {
        let sessions_dir = self.sessions_dir();
        if !sessions_dir.exists() {
            return Vec::new();
        }
        let mut file_by_dir: BTreeMap<PathBuf, ((u64, bool), PathBuf)> = BTreeMap::new();
        for entry in WalkDir::new(&sessions_dir) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    log::warn!("failed to scan DSH sessions: {error}");
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str() else {
                continue;
            };
            let Some(rank) = artifact_rank(name) else {
                continue;
            };
            let Some(dir) = entry.path().parent().map(std::path::Path::to_path_buf) else {
                continue;
            };
            file_by_dir
                .entry(dir)
                .and_modify(|(best_rank, best_path)| {
                    if rank > *best_rank {
                        *best_rank = rank;
                        *best_path = entry.path().to_path_buf();
                    }
                })
                .or_insert_with(|| (rank, entry.path().to_path_buf()));
        }
        file_by_dir.into_values().map(|(_, path)| path).collect()
    }
}

impl SessionProvider for DshProvider {
    fn provider(&self) -> Provider {
        Provider::Dsh
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
                "DSH session file not found: {source_path}"
            )));
        }
        let parsed = parser::parse_session_file(&path).ok_or_else(|| {
            ProviderError::Parse(format!("failed to parse DSH session file '{source_path}'"))
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
            descriptor.resume_command("session-abc", None),
            Some("dsh --profile tui --resume session-abc".to_string())
        );
    }

    #[test]
    fn descriptor_display_key() {
        let descriptor = Descriptor;
        assert_eq!(descriptor.display_key(None), "dsh");
    }

    #[test]
    fn descriptor_sort_order() {
        let descriptor = Descriptor;
        assert_eq!(descriptor.sort_order(), 12);
    }

    #[test]
    fn descriptor_color() {
        let descriptor = Descriptor;
        assert_eq!(descriptor.color(), "#4d6bfe");
    }

    #[test]
    fn collect_session_files_finds_both_encodings() {
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join("sessions");
        let project = sessions.join("--tmp-p--");
        let plain = project.join("session-1");
        let zstd = project.join("session-2");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::create_dir_all(&zstd).unwrap();
        std::fs::write(plain.join("session.jsonl"), "{}").unwrap();
        std::fs::write(zstd.join("session.jsonl.zstd"), "{}").unwrap();
        std::fs::write(project.join("ignored.txt"), "{}").unwrap();
        std::fs::write(project.join("other.jsonl"), "{}").unwrap();

        let files = DshProvider::with_home(home.path().to_path_buf()).collect_session_files();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn artifact_rank_orders_versions_and_encodings() {
        assert_eq!(artifact_rank("session.jsonl"), Some((0, false)));
        assert_eq!(artifact_rank("session.jsonl.zstd"), Some((0, true)));
        assert_eq!(artifact_rank("session.v2.jsonl.zstd"), Some((2, true)));
        assert_eq!(artifact_rank("session.v4.jsonl.zstd"), Some((4, true)));
        assert_eq!(artifact_rank("session.v3.jsonl"), Some((3, false)));
        // Non-artifacts never qualify.
        assert_eq!(artifact_rank("session.lock"), None);
        assert_eq!(artifact_rank("session.jsonl.zstd:Zone.Identifier"), None);
        assert_eq!(artifact_rank("other.jsonl"), None);
        assert_eq!(artifact_rank("session.v.jsonl.zstd"), None);
        assert_eq!(artifact_rank("session.vx.jsonl.zstd"), None);
        // Compression only breaks ties within one generation.
        assert!(artifact_rank("session.jsonl.zstd") > artifact_rank("session.jsonl"));
        assert!(artifact_rank("session.v2.jsonl") > artifact_rank("session.jsonl.zstd"));
        assert!(artifact_rank("session.v4.jsonl.zstd") > artifact_rank("session.v3.jsonl.zstd"));
    }

    #[test]
    fn collect_session_files_prefers_highest_artifact_version() {
        let home = tempfile::tempdir().unwrap();
        let session = home.path().join("sessions").join("--proj--").join("s-1");
        std::fs::create_dir_all(&session).unwrap();
        // A directory mid-migration holds several generations: newest wins.
        std::fs::write(session.join("session.jsonl.zstd"), "{}").unwrap();
        std::fs::write(session.join("session.v3.jsonl.zstd"), "{}").unwrap();
        std::fs::write(session.join("session.v4.jsonl.zstd"), "{}").unwrap();
        std::fs::write(session.join("session.lock"), "").unwrap();
        std::fs::write(session.join("session.v4.jsonl.zstd:Zone.Identifier"), "").unwrap();

        let files = DshProvider::with_home(home.path().to_path_buf()).collect_session_files();
        assert_eq!(files.len(), 1);
        assert!(
            files[0].ends_with("session.v4.jsonl.zstd"),
            "highest version must win: {files:?}"
        );
    }

    #[test]
    fn collect_session_files_finds_versioned_only_dirs() {
        let home = tempfile::tempdir().unwrap();
        let session = home.path().join("sessions").join("--proj--").join("s-9");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(session.join("session.v2.jsonl.zstd"), "{}").unwrap();

        let files = DshProvider::with_home(home.path().to_path_buf()).collect_session_files();
        assert_eq!(files.len(), 1);
        assert!(
            files[0].ends_with("session.v2.jsonl.zstd"),
            "versioned-only dir must index: {files:?}"
        );
    }

    #[test]
    fn collect_session_files_prefers_zstd_when_both_encodings_coexist() {
        let home = tempfile::tempdir().unwrap();
        let session = home.path().join("sessions").join("--tmp-p--").join("s-1");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(session.join(PLAIN_LOG_NAME), "{}").unwrap();
        std::fs::write(session.join(COMPRESSED_LOG_NAME), "{}").unwrap();

        let files = DshProvider::with_home(home.path().to_path_buf()).collect_session_files();
        assert_eq!(files.len(), 1);
        assert!(
            files[0].ends_with(COMPRESSED_LOG_NAME),
            "compressed artifact must win: {files:?}"
        );
    }
}
