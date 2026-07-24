//! Parser for Grok CLI sessions (verified against real 0.2.101+ data).
//!
//! ```text
//! ~/.grok/sessions/<url-encoded-cwd>/<session-uuid>/
//!   summary.json        # index entry: id, cwd, title, timestamps, model
//!   chat_history.jsonl  # transcript (system/user/reasoning/assistant/tool_result/backend_tool_call)
//!   updates.jsonl       # append-only ACP stream: timestamps, per-turn usage, tool results
//! ```
//!
//! Format quirks:
//! - `chat_history.jsonl` has no timestamps and no token usage. Both come
//!   from `updates.jsonl`: timestamps via `promptIndex` / `toolCallId`
//!   anchors, usage via per-turn `turn_completed` events. The wire's
//!   `inputTokens` INCLUDES `cachedReadTokens`; the parser subtracts it so
//!   `ParsedSession::usage_events` stores the two disjoint, and each turn's
//!   totals also attach to that turn's final assistant message for display.
//!   `reasoningTokens` are folded into output (no dedicated TokenUsage field).
//!   `costUsdTicks` (1e10 ticks = $1) becomes `UsageEvent::cost_usd`.
//! - Real user prompts carry a numeric `prompt_index` and a `<user_query>`
//!   wrapper; CLI-injected context (`<user_info>`, `synthetic_reason`
//!   entries) does not and is skipped.
//! - `reasoning` entries only expose `summary[].text` (the chain itself is
//!   encrypted); rendered as `[thinking]` system messages.
//! - `tool_calls[].arguments` is a JSON-encoded *string*, not an object.
//! - Backend tools (`backend_tool_call`: web_search / x_search) have no
//!   paired `tool_result` in chat_history — results come from updates.
//! - Auto-compact REWRITES chat_history.jsonl in place; dropped history is
//!   reconstructed from updates.jsonl (see `parser/history.rs`). In-place
//!   rewrites are also why `load_messages` retries transient parse failures.
//! - Incremental freshness = chat file `(size, mtime)` + a title comparison
//!   against summary.json (title regeneration rewrites only summary.json).
//!
//! Subagents: a child is a sibling session dir with
//! `session_kind: "subagent"` or `"subagent_fork"`; the typed parent→child
//! link lives in `<parent-dir>/subagents/<child-id>/meta.json` (and forks
//! also stamp `parent_session_id` on summary.json). Parents surface
//! `child_session_ids` (db sync back-fills), children resolve `parent_id`
//! by probing sibling dirs / summary, and `spawn_subagent` results carry a
//! `subagent_id:` line that is lifted to `structured.agentId` for the
//! frontend "Open subagent" button.

mod history;
mod types;
mod updates;

use std::collections::HashMap;
use std::io::BufReader;
use std::ops::ControlFlow;
use std::path::Path;

use serde_json::{Value, json};

use crate::models::{Message, MessageRole, Provider, SessionMeta, TokenUsage};
use crate::provider::ParsedSession;
use crate::provider::util::{
    ToolCallPairer, for_each_jsonl_record, project_name_from_path, session_title,
};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

use types::{
    GrokBackendToolKind, GrokChatEntry, GrokSubagentMeta, GrokSummary, GrokToolCall,
    content_text_raw, prompt_index_u64, user_content_to_text,
};
use updates::{ToolCallState, UpdateAnchors, scan_updates};

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
    matches!(
        summary.session_kind.as_deref(),
        Some("subagent" | "subagent_fork")
    )
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
    let (mut anchors, history_messages) =
        scan_updates(&session_dir.join("updates.jsonl"), history_cutoff);
    let parse_warning_count = stats
        .read_error_count
        .saturating_add(stats.parse_error_count)
        .saturating_add(anchors.parse_warning_count);

    let skip_preserved_prompts = !history_messages.is_empty();
    let (chat_messages, last_model) = build_messages(&entries, &anchors, skip_preserved_prompts);
    let mut messages = history_messages;
    messages.extend(chat_messages);
    // Session-level notes (plan / goal / recap) are ordered by stream time;
    // interleave them so the timeline stays chronological.
    let session_notes = std::mem::take(&mut anchors.session_notes);
    messages = interleave_session_notes(messages, session_notes);
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
            .and_then(|link| non_empty(link.parent_session_id))
            .or_else(|| {
                summary
                    .as_ref()
                    .and_then(|s| non_empty(s.parent_session_id.clone()))
            });
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
        .map(|ts| crate::provider::util::parse_rfc3339_timestamp(Some(ts)))
        .unwrap_or(mtime_fallback);
    let updated_at = summary
        .as_ref()
        .and_then(|s| s.updated_at.as_deref())
        .map(|ts| crate::provider::util::parse_rfc3339_timestamp(Some(ts)))
        .unwrap_or(mtime_fallback);

    let project_path = cwd.unwrap_or_else(|| {
        log::warn!("Grok session '{session_id}' has no resolvable cwd");
        crate::provider::util::NO_PROJECT.to_string()
    });

    let git_branch = summary
        .as_ref()
        .and_then(|s| non_empty(s.head_branch.clone()));
    let variant_name = summary
        .as_ref()
        .and_then(|s| non_empty(s.agent_name.clone()))
        .filter(|name| name != "grok-build" && name != "grok");

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
        variant_name,
        model: summary.and_then(|s| s.current_model_id).or(last_model),
        cc_version: None,
        git_branch,
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
                merge_tool_result(&mut messages, &pairer, tool_call_id, content, anchors);
            }
            GrokChatEntry::BackendToolCall { kind } => {
                push_backend_tool_call(&mut messages, kind, anchors);
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

fn push_backend_tool_call(
    messages: &mut Vec<Message>,
    kind: &GrokBackendToolKind,
    anchors: &UpdateAnchors,
) {
    let call_id = kind.id.as_deref().or(kind.call_id.as_deref()).unwrap_or("");
    let raw_name = kind
        .name
        .as_deref()
        .or(kind.tool_type.as_deref())
        .unwrap_or("web_search");
    let state = if call_id.is_empty() {
        None
    } else {
        anchors.tool_states.get(call_id)
    };

    // Input = call args only (query/limit). Prefer chat_history, then updates.
    let input_value: Option<Value> = kind
        .input
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .or_else(|| {
            // Web-search stores query under kind.action.query (not kind.input).
            kind.action
                .as_ref()
                .and_then(|a| a.get("query"))
                .and_then(Value::as_str)
                .map(|q| json!({ "query": q }))
        })
        .or_else(|| {
            state.and_then(|s| s.raw_output.as_ref()).and_then(|raw| {
                raw.get("input")
                    .and_then(Value::as_str)
                    .and_then(|s| serde_json::from_str(s).ok())
                    .or_else(|| {
                        raw.pointer("/action/query")
                            .and_then(Value::as_str)
                            .map(|q| json!({ "query": q }))
                    })
            })
        })
        .or_else(|| {
            state
                .and_then(|s| s.raw_input.clone())
                .filter(|v| !v.get("backend").and_then(Value::as_bool).unwrap_or(false))
        });

    let resolved_name = state
        .and_then(|s| s.raw_name.as_deref())
        .or(kind.name.as_deref())
        .or(kind.tool_type.as_deref())
        .unwrap_or(raw_name);
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Grok,
        raw_name: resolved_name,
        input: input_value.as_ref(),
        call_id: (!call_id.is_empty()).then_some(call_id),
        assistant_id: None,
    });

    let (content, result_value, status, is_error) =
        backend_result_from_kind_and_state(kind, state, input_value.as_ref());
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&result_value),
            is_error,
            status: status.as_deref(),
            artifact_path: None,
            raw_output: Some(false),
        },
    );

    messages.push(Message {
        timestamp: state
            .and_then(|s| s.timestamp.clone())
            .or_else(|| anchors.tool_timestamps.get(call_id).cloned()),
        tool_name: Some(metadata.canonical_name.clone()),
        tool_input: input_value
            .as_ref()
            .map(Value::to_string)
            .or_else(|| kind.input.clone().filter(|s| !s.is_empty())),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, content)
    });
}

/// Resolve result body + structured payload for a backend tool.
///
/// Priority: chat_history `kind.action` (web_search embeds sources here) →
/// updates.jsonl rawOutput/display → honest empty for X-search call echoes
/// (Grok never persists X hits on disk).
fn backend_result_from_kind_and_state(
    kind: &GrokBackendToolKind,
    state: Option<&ToolCallState>,
    input: Option<&Value>,
) -> (String, Value, Option<String>, Option<bool>) {
    let status = state
        .and_then(|s| s.status.clone())
        .or_else(|| kind.status.clone())
        .or_else(|| Some("completed".into()));
    let is_error = status
        .as_deref()
        .map(|s| matches!(s, "failed" | "error" | "cancelled" | "canceled"));

    // 1) Web-search result embedded on the chat entry itself.
    if let Some(action) = kind.action.as_ref()
        && let Some((text, structured)) = format_search_action(action, input)
    {
        return (text, structured, status, is_error);
    }

    // 2) Updates stream.
    if let Some(state) = state {
        if let Some(display) = state.display_text.as_deref().filter(|t| !t.is_empty()) {
            let structured = state
                .raw_output
                .as_ref()
                .map(|raw| backend_result_structured(raw, input))
                .unwrap_or_else(|| json!({}));
            return (display.to_string(), structured, status, is_error);
        }
        if let Some(raw) = state.raw_output.as_ref() {
            if is_backend_call_echo(raw) {
                // X-search: nothing to show beyond the call args.
                return (String::new(), json!({}), status, is_error);
            }
            if let Some(action) = raw.get("action")
                && let Some((text, structured)) = format_search_action(action, input)
            {
                return (text, structured, status, is_error);
            }
            return (
                String::new(),
                backend_result_structured(raw, input),
                status,
                is_error,
            );
        }
    }

    (String::new(), json!({}), status, is_error)
}

/// Format a web_search-style `action` object into (body text, structured).
/// Returns `None` when the action carries no listable hits.
fn format_search_action(action: &Value, input: Option<&Value>) -> Option<(String, Value)> {
    let sources = action.get("sources").and_then(Value::as_array)?;
    let urls: Vec<&str> = sources
        .iter()
        .filter_map(|s| s.get("url").and_then(Value::as_str))
        .filter(|u| !u.is_empty())
        .collect();
    // Empty sources array is still a real (empty) result.
    let text = if urls.is_empty() {
        String::new()
    } else {
        urls.iter()
            .enumerate()
            .map(|(i, u)| format!("{}. {u}", i + 1))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut structured = serde_json::Map::new();
    structured.insert("results".into(), Value::Array(sources.clone()));
    structured.insert("resultCount".into(), json!(urls.len()));
    let action_query = action.get("query").and_then(Value::as_str);
    let input_query = input.and_then(|v| v.get("query")).and_then(Value::as_str);
    if let Some(q) = action_query
        && input_query != Some(q)
    {
        structured.insert("query".into(), Value::String(q.to_string()));
    }
    Some((text, Value::Object(structured)))
}

/// True when rawOutput is only an echo of the backend tool call (no hits).
fn is_backend_call_echo(raw: &Value) -> bool {
    let Some(obj) = raw.as_object() else {
        return false;
    };
    let has_call_shape = obj.contains_key("input")
        && (obj.contains_key("name") || obj.contains_key("call_id") || obj.contains_key("id"));
    if !has_call_shape {
        return false;
    }
    !obj.contains_key("action")
        && !obj.contains_key("sources")
        && !obj.contains_key("results")
        && !obj.contains_key("posts")
        && !obj.contains_key("output")
}

/// Structured fields for result-side presentation from a raw updates payload.
fn backend_result_structured(raw: &Value, input: Option<&Value>) -> Value {
    if let Some(action) = raw.get("action")
        && let Some((_text, structured)) = format_search_action(action, input)
    {
        return structured;
    }
    let mut result = serde_json::Map::new();
    if let Some(results) = raw.get("results").cloned() {
        result.insert("results".into(), results);
    }
    if let Some(count) = raw
        .get("results")
        .and_then(Value::as_array)
        .map(|a| a.len())
    {
        result.insert("resultCount".into(), json!(count));
    }
    let result_query = raw.get("query").and_then(Value::as_str);
    let input_query = input.and_then(|v| v.get("query")).and_then(Value::as_str);
    if let Some(q) = result_query
        && input_query != Some(q)
    {
        result.insert("query".into(), Value::String(q.to_string()));
    }
    Value::Object(result)
}

fn merge_tool_result(
    messages: &mut Vec<Message>,
    pairer: &ToolCallPairer,
    tool_call_id: &str,
    content: &str,
    anchors: &UpdateAnchors,
) {
    let state = anchors.tool_states.get(tool_call_id);
    if let Some(message) = pairer.message_mut(Some(tool_call_id), messages) {
        message.content = content.to_string();
        if let Some(metadata) = message.tool_metadata.as_mut() {
            let mut result_value = grok_result_value(
                &metadata.canonical_name,
                content,
                message.tool_input.as_deref(),
            );
            // Prefer richer structured payloads from updates when present
            // (e.g. EditsApplied old/new, ImageGen path).
            if let Some(raw) = state.and_then(|s| s.raw_output.as_ref()) {
                result_value = merge_raw_output_into_result(result_value, raw);
            }
            let status = state.and_then(|s| s.status.as_deref());
            let is_error = status
                .map(|s| matches!(s, "failed" | "error" | "cancelled" | "canceled"))
                .or_else(|| content_looks_like_error(content).then_some(true));
            enrich_tool_metadata(
                metadata,
                ToolResultFacts {
                    raw_result: Some(&result_value),
                    is_error,
                    status,
                    artifact_path: None,
                    raw_output: Some(false),
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
    let result_value = grok_result_value(&metadata.canonical_name, content, None);
    let status = state.and_then(|s| s.status.as_deref());
    let is_error = status
        .map(|s| matches!(s, "failed" | "error" | "cancelled" | "canceled"))
        .or_else(|| content_looks_like_error(content).then_some(true));
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&result_value),
            is_error,
            status,
            artifact_path: None,
            raw_output: Some(false),
        },
    );
    messages.push(Message {
        tool_name: Some(metadata.canonical_name.clone()),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, content.to_string())
    });
}

fn content_looks_like_error(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains(" failed:")
        || lower.starts_with("error:")
        || lower.starts_with("tool `") && lower.contains("` failed")
        || lower.contains("ssrf blocked")
}

fn merge_raw_output_into_result(mut base: Value, raw: &Value) -> Value {
    // Prefer decoded structured extras from known rawOutput variants.
    if let Some(edits) = raw.get("EditsApplied") {
        if let (Some(old), Some(new)) = (
            edits.get("old_string").and_then(Value::as_str),
            edits.get("new_string").and_then(Value::as_str),
        ) && let Some(obj) = base.as_object_mut()
        {
            obj.insert("oldString".into(), Value::String(old.to_string()));
            obj.insert("newString".into(), Value::String(new.to_string()));
        }
        return base;
    }
    if raw.get("type").and_then(Value::as_str) == Some("ImageGen")
        && let Some(path) = raw.get("path").and_then(Value::as_str)
    {
        if let Some(obj) = base.as_object_mut() {
            obj.insert("path".into(), Value::String(path.to_string()));
            obj.insert("savedPath".into(), Value::String(path.to_string()));
        }
        return base;
    }
    if let Some(file) = raw.get("FileContent") {
        return file.clone();
    }
    if let Some(result) = raw.get("Result") {
        if let (Some(base_obj), Some(result_obj)) = (base.as_object_mut(), result.as_object()) {
            for (k, v) in result_obj {
                base_obj.entry(k.clone()).or_insert_with(|| v.clone());
            }
            return base;
        }
        return result.clone();
    }
    base
}

/// Structured result payload for a tool result. Bash mirrors its output so
/// the dedicated terminal renderer can read the stream; ordinary tools use
/// the shared message output. JSON results (image_gen, etc.) are lifted into
/// structured fields; Edit tools also promote old/new from the call input so
/// the Diff renderer can run without an EditsApplied rawOutput.
fn grok_result_value(canonical_name: &str, content: &str, tool_input: Option<&str>) -> Value {
    let mut result = serde_json::Map::new();

    // Prefer parsing tool_result content as JSON when it looks like an object
    // (image_gen, some TaskOutput payloads, etc.). Mirror tools (Bash) are
    // exempt: their output is opaque terminal text even when it happens to
    // be JSON, and lifting it would pollute structured presentation fields.
    if !should_mirror_output_into_structured(canonical_name)
        && content.trim_start().starts_with('{')
        && let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(content)
    {
        result = parsed;
        if result.get("savedPath").is_none()
            && let Some(path) = result
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string)
        {
            result.insert("savedPath".into(), Value::String(path));
        }
    }

    if should_mirror_output_into_structured(canonical_name) {
        result
            .entry("output".to_string())
            .or_insert_with(|| Value::String(content.to_string()));
    }

    // Lift "subagent_id: <id>" so it becomes structured.agentId.
    if let Some(child_id) = extract_subagent_id(content) {
        result.insert("agent_id".to_string(), Value::String(child_id));
    }

    // Edit / Write: promote path + old/new from the call arguments so Diff
    // presentation works even when chat_history only has a success string.
    if matches!(canonical_name, "Edit" | "Write")
        && let Some(input_str) = tool_input
        && let Ok(input) = serde_json::from_str::<Value>(input_str)
    {
        if result.get("file_path").is_none()
            && result.get("filePath").is_none()
            && let Some(path) = input
                .get("file_path")
                .or_else(|| input.get("filePath"))
                .or_else(|| input.get("path"))
                .cloned()
        {
            result.insert("file_path".into(), path);
        }
        if result.get("oldString").is_none()
            && result.get("old_string").is_none()
            && let (Some(old), Some(new)) = (
                input
                    .get("old_string")
                    .or_else(|| input.get("oldString"))
                    .cloned(),
                input
                    .get("new_string")
                    .or_else(|| input.get("newString"))
                    .cloned(),
            )
        {
            result.insert("oldString".into(), old);
            result.insert("newString".into(), new);
        }
        if result.get("content").is_none()
            && let Some(content_val) = input.get("content").cloned()
        {
            result.insert("content".into(), content_val);
        }
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

/// Tools whose dedicated renderer reads the structured output stream.
fn should_mirror_output_into_structured(canonical_name: &str) -> bool {
    canonical_name == "Bash"
}

/// Insert session notes (plan / goal / recap) into the message stream by
/// timestamp. Notes without a timestamp append at the end; messages without
/// a timestamp keep their relative order and stay before untimed notes.
fn interleave_session_notes(mut messages: Vec<Message>, notes: Vec<Message>) -> Vec<Message> {
    if notes.is_empty() {
        return messages;
    }
    // Stable merge: walk messages, insert notes whose timestamp is <= next
    // message timestamp. Untimed notes go last.
    let mut timed_notes: Vec<Message> = Vec::new();
    let mut untimed_notes: Vec<Message> = Vec::new();
    for note in notes {
        if note.timestamp.is_some() {
            timed_notes.push(note);
        } else {
            untimed_notes.push(note);
        }
    }
    if timed_notes.is_empty() {
        messages.extend(untimed_notes);
        return messages;
    }
    timed_notes.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    let mut out = Vec::with_capacity(messages.len() + timed_notes.len() + untimed_notes.len());
    let mut timed = timed_notes.into_iter().peekable();
    for msg in messages {
        // Untimed messages (`msg.timestamp == None`) never consume notes:
        // pending notes wait for the next timed message.
        if let Some(msg_ts) = msg.timestamp.as_deref() {
            while let Some(note) =
                timed.next_if(|note| note.timestamp.as_deref().unwrap_or("") <= msg_ts)
            {
                out.push(note);
            }
        }
        out.push(msg);
    }
    out.extend(timed);
    out.extend(untimed_notes);
    out
}
