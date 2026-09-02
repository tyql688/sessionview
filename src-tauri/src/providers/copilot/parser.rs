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
//! `user.message` (the prompt). The persisted wire exposes no stable call-id
//! alias on that event, so the parser correlates it only when exactly one open
//! typed `task` call has the same `arguments.prompt`; an ambiguous match stays
//! on the root with a warning. Each subagent becomes a child `ParsedSession`
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
//! the CLI rewrites on its own schedule, while per-call usage comes from the
//! shared store and WAL. `source_state` fingerprints every component's
//! presence, size, and nanosecond mtime, so a title-only or usage-only change
//! reparses immediately even when the event log is untouched.
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
use crate::provider::{ParsedSession, SourceState, UsageEvent};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

mod messages;

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

/// Freshness key for every source that can change this parsed session. The
/// `mtime` slot carries a deterministic metadata fingerprint rather than a
/// literal timestamp, so a same-length sidecar rewrite is still visible even
/// when another shared source has a newer modification time.
pub(crate) fn source_state(path: &Path, store_path: &Path) -> Option<SourceState> {
    let sidecar_path = path.parent()?.join("workspace.yaml");
    let wal_path = std::path::PathBuf::from(format!("{}-wal", store_path.to_string_lossy()));
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut size = 0_u64;

    fingerprint_file(&mut hash, &mut size, b"events", path, true, true)?;
    fingerprint_file(
        &mut hash,
        &mut size,
        b"workspace",
        &sidecar_path,
        false,
        true,
    )?;
    fingerprint_file(&mut hash, &mut size, b"store", store_path, false, true)?;
    fingerprint_file(&mut hash, &mut size, b"store-wal", &wal_path, false, false)?;

    Some(SourceState {
        size,
        mtime: i64::try_from(hash & (i64::MAX as u64))
            .unwrap_or(i64::MAX)
            .max(1),
        title: None,
    })
}

fn fingerprint_file(
    hash: &mut u64,
    total_size: &mut u64,
    tag: &[u8],
    path: &Path,
    required: bool,
    include_empty: bool,
) -> Option<()> {
    hash_bytes(hash, tag);
    match std::fs::metadata(path) {
        Ok(metadata) if include_empty || metadata.len() > 0 => {
            hash_bytes(hash, &[1]);
            hash_bytes(hash, &metadata.len().to_le_bytes());
            let modified = metadata
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos();
            hash_bytes(hash, &modified.to_le_bytes());
            *total_size = total_size.saturating_add(metadata.len());
            Some(())
        }
        Ok(_) => {
            hash_bytes(hash, &[0]);
            Some(())
        }
        Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => {
            hash_bytes(hash, &[0]);
            Some(())
        }
        Err(_) => None,
    }
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Parse one `events.jsonl` artifact into the root [`ParsedSession`] followed
/// by one child per subagent that produced messages. Returns an empty vec
/// when the file cannot be opened/read or carries no surfaced messages.
pub(crate) fn parse_session_file(
    path: &Path,
    usage_rows: &[UsageRow],
    source_state: &SourceState,
) -> Vec<ParsedSession> {
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
    let file_size = source_state.size;
    let source_mtime = source_state.mtime;

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
        let current_line_no = if terminated {
            line_no
        } else {
            line_no.saturating_add(1)
        };
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
                    "skipping non-UTF-8 Copilot record at line {current_line_no} in '{}': {error}",
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
                        "skipping malformed Copilot record at line {current_line_no} in '{}': {error}",
                        path.display()
                    );
                    state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                }
                continue;
            }
        };
        messages::handle_record(&record, path, current_line_no, state);
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
        let index = match row.parent_tool_call_id.as_deref() {
            None => 0,
            Some(call_id) => {
                let Some(index) = state.by_call_id.get(call_id).copied() else {
                    log::warn!(
                        "skipping Copilot usage row {} for unknown child scope",
                        row.row_id
                    );
                    state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                    continue;
                };
                index
            }
        };
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
mod tests;
