mod parser;

use parser::{extract_tokens, ms_to_rfc3339};

use std::path::PathBuf;

use rusqlite::{params, Connection};

use crate::models::{Message, MessageRole, Provider, SessionMeta};
use crate::provider::{
    ChildPlan, DeletionPlan, FileAction, LoadedSession, ParsedSession, ProviderError,
    SessionProvider,
};
use crate::provider_utils::{session_title, truncate_to_bytes, FTS_CONTENT_LIMIT};
use crate::tool_metadata::{
    build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

pub struct Descriptor;
impl crate::provider::ProviderDescriptor for Descriptor {
    fn owns_source_path(&self, source_path: &str) -> bool {
        source_path
            .replace('\\', "/")
            .contains("/opencode/opencode.db")
    }
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
    fn avatar_svg(&self) -> &'static str {
        r#"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path d="M16 6H8v12h8V6zm4 16H4V2h16v20z" fill="currentColor"/></svg>"#
    }
    fn watch_strategy(&self) -> crate::provider::WatchStrategy {
        crate::provider::WatchStrategy::Poll
    }
}

fn opencode_tool_input_value(state: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let input = state?.get("input")?;
    if let Some(text) = input.as_str() {
        serde_json::from_str(text)
            .ok()
            .or_else(|| Some(serde_json::json!({ "input": text })))
    } else {
        Some(input.clone())
    }
}

fn opencode_tool_result_value(
    state: Option<&serde_json::Value>,
    output: &str,
) -> Option<serde_json::Value> {
    let state = state?;
    let mut result = state.clone();
    if let Some(obj) = result.as_object_mut() {
        if !output.is_empty() && !obj.contains_key("output") {
            obj.insert("output".to_string(), serde_json::json!(output));
        }
    }
    Some(result)
}

fn opencode_patch_part_value(part: &serde_json::Value) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "hash": part.get("hash")?.as_str()?,
        "files": part.get("files")?.clone(),
    }))
}

pub struct OpenCodeProvider {
    db_path: PathBuf,
}

impl OpenCodeProvider {
    pub fn new() -> Option<Self> {
        // OpenCode stores its DB in XDG_DATA_HOME/opencode/ (~/.local/share/opencode/ on macOS/Linux)
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
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // Ensure WAL reads see latest committed data
        let _ = conn.pragma_update(None, "journal_mode", "wal");
        // Prevent accidental writes to external database
        let _ = conn.pragma_update(None, "query_only", "ON");
        Ok(conn)
    }
}

impl SessionProvider for OpenCodeProvider {
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
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
        {
            let mut stmt = conn.prepare(
                "SELECT session_id,
                        id,
                        json_extract(data, '$.modelID'),
                        json_extract(data, '$.tokens.input'),
                        json_extract(data, '$.tokens.output'),
                        json_extract(data, '$.tokens.cache.read'),
                        json_extract(data, '$.tokens.cache.write'),
                        time_created
                 FROM message
                 WHERE json_extract(data, '$.role') = 'assistant'
                   AND json_extract(data, '$.tokens') IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                ))
            })?;
            for r in rows {
                let r = r?;
                let (sid, msg_id, model, input, output, cache_read, cache_write, time_created) = r;
                let usage = crate::models::TokenUsage {
                    input_tokens: input.unwrap_or(0) as u32,
                    output_tokens: output.unwrap_or(0) as u32,
                    cache_read_input_tokens: cache_read.unwrap_or(0) as u32,
                    cache_creation_input_tokens: cache_write.unwrap_or(0) as u32,
                };
                let timestamp = time_created.and_then(ms_to_rfc3339);
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
                        session_title(Some(&content_text.chars().take(200).collect::<String>()))
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
                            file_size_bytes: 0,
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
                                role: MessageRole::Assistant,
                                content: String::new(),
                                timestamp: entry.timestamp.or_else(|| ms_to_rfc3339(time_updated)),
                                tool_name: None,
                                tool_input: None,
                                token_usage: Some(entry.usage),
                                model: entry.model,
                                usage_hash: entry.usage_hash,
                                tool_metadata: None,
                            })
                            .collect(),
                        content_text: truncate_to_bytes(&content_text, FTS_CONTENT_LIMIT),
                        parse_warning_count: 0,
                        child_session_ids: Vec::new(),
                        usage_events: Vec::new(),
                    }
                },
            )
            .collect();

        Ok(sessions)
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
                        "skipping malformed OpenCode part JSON for session {} message {}: {}",
                        session_id,
                        mid,
                        error
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
                        "skipping malformed OpenCode message JSON for session {} message {}: {}",
                        session_id,
                        msg_id,
                        error
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
                .and_then(ms_to_rfc3339);

            let parts = parts_by_msg.get(msg_id).cloned().unwrap_or_default();

            match role_str {
                "user" => {
                    // Collect text parts as user message content
                    let text_content: Vec<&str> = parts
                        .iter()
                        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                        .collect();

                    if !text_content.is_empty() {
                        messages.push(Message {
                            role: MessageRole::User,
                            content: text_content.join("\n"),
                            timestamp: timestamp.clone(),
                            tool_name: None,
                            tool_input: None,
                            token_usage: None,
                            model: None,
                            usage_hash: None,
                            tool_metadata: None,
                        });
                    }

                    // Check for file parts (images)
                    for part in &parts {
                        if part.get("type").and_then(|t| t.as_str()) == Some("file") {
                            let mime = part.get("mime").and_then(|m| m.as_str()).unwrap_or("");
                            if mime.starts_with("image/") {
                                let url = part.get("url").and_then(|u| u.as_str()).unwrap_or("");
                                if !url.is_empty() {
                                    messages.push(Message {
                                        role: MessageRole::User,
                                        content: format!("[Image: source: {url}]"),
                                        timestamp: timestamp.clone(),
                                        tool_name: None,
                                        tool_input: None,
                                        token_usage: None,
                                        model: None,
                                        usage_hash: None,
                                        tool_metadata: None,
                                    });
                                }
                            }
                        }
                    }
                }
                "assistant" => {
                    let token_usage = extract_tokens(&msg_json);

                    // Collect text parts
                    let mut text_parts: Vec<String> = Vec::new();
                    // Collect tool parts to emit after the text message
                    let mut tool_messages: Vec<Message> = Vec::new();
                    let mut patch_parts: Vec<serde_json::Value> = Vec::new();

                    for part in &parts {
                        let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match part_type {
                            "text" => {
                                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                    if !text.is_empty() {
                                        text_parts.push(text.to_string());
                                    }
                                }
                            }
                            "reasoning" => {
                                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                    if !text.trim().is_empty() {
                                        let reasoning_ts = part
                                            .get("time")
                                            .and_then(|t| t.get("start"))
                                            .and_then(|s| s.as_i64())
                                            .and_then(ms_to_rfc3339)
                                            .or_else(|| timestamp.clone());
                                        messages.push(Message {
                                            role: MessageRole::System,
                                            content: format!("[thinking]\n{text}"),
                                            timestamp: reasoning_ts,
                                            tool_name: None,
                                            tool_input: None,
                                            token_usage: None,
                                            model: None,
                                            usage_hash: None,
                                            tool_metadata: None,
                                        });
                                    }
                                }
                            }
                            "tool" => {
                                let tool_name = part
                                    .get("tool")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("tool")
                                    .to_string();
                                let state = part.get("state");
                                let status = state
                                    .and_then(|s| s.get("status"))
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("");

                                let input_value = opencode_tool_input_value(state);
                                let mut metadata = build_tool_metadata(ToolCallFacts {
                                    provider: Provider::OpenCode,
                                    raw_name: &tool_name,
                                    input: input_value.as_ref(),
                                    call_id: part
                                        .get("callID")
                                        .or_else(|| part.get("id"))
                                        .and_then(|v| v.as_str()),
                                    assistant_id: Some(msg_id.as_str()),
                                });

                                // Tool input
                                let tool_input = state.and_then(|s| s.get("input")).map(|i| {
                                    i.as_str()
                                        .map(str::to_string)
                                        .unwrap_or_else(|| i.to_string())
                                });

                                // Tool output
                                let output = match status {
                                    "completed" => state
                                        .and_then(|s| s.get("output"))
                                        .and_then(|o| o.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    "error" => state
                                        .and_then(|s| s.get("error"))
                                        .and_then(|e| e.as_str())
                                        .map(|e| format!("[Error] {e}"))
                                        .unwrap_or_default(),
                                    _ => String::new(),
                                };

                                let result_value = opencode_tool_result_value(state, &output);
                                enrich_tool_metadata(
                                    &mut metadata,
                                    ToolResultFacts {
                                        raw_result: result_value.as_ref(),
                                        is_error: Some(status == "error"),
                                        status: (!status.is_empty()).then_some(status),
                                        artifact_path: None,
                                    },
                                );

                                let tool_ts = state
                                    .and_then(|s| s.get("time"))
                                    .and_then(|t| t.get("start"))
                                    .and_then(|s| s.as_i64())
                                    .and_then(ms_to_rfc3339)
                                    .or_else(|| timestamp.clone());

                                // Emit tool use message
                                tool_messages.push(Message {
                                    role: MessageRole::Tool,
                                    content: output,
                                    timestamp: tool_ts,
                                    tool_name: Some(metadata.canonical_name.clone()),
                                    tool_input,
                                    token_usage: None,
                                    model: None,
                                    usage_hash: None,
                                    tool_metadata: Some(metadata),
                                });
                            }
                            "patch" => {
                                if let Some(patch) = opencode_patch_part_value(part) {
                                    patch_parts.push(patch);
                                }
                            }
                            // Skip step-start, step-finish, reasoning, snapshot, patch, etc.
                            _ => {}
                        }
                    }

                    if !patch_parts.is_empty() {
                        for tool_message in tool_messages.iter_mut().rev() {
                            let Some(metadata) = tool_message.tool_metadata.as_mut() else {
                                continue;
                            };
                            if metadata.raw_name != "apply_patch" {
                                continue;
                            }

                            let mut structured = metadata
                                .structured
                                .take()
                                .unwrap_or_else(|| serde_json::json!({}));
                            if !structured.is_object() {
                                structured = serde_json::json!({});
                            }
                            if let Some(obj) = structured.as_object_mut() {
                                if patch_parts.len() == 1 {
                                    obj.insert("patch".to_string(), patch_parts[0].clone());
                                } else {
                                    obj.insert(
                                        "patches".to_string(),
                                        serde_json::Value::Array(patch_parts.clone()),
                                    );
                                }
                            }
                            metadata.structured = Some(structured);
                            break;
                        }
                    }

                    // Emit text message first (with token usage on last text msg of this turn)
                    let msg_model = msg_json
                        .get("modelID")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string());

                    if !text_parts.is_empty() {
                        messages.push(Message {
                            role: MessageRole::Assistant,
                            content: text_parts.join("\n"),
                            timestamp: timestamp.clone(),
                            tool_name: None,
                            tool_input: None,
                            token_usage: if tool_messages.is_empty() {
                                token_usage.clone()
                            } else {
                                None
                            },
                            model: msg_model.clone(),
                            usage_hash: if tool_messages.is_empty() {
                                Some(msg_id.clone())
                            } else {
                                None
                            },
                            tool_metadata: None,
                        });
                    }

                    // Emit tool messages
                    if !tool_messages.is_empty() {
                        let last_idx = tool_messages.len() - 1;
                        for (i, mut tool_msg) in tool_messages.into_iter().enumerate() {
                            // Attach token usage to last tool message if no text parts,
                            // otherwise it was already attached to the text message above
                            if i == last_idx && text_parts.is_empty() {
                                tool_msg.token_usage = token_usage.clone();
                                tool_msg.usage_hash = Some(msg_id.clone());
                            }
                            messages.push(tool_msg);
                        }
                    }

                    // If assistant message had no text and no tools (rare), still emit for token tracking
                    if text_parts.is_empty()
                        && !parts
                            .iter()
                            .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("tool"))
                        && token_usage.is_some()
                    {
                        messages.push(Message {
                            role: MessageRole::Assistant,
                            content: String::new(),
                            timestamp,
                            tool_name: None,
                            tool_input: None,
                            token_usage,
                            model: msg_model,
                            usage_hash: Some(msg_id.clone()),
                            tool_metadata: None,
                        });
                    }
                }
                _ => {}
            }
        }

        Ok(LoadedSession::new(messages))
    }

    fn deletion_plan(&self, _meta: &SessionMeta, children: &[SessionMeta]) -> DeletionPlan {
        let child_plans = children
            .iter()
            .map(|c| ChildPlan {
                id: c.id.clone(),
                source_path: c.source_path.clone(),
                title: c.title.clone(),
                file_action: FileAction::Shared,
            })
            .collect();

        DeletionPlan {
            file_action: FileAction::Shared,
            child_plans,
            cleanup_dirs: Vec::new(),
        }
    }

    fn purge_from_source(&self, source_path: &str, session_id: &str) -> Result<(), ProviderError> {
        let mut conn = Connection::open(source_path)?;
        let tx = conn.transaction()?;

        tx.execute(
            "DELETE FROM part WHERE session_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM message WHERE session_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM todo WHERE session_id = ?1",
            params![session_id],
        )?;
        tx.execute(
            "DELETE FROM session_share WHERE session_id = ?1",
            params![session_id],
        )?;

        // Delete child sessions (subagents)
        let child_ids = {
            let mut child_stmt = tx.prepare("SELECT id FROM session WHERE parent_id = ?1")?;
            let ids = child_stmt
                .query_map(params![session_id], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            ids
        };
        for cid in &child_ids {
            tx.execute("DELETE FROM part WHERE session_id = ?1", params![cid])?;
            tx.execute("DELETE FROM message WHERE session_id = ?1", params![cid])?;
            tx.execute("DELETE FROM todo WHERE session_id = ?1", params![cid])?;
            tx.execute(
                "DELETE FROM session_share WHERE session_id = ?1",
                params![cid],
            )?;
            tx.execute("DELETE FROM session WHERE id = ?1", params![cid])?;
        }
        tx.execute("DELETE FROM session WHERE id = ?1", params![session_id])?;

        tx.commit()?;
        Ok(())
    }
}
