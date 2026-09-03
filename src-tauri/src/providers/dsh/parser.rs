//! DSH (DeepSeek Harness) session-log parser.
//!
//! On-disk format (authoritative spec: `@deepseek-ai/dsh-session` and
//! `@deepseek-ai/dsh-session-persistence-jsonl`):
//!
//! - One session lives in its own directory:
//!   `$DSH_HOME/sessions/<project-key>/<session-id>/session.jsonl[.zstd]`
//!   (`DSH_HOME` defaults to `~/.dsh`). The artifact is zstd-compressed JSONL
//!   when the suffix is `.jsonl.zstd`; uncompressed `.jsonl` is also accepted.
//! - The first record is the immutable session header
//!   `{type:"session", version, id, createdAt, cwd?, parentSession?, origin?, agentPreset?, ...}`.
//! - Every following record is a session event `{type, seq, time, data}` or a
//!   packed chunk row (`text-chunks` / `reasoning-chunks` / `tool-call-chunks`)
//!   that replays raw stream deltas in one storage line.
//! - Only three event types carry conversation surface semantics:
//!   `user/message`, `assistant/message`, `tool/result`. Everything else
//!   (`turn/*`, `step/*`, `request/*`, `assistant/chunk`, plugin rows such as
//!   `session/title`, `permission/preset`, …) is log-only and never contributes
//!   to the transcript.
//!
//! Surface mapping:
//! - `user/message` whose `source.kind` is `"user"` becomes a User message.
//!   Workspace-instruction dumps (`source.kind == "agent-instructions"`) and
//!   runtime-context snapshots (`source.kind == "plugin"` with
//!   `form == "snapshot"`/`"instructions"`) are system-injected context, not
//!   conversation — the DSH GUI collapses them, so we drop them. Every other
//!   surfaced user-role message (approval-policy changes, goal rounds, cron
//!   notices, …) renders as a System line.
//! - `assistant/message` blocks: `text` → Assistant message (flushed around
//!   tool calls), `reasoning` → System `[thinking]` line (Claude convention),
//!   `tool-call` → Tool message with metadata, `image` → `[Image]` marker.
//! - `tool/result` content is attached to the Tool message whose `callId` the
//!   `assistant/message` (or `tool/call`) surfaced; an orphan result creates a
//!   standalone Tool message.
//! - The creation-time `agentPreset` and later `agent-preset/selected` commits
//!   fold into `SessionMeta.variant_name`; the later committed selection wins.
//! - Usage (`assistant/message.usage`) is attached to the event's last
//!   non-System message, mirroring the Claude provider; a thinking-only step
//!   still gets a placeholder so accounting is never silently dropped.
//!
//! Robustness:
//! - A torn final record (no trailing newline) is a crash artifact; DSH's own
//!   scanner ignores it, so do we (no parse warning).
//! - Malformed lines and unknown *required* event types (no `ignorable: true`)
//!   are logged and counted into `parse_warning_count` so the UI shows the ⚠
//!   badge instead of rendering a silently wrong transcript. Unknown ignorable
//!   rows are dropped silently, as the spec intends.
//! - A step whose stream never assembled an `assistant/message` (interrupted
//!   mid-stream) is reconstructed from its buffered chunks at `step/end`
//!   (or end of file), so crash-orphaned steps still show their partial text.
//! - Compaction is honored: a surface event carrying
//!   `surfaceOp: {op: "replace", start, end}` shadows every surface node whose
//!   seq appears in its `sourceEventSeqs` (the `compaction/*` bracket rows are
//!   log-only). Shadowed messages are removed from the transcript and the
//!   replacement's messages take their place. Their token usage is preserved
//!   in `ParsedSession::usage_events` (DSH's own usage collector also folds
//!   the whole event stream), and the search `content_text` keeps the old text
//!   so compacted sessions stay searchable.
//!
//! Token usage rides `ParsedSession::usage_events` (one row per
//! `assistant/message` event that reports usage), which keeps stats complete
//! across compaction; per-message `token_usage` is still attached for display.
//! An event without its own `source.model` falls back to the session-level
//! model; a row that still lacks a model (or a timestamp) is skipped as a
//! counted parse warning, never silently.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use crate::models::{Message, MessageRole, Provider, SessionMeta, TokenUsage};
use crate::provider::util::{UsageKeys, epoch_ms_to_rfc3339, session_title, token_usage_from};
use crate::provider::{ParsedSession, ProviderError, UsageEvent, system_time_to_epoch_seconds};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

/// DSH `assistant/message.usage` field paths (camelCase, disjoint counts).
const DSH_USAGE_KEYS: UsageKeys = UsageKeys {
    input: &["inputTokens"],
    output: &["outputTokens"],
    cache_read: &["cacheReadTokens"],
    cache_write: &["cacheWriteTokens"],
};

const MISSING: Value = Value::Null;

#[derive(serde::Deserialize)]
struct DshHeader {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    version: Option<i64>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "createdAt")]
    created_at: Option<i64>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default, rename = "parentSession")]
    parent_session: Option<String>,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default, rename = "agentPreset")]
    agent_preset: Option<String>,
}

/// Buffered stream deltas for one step whose `assistant/message` never
/// assembled (interrupted stream). Keyed by stream block index so blocks keep
/// their relative order when flushed.
#[derive(Default)]
struct StepChunkBuf {
    text: BTreeMap<usize, String>,
    reasoning: BTreeMap<usize, String>,
    /// Block index → (call id, tool name, joined raw arguments).
    tool_calls: BTreeMap<usize, (String, Option<String>, String)>,
    /// Seq of the most recent chunk row in this buffer (`seq0` for packed
    /// rows). Gives chunk-reconstructed messages a provenance so a later
    /// compaction splice can remove them when it shadows the step.
    last_seq: Option<u64>,
    /// Last streamed usage chunk for the step. The assembled message's own
    /// usage supersedes it (the buffer is dropped on assembly), so this only
    /// feeds interrupted steps at flush — without it their tokens vanish.
    usage: Option<TokenUsage>,
    /// Timestamp of that usage chunk's row, for the usage event's bucket.
    usage_timestamp: Option<String>,
}

#[derive(Default)]
struct ParseState {
    messages: Vec<Message>,
    content_parts: Vec<String>,
    parse_warning_count: u32,
    /// `toolCallId` → index into `messages` for the surfaced Tool message.
    tool_by_call_id: HashMap<String, usize>,
    latest_title: Option<String>,
    first_user_text: Option<String>,
    /// `subagent/descriptor.label`: the delegation name the parent chose.
    /// A delegated session is never LLM-titled, so this is its display name.
    descriptor_label: Option<String>,
    last_event_time_ms: Option<i64>,
    model: Option<String>,
    /// Effective agent preset. A committed `agent-preset/selected` event
    /// overrides the immutable creation-time header value.
    agent_preset: Option<String>,
    /// Chunk deltas per (turn, step), dropped once the step's
    /// `assistant/message` arrives or flushed at `step/end` when it never does.
    chunk_bufs: HashMap<(u32, u32), StepChunkBuf>,
    /// Steps whose `assistant/message` already assembled; late chunk rows for
    /// them are ignored so an out-of-order log cannot duplicate their text.
    assembled_steps: HashSet<(u32, u32)>,
    /// Surface-event seq → indices of the messages it produced. Drives
    /// compaction `replace` splices; rebuilt after every splice.
    seq_to_messages: HashMap<u64, Vec<usize>>,
    /// Parallel to `messages`: the surface-event seq that produced each
    /// message (`None` for chunk-reconstructed or orphan messages).
    message_seq: Vec<Option<u64>>,
    /// One row per `assistant/message` event that reports usage. Kept even
    /// when compaction shadows the message, so token stats stay complete
    /// (DSH's own usage collector folds the whole event stream too).
    usage_events: Vec<UsageEvent>,
}

impl ParseState {
    fn push_user(&mut self, text: String, timestamp: Option<String>, seq: Option<u64>) {
        if self.first_user_text.is_none() {
            self.first_user_text = Some(text.clone());
        }
        self.content_parts.push(text.clone());
        self.push_with_provenance(
            Message {
                timestamp,
                ..Message::user(text)
            },
            seq,
        );
    }

    fn push_assistant(
        &mut self,
        text: String,
        model: Option<String>,
        timestamp: Option<String>,
        seq: Option<u64>,
    ) {
        self.content_parts.push(text.clone());
        self.push_with_provenance(
            Message {
                timestamp,
                model,
                ..Message::assistant(text)
            },
            seq,
        );
    }

    /// The single append point for `messages`: records the message's
    /// provenance seq (`None` when the record carried no seq) so
    /// `message_seq` stays index-parallel with `messages` — the invariant
    /// compaction splices depend on.
    fn push_with_provenance(&mut self, message: Message, seq: Option<u64>) {
        debug_assert_eq!(self.messages.len(), self.message_seq.len());
        let index = self.messages.len();
        self.messages.push(message);
        self.message_seq.push(seq);
        if let Some(seq) = seq {
            self.seq_to_messages.entry(seq).or_default().push(index);
        }
    }
}

/// Open a session artifact, transparently decompressing zstd frames when the
/// file name carries the `.zstd` suffix.
fn open_log_reader(path: &Path, file: File) -> Result<Box<dyn BufRead>, ProviderError> {
    let is_zstd = path.extension().and_then(|e| e.to_str()) == Some("zstd");
    if is_zstd {
        let decoder = zstd::stream::read::Decoder::new(BufReader::new(file))?;
        Ok(Box::new(BufReader::new(decoder)))
    } else {
        Ok(Box::new(BufReader::new(file)))
    }
}

/// Consume every record after the header line, dispatching surface events
/// into `state`. Returns `None` when the file is unusable (unreadable stream,
/// missing/malformed header); a torn final record without a trailing newline
/// is ignored like DSH's own scanner does.
fn scan_records(
    mut reader: Box<dyn BufRead>,
    path: &Path,
    state: &mut ParseState,
) -> Option<DshHeader> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut header: Option<DshHeader> = None;
    let mut line_no = 0usize;
    loop {
        buffer.clear();
        let n = match reader.read_until(b'\n', &mut buffer) {
            Ok(n) => n,
            Err(error) => {
                log::warn!("failed to read DSH session '{}': {error}", path.display());
                return None;
            }
        };
        if n == 0 {
            break;
        }
        if buffer.last() != Some(&b'\n') {
            // Torn tail: the log was cut mid-write. Not a parse warning.
            break;
        }
        line_no += 1;
        let line = match std::str::from_utf8(&buffer[..buffer.len() - 1]) {
            Ok(line) => line,
            Err(error) => {
                log::warn!(
                    "skipping non-UTF-8 DSH record at line {line_no} in '{}': {error}",
                    path.display()
                );
                state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if header.is_none() {
            match serde_json::from_str::<DshHeader>(line) {
                Ok(parsed) if parsed.kind == "session" => {
                    if parsed.version.is_some_and(|v| v != 0) {
                        log::warn!(
                            "DSH session '{}' has format version {:?}; expected 0 — parsing best-effort",
                            path.display(),
                            parsed.version
                        );
                        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                    }
                    header = Some(parsed);
                }
                Ok(parsed) => {
                    log::warn!(
                        "first DSH record of '{}' is not a session header (type '{}')",
                        path.display(),
                        parsed.kind
                    );
                    return None;
                }
                Err(error) => {
                    log::warn!(
                        "malformed DSH header at line {line_no} in '{}': {error}",
                        path.display()
                    );
                    return None;
                }
            }
            continue;
        }
        let record: Value = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(error) => {
                log::warn!(
                    "skipping malformed DSH record at line {line_no} in '{}': {error}",
                    path.display()
                );
                state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                continue;
            }
        };
        handle_record(&record, state);
    }
    header
}

fn handle_record(record: &Value, state: &mut ParseState) {
    let Some(event_type) = record.get("type").and_then(Value::as_str) else {
        log::warn!("skipping DSH record without a type tag");
        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
        return;
    };
    if let Some(time) = record.get("time").and_then(Value::as_i64) {
        state.last_event_time_ms = Some(time);
    }
    let timestamp = record
        .get("time")
        .and_then(Value::as_i64)
        .and_then(epoch_ms_to_rfc3339);
    let data = record.get("data").unwrap_or(&MISSING);
    // Compaction arrives as a surface event whose `surfaceOp` replaces the
    // surface nodes whose seqs are cited in `sourceEventSeqs`. The event's
    // own messages are dispatched normally, then the shadowed messages are
    // spliced out and the replacement's messages take their place.
    let replace_seqs: Option<Vec<u64>> = match record.get("surfaceOp") {
        Some(op) if op.get("op").and_then(Value::as_str) == Some("replace") => record
            .get("sourceEventSeqs")
            .and_then(Value::as_array)
            .map(|seqs| seqs.iter().filter_map(Value::as_u64).collect()),
        _ => None,
    };
    let produced_start = state.messages.len();
    let seq = record.get("seq").and_then(Value::as_u64);
    match event_type {
        "session/title" => {
            if let Some(title) = data.get("title").and_then(Value::as_str)
                && !title.trim().is_empty()
            {
                state.latest_title = Some(title.to_string());
            }
        }
        "user/message" => handle_user_message(data, state, timestamp, seq),
        "assistant/message" => handle_assistant_message(data, state, timestamp, seq),
        "tool/call" => handle_tool_call(data, state, timestamp, seq),
        "tool/result" => handle_tool_result(data, state, timestamp, seq),
        "assistant/chunk" | "text-chunks" | "reasoning-chunks" | "tool-call-chunks" => {
            // Packed chunk rows carry `seq0` instead of `seq`.
            let chunk_seq = seq.or_else(|| record.get("seq0").and_then(Value::as_u64));
            handle_chunk_event(event_type, data, state, chunk_seq, timestamp.clone());
        }
        // A delegated (subagent) session opens with its descriptor; the
        // `label` is the delegation name the parent chose — the only
        // human-meaningful title such a session ever gets.
        "subagent/descriptor" => {
            if let Some(label) = data
                .get("label")
                .and_then(Value::as_str)
                .filter(|label| !label.trim().is_empty())
            {
                state.descriptor_label = Some(label.to_string());
            }
        }
        "agent-preset/selected" => {
            if let Some(agent_preset) = data
                .get("agentPreset")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
            {
                state.agent_preset = Some(agent_preset.to_string());
            } else {
                log::warn!("skipping malformed DSH agent-preset/selected event");
                state.parse_warning_count = state.parse_warning_count.saturating_add(1);
            }
        }
        "step/end" => {
            if let (Some(turn), Some(step)) = (
                data.get("turn").and_then(Value::as_u64),
                data.get("step").and_then(Value::as_u64),
            ) {
                flush_step_chunks(state, turn as u32, step as u32, seq);
            }
        }
        // Known log-only event families: boundaries, request metadata, chunk
        // rows, compaction brackets, and informational plugin rows. None
        // carries surface semantics.
        "turn/start"
        | "turn/end"
        | "step/start"
        | "request/header"
        | "request/context"
        | "session/end-seed"
        | "todo/write"
        | "command/run"
        | "command/done"
        | "permission/preset"
        | "sandbox/mode"
        | "approval/policy"
        | "agent/inbox/spliced"
        | "session/title-llm-request"
        | "llm/retry"
        | "llm/retry-started"
        | "compaction/start"
        | "compaction/end"
        | "compaction/summary"
        | "approval/requested"
        | "approval/resolved"
        | "goal/change"
        | "plan/mode"
        | "question/requested"
        | "question/resolved"
        | "stream/error"
        | "tool/code-dispatch"
        | "tool/code-dispatch-start"
        | "session/created"
        | "session/event"
        | "web/deepseek-search-llm-request" => {}
        unknown => {
            let ignorable = record
                .get("ignorable")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if ignorable {
                log::debug!("skipping ignorable DSH event '{unknown}'");
            } else {
                // A required event we cannot interpret may change the meaning
                // of the surrounding log; surface it as a parse warning rather
                // than rendering a silently wrong transcript.
                log::warn!("skipping unknown required DSH event '{unknown}'");
                state.parse_warning_count = state.parse_warning_count.saturating_add(1);
            }
        }
    }
    if let Some(shadowed_seqs) = replace_seqs {
        apply_surface_replace(state, produced_start, &shadowed_seqs);
    }
}

/// Splice a compaction replacement into the transcript: the messages the
/// replacement event produced (indices `produced_start..`) take the place of
/// every message produced by the shadowed seqs.
fn apply_surface_replace(state: &mut ParseState, produced_start: usize, shadowed_seqs: &[u64]) {
    debug_assert_eq!(state.messages.len(), state.message_seq.len());
    let produced: Vec<Message> = state.messages.split_off(produced_start);
    let produced_seq: Vec<Option<u64>> = state.message_seq.split_off(produced_start);
    let mut shadowed: Vec<usize> = shadowed_seqs
        .iter()
        .filter_map(|seq| state.seq_to_messages.get(seq))
        .flatten()
        .copied()
        .collect();
    shadowed.sort_unstable();
    shadowed.dedup();
    if shadowed.is_empty() {
        // No shadowed messages matched (e.g. the cited seqs were log-only);
        // keep the replacement at the end rather than losing it.
        state.messages.extend(produced);
        state.message_seq.extend(produced_seq);
        return;
    }
    let insert_at = shadowed[0];
    let shadowed_set: HashSet<usize> = shadowed.iter().copied().collect();
    let mut kept = Vec::with_capacity(state.messages.len().saturating_sub(shadowed.len()));
    let mut kept_seq = Vec::with_capacity(state.message_seq.len().saturating_sub(shadowed.len()));
    for (index, (message, seq)) in state
        .messages
        .drain(..)
        .zip(state.message_seq.drain(..))
        .enumerate()
    {
        if !shadowed_set.contains(&index) {
            kept.push(message);
            kept_seq.push(seq);
        }
    }
    kept.splice(insert_at..insert_at, produced);
    kept_seq.splice(insert_at..insert_at, produced_seq);
    state.messages = kept;
    state.message_seq = kept_seq;
    rebuild_message_indexes(state);
}

/// Rebuild the derived index maps after a splice shifted message indices.
fn rebuild_message_indexes(state: &mut ParseState) {
    state.tool_by_call_id.clear();
    state.seq_to_messages.clear();
    for (index, message) in state.messages.iter().enumerate() {
        if message.role == MessageRole::Tool
            && let Some(metadata) = message.tool_metadata.as_ref()
            && let Some(call_id) = metadata.ids.get("tool_use_id")
        {
            state.tool_by_call_id.insert(call_id.clone(), index);
        }
    }
    for (index, seq) in state.message_seq.iter().enumerate() {
        if let Some(seq) = seq {
            state.seq_to_messages.entry(*seq).or_default().push(index);
        }
    }
}

/// Extract plain text from a message `content` payload, following the Claude
/// provider's convention of trailing `[Image]` markers for image blocks. The
/// payload is normally a `ContentBlock[]` array; a bare string (older or
/// foreign producers) is used verbatim.
fn extract_block_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let mut parts = Vec::new();
    let mut image_count = 0usize;
    if let Some(blocks) = content.as_array() {
        for block in blocks {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                    }
                }
                "image" => image_count += 1,
                other => {
                    log::debug!("skipping DSH content block '{other}'");
                }
            }
        }
    }
    for _ in 0..image_count {
        parts.push("[Image]".to_string());
    }
    parts.join("\n")
}

fn handle_user_message(
    data: &Value,
    state: &mut ParseState,
    timestamp: Option<String>,
    seq: Option<u64>,
) {
    let source_kind = data
        .pointer("/source/kind")
        .and_then(Value::as_str)
        .unwrap_or("");
    let form = data.pointer("/source/form").and_then(Value::as_str);
    let text = data
        .get("content")
        .map(extract_block_text)
        .unwrap_or_default();
    if text.trim().is_empty() {
        return;
    }
    match source_kind {
        // Direct human prompts are the user's own words.
        "user" => state.push_user(text, timestamp, seq),
        // System-injected context dumps — workspace instructions, runtime
        // snapshots, skill/tool catalogs, relayed or recalled context — are
        // not conversation; the DSH UI collapses them, so do we.
        "agent-instructions" => {
            log::debug!("skipping DSH agent-instructions user message");
        }
        "plugin"
            if matches!(
                form,
                Some("snapshot")
                    | Some("instructions")
                    | Some("catalog")
                    | Some("relay")
                    | Some("recall")
            ) =>
        {
            log::debug!("skipping DSH plugin {form:?} user message");
        }
        // A background subagent's relayed report. The first block is DSH's
        // boilerplate ("Background subagent <id> reported:") — drop it and
        // render the report body as a tagged, collapsible system row.
        "subagent-report" => {
            let body = match text.split_once('\n') {
                Some((first, rest)) if first.starts_with("Background subagent") => rest.trim(),
                _ => text.trim(),
            };
            if !body.is_empty() {
                state.push_with_provenance(
                    Message {
                        timestamp,
                        ..Message::system(format!("[subagent_report] {body}"))
                    },
                    seq,
                );
            }
        }
        // The settle notice: boilerplate ("Background subagent <id>
        // finished…" + "Its closing message:") wraps the child's closing
        // report — strip the wrapper, tag the report.
        "subagent-settled" => {
            let mut body = text.as_str();
            if let Some((first, rest)) = body.split_once('\n')
                && first.starts_with("Background subagent")
            {
                body = rest;
            }
            let body = body.trim_start();
            let body = body
                .strip_prefix("Its closing message:")
                .unwrap_or(body)
                .trim();
            if !body.is_empty() {
                state.push_with_provenance(
                    Message {
                        timestamp,
                        ..Message::system(format!("[subagent_settled] {body}"))
                    },
                    seq,
                );
            }
        }
        // Everything else that reaches the surface (policy changes, goal
        // rounds, notices, compaction checkpoints, …) renders as a system
        // line.
        _ => {
            state.push_with_provenance(
                Message {
                    timestamp,
                    ..Message::system(text)
                },
                seq,
            );
        }
    }
}

/// Parse a raw arguments JSON string into a `Value` for tool metadata, or
/// `None` when the model produced malformed JSON.
fn parse_tool_arguments(raw: &str) -> Option<Value> {
    serde_json::from_str::<Value>(raw).ok()
}

fn push_tool_message(
    state: &mut ParseState,
    raw_name: &str,
    arguments_raw: &str,
    call_id: Option<&str>,
    timestamp: Option<String>,
    provenance: Option<u64>,
) {
    let metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Dsh,
        raw_name,
        input: parse_tool_arguments(arguments_raw).as_ref(),
        call_id,
        assistant_id: None,
    });
    let canonical_name = metadata.canonical_name.clone();
    let idx = state.messages.len();
    state.push_with_provenance(
        Message {
            timestamp,
            tool_name: Some(canonical_name),
            tool_input: Some(arguments_raw.to_string()),
            tool_metadata: Some(metadata),
            ..Message::new(MessageRole::Tool, String::new())
        },
        provenance,
    );
    if let Some(call_id) = call_id {
        state.tool_by_call_id.insert(call_id.to_string(), idx);
    }
}

fn handle_assistant_message(
    data: &Value,
    state: &mut ParseState,
    timestamp: Option<String>,
    seq: Option<u64>,
) {
    let Some(message) = data.get("message") else {
        return;
    };
    let model = message
        .pointer("/source/model")
        .and_then(Value::as_str)
        .map(str::to_string);
    if state.model.is_none() {
        state.model = model.clone();
    }
    let usage = data
        .get("usage")
        .and_then(|usage| token_usage_from(usage, &DSH_USAGE_KEYS));
    let turn_start = state.messages.len();
    let mut text_parts: Vec<String> = Vec::new();
    let mut image_count = 0usize;
    if let Some(blocks) = message.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "reasoning" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str)
                        && !text.trim().is_empty()
                    {
                        state.push_with_provenance(
                            Message {
                                timestamp: timestamp.clone(),
                                ..Message::system(format!("[thinking]\n{text}"))
                            },
                            seq,
                        );
                    }
                }
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str)
                        && !text.trim().is_empty()
                    {
                        text_parts.push(text.to_string());
                    }
                }
                "tool-call" => {
                    if !text_parts.is_empty() {
                        let text = text_parts.join("\n");
                        state.push_assistant(text, model.clone(), timestamp.clone(), seq);
                        text_parts.clear();
                    }
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let call_id = block.get("id").and_then(Value::as_str);
                    // A tool/call event may already have surfaced this call
                    // (out-of-order log); don't duplicate the Tool message.
                    if call_id.is_some_and(|id| state.tool_by_call_id.contains_key(id)) {
                        continue;
                    }
                    let arguments_raw =
                        block.get("arguments").and_then(Value::as_str).unwrap_or("");
                    push_tool_message(state, name, arguments_raw, call_id, timestamp.clone(), seq);
                }
                "image" => image_count += 1,
                other => {
                    log::warn!("skipping unknown DSH assistant content block '{other}'");
                    state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                }
            }
        }
    }
    for _ in 0..image_count {
        text_parts.push("[Image]".to_string());
    }
    if !text_parts.is_empty() {
        state.push_assistant(text_parts.join("\n"), model.clone(), timestamp.clone(), seq);
    }
    if let Some(usage) = usage {
        // The usage also lands in `usage_events` (copied before the move
        // below), which survives compaction splices and matches how DSH's
        // own usage collector folds the log. An event without its own model
        // falls back to the session-level model; only when neither exists
        // (or the record has no timestamp to bucket by) is the row skipped,
        // and then as a counted parse warning, never silently.
        let usage_model = model.clone().or_else(|| state.model.clone());
        match (timestamp.as_deref(), usage_model) {
            (Some(timestamp), Some(model)) => state.usage_events.push(UsageEvent {
                timestamp: timestamp.to_string(),
                model,
                turn_count: 1,
                input_tokens: u64::from(usage.input_tokens),
                output_tokens: u64::from(usage.output_tokens),
                cache_read_input_tokens: u64::from(usage.cache_read_input_tokens),
                cache_creation_input_tokens: u64::from(usage.cache_creation_input_tokens),
                usage_hash: None,
                cost_usd: None,
            }),
            _ => {
                log::warn!("skipping DSH usage event without a timestamp or model");
                state.parse_warning_count = state.parse_warning_count.saturating_add(1);
            }
        }
        // Attach usage to the event's last non-System message so accounting
        // is never silently dropped; a thinking-only step gets a placeholder.
        if let Some(last) = state.messages[turn_start..]
            .iter_mut()
            .filter(|message| message.role != MessageRole::System)
            .last()
        {
            last.token_usage = Some(usage);
            if last.model.as_deref().map(str::is_empty).unwrap_or(true) {
                last.model = model.clone();
            }
            if last.timestamp.is_none() {
                last.timestamp = timestamp.clone();
            }
        } else {
            state.push_with_provenance(
                Message {
                    timestamp: timestamp.clone(),
                    token_usage: Some(usage),
                    model: model.clone(),
                    ..Message::assistant(String::new())
                },
                seq,
            );
        }
    }
    // The assembled message supersedes the step's raw chunks.
    if let (Some(turn), Some(step)) = (
        data.get("turn").and_then(Value::as_u64),
        data.get("step").and_then(Value::as_u64),
    ) {
        let key = (turn as u32, step as u32);
        state.chunk_bufs.remove(&key);
        state.assembled_steps.insert(key);
    }
}

fn handle_tool_call(
    data: &Value,
    state: &mut ParseState,
    timestamp: Option<String>,
    seq: Option<u64>,
) {
    let Some(call_id) = data.get("callId").and_then(Value::as_str) else {
        return;
    };
    if state.tool_by_call_id.contains_key(call_id) {
        // Already surfaced through the step's assistant/message blocks.
        return;
    }
    let name = data.get("name").and_then(Value::as_str).unwrap_or("tool");
    let arguments_raw = data.get("arguments").and_then(Value::as_str).unwrap_or("");
    push_tool_message(state, name, arguments_raw, Some(call_id), timestamp, seq);
}

fn handle_tool_result(
    data: &Value,
    state: &mut ParseState,
    timestamp: Option<String>,
    seq: Option<u64>,
) {
    let Some(message) = data.get("message") else {
        return;
    };
    let call_id = message
        .pointer("/source/callId")
        .and_then(Value::as_str)
        .or_else(|| {
            message
                .pointer("/content/0/toolCallId")
                .and_then(Value::as_str)
        });
    let block = message
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| blocks.first());
    let is_error = block
        .and_then(|block| block.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let result_text = block
        .and_then(|block| block.get("content"))
        .map(extract_block_text)
        .unwrap_or_default();
    let result_facts = ToolResultFacts {
        is_error: Some(is_error),
        ..ToolResultFacts::default()
    };
    if let Some(call_id) = call_id
        && let Some(&idx) = state.tool_by_call_id.get(call_id)
    {
        if !result_text.is_empty() {
            state.messages[idx].content = result_text;
        }
        if let Some(metadata) = state.messages[idx].tool_metadata.as_mut() {
            enrich_tool_metadata(metadata, result_facts);
        }
        return;
    }
    // Orphan result: no surfaced call to attach to (e.g. the model call was
    // interrupted before its message assembled). Standalone Tool message.
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Dsh,
        raw_name: "tool",
        input: None,
        call_id,
        assistant_id: None,
    });
    enrich_tool_metadata(&mut metadata, result_facts);
    let canonical_name = metadata.canonical_name.clone();
    state.push_with_provenance(
        Message {
            timestamp,
            tool_name: Some(canonical_name),
            tool_metadata: Some(metadata),
            ..Message::new(MessageRole::Tool, result_text)
        },
        seq,
    );
}

fn handle_chunk_event(
    event_type: &str,
    data: &Value,
    state: &mut ParseState,
    seq: Option<u64>,
    timestamp: Option<String>,
) {
    let (Some(turn), Some(step)) = (
        data.get("turn").and_then(Value::as_u64),
        data.get("step").and_then(Value::as_u64),
    ) else {
        return;
    };
    let key = (turn as u32, step as u32);
    if state.assembled_steps.contains(&key) {
        // The step's assistant/message already assembled; its chunks were
        // superseded, so an out-of-order chunk row must not re-buffer text.
        return;
    }
    let buf = state.chunk_bufs.entry(key).or_default();
    if seq.is_some() {
        buf.last_seq = seq;
    }
    match event_type {
        "assistant/chunk" => {
            let Some(chunk) = data.get("chunk") else {
                return;
            };
            // Usage chunks carry no block index — capture them before the
            // index gate, so an interrupted step keeps its token accounting.
            if chunk.get("type").and_then(Value::as_str) == Some("usage") {
                if let Some(usage) = chunk
                    .get("usage")
                    .and_then(|usage| token_usage_from(usage, &DSH_USAGE_KEYS))
                {
                    buf.usage = Some(usage);
                    if timestamp.is_some() {
                        buf.usage_timestamp = timestamp;
                    }
                }
                return;
            }
            let Some(index) = chunk.get("index").and_then(Value::as_u64) else {
                return;
            };
            let index = index as usize;
            match chunk.get("type").and_then(Value::as_str).unwrap_or("") {
                "text-delta" => {
                    if let Some(text) = chunk.get("text").and_then(Value::as_str) {
                        buf.text.entry(index).or_default().push_str(text);
                    }
                }
                "reasoning-delta" => {
                    if let Some(text) = chunk.get("text").and_then(Value::as_str) {
                        buf.reasoning.entry(index).or_default().push_str(text);
                    }
                }
                "tool-call-delta" => {
                    let id = chunk
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = chunk
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let arguments_delta = chunk
                        .get("argumentsDelta")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let entry = buf
                        .tool_calls
                        .entry(index)
                        .or_insert_with(|| (id.clone(), name.clone(), String::new()));
                    entry.0 = id;
                    if name.is_some() {
                        entry.1 = name;
                    }
                    entry.2.push_str(arguments_delta);
                }
                _ => {}
            }
        }
        "text-chunks" | "reasoning-chunks" => {
            let Some(index) = data.get("index").and_then(Value::as_u64) else {
                return;
            };
            let joined: String = data
                .get("texts")
                .and_then(Value::as_array)
                .map(|texts| texts.iter().filter_map(Value::as_str).collect::<String>())
                .unwrap_or_default();
            if joined.is_empty() {
                return;
            }
            let index = index as usize;
            let target = if event_type == "text-chunks" {
                &mut buf.text
            } else {
                &mut buf.reasoning
            };
            target.entry(index).or_default().push_str(&joined);
        }
        "tool-call-chunks" => {
            let Some(index) = data.get("index").and_then(Value::as_u64) else {
                return;
            };
            let id = data
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = data.get("name").and_then(Value::as_str).map(str::to_string);
            let args: String = data
                .get("args")
                .and_then(Value::as_array)
                .map(|args| args.iter().filter_map(Value::as_str).collect::<String>())
                .unwrap_or_default();
            let index = index as usize;
            let entry = buf
                .tool_calls
                .entry(index)
                .or_insert_with(|| (id.clone(), name.clone(), String::new()));
            entry.0 = id;
            if name.is_some() {
                entry.1 = name;
            }
            entry.2.push_str(&args);
        }
        _ => {}
    }
}

/// Flush a step's buffered chunk deltas as transcript messages. Only called
/// for steps whose `assistant/message` never assembled (interrupted stream);
/// the normal path removes the buffer when the message arrives. Flushed
/// messages carry `provenance` (the `step/end` seq, or the buffer's last
/// chunk-row seq at EOF) so a compaction splice can remove them when it
/// shadows the step.
fn flush_step_chunks(state: &mut ParseState, turn: u32, step: u32, provenance: Option<u64>) {
    let Some(buf) = state.chunk_bufs.remove(&(turn, step)) else {
        return;
    };
    let provenance = provenance.or(buf.last_seq);
    let produced_start = state.messages.len();
    let mut indices: Vec<usize> = buf
        .text
        .keys()
        .chain(buf.reasoning.keys())
        .chain(buf.tool_calls.keys())
        .copied()
        .collect();
    indices.sort_unstable();
    indices.dedup();
    for index in indices {
        if let Some(text) = buf
            .reasoning
            .get(&index)
            .filter(|text| !text.trim().is_empty())
        {
            state.push_with_provenance(Message::system(format!("[thinking]\n{text}")), provenance);
        }
        if let Some(text) = buf.text.get(&index).filter(|text| !text.trim().is_empty()) {
            state.content_parts.push(text.clone());
            state.push_with_provenance(Message::assistant(text.clone()), provenance);
        }
        if let Some((call_id, name, arguments)) = buf.tool_calls.get(&index)
            && !state.tool_by_call_id.contains_key(call_id)
        {
            let name = name.as_deref().unwrap_or("tool");
            push_tool_message(state, name, arguments, Some(call_id), None, provenance);
        }
    }
    // Mirror the assembled-message path: the step's streamed usage lands in
    // `usage_events` and on the flush's last non-System message, so an
    // interrupted step still counts toward the session's tokens.
    if let Some(usage) = buf.usage {
        match (buf.usage_timestamp.as_deref(), state.model.clone()) {
            (Some(timestamp), Some(model)) => state.usage_events.push(UsageEvent {
                timestamp: timestamp.to_string(),
                model,
                turn_count: 1,
                input_tokens: u64::from(usage.input_tokens),
                output_tokens: u64::from(usage.output_tokens),
                cache_read_input_tokens: u64::from(usage.cache_read_input_tokens),
                cache_creation_input_tokens: u64::from(usage.cache_creation_input_tokens),
                usage_hash: None,
                cost_usd: None,
            }),
            _ => {
                log::warn!("skipping DSH usage chunk without a timestamp or model");
                state.parse_warning_count = state.parse_warning_count.saturating_add(1);
            }
        }
        if let Some(last) = state.messages[produced_start..]
            .iter_mut()
            .filter(|message| message.role != MessageRole::System)
            .last()
        {
            last.token_usage = Some(usage);
        } else {
            state.push_with_provenance(
                Message {
                    token_usage: Some(usage),
                    ..Message::assistant(String::new())
                },
                provenance,
            );
        }
    }
}

/// Parse one DSH session artifact into a [`ParsedSession`]. Returns `None`
/// for files that cannot be opened/decompressed, lack a session header, or
/// contain no surfaced messages (a session that never started).
pub fn parse_session_file(path: &Path) -> Option<ParsedSession> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!("failed to open DSH session '{}': {error}", path.display());
            return None;
        }
    };
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            log::warn!(
                "failed to read DSH session metadata '{}': {error}",
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
    let reader = match open_log_reader(path, file) {
        Ok(reader) => reader,
        Err(error) => {
            log::warn!("failed to open DSH session '{}': {error}", path.display());
            return None;
        }
    };
    let mut state = ParseState::default();
    let header = scan_records(reader, path, &mut state)?;
    // Torn logs may lack the closing step/end; flush whatever is left.
    let mut pending_steps: Vec<(u32, u32)> = state.chunk_bufs.keys().copied().collect();
    // Deterministic order: flush in (turn, step) sequence, not HashMap order.
    pending_steps.sort_unstable();
    for (turn, step) in pending_steps {
        flush_step_chunks(&mut state, turn, step, None);
    }
    if state.messages.is_empty() {
        log::debug!(
            "skipping DSH session '{}': no surfaced messages",
            path.display()
        );
        return None;
    }
    let content_text = state.content_parts.join("\n");
    let meta = assemble_session_meta(path, &header, &state, file_size, source_mtime);
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

fn assemble_session_meta(
    path: &Path,
    header: &DshHeader,
    state: &ParseState,
    file_size: u64,
    source_mtime: i64,
) -> SessionMeta {
    let id = header.id.clone().unwrap_or_else(|| {
        path.parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
    });
    let project_path = header.cwd.clone().unwrap_or_default();
    let project_name = Path::new(&project_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let created_at = header.created_at.unwrap_or(0) / 1000;
    let updated_at = state
        .last_event_time_ms
        .map(|time| time / 1000)
        .unwrap_or(source_mtime);
    let parent_id = header.parent_session.clone().filter(|id| !id.is_empty());
    // `parentSession` alone is seed lineage — a forked/resumed branch carries
    // it too. Only `origin: "subagent"` marks a delegated child.
    let is_sidechain = header.origin.as_deref() == Some("subagent");
    // A delegated session's `session/title` is only the deterministic
    // fallback (auto-titling skips subagents), so the parent-chosen
    // delegation label wins when present. `first_user_text` is set by
    // `push_user` for every surfaced direct prompt, so it is exactly the
    // first User message's text; no scan needed.
    let title = if is_sidechain {
        state.descriptor_label.clone()
    } else {
        None
    }
    .or_else(|| state.latest_title.clone())
    .unwrap_or_else(|| session_title(state.first_user_text.as_deref()));
    SessionMeta {
        id,
        provider: Provider::Dsh,
        title,
        project_path,
        project_name,
        created_at,
        updated_at,
        message_count: state.messages.len() as u32,
        file_size_bytes: file_size,
        source_path: path.to_string_lossy().to_string(),
        is_sidechain,
        variant_name: state
            .agent_preset
            .clone()
            .or_else(|| header.agent_preset.clone()),
        model: state.model.clone(),
        cc_version: None,
        git_branch: None,
        parent_id,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MessageRole;
    use crate::provider::SessionProvider;
    use crate::providers::dsh::DshProvider;
    use tempfile::TempDir;

    const SESSION_ID: &str = "session-11111111-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    fn header_line(cwd: &str) -> String {
        format!(
            r#"{{"type":"session","version":0,"id":"{SESSION_ID}","createdAt":1786865077879,"cwd":"{cwd}","delegationDepth":0,"agentPreset":"standard"}}"#
        )
    }

    fn event_line(event_type: &str, seq: u64, time: i64, data: &str) -> String {
        format!(r#"{{"type":"{event_type}","seq":{seq},"time":{time},"data":{data}}}"#)
    }

    fn user_message_line(seq: u64, time: i64, source: &str, text: &str) -> String {
        event_line(
            "user/message",
            seq,
            time,
            &format!(
                r#"{{"content":[{{"type":"text","text":{text}}}],"source":{source},"role":"user","id":"u{seq}"}}"#
            ),
        )
    }

    fn assistant_message_line(seq: u64, time: i64, content: &str, usage: Option<&str>) -> String {
        let usage_json = usage.map_or(String::new(), |u| format!(r#","usage":{u}"#));
        event_line(
            "assistant/message",
            seq,
            time,
            &format!(
                r#"{{"turn":1,"step":1,"message":{{"role":"assistant","content":{content},"source":{{"kind":"model","provider":"opencode-go","model":"deepseek-v4-flash"}},"id":"a{seq}"}}{usage_json}}}"#
            ),
        )
    }

    fn tool_result_line(seq: u64, time: i64, call_id: &str, text: &str, is_error: bool) -> String {
        event_line(
            "tool/result",
            seq,
            time,
            &format!(
                r#"{{"turn":1,"step":1,"message":{{"source":{{"kind":"tool","callId":"{call_id}"}},"content":[{{"type":"tool-result","toolCallId":"{call_id}","content":[{{"type":"text","text":{text}}}],"isError":{is_error}}}],"role":"user","id":"r{seq}"}}}}"#
            ),
        )
    }

    /// A compaction checkpoint: a plugin `user/message` whose
    /// `surfaceOp: replace` shadows every surface event whose seq appears in
    /// `sourceEventSeqs`. `text` is a JSON string literal (quotes included).
    fn checkpoint_line(seq: u64, time: i64, text: &str, shadowed_seqs: &[u64]) -> String {
        let seqs = shadowed_seqs
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"type":"user/message","seq":{seq},"time":{time},"surfaceOp":{{"op":"replace","start":1,"end":3}},"sourceEventSeqs":[{seqs}],"data":{{"content":[{{"type":"text","text":{text}}}],"source":{{"kind":"plugin","plugin":"compact","compactionId":"c-{seq}","sourceCommandId":"cmd-{seq}"}},"role":"user","id":"cp-{seq}"}}}}"#
        )
    }

    fn write_log(dir: &TempDir, name: &str, lines: &[&str]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, lines.join("\n") + "\n").expect("fixture must be written");
        path
    }

    fn parse_lines(lines: &[&str]) -> ParsedSession {
        let dir = TempDir::new().expect("temp dir must be created");
        let path = write_log(&dir, "session.jsonl", lines);
        parse_session_file(&path).expect("fixture must parse")
    }

    /// A subagent header: `parentSession` + `origin` mark a delegated child.
    fn subagent_header_line(cwd: &str) -> String {
        format!(
            r#"{{"type":"session","version":0,"id":"{SESSION_ID}","createdAt":1786865077879,"cwd":"{cwd}","parentSession":"session-parent","origin":"subagent","delegationDepth":1,"agentPreset":"standard"}}"#
        )
    }

    #[test]
    fn subagent_title_prefers_descriptor_label() {
        let session = parse_lines(&[
            &subagent_header_line("/home/user/my-project"),
            &event_line(
                "subagent/descriptor",
                0,
                1786865077879,
                r#"{"version":2,"mode":"continuable","provider":"spawn","label":"Deep review all project code","agentProvider":"opencode-go","agentModel":"deepseek-v4-flash"}"#,
            ),
            &user_message_line(
                1,
                1786865077880,
                r#"{"kind":"user"}"#,
                r#""You are conducting a deep code review of…""#,
            ),
            &event_line(
                "session/title",
                2,
                1786865077881,
                r#"{"title":"You are conducting a deep","messageSeqs":[1],"source":{"kind":"fallback"}}"#,
            ),
        ]);
        assert!(session.meta.is_sidechain);
        assert_eq!(session.meta.parent_id.as_deref(), Some("session-parent"));
        assert_eq!(session.meta.title, "Deep review all project code");
    }

    #[test]
    fn subagent_report_renders_as_tagged_system_row() {
        let session = parse_lines(&[
            &header_line("/home/user/my-project"),
            &user_message_line(1, 1786865077880, r#"{"kind":"user"}"#, r#""hello""#),
            &event_line(
                "user/message",
                2,
                1786865077881,
                r#"{"content":[{"type":"text","text":"Background subagent cb61a6b7 reported:"},{"type":"text","text":"Review complete."}],"source":{"kind":"subagent-report","form":"relay","senderSessionId":"cb61a6b7"},"role":"user","id":"u2"}"#,
            ),
        ]);
        let report = session
            .messages
            .iter()
            .find(|message| message.content.starts_with("[subagent_report]"))
            .expect("report row");
        assert_eq!(report.role, MessageRole::System);
        assert_eq!(report.content, "[subagent_report] Review complete.");
    }

    #[test]
    fn subagent_settled_strips_wrapper_and_tags_closing_message() {
        let session = parse_lines(&[
            &header_line("/home/user/my-project"),
            &user_message_line(1, 1786865077880, r#"{"kind":"user"}"#, r#""hello""#),
            &event_line(
                "user/message",
                2,
                1786865077881,
                r#"{"content":[{"type":"text","text":"Background subagent 036bdb9c finished and will do no further work unless you send it more."},{"type":"text","text":"Its closing message:"},{"type":"text","text":"Review complete.\nAll good."}],"source":{"kind":"subagent-settled","form":"notice","senderSessionId":"036bdb9c"},"role":"user","id":"u2"}"#,
            ),
        ]);
        let settled = session
            .messages
            .iter()
            .find(|message| message.content.starts_with("[subagent_settled]"))
            .expect("settled row");
        assert_eq!(settled.role, MessageRole::System);
        assert_eq!(
            settled.content,
            "[subagent_settled] Review complete.\nAll good."
        );
        assert!(!settled.content.contains("Background subagent"));
    }

    #[test]
    fn forked_session_is_not_a_sidechain() {
        // `parentSession` without `origin: subagent` is seed lineage (a
        // fork/resume branch), not a delegated child.
        let session = parse_lines(&[
            &format!(
                r#"{{"type":"session","version":0,"id":"{SESSION_ID}","createdAt":1786865077879,"cwd":"/home/user/my-project","parentSession":"session-parent","seedLength":4}}"#
            ),
            &user_message_line(1, 1786865077880, r#"{"kind":"user"}"#, r#""hello""#),
        ]);
        assert!(!session.meta.is_sidechain);
        assert_eq!(session.meta.parent_id.as_deref(), Some("session-parent"));
    }

    #[test]
    fn interrupted_step_keeps_streamed_usage() {
        // A step whose assistant/message never assembled: text arrives as
        // chunks, usage as a usage chunk, then the stream dies. The flush
        // must keep both the text and the token accounting.
        let session = parse_lines(&[
            &header_line("/home/user/my-project"),
            &user_message_line(1, 1786865077880, r#"{"kind":"user"}"#, r#""hello""#),
            &assistant_message_line(
                2,
                1786865077881,
                r#"[{"type":"text","text":"first"}]"#,
                Some(r#"{"inputTokens":7,"outputTokens":3}"#),
            ),
            &event_line(
                "assistant/chunk",
                3,
                1786865077890,
                r#"{"turn":1,"step":2,"chunk":{"type":"text-delta","index":0,"text":"partial answer"}}"#,
            ),
            &event_line(
                "assistant/chunk",
                4,
                1786865077891,
                r#"{"turn":1,"step":2,"chunk":{"type":"usage","usage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":40}}}"#,
            ),
        ]);
        assert_eq!(session.usage_events.len(), 2);
        let orphan = &session.usage_events[1];
        assert_eq!(orphan.input_tokens, 100);
        assert_eq!(orphan.output_tokens, 20);
        assert_eq!(orphan.cache_read_input_tokens, 40);
        assert_eq!(orphan.model, "deepseek-v4-flash");
        let last = session.messages.last().expect("flushed message");
        let usage = last.token_usage.as_ref().expect("usage attached");
        assert_eq!(usage.input_tokens, 100);
    }

    /// Full fixture: user prompt, reasoning + text + tool call, tool result,
    /// step end, and a session title event.
    fn full_roundtrip_session() -> ParsedSession {
        let dir = TempDir::new().unwrap();
        let path = write_log(
            &dir,
            "session.jsonl",
            &[
                &header_line("/home/user/my-project"),
                &user_message_line(
                    1,
                    1786865127218,
                    r#"{"kind":"user","rpcId":"rpc-1"}"#,
                    r#""hello dsh""#,
                ),
                &assistant_message_line(
                    2,
                    1786865132303,
                    r#"[{"type":"reasoning","text":"let me think"},{"type":"text","text":"hi there"},{"type":"tool-call","id":"call_1","name":"bash","arguments":"{\"command\":\"ls\"}"}]"#,
                    Some(
                        r#"{"inputTokens":100,"outputTokens":25,"cacheReadTokens":50,"cacheWriteTokens":10}"#,
                    ),
                ),
                &tool_result_line(3, 1786865132305, "call_1", r#""file.txt""#, false),
                &event_line("step/end", 4, 1786865133000, r#"{"turn":1,"step":1}"#),
                &event_line(
                    "session/title",
                    5,
                    1786865134000,
                    r#"{"title":"A Great Title","messageSeqs":[1],"source":{"kind":"fallback"}}"#,
                ),
            ],
        );
        parse_session_file(&path).expect("fixture must parse")
    }

    #[test]
    fn parses_full_session_meta() {
        let session = full_roundtrip_session();
        assert_eq!(session.meta.id, SESSION_ID);
        assert_eq!(session.meta.title, "A Great Title");
        assert_eq!(session.meta.project_path, "/home/user/my-project");
        assert_eq!(session.meta.project_name, "my-project");
        assert_eq!(session.meta.created_at, 1786865077);
        assert_eq!(session.meta.updated_at, 1786865134);
        assert_eq!(session.meta.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(session.meta.parent_id, None);
        assert!(!session.meta.is_sidechain);
        assert_eq!(session.meta.message_count, 4);
        assert_eq!(session.parse_warning_count, 0);
        assert!(session.content_text.contains("hello dsh"));
        assert!(session.content_text.contains("hi there"));
        assert!(!session.content_text.contains("let me think"));
    }

    #[test]
    fn parses_full_session_messages() {
        let session = full_roundtrip_session();
        let messages = &session.messages;
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[0].content, "hello dsh");
        assert_eq!(messages[1].role, MessageRole::System);
        assert_eq!(messages[1].content, "[thinking]\nlet me think");
        assert_eq!(messages[2].role, MessageRole::Assistant);
        assert_eq!(messages[2].content, "hi there");
        assert_eq!(messages[2].model.as_deref(), Some("deepseek-v4-flash"));
        assert!(messages[2].token_usage.is_none());
        assert_eq!(messages[3].role, MessageRole::Tool);
        assert_eq!(messages[3].tool_name.as_deref(), Some("Bash"));
        // Usage attaches to the event's last non-System message (the tool
        // message), mirroring the Claude provider's convention.
        let usage = messages[3].token_usage.as_ref().expect("usage attached");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 25);
        assert_eq!(usage.cache_read_input_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, 10);
        assert_eq!(messages[3].content, "file.txt");
        let metadata = messages[3].tool_metadata.as_ref().unwrap();
        assert_eq!(metadata.status.as_deref(), Some("success"));
    }

    #[test]
    fn title_falls_back_to_first_user_message() {
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &user_message_line(1, 1, r#"{"kind":"user"}"#, r#""hello dsh""#),
        ]);
        assert_eq!(session.meta.title, "hello dsh");
    }

    #[test]
    fn maps_tool_result_error_status() {
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &assistant_message_line(
                1,
                1786865132303,
                r#"[{"type":"tool-call","id":"call_9","name":"glob","arguments":"{}"}]"#,
                None,
            ),
            &tool_result_line(2, 1786865132305, "call_9", r#""boom""#, true),
        ]);
        let tool = session.messages.last().unwrap();
        assert_eq!(tool.role, MessageRole::Tool);
        assert_eq!(tool.content, "boom");
        assert_eq!(
            tool.tool_metadata.as_ref().unwrap().status.as_deref(),
            Some("error")
        );
    }

    #[test]
    fn drops_agent_instructions_and_snapshot_noise() {
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &user_message_line(1, 1, r#"{"kind":"user"}"#, r#""real prompt""#),
            &user_message_line(
                2,
                2,
                r#"{"kind":"agent-instructions","form":"instructions"}"#,
                r#""<system-reminder>AGENTS.md</system-reminder>""#,
            ),
            &user_message_line(
                3,
                3,
                r#"{"kind":"plugin","plugin":"@deepseek-ai/dsh-system-prompt","form":"snapshot"}"#,
                r#""runtime snapshot""#,
            ),
            &user_message_line(
                4,
                4,
                r#"{"kind":"plugin","plugin":"dsh-tool-skill","form":"catalog"}"#,
                r#""skill catalog dump""#,
            ),
            &user_message_line(
                5,
                5,
                r#"{"kind":"plugin","plugin":"dsh-session-reference","form":"recall"}"#,
                r#""recalled context from another session""#,
            ),
            &user_message_line(
                6,
                6,
                r#"{"kind":"plugin","plugin":"user-approval"}"#,
                r#""The approval policy changed from ask to never""#,
            ),
        ]);
        let messages = &session.messages;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[0].content, "real prompt");
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("approval policy"));
    }

    #[test]
    fn plain_string_content_is_kept() {
        // Older/foreign producers may emit `content` as a bare string.
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &event_line(
                "user/message",
                1,
                1,
                r#"{"content":"plain string prompt","source":{"kind":"user"},"role":"user","id":"u1"}"#,
            ),
        ]);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, MessageRole::User);
        assert_eq!(session.messages[0].content, "plain string prompt");
    }

    #[test]
    fn interrupts_do_not_lose_streamed_text() {
        // A step whose assistant/message never assembled: chunks only.
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &event_line(
                "assistant/chunk",
                1,
                1000,
                r#"{"turn":1,"step":1,"chunk":{"type":"block-start","index":0,"blockType":"reasoning"}}"#,
            ),
            &event_line(
                "reasoning-chunks",
                2,
                1000,
                r#"{"turn":1,"step":1,"index":0,"dt":[1],"texts":["why not"]}"#,
            ),
            &event_line(
                "text-chunks",
                3,
                1000,
                r#"{"turn":1,"step":1,"index":1,"dt":[1],"texts":["partial ","answer"]}"#,
            ),
            &event_line(
                "tool-call-chunks",
                4,
                1000,
                r#"{"turn":1,"step":1,"index":2,"dt":[1],"id":"call_7","name":"bash","args":["{","}"]}"#,
            ),
            &event_line("step/end", 5, 2000, r#"{"turn":1,"step":1}"#),
            &tool_result_line(6, 2001, "call_7", r#""out""#, false),
        ]);
        let messages = &session.messages;
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[0].content, "[thinking]\nwhy not");
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[1].content, "partial answer");
        assert_eq!(messages[2].role, MessageRole::Tool);
        assert_eq!(messages[2].content, "out");
        assert_eq!(messages[2].tool_name.as_deref(), Some("Bash"));
    }

    #[test]
    fn late_chunk_rows_do_not_duplicate_assembled_text() {
        // The step's assistant/message arrived first, then an out-of-order
        // chunk row follows; the chunk must not re-buffer (and later flush)
        // a duplicate of the assembled text.
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &assistant_message_line(
                1,
                1000,
                r#"[{"type":"text","text":"assembled answer"}]"#,
                None,
            ),
            &event_line(
                "text-chunks",
                2,
                1001,
                r#"{"turn":1,"step":1,"index":0,"dt":[1],"texts":["assembled answer"]}"#,
            ),
            &event_line("step/end", 3, 2000, r#"{"turn":1,"step":1}"#),
        ]);
        let messages = &session.messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "assembled answer");
    }

    #[test]
    fn compaction_replace_shadows_old_surface_and_keeps_usage() {
        // Real compaction shape: bracket rows, then a user/message carrying
        // `surfaceOp: {op: "replace"}` whose sourceEventSeqs cite every
        // shadowed surface event.
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &user_message_line(1, 1000, r#"{"kind":"user"}"#, r#""old prompt""#),
            &assistant_message_line(
                2,
                1001,
                r#"[{"type":"text","text":"old answer"},{"type":"tool-call","id":"call_1","name":"bash","arguments":"{}"}]"#,
                Some(r#"{"inputTokens":300,"outputTokens":50}"#),
            ),
            &tool_result_line(3, 1002, "call_1", r#""old output""#, false),
            &event_line(
                "compaction/start",
                4,
                2000,
                r#"{"compactionId":"c-1","sourceCommandId":"cmd-1","turn":null}"#,
            ),
            &event_line(
                "compaction/summary",
                5,
                2001,
                r#"{"compactionId":"c-1","sourceCommandId":"cmd-1","summary":[{"type":"text","text":"checkpoint"}]}"#,
            ),
            &checkpoint_line(6, 2002, r#""[checkpoint] condensed history""#, &[1, 2, 3]),
            &event_line(
                "compaction/end",
                7,
                2003,
                r#"{"compactionId":"c-1","sourceCommandId":"cmd-1","turn":null}"#,
            ),
            &user_message_line(
                8,
                2004,
                r#"{"kind":"user"}"#,
                r#""new prompt after compaction""#,
            ),
        ]);
        let messages = &session.messages;
        // Old prompt/answer/tool are gone; checkpoint + new prompt remain.
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[0].content, "[checkpoint] condensed history");
        assert_eq!(messages[1].role, MessageRole::User);
        assert_eq!(messages[1].content, "new prompt after compaction");
        // Shadowed usage survives in usage_events (stats stay complete).
        assert_eq!(session.usage_events.len(), 1);
        assert_eq!(session.usage_events[0].input_tokens, 300);
        assert_eq!(session.usage_events[0].output_tokens, 50);
        // Search text keeps the old content even though it left the transcript.
        assert!(session.content_text.contains("old answer"));
        // No warnings from the compaction rows themselves.
        assert_eq!(session.parse_warning_count, 0);
    }

    #[test]
    fn compaction_removes_chunk_reconstructed_shadowed_step() {
        // An interrupted step (chunks but no assistant/message) inside a
        // compacted range: its flushed messages carry the step/end seq as
        // provenance, so the splice removes them along with the rest.
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &user_message_line(1, 1000, r#"{"kind":"user"}"#, r#""old prompt""#),
            &event_line(
                "text-chunks",
                2,
                1001,
                r#"{"turn":1,"step":1,"index":0,"dt":[1],"texts":["partial answer"]}"#,
            ),
            &event_line("step/end", 3, 1002, r#"{"turn":1,"step":1}"#),
            &checkpoint_line(4, 2002, r#""[checkpoint] condensed history""#, &[1, 2, 3]),
            &user_message_line(5, 2004, r#"{"kind":"user"}"#, r#""new prompt""#),
        ]);
        let messages = &session.messages;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[0].content, "[checkpoint] condensed history");
        assert_eq!(messages[1].content, "new prompt");
    }

    #[test]
    fn second_compaction_splices_correctly_after_first() {
        // Two successive compactions: the second cites the first checkpoint's
        // seq. Regression test for `message_seq` drifting out of parallel
        // with `messages` — misalignment made the second splice keep the
        // first checkpoint and remove the wrong messages instead.
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &user_message_line(1, 1000, r#"{"kind":"user"}"#, r#""old prompt""#),
            &assistant_message_line(
                2,
                1001,
                r#"[{"type":"text","text":"old answer"},{"type":"tool-call","id":"call_1","name":"bash","arguments":"{}"}]"#,
                None,
            ),
            &tool_result_line(3, 1002, "call_1", r#""old output""#, false),
            &checkpoint_line(4, 2000, r#""[checkpoint one]""#, &[1, 2, 3]),
            &user_message_line(5, 3000, r#"{"kind":"user"}"#, r#""mid prompt""#),
            &assistant_message_line(6, 3001, r#"[{"type":"text","text":"mid answer"}]"#, None),
            &checkpoint_line(7, 4000, r#""[checkpoint two]""#, &[4, 5, 6]),
            &user_message_line(8, 5000, r#"{"kind":"user"}"#, r#""new prompt""#),
        ]);
        let messages = &session.messages;
        assert_eq!(
            messages.len(),
            2,
            "checkpoint two must shadow checkpoint one and the mid turn: {:?}",
            messages.iter().map(|m| &m.content).collect::<Vec<_>>()
        );
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[0].content, "[checkpoint two]");
        assert_eq!(messages[1].role, MessageRole::User);
        assert_eq!(messages[1].content, "new prompt");
    }

    #[test]
    fn usage_event_model_falls_back_to_session_model() {
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &assistant_message_line(1, 1000, r#"[{"type":"text","text":"first"}]"#, None),
            // Second event reports usage but its source carries no model.
            &event_line(
                "assistant/message",
                2,
                1001,
                r#"{"turn":1,"step":2,"message":{"role":"assistant","content":[{"type":"text","text":"second"}],"source":{"kind":"model"},"id":"a2"},"usage":{"inputTokens":10,"outputTokens":5}}"#,
            ),
        ]);
        assert_eq!(session.usage_events.len(), 1);
        assert_eq!(session.usage_events[0].model, "deepseek-v4-flash");
        assert_eq!(session.usage_events[0].input_tokens, 10);
        assert_eq!(session.usage_events[0].output_tokens, 5);
        assert_eq!(session.parse_warning_count, 0);
    }

    #[test]
    fn usage_event_without_any_model_is_a_counted_warning() {
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &event_line(
                "assistant/message",
                1,
                1000,
                r#"{"turn":1,"step":1,"message":{"role":"assistant","content":[{"type":"text","text":"answer"}],"source":{"kind":"model"},"id":"a1"},"usage":{"inputTokens":10,"outputTokens":5}}"#,
            ),
        ]);
        assert!(session.usage_events.is_empty());
        assert_eq!(session.parse_warning_count, 1);
        // The message still carries its usage for display.
        let usage = session.messages[0].token_usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 10);
    }

    #[test]
    fn retry_and_subagent_metadata_events_are_not_warnings() {
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &event_line(
                "llm/retry",
                1,
                1000,
                r#"{"retryId":"r-1","turn":1,"step":1,"provider":"opencode-go","retry":1,"maxRetries":2,"failure":{"message":"boom","code":"TRANSPORT"}}"#,
            ),
            &event_line(
                "llm/retry-started",
                2,
                1001,
                r#"{"retryId":"r-1","turn":1,"step":1,"retry":1}"#,
            ),
            &event_line(
                "subagent/descriptor",
                3,
                1002,
                r#"{"version":2,"mode":"one-shot","provider":"spawn","label":"check code"}"#,
            ),
            &user_message_line(4, 1003, r#"{"kind":"user"}"#, r#""still works""#),
        ]);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.parse_warning_count, 0);
    }

    #[test]
    fn tolerates_torn_tail_and_malformed_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let body = format!(
            "{}\n{}\n{}\n{}",
            header_line("/tmp/p"),
            user_message_line(1, 1, r#"{"kind":"user"}"#, r#""ok""#),
            r#"{"type":"user/message","seq":2,"time":2,"data":{"content":[{"type":"text","text":"broken"}]"#,
            r#"{"type":"totally-unknown","seq":3,"time":3,"data":{},"ignorable":true}"#,
        );
        // No trailing newline: the final record is a torn tail.
        std::fs::write(&path, body).unwrap();

        let session = parse_session_file(&path).expect("fixture must parse");
        assert_eq!(session.messages.len(), 1);
        // Malformed line + missing newline: malformed counts, torn tail doesn't.
        assert_eq!(session.parse_warning_count, 1);
    }

    #[test]
    fn unknown_required_event_counts_as_parse_warning() {
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &user_message_line(1, 1, r#"{"kind":"user"}"#, r#""ok""#),
            &event_line("mystery/event", 2, 2, r#"{"x":1}"#),
        ]);
        assert_eq!(session.parse_warning_count, 1);
    }

    #[test]
    fn current_metadata_and_log_only_events_are_handled() {
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &user_message_line(1, 1, r#"{"kind":"user"}"#, r#""ok""#),
            &event_line(
                "web/deepseek-search-llm-request",
                2,
                2,
                r#"{"model":"deepseek-search"}"#,
            ),
            &event_line(
                "agent-preset/selected",
                3,
                3,
                r#"{"agentPreset":"reviewer"}"#,
            ),
        ]);

        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.meta.variant_name.as_deref(), Some("reviewer"));
        assert_eq!(session.parse_warning_count, 0);
    }

    #[test]
    fn malformed_agent_preset_selection_remains_a_parse_warning() {
        let session = parse_lines(&[
            &header_line("/tmp/p"),
            &user_message_line(1, 1, r#"{"kind":"user"}"#, r#""ok""#),
            &event_line("agent-preset/selected", 2, 2, r#"{}"#),
        ]);

        assert_eq!(session.meta.variant_name.as_deref(), Some("standard"));
        assert_eq!(session.parse_warning_count, 1);
    }

    #[test]
    fn reads_zstd_compressed_artifacts() {
        let dir = TempDir::new().unwrap();
        let plain = format!(
            "{}\n{}\n",
            header_line("/tmp/p"),
            user_message_line(1, 1, r#"{"kind":"user"}"#, r#""from zstd""#),
        );
        let path = dir.path().join("session.jsonl.zstd");
        let compressed = zstd::stream::encode_all(plain.as_bytes(), 3).unwrap();
        std::fs::write(&path, compressed).unwrap();

        let session = parse_session_file(&path).expect("zstd fixture must parse");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "from zstd");
        assert_eq!(session.meta.source_path, path.to_string_lossy());
    }

    #[test]
    fn provider_scans_fake_home_and_loads_messages() {
        let dir = TempDir::new().unwrap();
        let sessions = dir.path().join("sessions");
        let project = sessions.join("--tmp-p--");
        let session_dir = project.join(SESSION_ID);
        std::fs::create_dir_all(&session_dir).unwrap();
        let log = format!(
            "{}\n{}\n",
            header_line("/tmp/p"),
            user_message_line(1, 1, r#"{"kind":"user"}"#, r#""scan me""#),
        );
        std::fs::write(session_dir.join("session.jsonl"), log).unwrap();

        let provider = DshProvider::with_home(dir.path().to_path_buf());
        assert_eq!(provider.provider(), Provider::Dsh);

        let sessions = provider.scan_all().expect("scan must succeed");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].meta.id, SESSION_ID);
        assert_eq!(sessions[0].meta.project_path, "/tmp/p");

        let loaded = provider
            .load_messages(SESSION_ID, &sessions[0].meta.source_path)
            .expect("load must succeed");
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].role, MessageRole::User);

        // Incremental scan short-circuits the unchanged file.
        use std::collections::HashMap;
        let known: HashMap<String, crate::provider::SourceState> = provider
            .scan_all()
            .unwrap()
            .into_iter()
            .map(|s| {
                (
                    s.meta.source_path.clone(),
                    crate::provider::SourceState {
                        size: s.meta.file_size_bytes,
                        mtime: s.source_mtime,
                        title: None,
                    },
                )
            })
            .collect();
        let outcome = provider.scan_incremental(&known).expect("incremental scan");
        assert!(outcome.parsed.is_empty());
        assert_eq!(outcome.unchanged_source_paths.len(), 1);
    }
}
