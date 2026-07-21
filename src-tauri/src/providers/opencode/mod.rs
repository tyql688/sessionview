mod parser;

use crate::provider::util::epoch_ms_to_rfc3339;
use parser::{build_assistant_messages, build_user_messages, extract_tokens};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::models::{Message, Provider, SessionMeta};
use crate::provider::util::session_title;
use crate::provider::{
    LoadedSession, ParsedSession, ProviderError, ScanOutcome, SessionProvider, SourceState,
};

pub(crate) struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    fn resume_command(&self, session_id: &str, _variant_name: Option<&str>) -> Option<String> {
        Some(format!("opencode -s {session_id}"))
    }
    fn display_key(&self, _variant_name: Option<&str>) -> String {
        "opencode".into()
    }
    fn sort_order(&self) -> u32 {
        5
    }
    fn color(&self) -> &'static str {
        "#06b6d4"
    }
    fn cli_command(&self) -> &'static str {
        "opencode"
    }
}

pub struct OpenCodeProvider {
    db_path: PathBuf,
}

impl OpenCodeProvider {
    pub(crate) fn new() -> Option<Self> {
        // OpenCode's data dir follows env-paths semantics:
        // %LOCALAPPDATA%\opencode on Windows,
        // XDG_DATA_HOME/opencode (~/.local/share/opencode) on macOS/Linux.
        #[cfg(windows)]
        let base = dirs::data_local_dir()?;
        #[cfg(not(windows))]
        let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(xdg)
        } else {
            dirs::home_dir()?.join(".local").join("share")
        };
        let data_dir = base.join("opencode");
        Some(Self {
            db_path: data_dir.join("opencode.db"),
        })
    }

    /// Construct a provider pointing at an explicit DB path. Used in tests.
    pub fn with_db_path(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn open_db(&self) -> Result<Connection, ProviderError> {
        if !self.db_path.exists() {
            return Err(ProviderError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("OpenCode database not found: {}", self.db_path.display()),
            )));
        }
        let conn = Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(conn)
    }

    fn db_state(&self) -> Result<SourceState, ProviderError> {
        opencode_db_state(&self.db_path)
    }
}

fn file_state(path: &Path) -> Result<SourceState, ProviderError> {
    let metadata = std::fs::metadata(path)?;
    let mtime = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            ProviderError::Parse(format!(
                "OpenCode database mtime is before UNIX epoch for '{}': {error}",
                path.display()
            ))
        })
        .and_then(|duration| {
            i64::try_from(duration.as_nanos()).map_err(|error| {
                ProviderError::Parse(format!(
                    "OpenCode database mtime is too large for '{}': {error}",
                    path.display()
                ))
            })
        })?;
    Ok(SourceState {
        size: metadata.len(),
        mtime,
        title: None,
    })
}

fn opencode_db_state(db_path: &Path) -> Result<SourceState, ProviderError> {
    let db_state = file_state(db_path)?;
    let wal_path = PathBuf::from(format!("{}-wal", db_path.to_string_lossy()));
    let wal_state = match file_state(&wal_path) {
        // Opening SQLite can touch an empty WAL without changing visible data.
        Ok(state) if state.size == 0 => SourceState {
            size: 0,
            mtime: 0,
            title: None,
        },
        Ok(state) => state,
        Err(ProviderError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            SourceState {
                size: 0,
                mtime: 0,
                title: None,
            }
        }
        Err(error) => return Err(error),
    };

    Ok(SourceState {
        size: db_state.size.saturating_add(wal_state.size),
        mtime: db_state.mtime.max(wal_state.mtime),
        title: None,
    })
}

impl SessionProvider for OpenCodeProvider {
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }

    fn source_roots(&self) -> Vec<PathBuf> {
        if self.db_path.exists() {
            vec![self.db_path.parent().unwrap_or(&self.db_path).to_path_buf()]
        } else {
            Vec::new()
        }
    }

    fn scan_all(&self) -> Result<Vec<ParsedSession>, ProviderError> {
        if !self.db_path.exists() {
            return Ok(Vec::new());
        }
        let db_state = self.db_state()?;
        let conn = self.open_db()?;

        // Batch: message counts per session (avoids N+1)
        let mut msg_count_map: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        {
            let mut stmt =
                conn.prepare("SELECT session_id, COUNT(*) FROM message GROUP BY session_id")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for r in rows {
                let r = r?;
                msg_count_map.insert(r.0, r.1 as u32);
            }
        }

        // Batch: content text per session from text parts (avoids N+1)
        // We collect up to 50 text parts per session using a window function.
        let mut content_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT session_id, json_extract(data, '$.text') FROM part
                 WHERE json_extract(data, '$.type') = 'text'
                 ORDER BY session_id, time_created",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            let mut counts: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for r in rows {
                let r = r?;
                let (sid, text) = r;
                let count = counts.entry(sid.clone()).or_insert(0);
                if *count >= 50 {
                    continue;
                }
                *count += 1;
                if let Some(t) = text {
                    content_map
                        .entry(sid)
                        .or_default()
                        .push_str(&format!("{}\n", t));
                }
            }
        }

        // Batch: model per session (first assistant message model)
        let mut model_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        {
            let has_model_col: bool = conn
                .prepare("SELECT COUNT(*) FROM pragma_table_info('message') WHERE name = 'data'")
                .and_then(|mut s| s.query_row([], |row| row.get::<_, i64>(0)))?
                > 0;
            if has_model_col {
                let mut stmt = conn.prepare(
                    "SELECT session_id, json_extract(data, '$.modelID') FROM message
                     WHERE json_extract(data, '$.role') = 'assistant'
                       AND json_extract(data, '$.modelID') IS NOT NULL
                     GROUP BY session_id",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?;
                for r in rows {
                    let r = r?;
                    if let Some(m) = r.1 {
                        model_map.insert(r.0, m);
                    }
                }
            }
        }

        // Batch: aggregate token usage per session for indexer stats
        struct UsageEntry {
            model: Option<String>,
            usage: crate::models::TokenUsage,
            usage_hash: Option<String>,
            timestamp: Option<String>,
        }
        let mut usage_map: std::collections::HashMap<String, Vec<UsageEntry>> =
            std::collections::HashMap::new();
        // Usage rows dropped for malformed JSON or unusable counts, per
        // session — surfaced as the session's parse-warning badge so a
        // silently short total is visible, as for the other providers.
        let mut usage_warnings: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT session_id,
                        id,
                        data,
                        time_created
                 FROM message
                 WHERE json_extract(data, '$.role') = 'assistant'
                   AND json_extract(data, '$.tokens') IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?;
            for r in rows {
                let (sid, msg_id, data, time_created) = r?;
                let msg_json = match serde_json::from_str::<serde_json::Value>(&data) {
                    Ok(value) => value,
                    Err(error) => {
                        log::warn!("skipping malformed OpenCode usage message {msg_id}: {error}");
                        *usage_warnings.entry(sid).or_default() += 1;
                        continue;
                    }
                };
                let Some(usage) = extract_tokens(&msg_json) else {
                    *usage_warnings.entry(sid).or_default() += 1;
                    continue;
                };
                let model = msg_json
                    .get("modelID")
                    .and_then(serde_json::Value::as_str)
                    .filter(|model| !model.is_empty())
                    .map(str::to_string);
                let timestamp = time_created.and_then(epoch_ms_to_rfc3339);
                usage_map.entry(sid).or_default().push(UsageEntry {
                    model,
                    usage,
                    usage_hash: Some(msg_id),
                    timestamp,
                });
            }
        }

        // Batch: git branch per session from workspace
        let mut branch_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        {
            // Check if workspace table exists
            let has_workspace: bool = conn
                .prepare(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='workspace'",
                )
                .and_then(|mut s| s.query_row([], |row| row.get::<_, i64>(0)))?
                > 0;
            if has_workspace {
                let mut stmt = conn.prepare(
                    "SELECT s.id, w.branch
                     FROM session s
                     JOIN project p ON s.project_id = p.id
                     JOIN workspace w ON p.id = w.id
                     WHERE w.branch IS NOT NULL AND w.branch != ''",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for r in rows {
                    let r = r?;
                    branch_map.insert(r.0, r.1);
                }
            }
        }

        // Check if session table has 'version' column (may not exist in older DBs/test fixtures)
        let has_version: bool = conn
            .prepare("SELECT COUNT(*) FROM pragma_table_info('session') WHERE name = 'version'")
            .and_then(|mut s| s.query_row([], |row| row.get::<_, i64>(0)))?
            > 0;

        let query = if has_version {
            "SELECT s.id, s.title, s.directory, s.time_created, s.time_updated,
                    s.parent_id, p.worktree, p.name, s.version
             FROM session s
             LEFT JOIN project p ON s.project_id = p.id
             ORDER BY s.time_updated DESC"
        } else {
            "SELECT s.id, s.title, s.directory, s.time_created, s.time_updated,
                    s.parent_id, p.worktree, p.name, NULL AS version
             FROM session s
             LEFT JOIN project p ON s.project_id = p.id
             ORDER BY s.time_updated DESC"
        };

        let mut stmt = conn.prepare(query)?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,         // id
                row.get::<_, String>(1)?,         // title
                row.get::<_, String>(2)?,         // directory
                row.get::<_, i64>(3)?,            // time_created
                row.get::<_, i64>(4)?,            // time_updated
                row.get::<_, Option<String>>(5)?, // parent_id
                row.get::<_, Option<String>>(6)?, // worktree
                row.get::<_, Option<String>>(7)?, // project name
                row.get::<_, Option<String>>(8)?, // version
            ))
        })?;
        let sessions: Vec<ParsedSession> = rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(
                |(
                    id,
                    title,
                    directory,
                    time_created,
                    time_updated,
                    parent_id,
                    worktree,
                    project_name,
                    version,
                )| {
                    let msg_count = msg_count_map.get(&id).copied().unwrap_or(0);
                    let content_text = content_map.get(&id).cloned().unwrap_or_default();
                    let parse_warning_count = usage_warnings.remove(&id).unwrap_or(0);
                    let usage_entries = usage_map.remove(&id).unwrap_or_default();

                    // Prefer session.directory (actual working dir);
                    // fall back to project.worktree only if directory is empty.
                    // The "global" project has worktree="/", which is not useful.
                    let project_path = if directory.is_empty() || directory == "/" {
                        worktree
                            .filter(|w| w != "/")
                            .unwrap_or_else(|| directory.clone())
                    } else {
                        directory.clone()
                    };
                    let display_title = if title.is_empty() {
                        session_title(Some(&content_text))
                    } else {
                        title
                    };

                    let is_sidechain = parent_id.is_some();
                    let session_model = model_map.get(&id).cloned();
                    let session_branch = branch_map.get(&id).cloned();

                    ParsedSession {
                        meta: SessionMeta {
                            id,
                            provider: Provider::OpenCode,
                            title: display_title,
                            project_path: project_path.clone(),
                            project_name: project_name.unwrap_or_else(|| {
                                std::path::Path::new(&project_path)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default()
                            }),
                            created_at: time_created / 1000,
                            updated_at: time_updated / 1000,
                            message_count: msg_count,
                            file_size_bytes: db_state.size,
                            source_path: self.db_path.to_string_lossy().to_string(),
                            is_sidechain,
                            variant_name: None,
                            model: session_model,
                            cc_version: version.filter(|v| !v.is_empty()),
                            git_branch: session_branch,
                            parent_id: parent_id.clone(),
                            input_tokens: 0,
                            output_tokens: 0,
                            cache_read_tokens: 0,
                            cache_write_tokens: 0,
                        },
                        messages: usage_entries
                            .into_iter()
                            .map(|entry| Message {
                                timestamp: entry.timestamp,
                                token_usage: Some(entry.usage),
                                model: entry.model,
                                usage_hash: entry.usage_hash,
                                ..Message::assistant(String::new())
                            })
                            .collect(),
                        content_text,
                        parse_warning_count,
                        child_session_ids: Vec::new(),
                        usage_events: Vec::new(),
                        source_mtime: db_state.mtime,
                    }
                },
            )
            .collect();

        Ok(sessions)
    }

    fn scan_incremental(
        &self,
        known: &HashMap<String, SourceState>,
    ) -> Result<ScanOutcome, ProviderError> {
        if !self.db_path.exists() {
            return Ok(ScanOutcome::default());
        }

        let source_path = self.db_path.to_string_lossy().to_string();
        let current = self.db_state()?;
        if let Some(previous) = known.get(&source_path)
            && *previous == current
        {
            return Ok(ScanOutcome {
                parsed: Vec::new(),
                unchanged_source_paths: vec![source_path],
            });
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
        let conn = self.open_db()?;

        // Load all messages for this session
        let mut msg_stmt = conn.prepare(
            "SELECT m.id, m.data FROM message m
             WHERE m.session_id = ?1
             ORDER BY m.time_created",
        )?;

        let msg_rows = msg_stmt
            .query_map(params![session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Load all parts for this session, grouped by message_id
        let mut part_stmt = conn.prepare(
            "SELECT message_id, data FROM part
             WHERE session_id = ?1
             ORDER BY id",
        )?;

        let mut parts_by_msg: std::collections::HashMap<String, Vec<serde_json::Value>> =
            std::collections::HashMap::new();
        let part_rows = part_stmt
            .query_map(params![session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        for (mid, data) in part_rows {
            match serde_json::from_str::<serde_json::Value>(&data) {
                Ok(value) => {
                    parts_by_msg.entry(mid).or_default().push(value);
                }
                Err(error) => {
                    log::warn!(
                        "skipping malformed OpenCode part JSON for session {session_id} message {mid}: {error}"
                    );
                }
            }
        }

        let mut messages = Vec::new();

        for (msg_id, msg_data) in &msg_rows {
            let msg_json: serde_json::Value = match serde_json::from_str(msg_data) {
                Ok(value) => value,
                Err(error) => {
                    log::warn!(
                        "skipping malformed OpenCode message JSON for session {session_id} message {msg_id}: {error}"
                    );
                    continue;
                }
            };
            let role_str = msg_json
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user");

            let timestamp = msg_json
                .get("time")
                .and_then(|t| t.get("created"))
                .and_then(|c| c.as_i64())
                .and_then(epoch_ms_to_rfc3339);

            let parts = parts_by_msg.get(msg_id).cloned().unwrap_or_default();

            let ts = timestamp.as_deref();
            match role_str {
                "user" => messages.extend(build_user_messages(&parts, ts)),
                "assistant" => {
                    messages.extend(build_assistant_messages(&parts, &msg_json, msg_id, ts))
                }
                _ => {}
            }
        }

        Ok(LoadedSession::new(messages))
    }
}

#[cfg(test)]
mod tests {
    use super::OpenCodeProvider;

    #[test]
    fn open_db_refuses_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("CREATE TABLE records (id INTEGER)", [])
            .unwrap();

        let provider = OpenCodeProvider::with_db_path(path);
        assert!(
            provider
                .open_db()
                .unwrap()
                .execute("INSERT INTO records VALUES (1)", [])
                .is_err()
        );
    }
}
