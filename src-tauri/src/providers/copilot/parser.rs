//! GitHub Copilot `events.jsonl` parser.
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
//!   (datetime reminders, SQL-table notices). Attachments render as generic
//!   `[Attachment]` markers when content alone would be empty.
//! - `assistant.message`: `data.content` is the visible reply.
//!   `reasoningText`/`reasoningOpaque` are internal reasoning that Copilot
//!   never shows its own users, so they are skipped. `toolRequests` are
//!   announcements of work about to run and duplicate the authoritative
//!   `tool.execution_start` records, so they contribute no messages here.
//! - `tool.execution_start`: surfaces a Tool message. `arguments` arrives as
//!   a JSON object (newer builds) or a JSON string (older ones); both are
//!   accepted. The result from the paired `tool.execution_complete` is
//!   attached by `toolCallId`; an orphan completion becomes a standalone
//!   Tool message.
//! - `system.message`: the system prompt dump, dropped like DSH's
//!   agent-instructions rows.
//! - `session.shutdown`: the only persisted usage source. Its
//!   `modelMetrics.<model>.usage.inputTokens` is **cache-inclusive**
//!   (verified against GitHub staff statements and ecosystem parsers), so
//!   the cached portions are subtracted back out to keep SessionView's
//!   disjoint input / cache-read / cache-write invariant. Sessions that
//!   never shut down cleanly (crash, SIGKILL, still running) carry no
//!   usage events at all — partial
//!   accounting is never fabricated from per-message `outputTokens`.

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

#[derive(Default)]
struct ParseState {
    messages: Vec<Message>,
    content_parts: Vec<String>,
    parse_warning_count: u32,
    /// `toolCallId` → index into `messages` for the surfaced Tool message.
    tool_by_call_id: HashMap<String, usize>,
    first_user_text: Option<String>,
    last_timestamp_secs: Option<i64>,
    /// Most recent active model (`session.model_change`, then
    /// `session.shutdown.currentModel`). `session.start` carries none.
    model: Option<String>,
    session_id: Option<String>,
    copilot_version: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
    created_at: Option<i64>,
    usage_events: Vec<UsageEvent>,
}

/// Sidecar `workspace.yaml` metadata read next to the event log. Both fields
/// are optional; the transcript wins for everything it carries.
struct WorkspaceSidecar {
    title: Option<String>,
    cwd: Option<String>,
}

/// Parse one `events.jsonl` artifact into a [`ParsedSession`]. Returns `None`
/// when the file cannot be opened/read or carries no surfaced messages.
pub(crate) fn parse_session_file(path: &Path) -> Option<ParsedSession> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "failed to open Copilot session '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            log::warn!(
                "failed to read Copilot session metadata '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let file_size = metadata.len();
    let source_mtime = metadata
        .modified()
        .ok()
        .and_then(system_time_to_epoch_seconds)
        .unwrap_or(0);

    let mut state = ParseState::default();
    scan_records(BufReader::new(file), path, &mut state);
    if state.messages.is_empty() {
        log::debug!(
            "skipping Copilot session '{}': no surfaced messages",
            path.display()
        );
        return None;
    }

    // The sidecar fills gaps only; it can change without touching the
    // event log, so a regenerated summary simply appears on the next
    // reparse triggered by any activity.
    let sidecar = read_workspace_sidecar(path);

    let content_text = state.content_parts.join("\n");
    let meta = assemble_session_meta(path, &state, &sidecar, file_size, source_mtime);
    Some(ParsedSession {
        meta,
        messages: state.messages,
        content_text,
        parse_warning_count: state.parse_warning_count,
        child_session_ids: Vec::new(),
        usage_events: state.usage_events,
        source_mtime,
    })
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
        if buffer.last() != Some(&b'\n') {
            // Final chunk without a trailing newline. If it is a complete
            // record the writer just hasn't flushed the newline yet — parse
            // it; if it is mid-write garbage it fails to deserialize and is
            // dropped below like any torn tail.
        } else {
            line_no += 1;
        }
        let raw = if buffer.last() == Some(&b'\n') {
            &buffer[..buffer.len() - 1]
        } else {
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
                if buffer.last() != Some(&b'\n') {
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

fn handle_record(record: &Value, state: &mut ParseState) {
    let timestamp_secs = record
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_epoch_seconds);
    if let Some(secs) = timestamp_secs
        && state.last_timestamp_secs.is_none_or(|seen| secs >= seen)
    {
        state.last_timestamp_secs = Some(secs);
    }
    if state.created_at.is_none() {
        state.created_at = timestamp_secs;
    }
    let timestamp = record
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(event_type) = record.get("type").and_then(Value::as_str) else {
        log::warn!("skipping Copilot record without a type tag");
        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
        return;
    };
    let data = record.get("data").unwrap_or(&Value::Null);
    match event_type {
        "session.start" => handle_session_start(data, state),
        "user.message" => handle_user_message(data, timestamp, state),
        "assistant.message" => handle_assistant_message(data, timestamp, state),
        "tool.execution_start" => handle_tool_start(data, timestamp, state),
        "tool.execution_complete" => handle_tool_complete(data, timestamp, state),
        "session.model_change" => {
            if let Some(model) = data
                .get("model")
                .or_else(|| data.get("newModel"))
                .and_then(Value::as_str)
                .filter(|model| !model.is_empty())
            {
                state.model = Some(model.to_string());
            }
        }
        "session.shutdown" => handle_shutdown(data, timestamp, state),
        _ => {
            // The event stream intentionally carries internal/ephemeral
            // types (hooks, plan/mode changes, compaction brackets, usage
            // info, subagent bookkeeping, …) that no external consumer can
            // rely on; none alter the meaning of the surfaced transcript.
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

fn handle_user_message(data: &Value, timestamp: Option<String>, state: &mut ParseState) {
    let mut text = data
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if text.trim().is_empty() {
        let attachment_count = data
            .get("attachments")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if attachment_count == 0 {
            return;
        }
        text = vec!["[Attachment]"; attachment_count].join("\n");
    }
    if state.first_user_text.is_none() {
        state.first_user_text = Some(text.clone());
    }
    state.content_parts.push(text.clone());
    state.messages.push(Message {
        timestamp,
        ..Message::user(text)
    });
}

fn handle_assistant_message(data: &Value, timestamp: Option<String>, state: &mut ParseState) {
    let Some(content) = data.get("content").and_then(Value::as_str) else {
        return;
    };
    if content.trim().is_empty() {
        return;
    }
    state.content_parts.push(content.to_string());
    // Assistant messages carry no model on the wire; leave the field unset
    // rather than attributing the currently-active model guesswork.
    state.messages.push(Message {
        timestamp,
        ..Message::assistant(content.to_string())
    });
}

fn handle_tool_start(data: &Value, timestamp: Option<String>, state: &mut ParseState) {
    let Some(call_id) = data.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    if state.tool_by_call_id.contains_key(call_id) {
        // Out-of-order duplicate start for a call we already surfaced.
        return;
    }
    let raw_name = data
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let arguments_value = data.get("arguments").unwrap_or(&Value::Null);
    let arguments_raw = normalize_arguments(arguments_value);
    let metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Copilot,
        raw_name,
        input: if arguments_value.is_object() {
            Some(arguments_value)
        } else {
            None
        },
        call_id: Some(call_id),
        assistant_id: None,
    });
    let canonical_name = metadata.canonical_name.clone();
    let index = state.messages.len();
    state.messages.push(Message {
        timestamp,
        tool_name: Some(canonical_name),
        tool_input: Some(arguments_raw),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, String::new())
    });
    state.tool_by_call_id.insert(call_id.to_string(), index);
}

fn handle_tool_complete(data: &Value, timestamp: Option<String>, state: &mut ParseState) {
    let Some(call_id) = data.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    let is_error = data.get("success").and_then(Value::as_bool) == Some(false);
    let result_text = extract_result_text(data.get("result").unwrap_or(&Value::Null));
    let result_facts = ToolResultFacts {
        is_error: if data.get("success").is_some() {
            Some(is_error)
        } else {
            None
        },
        ..ToolResultFacts::default()
    };
    if let Some(&index) = state.tool_by_call_id.get(call_id) {
        if !result_text.is_empty() {
            state.messages[index].content = result_text;
        }
        if let Some(metadata) = state.messages[index].tool_metadata.as_mut() {
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
    state.messages.push(Message {
        timestamp,
        tool_name: Some(canonical_name),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, result_text)
    });
}

fn handle_shutdown(data: &Value, timestamp: Option<String>, state: &mut ParseState) {
    if state.model.is_none()
        && let Some(current) = data
            .get("currentModel")
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
    {
        state.model = Some(current.to_string());
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
        // `inputTokens` includes both cached portions (confirmed by GitHub
        // staff for the whole Copilot surface); subtract them so summing the
        // four components never double-counts. Saturating: a malformed row
        // must not produce a negative wrap.
        let cache_total = usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
        let pure_input = usage.input_tokens.saturating_sub(cache_total);
        let normalized = TokenUsage {
            input_tokens: pure_input,
            output_tokens: usage.output_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
        };
        if normalized.total_tokens() == 0 {
            continue;
        }
        state.usage_events.push(UsageEvent {
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
        let value = value.trim().trim_matches('"');
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" | "summary" => sidecar.title = sidecar.title.or_else(|| Some(value.to_string())),
            "cwd" => sidecar.cwd = sidecar.cwd.or_else(|| Some(value.to_string())),
            _ => {}
        }
    }
    sidecar
}

fn assemble_session_meta(
    path: &Path,
    state: &ParseState,
    sidecar: &WorkspaceSidecar,
    file_size: u64,
    source_mtime: i64,
) -> SessionMeta {
    let id = state
        .session_id
        .clone()
        .unwrap_or_else(|| fallback_session_id(path));
    let project_path = state
        .cwd
        .clone()
        .or_else(|| sidecar.cwd.clone())
        .unwrap_or_default();
    let project_name = if project_path.is_empty() {
        String::new()
    } else {
        // Sessions recorded on another OS (e.g. Windows paths viewed from
        // WSL) carry foreign separators; split on both so grouping works.
        path_basename(&project_path).to_string()
    };
    let title = sidecar
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| session_title(state.first_user_text.as_deref()));
    let updated_at = state
        .last_timestamp_secs
        .or(state.created_at)
        .unwrap_or(source_mtime);
    let token_totals = crate::provider::token_totals_from_usage_events(&state.usage_events);
    SessionMeta {
        id,
        provider: Provider::Copilot,
        title,
        project_path,
        project_name,
        created_at: state.created_at.unwrap_or(0),
        updated_at,
        message_count: state.messages.len() as u32,
        file_size_bytes: file_size,
        source_path: path.to_string_lossy().to_string(),
        is_sidechain: false,
        variant_name: None,
        model: state.model.clone(),
        cc_version: state.copilot_version.clone(),
        git_branch: state.git_branch.clone(),
        parent_id: None,
        input_tokens: token_totals.input_tokens,
        output_tokens: token_totals.output_tokens,
        cache_read_tokens: token_totals.cache_read_tokens,
        cache_write_tokens: token_totals.cache_write_tokens,
    }
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

    fn write_session(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
        let session_dir = dir.path().join("session-state").join(name);
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("events.jsonl");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn parse_str(body: &str) -> ParsedSession {
        let dir = TempDir::new().unwrap();
        let path = write_session(&dir, "sid", body);
        parse_session_file(&path).expect("fixture must parse")
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
            "id: sid\ncwd: c:\\code\\tmp\\proj\nname: Improve case resolution\nsummary_count: 0\n",
        )
        .unwrap();
        let log = r#"{"type":"user.message","data":{"content":"first prompt"},"id":"s0","timestamp":"2026-03-02T15:10:45.058Z"}"#;
        std::fs::write(session_dir.join("events.jsonl"), log).unwrap();

        let parsed =
            parse_session_file(&session_dir.join("events.jsonl")).expect("fixture must parse");
        assert_eq!(parsed.meta.title, "Improve case resolution");
        assert_eq!(parsed.meta.project_path, "c:\\code\\tmp\\proj");
        assert_eq!(parsed.meta.project_name, "proj");
    }

    #[test]
    fn empty_log_yields_no_session() {
        let dir = TempDir::new().unwrap();
        let path = write_session(&dir, "empty", "");
        assert!(parse_session_file(&path).is_none());
    }
}
