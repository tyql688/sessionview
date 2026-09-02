//! MiniMax Code (mcode) session provider.
//!
//! MiniMax Code is the CLI (`mcode`). Every conversation it writes —
//! the default `mavis` orchestrator plus `explore` / `worker` /
//! `verifier` delegates — shares one storage tree:
//!
//! ```text
//! ~/.minimax/v2/                   # or $MINIMAX_DATA_DIR/v2
//!   sqlite/runtime-state.sqlite    # metadata index
//!   sessions/YYYY/MM/DD/.../       # per-session payload
//! ```
//!
//! The split mirrors OpenCode: a single SQLite holds session-level metadata
//! (timestamps, parent links, project bindings) and per-session manifests and
//! JSONL files hold the full conversation wire. The incremental fingerprint
//! covers the database, a non-empty WAL, every manifest, and every referenced
//! messages file, so any source-only update triggers a coherent rescan.
//!
//! ## Why `runtime = 'pi-agent'` is the gate
//!
//! Every MiniMax Code session carries `runtime = 'pi-agent'` in
//! `local_runtime_sessions`; that's the only engine the CLI writes.
//! We don't filter on `agent_name`, so the orchestrator and its
//! delegates land under one provider cluster. Parent/child links are
//! the typed `parent_session_id` column — never derived from message
//! text. Scaffolding rows (`origin = 'root-repair'`) are hidden.

pub(crate) mod parser;
mod types;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::models::{Provider, SessionMeta};
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
};

/// Default root under which MiniMax Code's v2 runtime stores everything.
/// The CLI resolves `$MINIMAX_DATA_DIR` first, then the legacy
/// `$MAVIS_DATA_DIR`, then `~/.minimax`.
fn default_data_root() -> Option<PathBuf> {
    resolve_data_root(
        std::env::var_os("MINIMAX_DATA_DIR"),
        std::env::var_os("MAVIS_DATA_DIR"),
        dirs::home_dir(),
    )
}

fn resolve_data_root(
    minimax_data_dir: Option<std::ffi::OsString>,
    mavis_data_dir: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    let custom = minimax_data_dir
        .filter(|value| !value.is_empty())
        .or_else(|| mavis_data_dir.filter(|value| !value.is_empty()));
    if let Some(custom) = custom {
        return Some(PathBuf::from(custom).join("v2"));
    }
    Some(home?.join(".minimax").join("v2"))
}

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    // Documented resume form (`mcode --help`): `mcode --session <id>`.
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        Some(format!("mcode --session {session_id}"))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "mcode".into()
    }
    fn sort_order(&self) -> u32 {
        13
    }
    fn color(&self) -> &'static str {
        "#f23f5d"
    }
    fn cli_command(&self) -> &'static str {
        "mcode"
    }
}

pub struct McodeProvider {
    data_root: PathBuf,
    /// `session_id -> messages.jsonl path` map. Rebuilt on every `scan_all`
    /// because the on-disk session tree is the source of truth and we don't
    /// want a stale cache to route a load to a deleted file.
    jsonl_paths: Mutex<HashMap<String, PathBuf>>,
}

#[derive(Default)]
struct SessionSources {
    jsonl_paths: HashMap<String, PathBuf>,
    fingerprint_paths: Vec<PathBuf>,
}

impl McodeProvider {
    pub fn new() -> Option<Self> {
        let data_root = default_data_root()?;
        Some(Self {
            data_root,
            jsonl_paths: Mutex::new(HashMap::new()),
        })
    }

    /// Test constructor: point the provider at an arbitrary data root.
    pub fn with_data_root(data_root: PathBuf) -> Self {
        Self {
            data_root,
            jsonl_paths: Mutex::new(HashMap::new()),
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_root.join("sqlite").join("runtime-state.sqlite")
    }

    fn sessions_dir(&self) -> PathBuf {
        self.data_root.join("sessions")
    }

    fn open_db(&self) -> Result<Connection, ProviderError> {
        if !self.db_path().exists() {
            return Err(ProviderError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("mcode database not found: {}", self.db_path().display()),
            )));
        }
        // Read-only: the CLI is the writer; we are a passive reader.
        // NO_MUTEX keeps us out of the WAL contention path entirely.
        let conn = Connection::open_with_flags(
            self.db_path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(conn)
    }

    /// Fingerprint every mutable source that contributes to an indexed row.
    /// The `mtime` slot is a deterministic metadata hash, so a same-length
    /// manifest or JSONL rewrite cannot be hidden by a newer database mtime.
    fn source_graph_state(&self, sources: &SessionSources) -> Result<SourceState, ProviderError> {
        let db_path = self.db_path();
        let wal_path = PathBuf::from(format!("{}-wal", db_path.to_string_lossy()));
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut size = 0_u64;
        fingerprint_path(&mut hash, &mut size, &db_path, true, true)?;
        fingerprint_path(&mut hash, &mut size, &wal_path, false, false)?;

        let mut paths = sources.fingerprint_paths.clone();
        paths.sort();
        paths.dedup();
        for path in paths {
            fingerprint_path(&mut hash, &mut size, &path, false, true)?;
        }

        Ok(SourceState {
            size,
            mtime: i64::try_from(hash & (i64::MAX as u64))
                .unwrap_or(i64::MAX)
                .max(1),
            title: None,
        })
    }

    /// Walk the canonical session manifests once, retaining both the path map
    /// used for loading and the complete set used by incremental freshness.
    fn discover_session_sources(&self) -> SessionSources {
        let mut sources = SessionSources::default();
        let sessions_dir = self.sessions_dir();
        if !sessions_dir.exists() {
            return sources;
        }
        for entry in walkdir::WalkDir::new(&sessions_dir).max_depth(6) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    log::warn!(
                        "failed to walk mcode sessions under '{}': {error}",
                        sessions_dir.display()
                    );
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.file_name() != "manifest.json" {
                continue;
            }
            let manifest_path = entry.path();
            sources.fingerprint_paths.push(manifest_path.to_path_buf());
            let content = match std::fs::read_to_string(manifest_path) {
                Ok(content) => content,
                Err(error) => {
                    log::warn!(
                        "failed to read mcode manifest '{}': {error}",
                        manifest_path.display()
                    );
                    continue;
                }
            };
            let value = match serde_json::from_str::<Value>(&content) {
                Ok(value) => value,
                Err(error) => {
                    log::warn!(
                        "skipping malformed mcode manifest '{}': {error}",
                        manifest_path.display()
                    );
                    continue;
                }
            };
            let Some(session_id) = value.get("sessionId").and_then(Value::as_str) else {
                log::warn!(
                    "skipping mcode manifest without sessionId: '{}'",
                    manifest_path.display()
                );
                continue;
            };
            // Real manifests always carry `paths.messages`. Test fixtures
            // (and older wire shapes) may write only the sessionId — fall
            // back to the manifest's sibling so we still index the jsonl.
            let jsonl_path = value
                .get("paths")
                .and_then(|p| p.get("messages"))
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| {
                    if p.is_absolute() {
                        p
                    } else {
                        manifest_path.parent().unwrap_or(&sessions_dir).join(p)
                    }
                })
                .unwrap_or_else(|| {
                    manifest_path
                        .parent()
                        .unwrap_or(&sessions_dir)
                        .join("messages.jsonl")
                });
            sources.fingerprint_paths.push(jsonl_path.clone());
            if sources
                .jsonl_paths
                .insert(session_id.to_string(), jsonl_path)
                .is_some()
            {
                log::warn!("duplicate mcode manifest for one session id; using the last path");
            }
        }
        sources
    }

    fn lookup_jsonl(&self, session_id: &str) -> Option<PathBuf> {
        let guard = self.jsonl_paths.lock().ok()?;
        guard.get(session_id).cloned()
    }

    /// Resolve `messages.jsonl` for a session. Production loads go through a
    /// fresh provider (`require_runtime`), so the scan-time mutex is empty;
    /// rebuild the manifest map on a miss instead of guessing directory names.
    fn resolve_jsonl(&self, session_id: &str) -> Option<PathBuf> {
        if let Some(path) = self.lookup_jsonl(session_id) {
            return Some(path);
        }
        let map = self.discover_session_sources().jsonl_paths;
        let path = map.get(session_id).cloned();
        if let Ok(mut guard) = self.jsonl_paths.lock() {
            *guard = map;
        }
        path
    }

    fn cache_jsonl_paths(&self, paths: &HashMap<String, PathBuf>) {
        if let Ok(mut guard) = self.jsonl_paths.lock() {
            *guard = paths.clone();
        }
    }

    fn scan_sources(
        &self,
        sources: &SessionSources,
        source_state: &SourceState,
    ) -> Result<Vec<ParsedSession>, ProviderError> {
        let conn = self.open_db()?;
        self.cache_jsonl_paths(&sources.jsonl_paths);

        // Visible, non-archived conversations on the pi-agent runtime.
        // `peek` / `channel` / `cron` are bookkeeping kinds; `root-repair`
        // is the empty scaffolding session the CLI writes at first launch.
        let mut stmt = conn.prepare(
            "SELECT session_id, parent_session_id, title, workspace_dir, \
                    project_workspace_dir, created_at_ms, updated_at_ms, \
                    extra_data_json, agent_name \
             FROM local_runtime_sessions \
             WHERE runtime = 'pi-agent' \
               AND archived = 0 \
               AND visibility <> 'hidden' \
               AND session_kind NOT IN ('peek', 'channel', 'cron') \
               AND COALESCE(json_extract(extra_data_json, '$.origin'), '') <> 'root-repair' \
             ORDER BY updated_at_ms DESC",
        )?;

        let mut rows = stmt.query_map([], row_to_session_row)?;
        let mut out: Vec<ParsedSession> = Vec::new();
        for row in rows.by_ref() {
            let row = match row {
                Ok(row) => row,
                Err(error) => {
                    log::warn!("skipping unparseable mcode session row: {error}");
                    continue;
                }
            };
            let session_id = row.session_id.clone();
            match row.into_parsed(&self.data_root, &sources.jsonl_paths, source_state) {
                Ok(parsed) => out.push(parsed),
                Err(error) => {
                    log::warn!("skipping mcode session '{session_id}': {error}");
                }
            }
        }
        attach_children_from_parent_links(&mut out);
        Ok(out)
    }
}

fn fingerprint_path(
    hash: &mut u64,
    total_size: &mut u64,
    path: &Path,
    required: bool,
    include_empty: bool,
) -> Result<(), ProviderError> {
    hash_bytes(hash, path.to_string_lossy().as_bytes());
    match std::fs::metadata(path) {
        Ok(metadata) if include_empty || metadata.len() > 0 => {
            hash_bytes(hash, &[1]);
            hash_bytes(hash, &metadata.len().to_le_bytes());
            let modified = metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| {
                    ProviderError::Parse(format!(
                        "mcode source mtime predates UNIX epoch for '{}': {error}",
                        path.display()
                    ))
                })?
                .as_nanos();
            hash_bytes(hash, &modified.to_le_bytes());
            *total_size = total_size.saturating_add(metadata.len());
            Ok(())
        }
        Ok(_) => {
            hash_bytes(hash, &[0]);
            Ok(())
        }
        Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => {
            hash_bytes(hash, &[0]);
            Ok(())
        }
        Err(error) => Err(ProviderError::Io(error)),
    }
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

impl SessionProvider for McodeProvider {
    fn provider(&self) -> Provider {
        Provider::Mcode
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        let root = self.data_root.clone();
        if root.exists() {
            vec![root]
        } else {
            Vec::new()
        }
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        if !self.db_path().exists() {
            return Ok(Vec::new());
        }
        let sources = self.discover_session_sources();
        let source_state = self.source_graph_state(&sources)?;
        self.scan_sources(&sources, &source_state)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        if !self.db_path().exists() {
            return Ok(ScanOutcome::default());
        }

        let source_path = self.db_path().to_string_lossy().to_string();
        let sources = self.discover_session_sources();
        let current = self.source_graph_state(&sources)?;
        // `source_states_for_provider` keeps the parent row's session title
        // on `SourceState.title`; only compare the complete source fingerprint.
        if let Some(previous) = known.get(&source_path)
            && previous.size == current.size
            && previous.mtime == current.mtime
        {
            return Ok(ScanOutcome {
                parsed: Vec::new(),
                unchanged_source_paths: vec![source_path],
            });
        }

        // One shared source path backs all rows, so any source-graph change
        // coherently reparses the visible provider snapshot.
        Ok(ScanOutcome {
            parsed: self.scan_sources(&sources, &current)?,
            unchanged_source_paths: Vec::new(),
        })
    }

    fn load_messages(
        &self,
        session_id: &str,
        _source_path: &str,
    ) -> Result<LoadedSession, ProviderError> {
        let jsonl_path = self.resolve_jsonl(session_id).ok_or_else(|| {
            ProviderError::Parse(format!(
                "mcode session {session_id} has no messages.jsonl under {}",
                self.sessions_dir().display()
            ))
        })?;

        let parsed = parser::parse_messages_file(&jsonl_path).ok_or_else(|| {
            ProviderError::Parse(format!(
                "failed to parse mcode messages file '{}'",
                jsonl_path.display()
            ))
        })?;

        let token_totals = crate::provider::token_totals_from_usage_events(&parsed.usage_events);
        Ok(LoadedSession::from_parsed(ParsedSession {
            meta: SessionMeta {
                id: session_id.to_string(),
                provider: Provider::Mcode,
                title: String::new(),
                project_path: String::new(),
                project_name: String::new(),
                created_at: 0,
                updated_at: 0,
                message_count: parsed.messages.len() as u32,
                file_size_bytes: std::fs::metadata(&jsonl_path).map(|m| m.len()).unwrap_or(0),
                source_path: jsonl_path.to_string_lossy().to_string(),
                is_sidechain: false,
                variant_name: None,
                model: parsed.first_assistant_model.clone(),
                cc_version: None,
                git_branch: None,
                parent_id: None,
                input_tokens: token_totals.input_tokens,
                output_tokens: token_totals.output_tokens,
                cache_read_tokens: token_totals.cache_read_tokens,
                cache_write_tokens: token_totals.cache_write_tokens,
            },
            messages: parsed.messages,
            content_text: String::new(),
            parse_warning_count: parsed.parse_warning_count,
            child_session_ids: Vec::new(),
            usage_events: parsed.usage_events,
            source_mtime: std::fs::metadata(&jsonl_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_nanos()).ok())
                .unwrap_or(0),
        }))
    }
}

struct SessionRow {
    session_id: String,
    parent_session_id: Option<String>,
    title: Option<String>,
    workspace_dir: Option<String>,
    project_workspace_dir: Option<String>,
    created_at_ms: Option<i64>,
    updated_at_ms: i64,
    extra_data_json: Option<String>,
    agent_name: Option<String>,
}

impl SessionRow {
    fn into_parsed(
        self,
        data_root: &Path,
        jsonl_paths: &HashMap<String, PathBuf>,
        source_state: &SourceState,
    ) -> Result<ParsedSession, String> {
        let source_path = data_root
            .join("sqlite")
            .join("runtime-state.sqlite")
            .to_string_lossy()
            .to_string();

        let workspace_dir = self
            .project_workspace_dir
            .clone()
            .or_else(|| self.workspace_dir.clone())
            .unwrap_or_default();
        let project_path = if workspace_dir.is_empty() {
            String::new()
        } else {
            workspace_dir.clone()
        };
        let project_name = if workspace_dir.is_empty() {
            String::new()
        } else {
            std::path::Path::new(&workspace_dir)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| workspace_dir.clone())
        };

        // `session_type` (`root` vs `branch`) is internal bookkeeping
        // and does not mark a child. A real child session is identified
        // by `parent_session_id`.
        let is_sidechain = self.parent_session_id.is_some();

        let title = self
            .title
            .clone()
            .filter(|t| !t.is_empty() && t != "Main")
            .unwrap_or_default();

        let jsonl_path = jsonl_paths.get(&self.session_id).cloned();
        let parsed_jsonl = jsonl_path
            .as_ref()
            .filter(|path| path.exists())
            .and_then(|path| parser::parse_messages_file(path));
        // Shared source-graph fingerprint, not the per-session JSONL size —
        // `scan_incremental` keys on the database path and must see exactly
        // the same values it wrote for every row.
        let file_size_bytes = source_state.size;
        let message_count = parsed_jsonl
            .as_ref()
            .map(|parsed| parsed.messages.len() as u32)
            .unwrap_or(0);
        let parse_warning_count = parsed_jsonl
            .as_ref()
            .map(|parsed| parsed.parse_warning_count)
            .unwrap_or(0);
        let jsonl_model = parsed_jsonl
            .as_ref()
            .and_then(|parsed| parsed.first_assistant_model.clone());
        let usage_events = parsed_jsonl
            .as_ref()
            .map(|parsed| parsed.usage_events.clone())
            .unwrap_or_default();
        let content_text = parsed_jsonl
            .as_ref()
            .map(|parsed| content_text_from_messages(&parsed.messages))
            .unwrap_or_default();
        let child_session_ids = parsed_jsonl
            .as_ref()
            .map(|parsed| parsed.child_session_ids.clone())
            .unwrap_or_default();
        let token_totals = crate::provider::token_totals_from_usage_events(&usage_events);
        let variant_name = self
            .agent_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);

        let extra = self
            .extra_data_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok());
        let effective_model = extra
            .as_ref()
            .and_then(|v| {
                v.get("effectiveModel")
                    .and_then(Value::as_str)
                    .filter(|model| !model.is_empty())
                    .map(str::to_string)
            })
            // `effectiveModel` is optional; assistant jsonl lines always
            // carry `message.model`.
            .or(jsonl_model);

        let created_at = self.created_at_ms.unwrap_or(0) / 1000;
        let updated_at = self.updated_at_ms / 1000;

        let meta = SessionMeta {
            id: self.session_id.clone(),
            provider: Provider::Mcode,
            title,
            project_path,
            project_name,
            created_at,
            updated_at,
            message_count,
            file_size_bytes,
            source_path,
            is_sidechain,
            variant_name,
            model: effective_model,
            cc_version: None,
            git_branch: None,
            parent_id: self.parent_session_id.clone(),
            input_tokens: token_totals.input_tokens,
            output_tokens: token_totals.output_tokens,
            cache_read_tokens: token_totals.cache_read_tokens,
            cache_write_tokens: token_totals.cache_write_tokens,
        };

        Ok(ParsedSession {
            meta,
            messages: Vec::new(),
            content_text,
            parse_warning_count,
            child_session_ids,
            usage_events,
            source_mtime: source_state.mtime,
        })
    }
}

fn row_to_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        session_id: row.get(0)?,
        parent_session_id: row.get(1)?,
        title: row.get(2)?,
        workspace_dir: row.get(3)?,
        project_workspace_dir: row.get(4)?,
        created_at_ms: row.get(5)?,
        updated_at_ms: row.get(6)?,
        extra_data_json: row.get(7)?,
        agent_name: row.get(8)?,
    })
}

/// Fill `child_session_ids` from the typed `parent_session_id` column so
/// a parent whose jsonl never recorded the `task` result still links to
/// its children.
fn attach_children_from_parent_links(sessions: &mut [ParsedSession]) {
    let mut children_by_parent: HashMap<String, Vec<String>> = HashMap::new();
    for session in sessions.iter() {
        let Some(parent_id) = session.meta.parent_id.as_deref() else {
            continue;
        };
        children_by_parent
            .entry(parent_id.to_string())
            .or_default()
            .push(session.meta.id.clone());
    }
    for session in sessions.iter_mut() {
        let Some(children) = children_by_parent.get(&session.meta.id) else {
            continue;
        };
        for child_id in children {
            if !session.child_session_ids.iter().any(|id| id == child_id) {
                session.child_session_ids.push(child_id.clone());
            }
        }
    }
}

fn content_text_from_messages(messages: &[crate::models::Message]) -> String {
    let mut out = String::new();
    for message in messages {
        match message.role {
            crate::models::MessageRole::User | crate::models::MessageRole::Assistant => {
                if message.content.trim().is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&message.content);
            }
            crate::models::MessageRole::System => {
                let Some(thinking) = message.content.strip_prefix("[thinking]\n") else {
                    continue;
                };
                let snippet: String = thinking.chars().take(1000).collect();
                if snippet.trim().is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&snippet);
            }
            crate::models::MessageRole::Tool => {}
        }
    }
    out
}

#[cfg(test)]
mod tests;
