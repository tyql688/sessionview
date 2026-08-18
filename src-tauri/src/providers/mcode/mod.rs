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
//! The split mirrors OpenCode: a single SQLite holds session-level
//! metadata (timestamps, parent links, project bindings) and per-session
//! JSONL files hold the full conversation wire. We key the indexer's
//! freshness snapshot on the SQLite file's `(size, mtime, +wal)` and
//! resolve a session's JSONL lazily on `load_messages` via a path map
//! built during `scan_all`.
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

    /// Combined (size, mtime) for the database + its non-empty WAL. Mirrors
    /// OpenCode's snapshot so an idle runtime (no writes) short-circuits
    /// on every scan.
    fn db_state(&self) -> Result<SourceState, ProviderError> {
        let db_path = self.db_path();
        let db_meta = std::fs::metadata(&db_path)?;
        let mtime = db_meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| {
                ProviderError::Parse(format!(
                    "mcode database mtime predates UNIX epoch for '{}': {error}",
                    db_path.display()
                ))
            })
            .and_then(|d| {
                i64::try_from(d.as_nanos()).map_err(|error| {
                    ProviderError::Parse(format!(
                        "mcode database mtime overflows i64 for '{}': {error}",
                        db_path.display()
                    ))
                })
            })?;
        let wal_path = {
            let mut p = db_path.clone();
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            p.set_file_name(format!("{name}-wal"));
            p
        };
        let (wal_size, wal_mtime) = match std::fs::metadata(&wal_path) {
            Ok(meta) => {
                let wal_mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|d| i64::try_from(d.as_nanos()).ok())
                    .unwrap_or(0);
                // SQLite can touch an empty WAL without committing visible
                // data, so a zero-byte WAL doesn't move the snapshot.
                if meta.len() == 0 {
                    (0u64, 0i64)
                } else {
                    (meta.len(), wal_mtime)
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (0, 0),
            Err(error) => {
                return Err(ProviderError::Io(error));
            }
        };

        Ok(SourceState {
            size: db_meta.len().saturating_add(wal_size),
            mtime: mtime.max(wal_mtime),
            title: None,
        })
    }

    /// Build the `session_id -> messages.jsonl path` map. Walks the
    /// `sessions/` tree once, reading each `manifest.json` for the id.
    /// Returned HashMap is *cloned* (cheap — keys are short) so the
    /// caller can move it into the cache without holding the lock during
    /// the parse pipeline.
    fn build_jsonl_path_map(&self) -> HashMap<String, PathBuf> {
        let mut map = HashMap::new();
        let sessions_dir = self.sessions_dir();
        if !sessions_dir.exists() {
            return map;
        }
        for entry in walkdir::WalkDir::new(&sessions_dir).max_depth(6) {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.file_name() != "manifest.json" {
                continue;
            }
            let manifest_path = entry.path();
            let Ok(content) = std::fs::read_to_string(manifest_path) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&content) else {
                continue;
            };
            let Some(session_id) = value.get("sessionId").and_then(Value::as_str) else {
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
            map.insert(session_id.to_string(), jsonl_path);
        }
        map
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
        let map = self.build_jsonl_path_map();
        let path = map.get(session_id).cloned();
        if let Ok(mut guard) = self.jsonl_paths.lock() {
            *guard = map;
        }
        path
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
        let conn = self.open_db()?;
        let db_state = self.db_state()?;

        // Refresh the jsonl path cache. Cheap (single walk of `sessions/`),
        // and guarantees `load_messages` resolves the file even when the
        // indexer routes a request to a fresh provider instance.
        let jsonl_paths = self.build_jsonl_path_map();
        if let Ok(mut guard) = self.jsonl_paths.lock() {
            *guard = jsonl_paths.clone();
        }

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
            match row.into_parsed(&self.data_root, &jsonl_paths, &db_state) {
                Ok(parsed) => out.push(parsed),
                Err(error) => {
                    log::warn!("skipping mcode session '{session_id}': {error}");
                }
            }
        }
        attach_children_from_parent_links(&mut out);
        Ok(out)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        if !self.db_path().exists() {
            return Ok(ScanOutcome::default());
        }

        let source_path = self.db_path().to_string_lossy().to_string();
        let current = self.db_state()?;
        // Compare only the db+wal fingerprint. `source_states_for_provider`
        // keeps the parent row's session title on `SourceState.title`, which
        // is never equal to `db_state().title` (`None`).
        if let Some(previous) = known.get(&source_path)
            && previous.size == current.size
            && previous.mtime == current.mtime
        {
            return Ok(ScanOutcome {
                parsed: Vec::new(),
                unchanged_source_paths: vec![source_path],
            });
        }

        // Refresh cache and rescan. Even with thousands of sessions the
        // sqlite row read + jsonl parse is comfortably under a second, so
        // we don't bother with row-level freshness tracking.
        let jsonl_paths = self.build_jsonl_path_map();
        if let Ok(mut guard) = self.jsonl_paths.lock() {
            *guard = jsonl_paths;
        }
        Ok(ScanOutcome {
            parsed: self.scan_all()?,
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
        db_state: &SourceState,
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
        // Shared sqlite fingerprint, not the per-session jsonl size —
        // `scan_incremental` keys on this path and must see the same
        // `(size, mtime)` it wrote via `db_state()`.
        let file_size_bytes = db_state.size;
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
            source_mtime: db_state.mtime,
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
mod tests {
    use super::*;
    use crate::models::MessageRole;
    use crate::provider::ProviderDescriptor;
    use std::fs;

    fn write_minimal_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE local_runtime_sessions (
                session_id TEXT PRIMARY KEY,
                record_json TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                columnar_version INTEGER NOT NULL DEFAULT 0,
                agent_name TEXT, runtime TEXT, session_type TEXT,
                status TEXT, archived INTEGER NOT NULL DEFAULT 0,
                visibility TEXT NOT NULL DEFAULT 'visible',
                session_kind TEXT NOT NULL DEFAULT 'conversation',
                parent_session_id TEXT, workspace_dir TEXT,
                project_workspace_dir TEXT,
                is_default_workspace INTEGER NOT NULL DEFAULT 0,
                title TEXT, created_at_ms INTEGER,
                extra_data_json TEXT NOT NULL DEFAULT '{}',
                project_id INTEGER
            );
            "#,
        )
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_session(
        path: &Path,
        id: &str,
        agent: &str,
        session_type: &str,
        parent: Option<&str>,
        title: Option<&str>,
        workspace: &str,
        updated_at_ms: i64,
        extra: &str,
    ) {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "INSERT INTO local_runtime_sessions
                (session_id, record_json, updated_at_ms, agent_name, runtime,
                 session_type, archived, visibility, session_kind,
                 parent_session_id, workspace_dir, project_workspace_dir,
                 is_default_workspace, title, created_at_ms, extra_data_json)
             VALUES (?1, '{}', ?2, ?3, 'pi-agent', ?4, 0, 'visible',
                     'conversation', ?5, ?6, ?6, 0, ?7, ?2, ?8)",
            rusqlite::params![
                id,
                updated_at_ms,
                agent,
                session_type,
                parent,
                workspace,
                title,
                extra
            ],
        )
        .unwrap();
    }

    fn write_messages(dir: &Path, name: &str, body: &str) {
        let sub = dir.join(name);
        fs::create_dir_all(&sub).unwrap();
        let manifest = serde_json::json!({
            "schemaVersion": 1,
            "sessionId": name,
            "paths": {
                "messages": sub.join("messages.jsonl").to_string_lossy().to_string()
            }
        })
        .to_string();
        fs::write(sub.join("manifest.json"), manifest).unwrap();
        fs::write(sub.join("messages.jsonl"), body).unwrap();
    }

    #[test]
    fn resume_command_includes_session_id() {
        let descriptor = Descriptor;
        assert_eq!(
            descriptor.resume_command("mvs_abc", None),
            Some("mcode --session mvs_abc".to_string())
        );
    }

    #[test]
    fn descriptor_static_metadata() {
        let descriptor = Descriptor;
        assert_eq!(descriptor.display_key(None), "mcode");
        assert_eq!(descriptor.sort_order(), 13);
        assert_eq!(descriptor.cli_command(), "mcode");
        assert!(descriptor.color().starts_with('#'));
    }

    #[test]
    fn scan_all_returns_empty_when_db_missing() {
        let dir = tempfile::tempdir().unwrap();
        let provider = McodeProvider::with_data_root(dir.path().to_path_buf());
        let sessions = provider.scan_all().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn scan_all_indexes_user_sessions_and_skips_hidden() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("sqlite").join("runtime-state.sqlite");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        write_minimal_db(&db);

        // Two visible sessions; one is a `peek` bookkeeping row that
        // must NOT show up. session_kind is the right filter.
        insert_session(
            &db,
            "mvs_user1",
            "mavis",
            "root",
            None,
            Some("list /tmp"),
            "/private/tmp",
            1_787_049_058_444,
            r#"{"effectiveModel":"minimax/MiniMax-M3","effectiveModelVariant":"thinking"}"#,
        );
        insert_session(
            &db,
            "mvs_sub1",
            "worker",
            "branch",
            Some("mvs_user1"),
            Some("Main"),
            "/private/tmp",
            1_787_049_060_000,
            "{}",
        );
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO local_runtime_sessions
                (session_id, record_json, updated_at_ms, agent_name, runtime,
                 session_type, archived, visibility, session_kind,
                 parent_session_id, workspace_dir, project_workspace_dir,
                 is_default_workspace, title, created_at_ms, extra_data_json)
             VALUES ('mvs_bookkeeping', '{}', 1, 'mavis', 'pi-agent', 'root',
                     0, 'visible', 'peek', NULL, '/tmp', '/tmp', 0,
                     'Bookkeeping', 1, '{}')",
            [],
        )
        .unwrap();
        // Scaffolding the CLI writes at first launch — hidden by origin.
        conn.execute(
            "INSERT INTO local_runtime_sessions
                (session_id, record_json, updated_at_ms, agent_name, runtime,
                 session_type, archived, visibility, session_kind,
                 parent_session_id, workspace_dir, project_workspace_dir,
                 is_default_workspace, title, created_at_ms, extra_data_json)
             VALUES ('mvs_repair', '{}', 2, 'explore', 'pi-agent', 'root',
                     0, 'visible', 'conversation', NULL, '/tmp', '/tmp', 0,
                     'Main', 2, '{\"origin\":\"root-repair\"}')",
            [],
        )
        .unwrap();
        // Real delegated child: session_kind='task' must stay visible.
        conn.execute(
            "INSERT INTO local_runtime_sessions
                (session_id, record_json, updated_at_ms, agent_name, runtime,
                 session_type, archived, visibility, session_kind,
                 parent_session_id, workspace_dir, project_workspace_dir,
                 is_default_workspace, title, created_at_ms, extra_data_json)
             VALUES ('mvs_task1', '{}', 3, 'explore', 'pi-agent', 'branch',
                     0, 'visible', 'task', 'mvs_user1', '/private/tmp',
                     '/private/tmp', 0, 'Inspect workspace', 3, '{}')",
            [],
        )
        .unwrap();

        // Write minimal jsonl payloads so message_count and file_size work.
        let sessions_dir = root.path().join("sessions");
        write_messages(
            &sessions_dir,
            "mvs_user1",
            "{\"message_id\":\"u1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"<system-reminder>ctx</system-reminder>\\nhi\"}],\"canonicalTextRange\":{\"startOffset\":39,\"endOffset\":41},\"timestamp\":1}}\n{\"message_id\":\"a1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}],\"model\":\"MiniMax-M3\",\"usage\":{\"input\":7,\"output\":3,\"cacheRead\":2,\"cacheWrite\":0},\"timestamp\":1000}}\n",
        );
        write_messages(&sessions_dir, "mvs_sub1", "");
        write_messages(&sessions_dir, "mvs_task1", "");

        let provider = McodeProvider::with_data_root(root.path().to_path_buf());
        let parsed = provider.scan_all().unwrap();
        assert_eq!(
            parsed.len(),
            3,
            "peek and root-repair filtered; task child kept"
        );
        assert!(parsed.iter().all(|p| p.meta.id != "mvs_repair"));
        assert!(parsed.iter().all(|p| p.meta.id != "mvs_bookkeeping"));

        let user = parsed.iter().find(|p| p.meta.id == "mvs_user1").unwrap();
        assert_eq!(user.meta.provider, Provider::Mcode);
        assert_eq!(user.meta.title, "list /tmp");
        assert_eq!(user.meta.project_path, "/private/tmp");
        assert!(!user.meta.is_sidechain, "root session is not a sidechain");
        assert_eq!(user.meta.model.as_deref(), Some("minimax/MiniMax-M3"));
        assert_eq!(user.meta.message_count, 2);
        assert_eq!(user.content_text, "hi\nhello");
        assert_eq!(user.usage_events.len(), 1);
        assert_eq!(user.usage_events[0].input_tokens, 7);
        assert_eq!(user.usage_events[0].cache_read_input_tokens, 2);
        assert_eq!(user.meta.input_tokens, 7);
        assert_eq!(user.meta.output_tokens, 3);
        assert_eq!(user.meta.cache_read_tokens, 2);
        assert_eq!(user.meta.variant_name.as_deref(), Some("mavis"));
        assert_eq!(
            user.child_session_ids,
            vec!["mvs_sub1".to_string(), "mvs_task1".to_string()]
        );

        let sub = parsed.iter().find(|p| p.meta.id == "mvs_sub1").unwrap();
        assert!(sub.meta.is_sidechain, "branch+parent → sidechain");
        assert_eq!(sub.meta.parent_id.as_deref(), Some("mvs_user1"));
        assert_eq!(sub.meta.variant_name.as_deref(), Some("worker"));

        let task = parsed.iter().find(|p| p.meta.id == "mvs_task1").unwrap();
        assert!(task.meta.is_sidechain);
        assert_eq!(task.meta.parent_id.as_deref(), Some("mvs_user1"));
        assert_eq!(task.meta.variant_name.as_deref(), Some("explore"));
        assert_eq!(task.meta.title, "Inspect workspace");
    }

    #[test]
    fn load_messages_reads_jsonl_payload() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("sqlite").join("runtime-state.sqlite");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        write_minimal_db(&db);
        insert_session(
            &db,
            "mvs_xyz",
            "mavis",
            "root",
            None,
            Some("hi"),
            "/tmp",
            1,
            "{}",
        );

        let session_dir = root.path().join("sessions").join("mvs_xyz");
        fs::create_dir_all(&session_dir).unwrap();
        let body = "{\"message_id\":\"u1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1000}}\n\
                    {\"message_id\":\"a1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}],\"model\":\"MiniMax-M3\",\"usage\":{\"input\":7,\"output\":3,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":10},\"timestamp\":2000}}\n";
        let manifest = serde_json::json!({
            "schemaVersion": 1,
            "sessionId": "mvs_xyz",
            "paths": {
                "messages": session_dir.join("messages.jsonl").to_string_lossy().to_string()
            }
        })
        .to_string();
        fs::write(session_dir.join("manifest.json"), manifest).unwrap();
        fs::write(session_dir.join("messages.jsonl"), body).unwrap();

        let provider = McodeProvider::with_data_root(root.path().to_path_buf());
        // Force path-map build via scan_all
        let _ = provider.scan_all().unwrap();
        let loaded = provider
            .load_messages("mvs_xyz", "ignored")
            .expect("load ok");
        assert_eq!(loaded.messages.len(), 2);
        let assistant = &loaded.messages[1];
        assert_eq!(assistant.role, MessageRole::Assistant);
        assert_eq!(assistant.content, "hello");
        assert_eq!(assistant.model.as_deref(), Some("MiniMax-M3"));
        assert_eq!(assistant.token_usage.as_ref().unwrap().input_tokens, 7);
        assert_eq!(loaded.token_totals.input_tokens, 7);
        assert_eq!(loaded.token_totals.output_tokens, 3);
    }

    #[test]
    fn load_messages_resolves_dated_session_dir_without_scan() {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;

        let root = tempfile::tempdir().unwrap();
        let session_id = "mvs_dated1";
        let encoded = URL_SAFE_NO_PAD.encode(session_id.as_bytes());
        let session_dir = root
            .path()
            .join("sessions")
            .join("2026")
            .join("08")
            .join("18")
            .join(format!("18-30-38-471-session_{encoded}"));
        fs::create_dir_all(&session_dir).unwrap();
        let body = "{\"message_id\":\"u1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1}}\n";
        let manifest = serde_json::json!({
            "schemaVersion": 1,
            "sessionId": session_id,
            "paths": {
                "messages": session_dir.join("messages.jsonl").to_string_lossy().to_string()
            }
        })
        .to_string();
        fs::write(session_dir.join("manifest.json"), manifest).unwrap();
        fs::write(session_dir.join("messages.jsonl"), body).unwrap();

        let provider = McodeProvider::with_data_root(root.path().to_path_buf());
        let loaded = provider
            .load_messages(session_id, "ignored")
            .expect("load without scan_all");
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].content, "hi");
    }

    #[test]
    fn scan_incremental_short_circuits_when_db_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("sqlite").join("runtime-state.sqlite");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        write_minimal_db(&db);
        insert_session(
            &db,
            "mvs_user1",
            "mavis",
            "root",
            None,
            Some("hi"),
            "/tmp",
            1,
            "{}",
        );
        write_messages(
            &root.path().join("sessions"),
            "mvs_user1",
            "{\"message_id\":\"u1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1}}\n",
        );

        let provider = McodeProvider::with_data_root(root.path().to_path_buf());
        let first = provider.scan_incremental(&HashMap::new()).unwrap().parsed;
        assert_eq!(first.len(), 1);
        let db_state = provider.db_state().unwrap();
        assert_eq!(first[0].meta.file_size_bytes, db_state.size);
        assert_eq!(first[0].source_mtime, db_state.mtime);

        let mut known = HashMap::new();
        known.insert(
            provider.db_path().to_string_lossy().to_string(),
            SourceState {
                size: first[0].meta.file_size_bytes,
                mtime: first[0].source_mtime,
                title: Some(first[0].meta.title.clone()),
            },
        );
        let second = provider.scan_incremental(&known).unwrap();
        assert!(second.parsed.is_empty(), "unchanged db must not reparse");
        assert_eq!(
            second.unchanged_source_paths,
            vec![provider.db_path().to_string_lossy().to_string()]
        );
    }

    #[test]
    fn resolve_data_root_prefers_minimax_env() {
        assert_eq!(
            resolve_data_root(
                Some("/custom/minimax".into()),
                Some("/legacy/mavis".into()),
                Some(PathBuf::from("/home/u")),
            ),
            Some(PathBuf::from("/custom/minimax/v2"))
        );
        assert_eq!(
            resolve_data_root(
                None,
                Some("/legacy/mavis".into()),
                Some(PathBuf::from("/home/u"))
            ),
            Some(PathBuf::from("/legacy/mavis/v2"))
        );
        assert_eq!(
            resolve_data_root(None, None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.minimax/v2"))
        );
    }

    /// End-to-end smoke test against the real mcode runtime state on this
    /// machine. Skipped when `~/.minimax/v2/` is absent so the test stays
    /// green in CI / fresh checkouts. Run with:
    ///   cargo test --lib mcode::tests::smoke_against_real_runtime -- --ignored --nocapture
    #[test]
    #[ignore = "hits the real ~/.minimax/v2/ tree; run with --ignored"]
    fn smoke_against_real_runtime() {
        let Some(home) = dirs::home_dir() else {
            eprintln!("HOME not set; skipping smoke test");
            return;
        };
        let data_root = home.join(".minimax").join("v2");
        if !data_root.exists() {
            eprintln!("{} not present; skipping smoke test", data_root.display());
            return;
        }
        let provider = McodeProvider::with_data_root(data_root.clone());
        let sessions = provider.scan_all().expect("scan_all");
        assert!(
            sessions
                .iter()
                .any(|s| !s.meta.is_sidechain && s.meta.message_count > 0),
            "expected at least one non-sidechain user session with messages"
        );
        for child in sessions.iter().filter(|s| s.meta.is_sidechain) {
            assert!(
                child.meta.parent_id.is_some(),
                "sidechain {} missing parent_id",
                child.meta.id
            );
        }
        eprintln!(
            "scanned {} mcode sessions from {}",
            sessions.len(),
            data_root.display()
        );
        for parsed in &sessions {
            eprintln!(
                "  {:<42} sidechain={:5} parent={:?} variant={:?} children={:?} title={:?} project={} messages={} model={:?}",
                parsed.meta.id,
                parsed.meta.is_sidechain,
                parsed.meta.parent_id,
                parsed.meta.variant_name,
                parsed.child_session_ids,
                parsed.meta.title,
                parsed.meta.project_path,
                parsed.meta.message_count,
                parsed.meta.model,
            );
        }
        if let Some(first) = sessions.first() {
            eprintln!("loading first: {}", first.meta.id);
            let fresh = McodeProvider::with_data_root(data_root);
            let loaded = fresh
                .load_messages(&first.meta.id, "ignored")
                .expect("load without scan_all");
            assert_eq!(loaded.messages.len(), first.meta.message_count as usize);
            eprintln!(
                "  -> {} messages, {} parse warnings",
                loaded.messages.len(),
                loaded.parse_warning_count
            );
            for (i, m) in loaded.messages.iter().enumerate().take(5) {
                let preview: String = m.content.chars().take(60).collect();
                eprintln!(
                    "  [{}] role={:?} tool={:?} content={:?}",
                    i, m.role, m.tool_name, preview
                );
            }
        }
    }
}
