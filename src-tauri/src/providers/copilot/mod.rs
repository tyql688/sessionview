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
//! transcript and are skipped — there is nothing to render. Subagents live
//! inline in their parent's log and come out as child sessions sharing its
//! `source_path` (see `parser`).
//!
//! `$COPILOT_HOME/session-store.db` (SQLite, WAL, written by the running
//! CLI) holds one `assistant_usage_events` row per model call. It is read
//! read-only, keyed by the session id (= the session directory name), and
//! handed to the parser as the preferred usage source; a missing or
//! unreadable store degrades to the log's own `session.shutdown` totals
//! with a warning, never to fabricated numbers.
//!
//! Freshness keys on each event log's `(size, mtime)` via
//! `partition_files_by_freshness`, like DSH: every model call also appends
//! to the log, so a store change never goes unnoticed for long. Legacy
//! `~/.copilot/history-session-state/` pretty-printed JSON is not supported
//! (upstream dropped it too).

pub(crate) mod parser;

use std::collections::HashMap;
use std::path::PathBuf;

use rayon::prelude::*;
use rusqlite::Connection;

use crate::models::{Provider, TokenUsage};
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
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

    fn session_store_path(&self) -> PathBuf {
        self.copilot_home.join("session-store.db")
    }

    /// `assistant_usage_events` rows grouped by session id. `session_id`
    /// narrows the query to one session (message loads); `None` fetches
    /// everything for a scan. Empty when the store is absent or unreadable.
    fn load_usage_rows(&self, session_id: Option<&str>) -> HashMap<String, Vec<parser::UsageRow>> {
        let path = self.session_store_path();
        if !path.is_file() {
            return HashMap::new();
        }
        match read_usage_rows(&path, session_id) {
            Ok(rows) => rows,
            Err(error) => {
                log::warn!(
                    "failed to read Copilot session store '{}': {error}; falling back to shutdown totals",
                    path.display()
                );
                HashMap::new()
            }
        }
    }

    /// Parse one event log with its store rows. The session id is the
    /// directory name, which the CLI keeps equal to `session.start.sessionId`.
    fn parse_file(
        &self,
        path: &std::path::Path,
        rows: &HashMap<String, Vec<parser::UsageRow>>,
    ) -> Vec<ParsedSession> {
        let session_rows = path
            .parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| rows.get(name.to_string_lossy().as_ref()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        parser::parse_session_file(path, session_rows)
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

fn read_usage_rows(
    path: &std::path::Path,
    session_id: Option<&str>,
) -> Result<HashMap<String, Vec<parser::UsageRow>>, ProviderError> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let sql = "SELECT id, session_id, parent_tool_call_id, model, input_tokens, output_tokens,
                      cache_read_tokens, cache_write_tokens, created_at
               FROM assistant_usage_events
               WHERE (?1 IS NULL OR session_id = ?1)
               ORDER BY id";
    let mut stmt = conn.prepare(sql)?;
    let mut out: HashMap<String, Vec<parser::UsageRow>> = HashMap::new();
    let rows = stmt.query_map([session_id], |row| {
        // NULL counts mean "not reported" and read as zero; anything outside
        // u32 is corrupt and rejects the whole row below.
        let count = |index: usize| -> rusqlite::Result<Option<u32>> {
            let value: Option<i64> = row.get(index)?;
            Ok(u32::try_from(value.unwrap_or(0)).ok())
        };
        let row_id: i64 = row.get(0)?;
        let usage = match (count(4)?, count(5)?, count(6)?, count(7)?) {
            (Some(input), Some(output), Some(read), Some(write)) => Some(TokenUsage {
                input_tokens: input,
                output_tokens: output,
                cache_read_input_tokens: read,
                cache_creation_input_tokens: write,
            }),
            _ => None,
        };
        let usage_row = match usage {
            Some(usage) => Some(parser::UsageRow {
                row_id,
                parent_tool_call_id: row.get(2)?,
                model: row.get(3)?,
                usage,
                created_at: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
            }),
            None => None,
        };
        Ok((row.get::<_, String>(1)?, row_id, usage_row))
    })?;
    for row in rows {
        let (session, row_id, usage_row) = row?;
        match usage_row {
            Some(usage_row) => out.entry(session).or_default().push(usage_row),
            None => log::warn!(
                "skipping Copilot usage row {row_id} for session {session}: token count outside u32"
            ),
        }
    }
    Ok(out)
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
        let rows = self.load_usage_rows(None);
        let sessions: Vec<ParsedSession> = files
            .par_iter()
            .flat_map(|path| self.parse_file(path, &rows))
            .collect();
        Ok(sessions)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        let files = self.collect_session_files();
        // The sidecar's mtime is folded into the key (see `parser::source_state`),
        // so a `workspace.yaml` rename re-parses an otherwise untouched log.
        let mut fresh = Vec::with_capacity(files.len());
        let mut stale = Vec::new();
        for file in files {
            let path_str = file.to_string_lossy().to_string();
            match (known.get(&path_str), parser::source_state(&file)) {
                (Some(known), Some(current))
                    if known.size == current.size && known.mtime == current.mtime =>
                {
                    stale.push(path_str)
                }
                _ => fresh.push(file),
            }
        }
        let rows = if fresh.is_empty() {
            HashMap::new()
        } else {
            self.load_usage_rows(None)
        };

        let parsed: Vec<ParsedSession> = fresh
            .par_iter()
            .flat_map(|path| self.parse_file(path, &rows))
            .collect();

        Ok(ScanOutcome {
            parsed,
            unchanged_source_paths: stale,
        })
    }

    fn load_messages(
        &self,
        session_id: &str,
        source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        let path = PathBuf::from(source_path);
        if !path.exists() {
            return Err(ProviderError::Parse(format!(
                "Copilot session file not found: {source_path}"
            )));
        }
        // Children share the parent's log; their id is `<root>:<callId>`.
        let root_id = session_id.split(':').next().unwrap_or(session_id);
        let rows = self.load_usage_rows(Some(root_id));
        let parsed = self
            .parse_file(&path, &rows)
            .into_iter()
            .find(|parsed| parsed.meta.id == session_id)
            .ok_or_else(|| {
                ProviderError::Parse(format!(
                    "session '{session_id}' not found in Copilot session file '{source_path}'"
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
                "  {:<60} parent={:?} title={:?} msgs={} model={:?} in={} out={} cr={} cw={} events={}",
                parsed.meta.id,
                parsed.meta.parent_id,
                parsed.meta.title.chars().take(40).collect::<String>(),
                parsed.meta.message_count,
                parsed.meta.model,
                parsed.meta.input_tokens,
                parsed.meta.output_tokens,
                parsed.meta.cache_read_tokens,
                parsed.meta.cache_write_tokens,
                parsed.usage_events.len(),
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
