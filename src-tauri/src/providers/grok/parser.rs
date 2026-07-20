//! Parser for Grok CLI sessions (verified against real 0.2.101 data).
//!
//! ```text
//! ~/.grok/sessions/<url-encoded-cwd>/<session-uuid>/
//!   summary.json        # index entry: id, cwd, title, timestamps, model
//!   chat_history.jsonl  # transcript (system/user/reasoning/assistant/tool_result)
//!   updates.jsonl       # append-only ACP stream: timestamps, per-turn usage
//! ```
//!
//! Format quirks:
//! - `chat_history.jsonl` has no timestamps and no token usage. Both come
//!   from `updates.jsonl`: timestamps via `promptIndex` / `toolCallId`
//!   anchors, usage via per-turn `turn_completed` events. The wire's
//!   `inputTokens` INCLUDES `cachedReadTokens`; the parser subtracts it so
//!   `ParsedSession::usage_events` stores the two disjoint, and each turn's
//!   totals also attach to that turn's final assistant message for display.
//! - Real user prompts carry a numeric `prompt_index` and a `<user_query>`
//!   wrapper; CLI-injected context (`<user_info>`, `synthetic_reason`
//!   entries) does not and is skipped.
//! - `reasoning` entries only expose `summary[].text` (the chain itself is
//!   encrypted); rendered as `[thinking]` system messages.
//! - `tool_calls[].arguments` is a JSON-encoded *string*, not an object.
//! - Auto-compact REWRITES chat_history.jsonl in place; dropped history is
//!   reconstructed from updates.jsonl (see `parser/history.rs`). In-place
//!   rewrites are also why `load_messages` retries transient parse failures.
//! - Incremental freshness = chat file `(size, mtime)` + a title comparison
//!   against summary.json (title regeneration rewrites only summary.json).
//!
//! Subagents: a child is a sibling session dir with
//! `session_kind: "subagent"`; the typed parent→child link lives in
//! `<parent-dir>/subagents/<child-id>/meta.json`. Parents surface
//! `child_session_ids` (db sync back-fills), children resolve `parent_id`
//! by probing sibling dirs, and `spawn_subagent` results carry a
//! `subagent_id:` line that is lifted to `structured.agentId` for the
//! frontend "Open subagent" button.

mod history;
mod types;
mod updates;

use std::collections::HashMap;
use std::io::BufReader;
use std::ops::ControlFlow;
use std::path::Path;

use serde_json::Value;

use crate::models::{Message, MessageRole, Provider, SessionMeta, TokenUsage};
use crate::provider::ParsedSession;
use crate::provider_utils::{
    ToolCallPairer, for_each_jsonl_record, project_name_from_path, session_title,
};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

use types::{
    GrokChatEntry, GrokSubagentMeta, GrokSummary, GrokToolCall, content_text_raw, prompt_index_u64,
    user_content_to_text,
};
use updates::{UpdateAnchors, scan_updates};

/// Provider-derived title. Shared by the full parse and `scan_incremental`'s
/// staleness check — the two derivations must stay identical or unchanged
/// sessions re-parse forever.
pub(super) fn derive_title_of(session_dir: &Path) -> Option<String> {
    let summary = read_summary(&session_dir.join("summary.json"))?;
    derive_title(session_dir, &summary)
}

fn derive_title(session_dir: &Path, summary: &GrokSummary) -> Option<String> {
    // Subagents: the parent-side meta.json description reads far better
    // than the child's generated_title (first line of the task prompt).
    if is_subagent(summary)
        && let Some((_parent_id, Some(description))) =
            find_parent_link(session_dir, &summary.info.id)
                .map(|link| (link.parent_session_id, non_empty(link.description)))
    {
        return Some(description);
    }
    [&summary.generated_title, &summary.session_summary]
        .into_iter()
        .find_map(|title| non_empty(title.clone()))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

fn is_subagent(summary: &GrokSummary) -> bool {
    summary.session_kind.as_deref() == Some("subagent")
}

/// Resolve the parent-side link for a subagent child: probe every sibling
/// session dir under the same encoded-cwd directory for
/// `subagents/<child-id>/meta.json`.
fn find_parent_link(session_dir: &Path, child_id: &str) -> Option<GrokSubagentMeta> {
    let cwd_dir = session_dir.parent()?;
    let siblings = std::fs::read_dir(cwd_dir).ok()?;
    for sibling in siblings.filter_map(Result::ok) {
        let meta_path = sibling
            .path()
            .join("subagents")
            .join(child_id)
            .join("meta.json");
        if !meta_path.is_file() {
            continue;
        }
        match std::fs::read_to_string(&meta_path)
            .map_err(|e| e.to_string())
            .and_then(|content| {
                serde_json::from_str::<GrokSubagentMeta>(&content).map_err(|e| e.to_string())
            }) {
            Ok(link) => return Some(link),
            Err(error) => {
                log::warn!(
                    "failed to read Grok subagent link '{}': {error}",
                    meta_path.display()
                );
                return None;
            }
        }
    }
    None
}

/// Child session ids this parent spawned, from `<dir>/subagents/*/meta.json`.
fn child_session_ids(session_dir: &Path) -> Vec<String> {
    let subagents_dir = session_dir.join("subagents");
    let Ok(entries) = std::fs::read_dir(&subagents_dir) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let meta_path = entry.path().join("meta.json");
        if !meta_path.is_file() {
            continue;
        }
        let child_id = std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|content| serde_json::from_str::<GrokSubagentMeta>(&content).ok())
            .and_then(|link| non_empty(link.child_session_id))
            .or_else(|| entry.file_name().to_str().map(str::to_string));
        match child_id {
            Some(id) => ids.push(id),
            None => log::warn!(
                "skipping Grok subagent link without child id: {}",
                meta_path.display()
            ),
        }
    }
    ids.sort();
    ids
}

fn read_summary(summary_path: &Path) -> Option<GrokSummary> {
    let content = match std::fs::read_to_string(summary_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!(
                "failed to read Grok summary '{}': {error}",
                summary_path.display()
            );
            return None;
        }
    };
    match serde_json::from_str::<GrokSummary>(&content) {
        Ok(summary) => Some(summary),
        Err(error) => {
            log::warn!(
                "failed to parse Grok summary '{}': {error}",
                summary_path.display()
            );
            None
        }
    }
}

/// Parse one session from its `chat_history.jsonl` path. Returns `None`
/// (with a logged warning) when the transcript is empty or unreadable.
///
/// A missing `summary.json` is survivable because the session id / cwd are
/// also encoded in the path
/// (`<percent-encoded-cwd>/<session-id>/chat_history.jsonl`).
pub(crate) fn parse_session_file(chat_path: &Path) -> Option<ParsedSession> {
    let session_dir = chat_path.parent()?;
    let summary = read_summary(&session_dir.join("summary.json"));

    let file = match std::fs::File::open(chat_path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "failed to open Grok chat history '{}': {error}",
                chat_path.display()
            );
            return None;
        }
    };

    let mut entries: Vec<GrokChatEntry> = Vec::new();
    let stats = for_each_jsonl_record(BufReader::new(file), chat_path, |_, entry| {
        entries.push(entry);
        ControlFlow::Continue(())
    });

    let history_cutoff = compaction_history_cutoff(&entries);
    let (anchors, history_messages) =
        scan_updates(&session_dir.join("updates.jsonl"), history_cutoff);
    let parse_warning_count = stats
        .read_error_count
        .saturating_add(stats.parse_error_count)
        .saturating_add(anchors.parse_warning_count);

    let skip_preserved_prompts = !history_messages.is_empty();
    let (chat_messages, last_model) = build_messages(&entries, &anchors, skip_preserved_prompts);
    let mut messages = history_messages;
    messages.extend(chat_messages);
    if messages.is_empty() {
        return None;
    }

    let session_id = summary.as_ref().map(|s| s.info.id.clone()).or_else(|| {
        session_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
    })?;
    let cwd = summary
        .as_ref()
        .and_then(|s| s.info.cwd.clone())
        .or_else(|| {
            session_dir
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(percent_decode)
        });

    let title = summary
        .as_ref()
        .and_then(|s| derive_title(session_dir, s))
        .unwrap_or_else(|| {
            let first_user = messages
                .iter()
                .find(|m| m.role == MessageRole::User)
                .map(|m| m.content.as_str());
            session_title(first_user)
        });

    let is_sidechain = summary.as_ref().is_some_and(is_subagent);
    let parent_id = if is_sidechain {
        let parent = find_parent_link(session_dir, &session_id)
            .and_then(|link| non_empty(link.parent_session_id));
        if parent.is_none() {
            log::warn!("Grok subagent '{session_id}' has no resolvable parent link");
        }
        parent
    } else {
        None
    };
    let children = child_session_ids(session_dir);

    let file_metadata = std::fs::metadata(chat_path).ok();
    let mtime_fallback = file_metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(crate::provider::system_time_to_epoch_seconds)
        .unwrap_or(0);
    let created_at = summary
        .as_ref()
        .and_then(|s| s.created_at.as_deref())
        .map(|ts| crate::provider_utils::parse_rfc3339_timestamp(Some(ts)))
        .unwrap_or(mtime_fallback);
    let updated_at = summary
        .as_ref()
        .and_then(|s| s.updated_at.as_deref())
        .map(|ts| crate::provider_utils::parse_rfc3339_timestamp(Some(ts)))
        .unwrap_or(mtime_fallback);

    let project_path = cwd.unwrap_or_else(|| {
        log::warn!("Grok session '{session_id}' has no resolvable cwd");
        crate::provider_utils::NO_PROJECT.to_string()
    });

    let meta = SessionMeta {
        id: session_id,
        provider: Provider::Grok,
        title,
        project_name: project_name_from_path(&project_path),
        project_path,
        created_at,
        updated_at,
        message_count: messages.len() as u32,
        file_size_bytes: file_metadata.as_ref().map(|m| m.len()).unwrap_or(0),
        source_path: chat_path.to_string_lossy().to_string(),
        is_sidechain,
        variant_name: None,
        model: summary.and_then(|s| s.current_model_id).or(last_model),
        cc_version: None,
        git_branch: None,
        parent_id,
        // Usage lives in usage_events, not on meta (Codex pattern).
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };

    let content_text = messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    Some(ParsedSession {
        meta,
        messages,
        content_text,
        parse_warning_count,
        child_session_ids: children,
        usage_events: anchors.usage_events,
        source_mtime: file_metadata
            .and_then(|m| m.modified().ok())
            .and_then(crate::provider::system_time_to_epoch_seconds)
            .unwrap_or(0),
    })
}

fn build_messages(
    entries: &[GrokChatEntry],
    anchors: &UpdateAnchors,
    skip_preserved_prompts: bool,
) -> (Vec<Message>, Option<String>) {
    let mut messages: Vec<Message> = Vec::new();
    let mut pairer = ToolCallPairer::default();
    let mut last_model: Option<String> = None;
    let mut current_prompt: Option<u64> = None;
    let mut turn_last_assistant: Option<usize> = None;
    let mut attached_turns: HashMap<u64, usize> = HashMap::new();

    for entry in entries {
        match entry {
            GrokChatEntry::System {} | GrokChatEntry::Unknown => {}
            GrokChatEntry::User {
                content,
                prompt_index,
                synthetic_reason,
            } => {
                // Of the CLI-injected entries, only the compaction
                // summary is worth showing; the rest is context noise.
                if let Some(reason) = synthetic_reason.as_deref() {
                    if reason == "compaction_meta" {
                        let text = user_content_to_text(content);
                        if !text.is_empty() && !text.starts_with("<user_info>") {
                            messages.push(Message::system(format!("[Compaction] {text}")));
                        }
                    }
                    continue;
                }
                // Compaction preserves recent prompts verbatim but strips
                // their index — recognize those by the <user_query> wrapper.
                let index = prompt_index.as_ref().and_then(prompt_index_u64);
                let raw_text = content_text_raw(content);
                if index.is_none() && !raw_text.trim_start().starts_with("<user_query>") {
                    continue;
                }
                // Already covered by the reconstructed history.
                if index.is_none() && skip_preserved_prompts {
                    continue;
                }
                attach_turn_usage(
                    &mut messages,
                    anchors,
                    current_prompt,
                    turn_last_assistant,
                    &mut attached_turns,
                );
                current_prompt = index;
                turn_last_assistant = None;
                let text = user_content_to_text(content);
                if text.is_empty() {
                    continue;
                }
                messages.push(Message {
                    timestamp: index.and_then(|i| anchors.user_timestamps.get(&i).cloned()),
                    ..Message::user(text)
                });
            }
            GrokChatEntry::Reasoning { summary } => {
                let text = summary
                    .iter()
                    .map(|s| s.text.as_str())
                    .filter(|t| !t.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    messages.push(Message::system(format!("[thinking]\n{text}")));
                }
            }
            GrokChatEntry::Assistant {
                content,
                model_id,
                tool_calls,
            } => {
                if model_id.is_some() {
                    last_model = model_id.clone();
                }
                if !content.trim().is_empty() {
                    turn_last_assistant = Some(messages.len());
                    messages.push(Message {
                        model: model_id.clone(),
                        ..Message::assistant(content.clone())
                    });
                }
                for call in tool_calls {
                    push_tool_call(&mut messages, &mut pairer, call, model_id.clone(), anchors);
                }
            }
            GrokChatEntry::ToolResult {
                tool_call_id,
                content,
            } => {
                merge_tool_result(&mut messages, &pairer, tool_call_id, content);
            }
        }
    }
    attach_turn_usage(
        &mut messages,
        anchors,
        current_prompt,
        turn_last_assistant,
        &mut attached_turns,
    );

    (messages, last_model)
}

/// Attach the completed turn's totals (and turn-end timestamp) to the
/// turn's final assistant message.
fn attach_turn_usage(
    messages: &mut [Message],
    anchors: &UpdateAnchors,
    prompt_index: Option<u64>,
    last_assistant_idx: Option<usize>,
    attached_turns: &mut HashMap<u64, usize>,
) {
    let (Some(prompt_index), Some(idx)) = (prompt_index, last_assistant_idx) else {
        return;
    };
    let Some(turn) = anchors.turn_usages.get(&prompt_index) else {
        return;
    };
    // Regeneration repeats an index; only the final occurrence keeps usage.
    if let Some(previous_idx) = attached_turns.insert(prompt_index, idx)
        && let Some(previous) = messages.get_mut(previous_idx)
    {
        previous.token_usage = None;
    }
    let Some(message) = messages.get_mut(idx) else {
        return;
    };
    message.token_usage = Some(TokenUsage {
        input_tokens: turn.input_tokens.saturating_sub(turn.cache_read_tokens) as u32,
        output_tokens: turn.output_tokens as u32,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: turn.cache_read_tokens as u32,
    });
    if message.timestamp.is_none() {
        message.timestamp = Some(turn.timestamp.clone());
    }
}

fn push_tool_call(
    messages: &mut Vec<Message>,
    pairer: &mut ToolCallPairer,
    call: &GrokToolCall,
    model: Option<String>,
    anchors: &UpdateAnchors,
) {
    let input_value: Option<Value> = serde_json::from_str(&call.arguments).ok();
    let metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Grok,
        raw_name: &call.name,
        input: input_value.as_ref(),
        call_id: Some(&call.id),
        assistant_id: None,
    });
    let tool_input = input_value
        .as_ref()
        .map(Value::to_string)
        .unwrap_or_else(|| call.arguments.clone());
    let idx = messages.len();
    messages.push(Message {
        timestamp: anchors.tool_timestamps.get(&call.id).cloned(),
        tool_name: Some(metadata.canonical_name.clone()),
        tool_input: Some(tool_input),
        tool_metadata: Some(metadata),
        model,
        ..Message::new(MessageRole::Tool, String::new())
    });
    pairer.register(Some(&call.id), idx);
}

fn merge_tool_result(
    messages: &mut Vec<Message>,
    pairer: &ToolCallPairer,
    tool_call_id: &str,
    content: &str,
) {
    if let Some(message) = pairer.message_mut(Some(tool_call_id), messages) {
        message.content = content.to_string();
        if let Some(metadata) = message.tool_metadata.as_mut() {
            let result_value = grok_result_value(&metadata.canonical_name, content);
            enrich_tool_metadata(
                metadata,
                ToolResultFacts {
                    raw_result: Some(&result_value),
                    is_error: None,
                    status: None,
                    artifact_path: None,
                },
            );
        }
        return;
    }

    log::warn!("Grok tool_result '{tool_call_id}' has no matching tool call; emitting standalone");
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Grok,
        raw_name: "unknown",
        input: None,
        call_id: Some(tool_call_id),
        assistant_id: None,
    });
    let result_value = grok_result_value(&metadata.canonical_name, content);
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&result_value),
            is_error: None,
            status: None,
            artifact_path: None,
        },
    );
    messages.push(Message {
        tool_name: Some(metadata.canonical_name.clone()),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, content.to_string())
    });
}

/// Structured result payload for a tool result. Output is mirrored into
/// `structured.output` only for tools whose result kind suppresses the raw
/// block — otherwise the same text would render twice.
fn grok_result_value(canonical_name: &str, content: &str) -> Value {
    let mut result = serde_json::Map::new();
    if should_mirror_output_into_structured(canonical_name) {
        result.insert("output".to_string(), Value::String(content.to_string()));
    }
    // Lift "subagent_id: <id>" so it becomes structured.agentId.
    if let Some(child_id) = extract_subagent_id(content) {
        result.insert("agent_id".to_string(), Value::String(child_id));
    }
    Value::Object(result)
}

fn extract_subagent_id(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.trim().strip_prefix("subagent_id:"))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Decode the percent-encoded cwd grok uses as the per-project directory
/// name (e.g. `%2Ftmp%2Fdemo` → `/tmp/demo`).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let decoded = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| u8::from_str_radix(&input[i + 1..i + 3], 16).ok())
            .flatten();
        match decoded {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Oldest prompt_index still present in a compacted transcript (`None` when
/// never compacted; `u64::MAX` when nothing indexed survived).
fn compaction_history_cutoff(entries: &[GrokChatEntry]) -> Option<u64> {
    let compacted = entries.iter().any(|entry| {
        matches!(
            entry,
            GrokChatEntry::User { synthetic_reason: Some(reason), .. }
                if reason == "compaction_meta"
        )
    });
    if !compacted {
        return None;
    }
    Some(
        entries
            .iter()
            .filter_map(|entry| match entry {
                GrokChatEntry::User { prompt_index, .. } => {
                    prompt_index.as_ref().and_then(prompt_index_u64)
                }
                _ => None,
            })
            .min()
            .unwrap_or(u64::MAX),
    )
}

/// Tools whose result kind (`terminal_output`) suppresses the raw output
/// block, making `structured.output` the single render.
fn should_mirror_output_into_structured(canonical_name: &str) -> bool {
    matches!(canonical_name, "Bash" | "TaskOutput")
}
