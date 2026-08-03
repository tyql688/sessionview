pub mod parser;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::models::Provider;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
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

/// Fingerprint every sidecar the parser reads. Grok appends usage and
/// lifecycle data to updates.jsonl after chat_history.jsonl can stop changing.
fn source_fingerprint(chat_path: &Path) -> Option<(u64, i64)> {
    let session_dir = chat_path.parent()?;
    let paths = [
        chat_path.to_path_buf(),
        session_dir.join("updates.jsonl"),
        session_dir.join("summary.json"),
    ];
    let mut size = 0_u64;
    let mut latest_mtime_ns = 0_i64;
    for (index, path) in paths.iter().enumerate() {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if index > 0 && error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                log::warn!("cannot stat Grok source '{}': {error}", path.display());
                return None;
            }
        };
        size = size.saturating_add(metadata.len());
        let nanos = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        latest_mtime_ns = latest_mtime_ns.max(i64::try_from(nanos).ok()?);
    }
    Some((size, latest_mtime_ns))
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
        let (mut to_parse, mut unchanged_source_paths) = (Vec::new(), Vec::new());
        for path in files {
            let path_str = path.to_string_lossy().to_string();
            let unchanged = known
                .get(&path_str)
                .zip(source_fingerprint(&path))
                .is_some_and(|(state, current)| current == (state.size, state.mtime));
            if unchanged {
                unchanged_source_paths.push(path_str);
            } else {
                to_parse.push(path);
            }
        }
        // Also repair provider-derived titles stored by an older parser;
        // user-customized titles are None and are never promoted.
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

    #[test]
    fn incremental_scan_reparses_when_updates_sidecar_changes() {
        let root = tempfile::tempdir().unwrap();
        let session_dir = root
            .path()
            .join("sessions/%2Ftmp%2Fdemo/11111111-1111-4111-a111-111111111111");
        std::fs::create_dir_all(&session_dir).unwrap();
        let chat_path = session_dir.join("chat_history.jsonl");
        std::fs::write(
            &chat_path,
            concat!(
                "{\"type\":\"user\",\"content\":\"hello\",\"prompt_index\":0}\n",
                "{\"type\":\"assistant\",\"content\":\"hi\"}\n",
            ),
        )
        .unwrap();

        let provider = GrokProvider::with_root(root.path().to_path_buf());
        let first = provider.scan_all().unwrap();
        assert_eq!(first.len(), 1);
        let mut known = HashMap::new();
        known.insert(
            first[0].meta.source_path.clone(),
            SourceState {
                size: first[0].meta.file_size_bytes,
                mtime: first[0].source_mtime,
                title: Some(first[0].meta.title.clone()),
            },
        );
        std::fs::write(
            session_dir.join("updates.jsonl"),
            "{\"timestamp\":1782892920,\"params\":{\"update\":{\"sessionUpdate\":\"session_recap\",\"summary\":\"new recap\"}}}\n",
        )
        .unwrap();

        let outcome = provider.scan_incremental(&known).unwrap();
        assert_eq!(outcome.parsed.len(), 1);
        assert!(
            outcome.parsed[0]
                .messages
                .iter()
                .any(|message| message.content == "[Recap] new recap")
        );
    }
}
