//! GitHub Copilot CLI `events.jsonl` parser.
//!
//! One session lives in a single JSONL event log (producer `copilot-agent`)
//! at `$COPILOT_HOME/session-state/<uuid>/events.jsonl` (`COPILOT_HOME`
//! defaults to `~/.copilot`). Sessions carry `data.context.{cwd,gitRoot,branch}`
//! on the `session.start` event.
//!
//! Envelope: every line is `{"type", "data", "id", "timestamp", "parentId"}`
//! with an RFC 3339 `timestamp`. The format is explicitly not a stable
//! contract (GitHub documents 20+ event types, several marked ephemeral or
//! internal), so unknown event types are dropped with a debug log rather
//! than counted as parse warnings; only malformed lines warn.
//!
//! Surface mapping:
//! - `user.message`: `data.content` is the user's own words — never
//!   `transformedContent`, which wraps the prompt in injected system context
//!   (datetime reminders, SQL-table notices). Image attachments reference a
//!   preceding `session.binary_asset` by `assetId`; that asset's base64
//!   payload replaces the `[image: <name>]` placeholder with an
//!   `[Image: source: data:<mime>;base64,…]` marker the renderer understands.
//!   Attachments without a captured asset render as generic `[Attachment]`
//!   markers when content alone would be empty.
//! - `assistant.message`: `data.content` is the visible reply.
//!   `reasoningText`/`reasoningOpaque` are internal reasoning that Copilot
//!   never shows its own users, so they are skipped. `toolRequests` are
//!   announcements of work about to run and duplicate the authoritative
//!   `tool.execution_start` records, so they contribute no messages here.
//!   `data.model` names the model that actually produced the reply; it
//!   becomes the per-message model and the session's current model.
//! - `session.model_change`: the user's selection. Under auto mode this is
//!   the literal `auto` — kept as-is, since it is what the user chose — and
//!   each subsequent `assistant.message.model` refines it to the model the
//!   router actually picked. A session with no reply yet stays `auto`.
//! - `tool.execution_start`: surfaces a Tool message. `arguments` arrives as
//!   a JSON object (newer builds) or a JSON string (older ones); both are
//!   accepted. The result from the paired `tool.execution_complete` is
//!   attached by `toolCallId`; an orphan completion becomes a standalone
//!   Tool message.
//! - `system.message`: the system prompt dump, dropped like DSH's
//!   agent-instructions rows.
//!
//! Subagents (`task` tool) run **inline in the same log**, not in a separate
//! file. `subagent.started` / `subagent.completed` bracket the run and carry
//! the display name, agent type and model; every event the subagent itself
//! produces (`assistant.message`, `tool.execution_*`) carries
//! `parentToolCallId` = the `toolCallId` of the parent's `task` call. That
//! field — not the bracket — is the typed routing signal: in sync mode the
//! parent's own `task` completion lands *inside* the bracket, and in
//! background mode the user keeps talking to the parent while the subagent
//! runs. The one subagent event without the field is its opening
//! `user.message` (the prompt), which is matched against the `task` call's
//! `arguments.prompt`. Each subagent becomes a child `ParsedSession`
//! (`parent_id` set, `is_sidechain`, id `<parent id>:<toolCallId>`, so a
//! nested subagent is `<root>:<outer>:<inner>`), and the parent's Agent
//! tool message carries that `toolCallId` as `agentId` so "Open subagent"
//! resolves it. A background `task` answers with a runtime agent hash
//! (`Agent started in background with agent_id: …`) that later
//! `read_agent` calls name in `arguments.agent_id`; the parser maps that
//! hash back to the `task` call id so those calls link to the same child.
//! Nested subagents chain through the session that issued the inner call.
//!
//! Freshness: the root title comes from the `workspace.yaml` sidecar, which
//! the CLI rewrites on its own schedule (the generated name lands a few
//! seconds after the log's last event), so `source_state` folds the
//! sidecar's mtime into the session's `(size, mtime)` key and a rename
//! alone triggers a re-parse.
//!
//! Usage: `session-store.db` (`assistant_usage_events`, read by the
//! provider and passed in as [`UsageRow`]s) records one row per model call
//! with a timestamp, the model, and `parent_tool_call_id` for subagent
//! calls — so usage lands in the right 15-minute bucket and on the right
//! child session, and live sessions accrue usage before they end. Every
//! model call also yields exactly one `assistant.message` event (empty
//! content when the reply was only tool requests), written right after the
//! store row; within one scope (root or a given subagent) the k-th row is
//! the k-th assistant event, so per-message usage is attached by that
//! order, guarded by matching model names, for display only — the stats
//! pipeline reads the `UsageEvent`s. When the
//! store has no rows for a session the parser falls back to
//! `session.shutdown.modelMetrics`, the only usage the log itself persists.
//! Both sources report cache-inclusive `inputTokens` (verified against the
//! shutdown `tokenDetails` breakdown and GitHub staff statements), so the
//! cached portions are subtracted back out to keep SessionView's disjoint
//! input / cache-read / cache-write invariant. Sessions with neither source
//! carry no usage events at all — partial accounting is never fabricated
//! from per-message `outputTokens`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use crate::models::{Message, MessageRole, Provider, SessionMeta, TokenUsage};
use crate::provider::util::{
    UsageKeys, parse_rfc3339_epoch_seconds, session_title, token_usage_from,
};
use crate::provider::{ParsedSession, UsageEvent, system_time_to_epoch_seconds};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

/// Shutdown usage field paths (camelCase, cache-inclusive input).
const SHUTDOWN_USAGE_KEYS: UsageKeys = UsageKeys {
    input: &["inputTokens"],
    output: &["outputTokens"],
    cache_read: &["cacheReadTokens"],
    cache_write: &["cacheWriteTokens"],
};

/// One `assistant_usage_events` row from `session-store.db`.
#[derive(Debug, Clone)]
pub(crate) struct UsageRow {
    pub row_id: i64,
    /// `task` call this model call ran under; `None` for the root agent.
    pub parent_tool_call_id: Option<String>,
    pub model: String,
    /// `created_at` as stored (RFC 3339 in practice).
    pub created_at: String,
    /// Cache-inclusive input, like `session.shutdown`.
    pub usage: TokenUsage,
}

/// Per-session accumulator. Index 0 of `ParseState::sessions` is the root;
/// every other entry is a subagent keyed by its `task` call id.
#[derive(Default)]
struct SessionAccum {
    /// `task` call id for a subagent; `None` for the root.
    call_id: Option<String>,
    /// Index of the session that issued the `task` call.
    parent: Option<usize>,
    messages: Vec<Message>,
    content_parts: Vec<String>,
    /// `toolCallId` → index into `messages` for the surfaced Tool message.
    tool_by_call_id: HashMap<String, usize>,
    first_user_text: Option<String>,
    first_timestamp_secs: Option<i64>,
    last_timestamp_secs: Option<i64>,
    /// Most recent active model.
    model: Option<String>,
    /// `agentDisplayName` / `agentDescription` from `subagent.started`.
    title: Option<String>,
    /// `agentName` from `subagent.started` (`explore`, `task`, …).
    agent_name: Option<String>,
    /// Prompt the parent passed to `task`; identifies the subagent's
    /// opening `user.message`, which carries no `parentToolCallId`.
    prompt: Option<String>,
    /// Between `subagent.started` and `subagent.completed`.
    open: bool,
    usage_events: Vec<UsageEvent>,
    /// One entry per `assistant.message` event in order — the surfaced
    /// message's index, or `None` when the reply carried no visible text —
    /// so store rows can be attached by position.
    assistant_calls: Vec<Option<usize>>,
}

#[derive(Default)]
struct ParseState {
    sessions: Vec<SessionAccum>,
    /// `task` `toolCallId` → subagent index.
    by_call_id: HashMap<String, usize>,
    /// Any tool `toolCallId` → session that issued it (parents nested subagents).
    call_owner: HashMap<String, usize>,
    /// `task` call id → `arguments.prompt`, consumed by `subagent.started`.
    task_prompts: HashMap<String, String>,
    /// `session.binary_asset` id → rendered image marker.
    assets: HashMap<String, String>,
    /// Background agent runtime hash → the `task` call id that spawned it.
    agent_hash_to_call: HashMap<String, String>,
    parse_warning_count: u32,
    session_id: Option<String>,
    copilot_version: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
}

impl ParseState {
    fn new() -> Self {
        Self {
            sessions: vec![SessionAccum::default()],
            ..Self::default()
        }
    }

    /// Session an event belongs to, by its `parentToolCallId`. An id no
    /// `subagent.started` announced is a broken log (torn or reordered):
    /// the event stays with the root, and the session gets a warning badge.
    fn route(&mut self, data: &Value) -> usize {
        let Some(id) = data.get("parentToolCallId").and_then(Value::as_str) else {
            return 0;
        };
        match self.by_call_id.get(id) {
            Some(index) => *index,
            None => {
                log::warn!(
                    "Copilot event names unknown subagent {id:?} in session {:?}; keeping it on the root",
                    self.session_id
                );
                self.parse_warning_count = self.parse_warning_count.saturating_add(1);
                0
            }
        }
    }
}

/// Sidecar `workspace.yaml` metadata read next to the event log. Both fields
/// are optional; the transcript wins for everything it carries.
struct WorkspaceSidecar {
    title: Option<String>,
    cwd: Option<String>,
}

/// Freshness key for an event log: its own size and the newer of its mtime
/// and the sidecar's, so a `workspace.yaml` rename alone re-parses.
pub(crate) fn source_state(path: &Path) -> Option<crate::provider::SourceState> {
    let metadata = std::fs::metadata(path).ok()?;
    let log_mtime = metadata
        .modified()
        .ok()
        .and_then(system_time_to_epoch_seconds)?;
    let sidecar_mtime = path
        .parent()
        .and_then(|dir| std::fs::metadata(dir.join("workspace.yaml")).ok())
        .and_then(|meta| meta.modified().ok())
        .and_then(system_time_to_epoch_seconds)
        .unwrap_or(0);
    Some(crate::provider::SourceState {
        size: metadata.len(),
        mtime: log_mtime.max(sidecar_mtime),
        title: None,
    })
}

/// Parse one `events.jsonl` artifact into the root [`ParsedSession`] followed
/// by one child per subagent that produced messages. Returns an empty vec
/// when the file cannot be opened/read or carries no surfaced messages.
pub(crate) fn parse_session_file(path: &Path, usage_rows: &[UsageRow]) -> Vec<ParsedSession> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "failed to open Copilot session '{}': {error}",
                path.display()
            );
            return Vec::new();
        }
    };
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            log::warn!(
                "failed to read Copilot session metadata '{}': {error}",
                path.display()
            );
            return Vec::new();
        }
    };
    let file_size = metadata.len();
    let source_mtime = source_state(path).map(|state| state.mtime).unwrap_or(0);

    let mut state = ParseState::new();
    scan_records(BufReader::new(file), path, &mut state);
    if state.sessions[0].messages.is_empty() {
        log::debug!(
            "skipping Copilot session '{}': no surfaced messages",
            path.display()
        );
        return Vec::new();
    }
    attach_store_usage(&mut state, usage_rows);

    // The sidecar fills gaps only; it can change without touching the
    // event log, so a regenerated summary simply appears on the next
    // reparse triggered by any activity.
    let sidecar = read_workspace_sidecar(path);
    let root_id = state
        .session_id
        .clone()
        .unwrap_or_else(|| fallback_session_id(path));
    let project_path = state
        .cwd
        .clone()
        .or_else(|| sidecar.cwd.clone())
        .unwrap_or_default();

    // Parents are always created before their children, so each id can
    // chain onto an already-computed parent id.
    let mut ids: Vec<String> = Vec::with_capacity(state.sessions.len());
    for accum in &state.sessions {
        let id = match (&accum.call_id, accum.parent) {
            (Some(call_id), Some(parent)) => format!("{}:{call_id}", ids[parent]),
            _ => root_id.clone(),
        };
        ids.push(id);
    }
    let mut child_ids_by_parent: HashMap<usize, Vec<String>> = HashMap::new();
    for (index, accum) in state.sessions.iter().enumerate() {
        if let Some(parent) = accum.parent
            && !accum.messages.is_empty()
        {
            child_ids_by_parent
                .entry(parent)
                .or_default()
                .push(ids[index].clone());
        }
    }

    let mut out = Vec::with_capacity(state.sessions.len());
    for (index, accum) in state.sessions.into_iter().enumerate() {
        if accum.messages.is_empty() {
            continue;
        }
        let is_child = accum.parent.is_some();
        let title = if is_child {
            accum
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| session_title(accum.first_user_text.as_deref()))
        } else {
            sidecar
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| session_title(accum.first_user_text.as_deref()))
        };
        let created_at = accum.first_timestamp_secs.unwrap_or(0);
        let updated_at = accum
            .last_timestamp_secs
            .or(accum.first_timestamp_secs)
            .unwrap_or(source_mtime);
        let token_totals = crate::provider::token_totals_from_usage_events(&accum.usage_events);
        let meta = SessionMeta {
            id: ids[index].clone(),
            provider: Provider::Copilot,
            title,
            project_path: project_path.clone(),
            project_name: if project_path.is_empty() {
                String::new()
            } else {
                path_basename(&project_path).to_string()
            },
            created_at,
            updated_at,
            message_count: accum.messages.len() as u32,
            file_size_bytes: file_size,
            source_path: path.to_string_lossy().to_string(),
            is_sidechain: is_child,
            variant_name: accum.agent_name.clone(),
            model: accum.model.clone(),
            cc_version: state.copilot_version.clone(),
            git_branch: state.git_branch.clone(),
            parent_id: accum.parent.map(|parent| ids[parent].clone()),
            input_tokens: token_totals.input_tokens,
            output_tokens: token_totals.output_tokens,
            cache_read_tokens: token_totals.cache_read_tokens,
            cache_write_tokens: token_totals.cache_write_tokens,
        };
        out.push(ParsedSession {
            meta,
            messages: accum.messages,
            content_text: accum.content_parts.join("\n"),
            // Line-level damage is a property of the file; report it once.
            parse_warning_count: if is_child {
                0
            } else {
                state.parse_warning_count
            },
            child_session_ids: child_ids_by_parent.remove(&index).unwrap_or_default(),
            usage_events: accum.usage_events,
            source_mtime,
        });
    }
    out
}

fn scan_records(mut reader: BufReader<File>, path: &Path, state: &mut ParseState) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut line_no = 0usize;
    loop {
        buffer.clear();
        let n = match reader.read_until(b'\n', &mut buffer) {
            Ok(n) => n,
            Err(error) => {
                log::warn!(
                    "failed to read Copilot session '{}': {error}",
                    path.display()
                );
                return;
            }
        };
        if n == 0 {
            break;
        }
        let terminated = buffer.last() == Some(&b'\n');
        if terminated {
            line_no += 1;
        }
        let raw = if terminated {
            &buffer[..buffer.len() - 1]
        } else {
            // Final chunk without a trailing newline. If it is a complete
            // record the writer just hasn't flushed the newline yet — parse
            // it; if it is mid-write garbage it fails to deserialize and is
            // dropped below like any torn tail.
            &buffer[..]
        };
        let line = match std::str::from_utf8(raw) {
            Ok(line) => line,
            Err(error) => {
                log::warn!(
                    "skipping non-UTF-8 Copilot record at line {line_no} in '{}': {error}",
                    path.display()
                );
                state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(error) => {
                // An unterminated final fragment is a crash artifact, not a
                // data problem; anything else is a real malformed line.
                if !terminated {
                    log::debug!("dropping torn final record in '{}'", path.display());
                } else {
                    log::warn!(
                        "skipping malformed Copilot record at line {line_no} in '{}': {error}",
                        path.display()
                    );
                    state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                }
                continue;
            }
        };
        handle_record(&record, state);
    }
}

fn touch_timestamps(accum: &mut SessionAccum, secs: Option<i64>) {
    let Some(secs) = secs else {
        return;
    };
    if accum.first_timestamp_secs.is_none() {
        accum.first_timestamp_secs = Some(secs);
    }
    if accum.last_timestamp_secs.is_none_or(|seen| secs >= seen) {
        accum.last_timestamp_secs = Some(secs);
    }
}

fn handle_record(record: &Value, state: &mut ParseState) {
    let timestamp = record
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);
    let timestamp_secs = timestamp.as_deref().and_then(parse_rfc3339_epoch_seconds);
    // The root's span covers everything in the file, subagents included.
    touch_timestamps(&mut state.sessions[0], timestamp_secs);
    let Some(event_type) = record.get("type").and_then(Value::as_str) else {
        log::warn!("skipping Copilot record without a type tag");
        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
        return;
    };
    let data = record.get("data").unwrap_or(&Value::Null);
    match event_type {
        "session.start" => handle_session_start(data, state),
        "session.binary_asset" => handle_binary_asset(data, state),
        "user.message" => handle_user_message(data, timestamp, timestamp_secs, state),
        "assistant.message" => handle_assistant_message(data, timestamp, timestamp_secs, state),
        "tool.execution_start" => handle_tool_start(data, timestamp, timestamp_secs, state),
        "tool.execution_complete" => handle_tool_complete(data, timestamp, timestamp_secs, state),
        "session.model_change" => {
            if let Some(model) = data
                .get("model")
                .or_else(|| data.get("newModel"))
                .and_then(Value::as_str)
                .filter(|model| !model.is_empty())
            {
                state.sessions[0].model = Some(model.to_string());
            }
        }
        "subagent.started" => handle_subagent_started(data, timestamp_secs, state),
        "subagent.completed" => {
            if let Some(index) = data
                .get("toolCallId")
                .and_then(Value::as_str)
                .and_then(|id| state.by_call_id.get(id).copied())
            {
                state.sessions[index].open = false;
                touch_timestamps(&mut state.sessions[index], timestamp_secs);
            }
        }
        "session.shutdown" => handle_shutdown(data, timestamp, state),
        _ => {
            // The event stream intentionally carries internal/ephemeral
            // types (hooks, plan/mode changes, compaction brackets, usage
            // checkpoints, model call telemetry, …) that no external
            // consumer can rely on; none alter the surfaced transcript.
            log::debug!("skipping unknown Copilot event '{event_type}'");
        }
    }
}

fn handle_session_start(data: &Value, state: &mut ParseState) {
    if state.session_id.is_none() {
        state.session_id = data
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if state.copilot_version.is_none() {
        state.copilot_version = data
            .get("copilotVersion")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    let context = data.get("context");
    if state.cwd.is_none() {
        state.cwd = context
            .and_then(|c| c.get("cwd"))
            .and_then(Value::as_str)
            .filter(|cwd| !cwd.is_empty())
            .map(str::to_string);
    }
    if state.git_branch.is_none() {
        state.git_branch = context
            .and_then(|c| c.get("branch"))
            .and_then(Value::as_str)
            .filter(|branch| !branch.is_empty())
            .map(str::to_string);
    }
}

fn handle_binary_asset(data: &Value, state: &mut ParseState) {
    if data.get("type").and_then(Value::as_str) != Some("image") {
        return;
    }
    let Some(asset_id) = data.get("assetId").and_then(Value::as_str) else {
        return;
    };
    let Some(payload) = data
        .get("data")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|payload| !payload.is_empty())
    else {
        return;
    };
    let mime = data
        .get("mimeType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|mime| !mime.is_empty())
        .unwrap_or("image/png");
    state.assets.insert(
        asset_id.to_string(),
        format!("[Image: source: data:{mime};base64,{payload}]"),
    );
}

fn handle_user_message(
    data: &Value,
    timestamp: Option<String>,
    timestamp_secs: Option<i64>,
    state: &mut ParseState,
) {
    let text = data
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let attachments = data
        .get("attachments")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    // A subagent's opening prompt has no `parentToolCallId`; it is the
    // `task` prompt verbatim, delivered while that subagent is open.
    let trimmed = text.trim();
    let index = state
        .sessions
        .iter()
        .position(|accum| {
            accum.open
                && accum.first_user_text.is_none()
                && accum.prompt.as_deref().map(str::trim) == Some(trimmed)
        })
        .unwrap_or(0);

    let mut body = text.clone();
    let mut plain = text.clone();
    let mut markers: Vec<String> = Vec::new();
    let mut resolved = false;
    let mut unresolved = 0usize;
    for attachment in attachments {
        let marker = attachment
            .get("assetId")
            .and_then(Value::as_str)
            .and_then(|id| state.assets.get(id));
        match marker {
            Some(marker) => {
                resolved = true;
                let placeholder = attachment
                    .get("displayName")
                    .and_then(Value::as_str)
                    .map(|name| format!("[image: {name}]"));
                match placeholder.filter(|placeholder| body.contains(placeholder.as_str())) {
                    Some(placeholder) => {
                        body = body.replace(&placeholder, marker);
                        plain = plain.replace(&placeholder, "").trim().to_string();
                    }
                    None => markers.push(marker.clone()),
                }
            }
            None => unresolved += 1,
        }
    }
    if trimmed.is_empty() && markers.is_empty() {
        if unresolved == 0 {
            return;
        }
        body = vec!["[Attachment]"; unresolved].join("\n");
    }
    if !markers.is_empty() {
        if !body.trim().is_empty() {
            body.push('\n');
        }
        body.push_str(&markers.join("\n"));
    }

    let accum = &mut state.sessions[index];
    touch_timestamps(accum, timestamp_secs);
    // Search text and the title keep the user's words, never the base64
    // payload; an image-only message contributes nothing to either.
    if !resolved {
        plain = body.clone();
    }
    if !plain.trim().is_empty() {
        if accum.first_user_text.is_none() {
            accum.first_user_text = Some(plain.clone());
        }
        accum.content_parts.push(plain);
    }
    accum.messages.push(Message {
        timestamp,
        ..Message::user(body)
    });
}

fn handle_assistant_message(
    data: &Value,
    timestamp: Option<String>,
    timestamp_secs: Option<i64>,
    state: &mut ParseState,
) {
    let index = state.route(data);
    let model = data
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .map(str::to_string);
    let accum = &mut state.sessions[index];
    if model.is_some() {
        accum.model = model.clone();
    }
    let content = data
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.trim().is_empty());
    let Some(content) = content else {
        accum.assistant_calls.push(None);
        return;
    };
    touch_timestamps(accum, timestamp_secs);
    accum.content_parts.push(content.to_string());
    accum.assistant_calls.push(Some(accum.messages.len()));
    accum.messages.push(Message {
        timestamp,
        model,
        ..Message::assistant(content.to_string())
    });
}

fn handle_tool_start(
    data: &Value,
    timestamp: Option<String>,
    timestamp_secs: Option<i64>,
    state: &mut ParseState,
) {
    let Some(call_id) = data.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    let index = state.route(data);
    if state.sessions[index].tool_by_call_id.contains_key(call_id) {
        // Out-of-order duplicate start for a call we already surfaced.
        return;
    }
    let raw_name = data
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let arguments_value = data.get("arguments").unwrap_or(&Value::Null);
    // Older builds serialise `arguments` as a JSON string; decode it so
    // the prompt lookup and the tool summary see the same object.
    let decoded_arguments = match arguments_value {
        Value::String(raw) => serde_json::from_str::<Value>(raw).unwrap_or(Value::Null),
        other => other.clone(),
    };
    if raw_name == "task"
        && let Some(prompt) = decoded_arguments.get("prompt").and_then(Value::as_str)
    {
        state
            .task_prompts
            .insert(call_id.to_string(), prompt.to_string());
    }
    state.call_owner.insert(call_id.to_string(), index);
    let arguments_raw = normalize_arguments(arguments_value);
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Copilot,
        raw_name,
        input: decoded_arguments.is_object().then_some(&decoded_arguments),
        call_id: Some(call_id),
        assistant_id: None,
    });
    // `read_agent` & co. address a background agent by its runtime hash;
    // point them at the child the spawning `task` call created.
    if let Some(task_call) = decoded_arguments
        .get("agent_id")
        .and_then(Value::as_str)
        .and_then(|hash| state.agent_hash_to_call.get(hash))
    {
        let link = serde_json::json!({ "agent_id": task_call });
        enrich_tool_metadata(
            &mut metadata,
            ToolResultFacts {
                raw_result: Some(&link),
                ..ToolResultFacts::default()
            },
        );
    }
    let canonical_name = metadata.canonical_name.clone();
    let accum = &mut state.sessions[index];
    touch_timestamps(accum, timestamp_secs);
    let message_index = accum.messages.len();
    accum.messages.push(Message {
        timestamp,
        tool_name: Some(canonical_name),
        tool_input: Some(arguments_raw),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, String::new())
    });
    accum
        .tool_by_call_id
        .insert(call_id.to_string(), message_index);
}

fn handle_tool_complete(
    data: &Value,
    timestamp: Option<String>,
    timestamp_secs: Option<i64>,
    state: &mut ParseState,
) {
    let Some(call_id) = data.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    let is_error = data.get("success").and_then(Value::as_bool) == Some(false);
    let result_text = extract_result_text(data.get("result").unwrap_or(&Value::Null));
    if (state.task_prompts.contains_key(call_id) || state.by_call_id.contains_key(call_id))
        && let Some(hash) = background_agent_hash(&result_text)
    {
        state
            .agent_hash_to_call
            .insert(hash.to_string(), call_id.to_string());
    }
    let result_facts = ToolResultFacts {
        is_error: if data.get("success").is_some() {
            Some(is_error)
        } else {
            None
        },
        ..ToolResultFacts::default()
    };
    // The completion carries the same `parentToolCallId` as its start; the
    // issuing session is the authoritative owner either way.
    let index = state
        .call_owner
        .get(call_id)
        .copied()
        .unwrap_or_else(|| state.route(data));
    let accum = &mut state.sessions[index];
    touch_timestamps(accum, timestamp_secs);
    if let Some(&message_index) = accum.tool_by_call_id.get(call_id) {
        if !result_text.is_empty() {
            accum.messages[message_index].content = result_text;
        }
        if let Some(metadata) = accum.messages[message_index].tool_metadata.as_mut() {
            metadata
                .ids
                .insert("tool_use_id".to_string(), call_id.to_string());
            enrich_tool_metadata(metadata, result_facts);
        }
        return;
    }
    // Orphan completion: the paired start was never logged (interrupted
    // turn). Standalone Tool message so the result is not lost.
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Copilot,
        raw_name: "tool",
        input: None,
        call_id: Some(call_id),
        assistant_id: None,
    });
    enrich_tool_metadata(&mut metadata, result_facts);
    let canonical_name = metadata.canonical_name.clone();
    accum.messages.push(Message {
        timestamp,
        tool_name: Some(canonical_name),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, result_text)
    });
}

fn handle_subagent_started(data: &Value, timestamp_secs: Option<i64>, state: &mut ParseState) {
    let Some(call_id) = data.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    if state.by_call_id.contains_key(call_id) {
        return;
    }
    let parent = state.call_owner.get(call_id).copied().unwrap_or(0);
    let string = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let mut child = SessionAccum {
        call_id: Some(call_id.to_string()),
        parent: Some(parent),
        title: string("agentDisplayName").or_else(|| string("agentDescription")),
        agent_name: string("agentName").or_else(|| string("agentType")),
        model: string("model"),
        prompt: state.task_prompts.remove(call_id),
        open: true,
        ..SessionAccum::default()
    };
    touch_timestamps(&mut child, timestamp_secs);
    let index = state.sessions.len();
    state.sessions.push(child);
    state.by_call_id.insert(call_id.to_string(), index);

    // Link the parent's Agent tool message to the child so the UI can open it.
    let owner = &mut state.sessions[parent];
    if let Some(&message_index) = owner.tool_by_call_id.get(call_id)
        && let Some(metadata) = owner.messages[message_index].tool_metadata.as_mut()
    {
        let link = serde_json::json!({ "agent_id": call_id });
        enrich_tool_metadata(
            metadata,
            ToolResultFacts {
                raw_result: Some(&link),
                ..ToolResultFacts::default()
            },
        );
    }
}

fn handle_shutdown(data: &Value, timestamp: Option<String>, state: &mut ParseState) {
    let root = &mut state.sessions[0];
    if root.model.is_none()
        && let Some(current) = data
            .get("currentModel")
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
    {
        root.model = Some(current.to_string());
    }
    let Some(metrics) = data.get("modelMetrics").and_then(Value::as_object) else {
        return;
    };
    let Some(timestamp) = timestamp else {
        log::warn!(
            "skipping Copilot shutdown metrics without a timestamp in session {:?}",
            state.session_id
        );
        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
        return;
    };
    for (model, entry) in metrics {
        let Some(usage) = token_usage_from(
            entry.get("usage").unwrap_or(&Value::Null),
            &SHUTDOWN_USAGE_KEYS,
        ) else {
            continue;
        };
        let turn_count = entry
            .pointer("/requests/count")
            .and_then(Value::as_u64)
            .filter(|count| *count > 0)
            .unwrap_or(1);
        let Some(normalized) = disjoint_usage(&usage) else {
            continue;
        };
        root.usage_events.push(UsageEvent {
            timestamp: timestamp.clone(),
            model: model.clone(),
            turn_count,
            input_tokens: u64::from(normalized.input_tokens),
            output_tokens: u64::from(normalized.output_tokens),
            cache_read_input_tokens: u64::from(normalized.cache_read_input_tokens),
            cache_creation_input_tokens: u64::from(normalized.cache_creation_input_tokens),
            usage_hash: None,
            cost_usd: None,
        });
    }
}

/// Subtract the cached portions from a cache-inclusive `input` so summing
/// the four components never double-counts. Saturating: a malformed row
/// must not produce a negative wrap. `None` when nothing remains.
fn disjoint_usage(usage: &TokenUsage) -> Option<TokenUsage> {
    let cache_total = usage
        .cache_read_input_tokens
        .saturating_add(usage.cache_creation_input_tokens);
    let normalized = TokenUsage {
        input_tokens: usage.input_tokens.saturating_sub(cache_total),
        output_tokens: usage.output_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        cache_creation_input_tokens: usage.cache_creation_input_tokens,
    };
    (normalized.total_tokens() > 0).then_some(normalized)
}

/// Replace the shutdown aggregate with per-call store rows when the store
/// has any for this session, routing subagent calls to their child.
fn attach_store_usage(state: &mut ParseState, rows: &[UsageRow]) {
    if rows.is_empty() {
        return;
    }
    for accum in &mut state.sessions {
        accum.usage_events.clear();
    }
    // Position of the next unmatched assistant event per scope.
    let mut next_call: Vec<usize> = vec![0; state.sessions.len()];
    for row in rows {
        let Some(timestamp) = rfc3339_timestamp(&row.created_at) else {
            log::warn!(
                "skipping Copilot usage row {} with unparseable created_at {:?}",
                row.row_id,
                row.created_at
            );
            state.parse_warning_count = state.parse_warning_count.saturating_add(1);
            continue;
        };
        let Some(normalized) = disjoint_usage(&row.usage) else {
            continue;
        };
        let index = row
            .parent_tool_call_id
            .as_deref()
            .and_then(|id| state.by_call_id.get(id).copied())
            .unwrap_or(0);
        attach_row_to_message(
            &mut state.sessions[index],
            &mut next_call[index],
            row,
            &normalized,
        );
        state.sessions[index].usage_events.push(UsageEvent {
            timestamp,
            model: row.model.clone(),
            turn_count: 1,
            input_tokens: u64::from(normalized.input_tokens),
            output_tokens: u64::from(normalized.output_tokens),
            cache_read_input_tokens: u64::from(normalized.cache_read_input_tokens),
            cache_creation_input_tokens: u64::from(normalized.cache_creation_input_tokens),
            usage_hash: Some(format!("copilot-store:{}", row.row_id)),
            cost_usd: None,
        });
    }
}

/// Attach a store row's usage to the assistant message at the same
/// position in this scope. A model mismatch means the order assumption
/// broke for this session; stop attaching rather than mislabel.
fn attach_row_to_message(
    accum: &mut SessionAccum,
    next_call: &mut usize,
    row: &UsageRow,
    normalized: &TokenUsage,
) {
    let Some(slot) = accum.assistant_calls.get(*next_call) else {
        return;
    };
    *next_call += 1;
    let Some(message_index) = *slot else {
        return;
    };
    let message = &mut accum.messages[message_index];
    if message
        .model
        .as_deref()
        .is_some_and(|model| model != row.model)
    {
        log::warn!(
            "Copilot usage row {} model {:?} does not match its assistant message ({:?}); leaving per-message usage unset",
            row.row_id,
            row.model,
            message.model
        );
        *next_call = usize::MAX;
        return;
    }
    message.token_usage = Some(normalized.clone());
}

/// `created_at` is RFC 3339 as written by the CLI; the column default
/// (`datetime('now')`, `YYYY-MM-DD HH:MM:SS` UTC) is accepted too.
fn rfc3339_timestamp(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if parse_rfc3339_epoch_seconds(raw).is_some() {
        return Some(raw.to_string());
    }
    let candidate = format!("{}Z", raw.replacen(' ', "T", 1));
    parse_rfc3339_epoch_seconds(&candidate).map(|_| candidate)
}

/// Runtime hash a background `task` reports: `… agent_id: <hash>…`.
fn background_agent_hash(result_text: &str) -> Option<&str> {
    let rest = &result_text[result_text.find("agent_id:")? + "agent_id:".len()..];
    rest.trim_start()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .next()
        .filter(|hash| !hash.is_empty())
}

/// Accept both argument shapes seen in the wild: a JSON object (newer
/// builds) or a JSON-encoded string (older ones).
fn normalize_arguments(value: &Value) -> String {
    match value {
        Value::String(raw) => raw.clone(),
        Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Pull displayable text out of a completion payload. Strings are used
/// verbatim; objects expose their text under provider-specific keys, and
/// anything else renders as pretty JSON — the payload itself, uninterpreted.
fn extract_result_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Object(_) => ["text", "content", "output"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default()),
        other => other.to_string(),
    }
}

/// Read the optional `workspace.yaml` sidecar next to an event log: minimal
/// flat `key: value` lines, parsed by hand because the interesting keys are
/// a fixed handful and no YAML dependency exists.
fn read_workspace_sidecar(events_path: &Path) -> WorkspaceSidecar {
    let mut sidecar = WorkspaceSidecar {
        title: None,
        cwd: None,
    };
    let Some(dir) = events_path.parent() else {
        return sidecar;
    };
    let Ok(content) = std::fs::read_to_string(dir.join("workspace.yaml")) else {
        return sidecar;
    };
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        // YAML single-quoted scalars escape a quote by doubling it.
        let value = match value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            Some(inner) => inner.replace("''", "'"),
            None => value.trim_matches('"').to_string(),
        };
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" | "summary" => sidecar.title = sidecar.title.or(Some(value)),
            "cwd" => sidecar.cwd = sidecar.cwd.or(Some(value)),
            _ => {}
        }
    }
    sidecar
}

/// Basename that understands both separator conventions, so a Windows
/// session directory read from WSL/macOS still groups under its project.
fn path_basename(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

/// Fall back to the containing directory name when `session.start` was torn
/// off (the CLI writes it as the first line, so this is vanishingly rare).
fn fallback_session_id(path: &Path) -> String {
    path.parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MessageRole;
    use tempfile::TempDir;

    /// Real-shape CLI event log: context on `session.start`, object-shaped
    /// tool arguments, a completion carrying a result, and shutdown metrics
    /// whose `inputTokens` are cache-inclusive.
    const CLI_LOG: &str = r#"{"type":"session.start","data":{"sessionId":"09371a50-9a50-484a-8743-5c696de1623a","version":1,"producer":"copilot-agent","copilotVersion":"0.0.420","startTime":"2026-03-02T15:10:04.678Z","context":{"cwd":"/home/dev/my-project","gitRoot":"/home/dev/my-project","branch":"master"}},"id":"e0","timestamp":"2026-03-02T15:10:04.817Z","parentId":null}
{"type":"user.message","data":{"content":"review my staged changes","transformedContent":"<current_datetime>noise</current_datetime>\n\nreview my staged changes","attachments":[],"interactionId":"i1"},"id":"e1","timestamp":"2026-03-02T15:10:45.058Z","parentId":"e0"}
{"type":"assistant.message","data":{"messageId":"m1","content":"I'll review the staged diff.","toolRequests":[{"toolCallId":"tooluse_1","name":"powershell","arguments":{"command":"git --no-pager diff --cached"},"type":"function"}],"reasoningText":"internal reasoning"},"id":"e2","timestamp":"2026-03-02T15:10:50.235Z","parentId":"e1"}
{"type":"tool.execution_start","data":{"toolCallId":"tooluse_1","toolName":"bash","arguments":{"command":"git --no-pager diff --cached"}},"id":"e3","timestamp":"2026-03-02T15:10:50.500Z","parentId":"e2"}
{"type":"tool.execution_complete","data":{"toolCallId":"tooluse_1","success":true,"result":"diff --git a/src/main.rs"},"id":"e4","timestamp":"2026-03-02T15:10:51.000Z","parentId":"e3"}
{"type":"assistant.message","data":{"messageId":"m2","content":"The staged diff looks good.","toolRequests":[],"reasoningText":""},"id":"e5","timestamp":"2026-03-02T15:10:55.000Z","parentId":"e4"}
{"type":"session.shutdown","data":{"shutdownType":"routine","totalPremiumRequests":2,"modelMetrics":{"claude-sonnet-4.5":{"requests":{"count":10,"cost":2},"usage":{"inputTokens":71282,"outputTokens":900,"cacheReadTokens":35495,"cacheWriteTokens":35783}}},"currentModel":"claude-sonnet-4.5"},"id":"e6","timestamp":"2026-03-06T17:08:10.988Z","parentId":"e5"}
"#;

    /// Real-shape sync `task` run (Copilot CLI 1.0.82): the subagent's own
    /// events carry `parentToolCallId`; its opening prompt does not; the
    /// parent's `task` completion lands *before* `subagent.completed`.
    const SUBAGENT_LOG: &str = r#"{"type":"session.start","data":{"sessionId":"82d820c4-0000-0000-0000-000000000000","copilotVersion":"1.0.82","context":{"cwd":"/home/dev/my-project","branch":"main"}},"id":"s0","timestamp":"2026-09-02T04:53:36.000Z"}
{"type":"session.model_change","data":{"newModel":"auto"},"id":"s1","timestamp":"2026-09-02T04:53:36.100Z"}
{"type":"user.message","data":{"content":"delegate the git log to a subagent"},"id":"s2","timestamp":"2026-09-02T04:53:37.000Z"}
{"type":"assistant.message","data":{"messageId":"m0","model":"mai-code-1.1-flash","content":"","toolRequests":[]},"id":"s3","timestamp":"2026-09-02T04:53:40.000Z"}
{"type":"tool.execution_start","data":{"toolCallId":"call_task1","toolName":"task","arguments":{"description":"Get recent commit subjects","prompt":"Run `git log --oneline -3` and return the subjects.","agent_type":"task","name":"recent-commits","mode":"sync"},"model":"mai-code-1.1-flash"},"id":"s4","timestamp":"2026-09-02T04:53:41.000Z"}
{"type":"subagent.started","data":{"toolCallId":"call_task1","agentName":"task","agentDisplayName":"recent-commits","agentDescription":"Get recent commit subjects","model":"claude-haiku-4.5","agentType":"task","executionMode":"sync"},"id":"s5","timestamp":"2026-09-02T04:53:41.500Z"}
{"type":"user.message","data":{"content":"Run `git log --oneline -3` and return the subjects."},"id":"s6","timestamp":"2026-09-02T04:53:42.000Z"}
{"type":"assistant.message","data":{"messageId":"m1","model":"mai-code-1.1-flash","content":"","toolRequests":[],"parentToolCallId":"call_task1"},"id":"s7","timestamp":"2026-09-02T04:53:43.000Z"}
{"type":"tool.execution_start","data":{"toolCallId":"call_bash1","toolName":"bash","arguments":{"command":"git --no-pager log --oneline -3"},"parentToolCallId":"call_task1"},"id":"s8","timestamp":"2026-09-02T04:53:44.000Z"}
{"type":"tool.execution_complete","data":{"toolCallId":"call_bash1","success":true,"result":{"content":"abc first\ndef second\n123 third"},"parentToolCallId":"call_task1"},"id":"s9","timestamp":"2026-09-02T04:53:45.000Z"}
{"type":"tool.execution_complete","data":{"toolCallId":"call_task1","success":true,"result":{"content":"abc first\ndef second\n123 third"}},"id":"s10","timestamp":"2026-09-02T04:53:46.000Z"}
{"type":"assistant.message","data":{"messageId":"m2","model":"mai-code-1.1-flash","content":"abc first\ndef second\n123 third","toolRequests":[],"parentToolCallId":"call_task1"},"id":"s11","timestamp":"2026-09-02T04:53:47.000Z"}
{"type":"subagent.completed","data":{"toolCallId":"call_task1","agentName":"task","agentDisplayName":"recent-commits","model":"claude-haiku-4.5","totalToolCalls":1,"totalTokens":36285},"id":"s12","timestamp":"2026-09-02T04:53:48.000Z"}
{"type":"assistant.message","data":{"messageId":"m3","model":"mai-code-1.1-flash","content":"The subagent reports: abc first, def second, 123 third.","toolRequests":[]},"id":"s13","timestamp":"2026-09-02T04:53:49.000Z"}
"#;

    fn write_session(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
        let session_dir = dir.path().join("session-state").join(name);
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("events.jsonl");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn parse_all(body: &str, rows: &[UsageRow]) -> Vec<ParsedSession> {
        let dir = TempDir::new().unwrap();
        let path = write_session(&dir, "sid", body);
        parse_session_file(&path, rows)
    }

    fn parse_str(body: &str) -> ParsedSession {
        let mut sessions = parse_all(body, &[]);
        assert!(!sessions.is_empty(), "fixture must parse");
        sessions.remove(0)
    }

    fn row(
        row_id: i64,
        parent: Option<&str>,
        created_at: &str,
        input: u32,
        output: u32,
        read: u32,
        write: u32,
    ) -> UsageRow {
        UsageRow {
            row_id,
            parent_tool_call_id: parent.map(str::to_string),
            model: "mai-code-1.1-flash".to_string(),
            created_at: created_at.to_string(),
            usage: TokenUsage {
                input_tokens: input,
                output_tokens: output,
                cache_read_input_tokens: read,
                cache_creation_input_tokens: write,
            },
        }
    }

    #[test]
    fn cli_session_parses_surface_and_context() {
        let parsed = parse_str(CLI_LOG);
        assert_eq!(parsed.meta.id, "09371a50-9a50-484a-8743-5c696de1623a");
        assert_eq!(parsed.meta.provider, Provider::Copilot);
        assert_eq!(parsed.meta.project_path, "/home/dev/my-project");
        assert_eq!(parsed.meta.project_name, "my-project");
        assert_eq!(parsed.meta.git_branch.as_deref(), Some("master"));
        assert_eq!(parsed.meta.cc_version.as_deref(), Some("0.0.420"));
        assert_eq!(parsed.meta.created_at, 1_772_464_204); // 2026-03-02T15:10:04Z
        assert!(!parsed.meta.is_sidechain);
        assert!(parsed.meta.parent_id.is_none());
        // transformedContent must never leak into the transcript.
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content, "review my staged changes");
        // Assistant reasoningText stays out; visible text stays in.
        assert!(
            parsed
                .messages
                .iter()
                .all(|m| !m.content.contains("internal reasoning"))
        );
        assert!(parsed.content_text.contains("I'll review the staged diff."));
    }

    #[test]
    fn tool_call_pairs_with_result() {
        let parsed = parse_str(CLI_LOG);
        let tools: Vec<&Message> = parsed
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Tool)
            .collect();
        assert_eq!(tools.len(), 1, "announced requests don't duplicate");
        let tool = tools[0];
        assert_eq!(tool.content, "diff --git a/src/main.rs");
        let metadata = tool.tool_metadata.as_ref().unwrap();
        assert_eq!(
            metadata.ids.get("tool_use_id").map(String::as_str),
            Some("tooluse_1")
        );
        assert!(metadata.status.as_deref() != Some("error"));
    }

    #[test]
    fn shutdown_metrics_normalize_cache_inclusive_input() {
        let parsed = parse_str(CLI_LOG);
        // 71282 input - 35495 read - 35783 write = 4 pure input.
        assert_eq!(parsed.usage_events.len(), 1);
        let event = &parsed.usage_events[0];
        assert_eq!(event.model, "claude-sonnet-4.5");
        assert_eq!(event.input_tokens, 4);
        assert_eq!(event.output_tokens, 900);
        assert_eq!(event.cache_read_input_tokens, 35_495);
        assert_eq!(event.cache_creation_input_tokens, 35_783);
        assert_eq!(event.turn_count, 10);
        assert_eq!(parsed.meta.input_tokens, 4);
        assert_eq!(parsed.meta.cache_read_tokens, 35_495);
        assert_eq!(parsed.meta.cache_write_tokens, 35_783);
    }

    /// Store rows replace the shutdown aggregate: per-call timestamps, the
    /// same cache-inclusive normalisation, and a stable dedup hash.
    #[test]
    fn store_rows_replace_shutdown_usage() {
        let rows = [
            row(7, None, "2026-03-02T15:10:50.000Z", 18074, 48, 5888, 0),
            row(8, None, "2026-03-02 15:10:55", 18159, 26, 18048, 0),
        ];
        let parsed = parse_all(CLI_LOG, &rows).remove(0);
        assert_eq!(parsed.usage_events.len(), 2, "shutdown aggregate dropped");
        assert_eq!(parsed.usage_events[0].input_tokens, 12_186);
        assert_eq!(parsed.usage_events[0].cache_read_input_tokens, 5_888);
        assert_eq!(parsed.usage_events[0].turn_count, 1);
        assert_eq!(
            parsed.usage_events[0].usage_hash.as_deref(),
            Some("copilot-store:7")
        );
        // SQLite's default `datetime('now')` shape is accepted too.
        assert_eq!(parsed.usage_events[1].timestamp, "2026-03-02T15:10:55Z");
        assert_eq!(parsed.meta.input_tokens, 12_186 + 111);
        assert_eq!(parsed.meta.output_tokens, 74);
    }

    /// Auto mode: `session.model_change.newModel` is the literal `auto`
    /// (the selection); `assistant.message.data.model` then names the model
    /// the router actually used and refines the session model.
    #[test]
    fn auto_mode_refines_model_from_assistant_message() {
        let change = r#"{"type":"session.model_change","data":{"newModel":"auto"},"id":"a0","timestamp":"2026-03-02T15:10:44.000Z"}"#;
        let user = r#"{"type":"user.message","data":{"content":"hi"},"id":"a1","timestamp":"2026-03-02T15:10:45.058Z"}"#;
        let reply = r#"{"type":"assistant.message","data":{"messageId":"m1","model":"claude-haiku-4.5","content":"Hey!","toolRequests":[]},"id":"a2","timestamp":"2026-03-02T15:10:46.000Z"}"#;

        let no_reply = parse_str(&format!("{change}\n{user}\n"));
        assert_eq!(no_reply.meta.model.as_deref(), Some("auto"));

        let parsed = parse_str(&format!("{change}\n{user}\n{reply}\n"));
        assert_eq!(parsed.meta.model.as_deref(), Some("claude-haiku-4.5"));
        assert_eq!(
            parsed.messages[1].model.as_deref(),
            Some("claude-haiku-4.5")
        );
    }

    #[test]
    fn subagent_events_split_into_child_session() {
        let sessions = parse_all(SUBAGENT_LOG, &[]);
        assert_eq!(sessions.len(), 2, "root + one subagent");
        let (root, child) = (&sessions[0], &sessions[1]);
        let root_id = "82d820c4-0000-0000-0000-000000000000";

        assert_eq!(root.meta.id, root_id);
        assert_eq!(
            root.child_session_ids,
            vec![format!("{root_id}:call_task1")]
        );
        // Parent keeps: user prompt, the Agent tool call (with its own
        // completion, which landed inside the bracket), final reply.
        let root_roles: Vec<MessageRole> = root.messages.iter().map(|m| m.role.clone()).collect();
        assert_eq!(
            root_roles,
            vec![MessageRole::User, MessageRole::Tool, MessageRole::Assistant]
        );
        assert_eq!(
            root.messages[0].content,
            "delegate the git log to a subagent"
        );
        let agent_tool = &root.messages[1];
        assert_eq!(agent_tool.tool_name.as_deref(), Some("Agent"));
        assert_eq!(agent_tool.content, "abc first\ndef second\n123 third");
        let structured = agent_tool
            .tool_metadata
            .as_ref()
            .unwrap()
            .structured
            .as_ref()
            .unwrap();
        assert_eq!(
            structured.get("agentId").and_then(Value::as_str),
            Some("call_task1"),
            "Agent tool links to the child by task call id"
        );
        assert_eq!(root.meta.model.as_deref(), Some("mai-code-1.1-flash"));

        assert_eq!(child.meta.id, format!("{root_id}:call_task1"));
        assert_eq!(child.meta.parent_id.as_deref(), Some(root_id));
        assert!(child.meta.is_sidechain);
        assert_eq!(child.meta.title, "recent-commits");
        assert_eq!(child.meta.variant_name.as_deref(), Some("task"));
        assert_eq!(child.meta.project_path, "/home/dev/my-project");
        assert_eq!(child.meta.created_at, 1_788_324_821); // subagent.started
        let child_roles: Vec<MessageRole> = child.messages.iter().map(|m| m.role.clone()).collect();
        assert_eq!(
            child_roles,
            vec![MessageRole::User, MessageRole::Tool, MessageRole::Assistant]
        );
        assert_eq!(
            child.messages[0].content,
            "Run `git log --oneline -3` and return the subjects."
        );
        assert_eq!(child.messages[1].tool_name.as_deref(), Some("Bash"));
        assert_eq!(
            child.messages[1].content,
            "abc first\ndef second\n123 third"
        );
        assert!(child.child_session_ids.is_empty());
    }

    /// Background mode: the user keeps talking to the parent while the
    /// subagent is open. A user message that is not the task prompt stays
    /// with the parent even though the bracket is still open.
    #[test]
    fn background_subagent_does_not_capture_parent_user_messages() {
        let log = concat!(
            r#"{"type":"session.start","data":{"sessionId":"bg"},"id":"b0","timestamp":"2026-09-02T04:00:00.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"start a background research agent"},"id":"b1","timestamp":"2026-09-02T04:00:01.000Z"}"#,
            "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"toolu_bg","toolName":"task","arguments":{"prompt":"research the codebase","mode":"background","agent_type":"explore"}},"id":"b2","timestamp":"2026-09-02T04:00:02.000Z"}"#,
            "\n",
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"toolu_bg","success":true,"result":"Agent started in background with agent_id: agent_00000000. You'll be notified."},"id":"b3","timestamp":"2026-09-02T04:00:03.000Z"}"#,
            "\n",
            r#"{"type":"subagent.started","data":{"toolCallId":"toolu_bg","agentName":"explore","agentDisplayName":"研究","model":"claude-haiku-4.5","executionMode":"background"},"id":"b4","timestamp":"2026-09-02T04:00:04.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"research the codebase"},"id":"b5","timestamp":"2026-09-02T04:00:05.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"meanwhile, what time is it?"},"id":"b6","timestamp":"2026-09-02T04:00:06.000Z"}"#,
            "\n",
            r#"{"type":"assistant.message","data":{"model":"claude-haiku-4.5","content":"About four."},"id":"b7","timestamp":"2026-09-02T04:00:07.000Z"}"#,
            "\n",
            r#"{"type":"assistant.message","data":{"model":"claude-haiku-4.5","content":"Findings: …","parentToolCallId":"toolu_bg"},"id":"b8","timestamp":"2026-09-02T04:00:08.000Z"}"#,
            "\n",
            r#"{"type":"subagent.completed","data":{"toolCallId":"toolu_bg"},"id":"b9","timestamp":"2026-09-02T04:00:09.000Z"}"#,
            "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"toolu_read","toolName":"read_agent","arguments":"{\"agent_id\":\"agent_00000000\",\"wait\":true}"},"id":"b10","timestamp":"2026-09-02T04:00:10.000Z"}"#,
            "\n",
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"toolu_read","success":true,"result":"Agent is idle. agent_id: agent_00000000"},"id":"b11","timestamp":"2026-09-02T04:00:11.000Z"}"#,
            "\n",
        );
        let sessions = parse_all(log, &[]);
        assert_eq!(sessions.len(), 2);
        // `read_agent` (string-shaped arguments) links to the same child as
        // the spawning task, via the runtime hash the task reported.
        let read_agent = sessions[0].messages.last().unwrap();
        assert_eq!(read_agent.tool_name.as_deref(), Some("Agent"));
        let structured = read_agent
            .tool_metadata
            .as_ref()
            .unwrap()
            .structured
            .as_ref()
            .unwrap();
        assert_eq!(
            structured.get("agentId").and_then(Value::as_str),
            Some("toolu_bg")
        );
        let root_text: Vec<&str> = sessions[0]
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(
            root_text,
            vec![
                "start a background research agent",
                "Agent started in background with agent_id: agent_00000000. You'll be notified.",
                "meanwhile, what time is it?",
                "About four.",
                "Agent is idle. agent_id: agent_00000000",
            ]
        );
        let child_text: Vec<&str> = sessions[1]
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(child_text, vec!["research the codebase", "Findings: …"]);
        assert_eq!(sessions[1].meta.title, "研究");
        assert_eq!(sessions[1].meta.variant_name.as_deref(), Some("explore"));
        assert_eq!(sessions[1].meta.model.as_deref(), Some("claude-haiku-4.5"));
    }

    /// A subagent spawning its own subagent chains ids through its parent,
    /// which is the shape the frontend matcher (`<parent>:<agentId>`) expects.
    #[test]
    fn nested_subagent_chains_ids_through_parent() {
        let log = concat!(
            r#"{"type":"session.start","data":{"sessionId":"root"},"id":"n0","timestamp":"2026-09-02T05:00:00.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"go"},"id":"n1","timestamp":"2026-09-02T05:00:01.000Z"}"#,
            "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"call_outer","toolName":"task","arguments":"{\"prompt\":\"outer job\"}"},"id":"n2","timestamp":"2026-09-02T05:00:02.000Z"}"#,
            "\n",
            r#"{"type":"subagent.started","data":{"toolCallId":"call_outer","agentName":"task","agentDisplayName":"outer"},"id":"n3","timestamp":"2026-09-02T05:00:03.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"outer job"},"id":"n4","timestamp":"2026-09-02T05:00:04.000Z"}"#,
            "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"call_inner","toolName":"task","arguments":{"prompt":"inner job"},"parentToolCallId":"call_outer"},"id":"n5","timestamp":"2026-09-02T05:00:05.000Z"}"#,
            "\n",
            r#"{"type":"subagent.started","data":{"toolCallId":"call_inner","agentName":"task","agentDisplayName":"inner"},"id":"n6","timestamp":"2026-09-02T05:00:06.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"inner job"},"id":"n7","timestamp":"2026-09-02T05:00:07.000Z"}"#,
            "\n",
            r#"{"type":"assistant.message","data":{"content":"inner done","parentToolCallId":"call_inner"},"id":"n8","timestamp":"2026-09-02T05:00:08.000Z"}"#,
            "\n",
            r#"{"type":"assistant.message","data":{"content":"outer done","parentToolCallId":"call_outer"},"id":"n9","timestamp":"2026-09-02T05:00:09.000Z"}"#,
            "\n",
        );
        let sessions = parse_all(log, &[]);
        let ids: Vec<&str> = sessions.iter().map(|s| s.meta.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["root", "root:call_outer", "root:call_outer:call_inner"]
        );
        assert_eq!(
            sessions[2].meta.parent_id.as_deref(),
            Some("root:call_outer")
        );
        assert_eq!(
            sessions[1].child_session_ids,
            vec!["root:call_outer:call_inner"]
        );
        // String-shaped `arguments` still yield the prompt, so the opening
        // user message lands on the child, not the root.
        assert_eq!(sessions[0].messages.len(), 2, "root: user + Agent tool");
        assert_eq!(sessions[1].messages[0].content, "outer job");
        assert_eq!(sessions[2].messages[0].content, "inner job");
    }

    /// An event naming a subagent nobody announced stays on the root and
    /// flags the session instead of vanishing silently.
    #[test]
    fn unknown_parent_tool_call_id_warns() {
        let parsed = parse_str(concat!(
            r#"{"type":"user.message","data":{"content":"hi"},"id":"k0","timestamp":"2026-03-02T15:10:45.058Z"}"#,
            "\n",
            r#"{"type":"assistant.message","data":{"content":"stray","parentToolCallId":"call_nobody"},"id":"k1","timestamp":"2026-03-02T15:10:46.000Z"}"#,
            "\n",
        ));
        assert_eq!(parsed.parse_warning_count, 1);
        assert_eq!(parsed.messages.len(), 2);
    }

    /// Store rows carrying `parent_tool_call_id` land on the child session.
    #[test]
    fn store_rows_route_subagent_usage_to_child() {
        let rows = [
            row(1, None, "2026-09-02T04:53:40.000Z", 18121, 68, 17408, 0),
            row(
                2,
                Some("call_task1"),
                "2026-09-02T04:53:43.000Z",
                18019,
                50,
                0,
                0,
            ),
            row(
                3,
                Some("call_task1"),
                "2026-09-02T04:53:47.000Z",
                18151,
                65,
                17920,
                0,
            ),
            row(4, None, "2026-09-02T04:53:49.000Z", 18257, 65, 18048, 0),
        ];
        let sessions = parse_all(SUBAGENT_LOG, &rows);
        assert_eq!(sessions[0].usage_events.len(), 2);
        assert_eq!(sessions[1].usage_events.len(), 2);
        assert_eq!(sessions[1].meta.input_tokens, 18_019 + 231);
        assert_eq!(sessions[1].meta.output_tokens, 115);
        assert_eq!(sessions[0].meta.input_tokens, 713 + 209);
        // Per-message: the k-th row in a scope is the k-th assistant event.
        // Root: m0 (tool-only, no message) ← row 1; m3 ← row 4.
        let root_final = &sessions[0].messages[2];
        assert_eq!(root_final.role, MessageRole::Assistant);
        let usage = root_final.token_usage.as_ref().unwrap();
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens
            ),
            (209, 65, 18_048)
        );
        // Child: m1 (tool-only) ← row 2; m2 ← row 3.
        let child_final = &sessions[1].messages[2];
        let usage = child_final.token_usage.as_ref().unwrap();
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens
            ),
            (231, 65, 17_920)
        );
    }

    /// A model mismatch means the row/message order assumption broke;
    /// session totals stay, per-message usage is left unset.
    #[test]
    fn store_rows_with_mismatched_model_do_not_label_messages() {
        let mut rows = [
            row(1, None, "2026-09-02T04:53:40.000Z", 100, 1, 0, 0),
            row(2, None, "2026-09-02T04:53:49.000Z", 200, 2, 0, 0),
        ];
        rows[1].model = "some-other-model".to_string();
        let sessions = parse_all(SUBAGENT_LOG, &rows);
        assert_eq!(sessions[0].usage_events.len(), 2);
        assert!(sessions[0].messages[2].token_usage.is_none());
    }

    #[test]
    fn image_attachment_resolves_binary_asset() {
        let log = concat!(
            r#"{"type":"session.start","data":{"sessionId":"img"},"id":"i0","timestamp":"2026-09-02T04:19:00.000Z"}"#,
            "\n",
            r#"{"type":"session.binary_asset","data":{"assetId":"sha256:abc","type":"image","mimeType":"image/png","byteLength":4,"data":"AAAA"},"id":"i1","timestamp":"2026-09-02T04:19:53.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"[image: shot.png] what is this","attachments":[{"type":"file","path":"/tmp/shot.png","displayName":"shot.png","assetId":"sha256:abc","mimeType":"image/png"}]},"id":"i2","timestamp":"2026-09-02T04:19:54.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"","attachments":[{"type":"file","displayName":"other.png","assetId":"sha256:abc"}]},"id":"i3","timestamp":"2026-09-02T04:19:55.000Z"}"#,
            "\n",
        );
        let parsed = parse_str(log);
        assert_eq!(
            parsed.messages[0].content,
            "[Image: source: data:image/png;base64,AAAA] what is this"
        );
        // Placeholder absent from the text: the marker is appended instead.
        assert_eq!(
            parsed.messages[1].content,
            "[Image: source: data:image/png;base64,AAAA]"
        );
        // Search text carries the user's words, never the payload.
        assert!(parsed.content_text.contains("what is this"));
        assert!(!parsed.content_text.contains("[image:"));
        assert!(!parsed.content_text.contains("base64,AAAA"));
        assert_eq!(parsed.meta.title, "what is this");
    }

    #[test]
    fn attachment_only_user_message_renders_markers() {
        let parsed = parse_str(
            r#"{"type":"user.message","data":{"content":"","attachments":[{"name":"a.png"}]},"id":"u1","timestamp":"2026-03-02T15:10:45.058Z"}"#,
        );
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].content, "[Attachment]");
    }

    #[test]
    fn orphan_completion_becomes_standalone_tool_message() {
        let parsed = parse_str(
            r#"{"type":"user.message","data":{"content":"go"},"id":"o0","timestamp":"2026-03-02T15:10:45.058Z"}
{"type":"tool.execution_complete","data":{"toolCallId":"call_y","success":false,"result":"boom"},"id":"o1","timestamp":"2026-03-02T15:11:00.000Z"}"#,
        );
        let tools: Vec<&Message> = parsed
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Tool)
            .collect();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].content, "boom");
        assert_eq!(
            tools[0].tool_metadata.as_ref().unwrap().status.as_deref(),
            Some("error")
        );
    }

    #[test]
    fn malformed_line_warns_but_unknown_events_do_not() {
        let parsed = parse_str(concat!(
            r#"{"type":"user.message","data":{"content":"hi"},"id":"w0","timestamp":"2026-03-02T15:10:45.058Z"}"#,
            "\nnot json\n",
            r#"{"type":"hook.start","data":{}}"#,
            "\n",
            r#"{"no-type-here":true}"#,
            "\n",
        ));
        assert_eq!(parsed.parse_warning_count, 2, "bad json + missing type");
        assert_eq!(parsed.messages.len(), 1, "unknown/lifecycle rows stay out");
    }

    #[test]
    fn torn_final_line_is_not_a_warning() {
        let body = concat!(
            r#"{"type":"user.message","data":{"content":"hi"},"id":"z0","timestamp":"2026-03-02T15:10:45.058Z"}"#,
            "\n{\"type\":\"assistant.mess",
        );
        let parsed = parse_str(body);
        assert_eq!(parsed.parse_warning_count, 0);
        assert_eq!(parsed.messages.len(), 1);
    }

    #[test]
    fn sidecar_title_and_cwd_fill_gaps() {
        let dir = TempDir::new().unwrap();
        let session_dir = dir.path().join("session-state").join("sid");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("workspace.yaml"),
            "id: sid\ncwd: c:\\code\\tmp\\proj\nname: 'Improve ''case'' resolution'\nsummary_count: 0\n",
        )
        .unwrap();
        let log = r#"{"type":"user.message","data":{"content":"first prompt"},"id":"s0","timestamp":"2026-03-02T15:10:45.058Z"}"#;
        std::fs::write(session_dir.join("events.jsonl"), log).unwrap();

        let parsed = parse_session_file(&session_dir.join("events.jsonl"), &[]).remove(0);
        assert_eq!(parsed.meta.title, "Improve 'case' resolution");
        assert_eq!(parsed.meta.project_path, "c:\\code\\tmp\\proj");
        assert_eq!(parsed.meta.project_name, "proj");
    }

    #[test]
    fn empty_log_yields_no_session() {
        let dir = TempDir::new().unwrap();
        let path = write_session(&dir, "empty", "");
        assert!(parse_session_file(&path, &[]).is_empty());
    }
}
