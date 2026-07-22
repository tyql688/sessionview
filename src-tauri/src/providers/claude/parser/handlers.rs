//! Per-line / per-message-type handlers for the Claude JSONL scan loop.
//! Each handler folds one parsed record into the shared `ParseState`
//! (or `ScanAccum` for cross-line metadata like mode transitions).

use serde_json::Value;

use crate::models::{Message, MessageKind, MessageRole, Provider, TokenUsage};
use crate::provider::util::is_system_content;
use crate::tool_metadata::{
    ToolCallFacts, build_tool_metadata, canonical_tool_name, enrich_tool_metadata,
};

use super::super::images::{
    contains_image_placeholder_without_source, contains_image_source,
    merge_image_placeholders_with_sources,
};
use super::content::{
    extract_message_content, extract_token_usage, extract_tool_result_content,
    is_tool_result_message, tool_result_facts, unique_hash_from_entry,
};
use super::text_clean::{LocalCommandText, extract_teammate_mail, format_local_command_text};
use super::{ParseState, PendingToolResult, ScanAccum};

pub(super) fn preserves_pending_user_message(line_type: &str) -> bool {
    // `agent-name` is intentionally NOT listed here: the explicit
    // dispatch arm above already handles it without flushing, and
    // including it would be dead code (the fall-through never fires).
    matches!(
        line_type,
        "attachment"
            | "file-history-snapshot"
            | "permission-mode"
            | "progress"
            | "queue-operation"
            | "last-prompt"
    )
}

/// Surface mode transitions (`normal` ↔ `plan` ↔ `accept_edits` ↔
/// `bypass_permissions`) as `[mode] <value>` System messages. Claude
/// Code emits a `mode` line on every turn, so we deduplicate against
/// the last-seen value to avoid spamming the timeline.
pub(super) fn handle_mode(entry: &Value, accum: &mut ScanAccum, timestamp: Option<String>) {
    let Some(value) = entry
        .get("mode")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    if accum.last_mode.as_deref() == Some(value) {
        return;
    }
    accum.last_mode = Some(value.to_string());
    flush_pending(&mut accum.state);
    append_system_message(&mut accum.state, format!("[mode] {value}"), timestamp);
}

fn infer_tool_name_from_orphan_result(result: Option<&Value>, use_id: Option<&str>) -> String {
    let inferred = result.and_then(|r| {
        if r.get("commandName").is_some() && r.get("allowedTools").is_some() {
            return Some("SlashCommand");
        }
        if r.get("oldString").is_some() && r.get("newString").is_some() {
            return Some("Edit");
        }
        if r.get("stdout").is_some() || r.get("stderr").is_some() {
            return Some("Bash");
        }
        if r.get("matches").is_some() && r.get("total_deferred_tools").is_some() {
            return Some("ToolSearch");
        }
        if r.get("taskId").is_some() && r.get("updatedFields").is_some() {
            return Some("TaskUpdate");
        }
        if r.get("task").is_some() {
            return Some("TaskCreate");
        }
        if r.get("agentId").is_some() {
            return Some("Agent");
        }
        if r.get("url").is_some() && r.get("durationMs").is_some() {
            return Some("WebFetch");
        }
        if r.get("filePath").is_some() {
            return Some("Write");
        }
        None
    });
    match (inferred, use_id) {
        (Some(name), _) => {
            log::debug!(
                "Claude tool_result has no matching tool_use (use_id={use_id:?}); heuristic inferred name {name:?}"
            );
            name.to_string()
        }
        (None, Some(id)) => {
            log::debug!(
                "Claude tool_result has no matching tool_use; falling back to use_id={id:?}"
            );
            id.to_string()
        }
        (None, None) => {
            log::warn!(
                "Claude tool_result has no matching tool_use and no use_id — emitting as UnknownToolResult"
            );
            "UnknownToolResult".to_string()
        }
    }
}

fn is_image_scale_note(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("[Image: original ")
        && trimmed.contains("Multiply coordinates by")
        && trimmed.ends_with("to map to original image.]")
}

fn standalone_tool_result_message(
    result_text: String,
    is_raw: bool,
    result_item: &Value,
    top_level_result: Option<&Value>,
    use_id: Option<&str>,
    source_tool_assistant_uuid: Option<&str>,
    timestamp: Option<String>,
) -> Message {
    let raw_name = infer_tool_name_from_orphan_result(top_level_result, use_id);
    let canonical_name = canonical_tool_name(Provider::Claude, &raw_name);
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: &raw_name,
        input: None,
        call_id: use_id,
        assistant_id: source_tool_assistant_uuid,
    });
    enrich_tool_metadata(
        &mut metadata,
        tool_result_facts(result_item, top_level_result, is_raw),
    );

    Message {
        timestamp,
        tool_name: Some(canonical_name),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, result_text)
    }
}

pub(super) fn flush_pending_tool_results(state: &mut ParseState) {
    let pending = std::mem::take(&mut state.pending_tool_results_by_use_id);
    for (use_id, result) in pending {
        state.messages.push(standalone_tool_result_message(
            result.result_text,
            result.is_raw,
            &result.result_item,
            result.top_level_result.as_ref(),
            Some(&use_id),
            result.source_tool_assistant_uuid.as_deref(),
            result.timestamp,
        ));
    }
}

/// Handle a "user" line, which may be a real user message or a tool_result turn.
pub(super) fn handle_user_message(
    entry: &Value,
    state: &mut ParseState,
    timestamp: Option<String>,
) {
    let msg = match entry.get("message") {
        Some(m) => m,
        None => return,
    };

    // Check if this "user" entry is actually a tool_result
    // (the Anthropic API sends tool results as user-role turns)
    if is_tool_result_message(msg) {
        handle_tool_result(entry, msg, state, &timestamp);
        return;
    }

    let text = extract_message_content(msg);
    if text.trim().is_empty() {
        return;
    }
    // Compaction summaries arrive as user-role lines flagged `isCompactSummary`
    // — harness output, not typed input. Route them to the same collapsed
    // [context_compacted] row as Codex/Grok/Pi compaction markers.
    if entry
        .get("isCompactSummary")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        flush_pending(state);
        append_system_message(state, format!("[context_compacted]\n{text}"), timestamp);
        return;
    }
    // Teammate mail from another Claude session is likewise harness-injected
    // as a user turn; surface each block as an [agent_mail] system row.
    if let Some(mails) = extract_teammate_mail(&text) {
        flush_pending(state);
        for mail in mails {
            append_system_message(state, mail, timestamp.clone());
        }
        return;
    }
    if let Some(command) = format_local_command_text(&text) {
        flush_pending(state);
        append_local_command_message(state, command, timestamp);
        return;
    }
    let is_meta = entry
        .get("isMeta")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    // The harness injects a coordinate-scale note after every image it
    // downsizes ("[Image: original WxH, displayed at WxH. Multiply
    // coordinates ..."). It is model-facing plumbing, not user input — and
    // because it starts with "[Image" it would otherwise be mistaken for an
    // image placeholder awaiting its source and surface as a user bubble.
    if is_meta && is_image_scale_note(&text) {
        return;
    }

    if let Some((pending_text, pending_timestamp)) = state.pending_user_message.take() {
        if is_meta
            && contains_image_placeholder_without_source(&pending_text)
            && contains_image_source(&text)
        {
            append_user_message(
                &mut state.messages,
                &mut state.content_parts,
                &mut state.first_user_message,
                merge_image_placeholders_with_sources(&pending_text, &text),
                pending_timestamp,
            );
            return;
        }

        append_user_message(
            &mut state.messages,
            &mut state.content_parts,
            &mut state.first_user_message,
            pending_text,
            pending_timestamp,
        );
    }

    if contains_image_placeholder_without_source(&text) {
        state.pending_user_message = Some((text, timestamp));
        return;
    }

    if !is_meta || contains_image_source(&text) {
        append_user_message(
            &mut state.messages,
            &mut state.content_parts,
            &mut state.first_user_message,
            text,
            timestamp,
        );
    }
}

/// Merge tool_result blocks from a user-role turn into their matching tool_use messages.
fn handle_tool_result(
    entry: &Value,
    msg: &Value,
    state: &mut ParseState,
    timestamp: &Option<String>,
) {
    flush_pending(state);
    let top_level_result = entry.get("toolUseResult");
    // Merge each tool_result into its matching tool_use message
    if let Some(Value::Array(arr)) = msg.get("content") {
        for result_item in arr {
            if result_item.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            let rendered = extract_tool_result_content(result_item);
            if rendered.text.trim().is_empty() && top_level_result.is_none() {
                continue;
            }
            if !rendered.text.trim().is_empty() {
                state.content_parts.push(rendered.text.clone());
            }
            let use_id = result_item.get("tool_use_id").and_then(|i| i.as_str());
            if let Some(idx) = state.tool_use_id_map.index_of(use_id) {
                // Merge result into the existing tool_use message
                state.messages[idx].content = rendered.text;
                if let Some(metadata) = state.messages[idx].tool_metadata.as_mut() {
                    enrich_tool_metadata(
                        metadata,
                        tool_result_facts(result_item, top_level_result, rendered.is_raw),
                    );
                }
            } else if let Some(idx) = entry
                .get("sourceToolAssistantUUID")
                .and_then(|id| id.as_str())
                .and_then(|uuid| state.assistant_tool_indices_by_uuid.get(uuid))
                .and_then(|indices| {
                    if indices.len() == 1 {
                        indices.first().copied()
                    } else {
                        None
                    }
                })
            {
                state.messages[idx].content = rendered.text;
                if let Some(metadata) = state.messages[idx].tool_metadata.as_mut() {
                    enrich_tool_metadata(
                        metadata,
                        tool_result_facts(result_item, top_level_result, rendered.is_raw),
                    );
                }
            } else {
                if let Some(use_id) = use_id {
                    state.pending_tool_results_by_use_id.insert(
                        use_id.to_string(),
                        PendingToolResult {
                            result_text: rendered.text,
                            is_raw: rendered.is_raw,
                            result_item: result_item.clone(),
                            top_level_result: top_level_result.cloned(),
                            timestamp: timestamp.clone(),
                            source_tool_assistant_uuid: entry
                                .get("sourceToolAssistantUUID")
                                .and_then(|id| id.as_str())
                                .map(str::to_string),
                        },
                    );
                    continue;
                }

                // No matching tool_use found -- emit as standalone
                state.messages.push(standalone_tool_result_message(
                    rendered.text,
                    rendered.is_raw,
                    result_item,
                    top_level_result,
                    None,
                    entry
                        .get("sourceToolAssistantUUID")
                        .and_then(|id| id.as_str()),
                    timestamp.clone(),
                ));
            }
        }
    }
}

/// Handle an "assistant" line: split content into text, thinking, and tool_use messages.
pub(super) fn handle_assistant_message(
    entry: &Value,
    state: &mut ParseState,
    timestamp: Option<String>,
) {
    flush_pending(state);
    let msg = match entry.get("message") {
        Some(m) => m,
        None => return,
    };

    // Extract per-message model
    let per_message_model = msg
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());

    // Extract token usage for this assistant turn
    let turn_usage = extract_token_usage(msg);
    let turn_start = state.messages.len();
    let mut tool_indices = Vec::new();

    // Split assistant messages: text parts as assistant, tool_use as tool
    if let Some(Value::Array(arr)) = msg.get("content") {
        let mut text_parts = Vec::new();
        for item in arr {
            let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match item_type {
                "thinking" => {
                    if let Some(t) = item.get("thinking").and_then(|t| t.as_str())
                        && !t.trim().is_empty()
                    {
                        // Emit thinking as a separate assistant message with marker
                        state.messages.push(Message {
                            timestamp: timestamp.clone(),
                            ..Message::system(format!("[thinking]\n{t}"))
                        });
                    }
                }
                "text" => {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str())
                        && !t.trim().is_empty()
                    {
                        text_parts.push(t.to_string());
                    }
                }
                "tool_use" => {
                    // Flush accumulated text as assistant message
                    if !text_parts.is_empty() {
                        let text = text_parts.join("\n");
                        state.content_parts.push(text.clone());
                        state.messages.push(Message {
                            timestamp: timestamp.clone(),
                            model: per_message_model.clone(),
                            ..Message::assistant(text)
                        });
                        text_parts.clear();
                    }
                    // Emit tool_use as a Tool message
                    let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                    let use_id = item.get("id").and_then(|i| i.as_str());
                    let metadata = build_tool_metadata(ToolCallFacts {
                        provider: Provider::Claude,
                        raw_name: name,
                        input: item.get("input"),
                        call_id: use_id,
                        assistant_id: entry.get("uuid").and_then(|u| u.as_str()),
                    });
                    let canonical_name = metadata.canonical_name.clone();
                    let input = item.get("input").map(std::string::ToString::to_string);
                    let msg_idx = state.messages.len();
                    state.messages.push(Message {
                        timestamp: timestamp.clone(),
                        tool_name: Some(canonical_name),
                        tool_input: input,
                        tool_metadata: Some(metadata),
                        ..Message::new(MessageRole::Tool, String::new())
                    });
                    tool_indices.push(msg_idx);
                    // Record tool_use_id for merging results later
                    if let Some(id) = use_id {
                        state.tool_use_id_map.register(Some(id), msg_idx);
                        if let Some(pending) = state.pending_tool_results_by_use_id.remove(id) {
                            state.messages[msg_idx].content = pending.result_text;
                            if let Some(metadata) = state.messages[msg_idx].tool_metadata.as_mut() {
                                enrich_tool_metadata(
                                    metadata,
                                    tool_result_facts(
                                        &pending.result_item,
                                        pending.top_level_result.as_ref(),
                                        pending.is_raw,
                                    ),
                                );
                            }
                        }
                    }
                }
                "fallback" => {
                    let from = item.pointer("/from/model").and_then(Value::as_str);
                    let to = item.pointer("/to/model").and_then(Value::as_str);
                    let (Some(from), Some(to)) = (from, to) else {
                        log::warn!("Claude fallback block missing from/to models");
                        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                        continue;
                    };
                    state.messages.push(Message {
                        timestamp: timestamp.clone(),
                        ..Message::system(format!("[model_fallback] {from} \u{2192} {to}"))
                    });
                }
                "redacted_thinking" => {
                    state.messages.push(Message {
                        timestamp: timestamp.clone(),
                        ..Message::system("[thinking]\n(redacted)".to_string())
                    });
                }
                unknown => {
                    log::warn!("skipping unknown Claude assistant content block '{unknown}'");
                    state.parse_warning_count = state.parse_warning_count.saturating_add(1);
                }
            }
        }
        if let Some(uuid) = entry.get("uuid").and_then(|u| u.as_str())
            && !tool_indices.is_empty()
        {
            state
                .assistant_tool_indices_by_uuid
                .insert(uuid.to_string(), tool_indices);
        }
        // Flush remaining text
        if !text_parts.is_empty() {
            let text = text_parts.join("\n");
            state.content_parts.push(text.clone());
            state.messages.push(Message {
                timestamp: timestamp.clone(),
                model: per_message_model.clone(),
                ..Message::assistant(text)
            });
        }
    } else {
        // content is a plain string
        let text = extract_message_content(msg);
        if !text.trim().is_empty() {
            state.content_parts.push(text.clone());
            state.messages.push(Message {
                timestamp: timestamp.clone(),
                model: per_message_model.clone(),
                ..Message::assistant(text)
            });
        }
    }

    // Attach token usage + dedup hash to the last assistant/tool message of this turn.
    attach_turn_usage(
        entry,
        state,
        turn_usage,
        turn_start,
        per_message_model,
        timestamp,
    );
}

/// Attach token usage + dedup hash to the last assistant/tool message of this turn.
/// When the turn produced only thinking (System) or empty content, insert a
/// minimal placeholder so the usage is never silently dropped.
///
/// Tool messages (and the empty placeholder below) carry model=None by
/// design, so we always force the usage-bearing message's model and
/// timestamp to the assistant entry's values. Without this, usage attached
/// to a tool message is dropped later by `compute_token_stats_dedup`'s
/// "missing model" filter.
fn attach_turn_usage(
    entry: &Value,
    state: &mut ParseState,
    turn_usage: Option<TokenUsage>,
    turn_start: usize,
    per_message_model: Option<String>,
    timestamp: Option<String>,
) {
    if let Some(usage) = turn_usage {
        let hash = unique_hash_from_entry(entry);
        if let Some(last_msg) = state.messages[turn_start..]
            .iter_mut()
            .filter(|m| m.role != MessageRole::System)
            .last()
        {
            last_msg.token_usage = Some(usage);
            last_msg.usage_hash = hash;
            if last_msg.model.as_deref().map(str::is_empty).unwrap_or(true) {
                last_msg.model = per_message_model.clone();
            }
            if last_msg.timestamp.is_none() {
                last_msg.timestamp = timestamp.clone();
            }
        } else {
            state.messages.push(Message {
                timestamp: timestamp.clone(),
                token_usage: Some(usage),
                model: per_message_model.clone(),
                usage_hash: hash,
                ..Message::assistant(String::new())
            });
        }
    }
}

/// Handle a "summary" line: capture the first non-empty summary text.
pub(super) fn handle_summary(
    entry: &Value,
    summary_text: &mut Option<String>,
    state: &mut ParseState,
) {
    if summary_text.is_none()
        && let Some(s) = entry.get("summary").and_then(|s| s.as_str())
        && !s.trim().is_empty()
    {
        *summary_text = Some(s.to_string());
    }
    flush_pending(state);
}

/// Handle a "system" line: emit human-readable summaries of system subtypes.
pub(super) fn handle_system_message(
    entry: &Value,
    state: &mut ParseState,
    timestamp: Option<String>,
) {
    flush_pending(state);

    let subtype = match entry.get("subtype").and_then(|s| s.as_str()) {
        Some(s) => s,
        None => return,
    };

    let content = match subtype {
        "turn_duration" => {
            let duration_ms = entry
                .get("durationMs")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let message_count = entry
                .get("messageCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!(
                "[turn_duration] {:.1}s, {} messages",
                duration_ms / 1000.0,
                message_count
            )
        }
        "compact_boundary" => {
            let pre_tokens = entry
                .get("compactMetadata")
                .and_then(|m| m.get("preTokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if pre_tokens < 1000 {
                format!("[compact_boundary] {} tokens", pre_tokens)
            } else {
                format!(
                    "[compact_boundary] {:.1}k tokens",
                    pre_tokens as f64 / 1000.0
                )
            }
        }
        "microcompact_boundary" => {
            let metadata = entry.get("microcompactMetadata");
            let pre_tokens = metadata
                .and_then(|m| m.get("preTokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let tokens_saved = metadata
                .and_then(|m| m.get("tokensSaved"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!(
                "[microcompact_boundary] {:.1}k tokens saved {:.1}k",
                pre_tokens as f64 / 1000.0,
                tokens_saved as f64 / 1000.0
            )
        }
        "stop_hook_summary" => {
            let hook_count = entry.get("hookCount").and_then(|v| v.as_u64()).unwrap_or(0);
            let hook_details: Vec<String> = entry
                .get("hookInfos")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|h| {
                            let cmd = h
                                .get("command")
                                .and_then(|c| c.as_str())
                                .unwrap_or("unknown");
                            let ms = h.get("durationMs").and_then(|d| d.as_u64()).unwrap_or(0);
                            format!("{cmd} ({ms}ms)")
                        })
                        .collect()
                })
                .unwrap_or_default();
            format!(
                "[stop_hook_summary] {} hooks: {}",
                hook_count,
                hook_details.join(", ")
            )
        }
        "api_error" => {
            let code = entry
                .get("cause")
                .and_then(|c| c.get("code"))
                .and_then(|c| c.as_str())
                .unwrap_or("Unknown");
            let retry = entry
                .get("retryAttempt")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let max_retries = entry
                .get("maxRetries")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            format!("[api_error] {code} (retry {retry}/{max_retries})")
        }
        "away_summary" => entry
            .get("content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|content| format!("[away_summary] {content}"))
            .unwrap_or_else(|| "[away_summary]".to_string()),
        "scheduled_task_fire" => entry
            .get("content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|content| format!("[scheduled_task_fire] {content}"))
            .unwrap_or_else(|| "[scheduled_task_fire]".to_string()),
        "local_command" => {
            let Some(command) = entry
                .get("content")
                .and_then(|v| v.as_str())
                .and_then(format_local_command_text)
            else {
                return;
            };
            append_local_command_message(state, command, timestamp);
            return;
        }
        "informational" => {
            let Some(content) = entry
                .get("content")
                .and_then(|v| v.as_str())
                .map(super::text_clean::clean_system_text)
                .filter(|s| !s.is_empty())
            else {
                return;
            };
            format!("[informational] {content}")
        }
        _ => return,
    };

    append_system_message(state, content, timestamp);
}

fn append_system_message(state: &mut ParseState, content: String, timestamp: Option<String>) {
    state.messages.push(Message {
        timestamp,
        ..Message::system(content)
    });
}

fn append_local_command_message(
    state: &mut ParseState,
    command: LocalCommandText,
    timestamp: Option<String>,
) {
    let message = match command.kind {
        MessageKind::CommandInput => Message::command_input(command.content),
        MessageKind::CommandOutput => Message::command_output(command.content),
    };
    state.messages.push(Message {
        timestamp,
        ..message
    });
}

pub(super) fn handle_pr_link(entry: &Value, state: &mut ParseState, timestamp: Option<String>) {
    let pr_url = entry
        .get("prUrl")
        .or_else(|| entry.get("pr_url"))
        .or_else(|| entry.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if pr_url.is_empty() {
        return;
    }

    let label = entry
        .get("prNumber")
        .or_else(|| entry.get("number"))
        .and_then(|v| v.as_u64())
        .map(|number| format!("PR #{number}"))
        .unwrap_or_else(|| "PR".to_string());

    state.messages.push(Message {
        timestamp,
        ..Message::system(format!("[pr_link] {label}: {pr_url}"))
    });
}

/// Flush any pending user message that was waiting for an image-source merge.
pub(super) fn flush_pending(state: &mut ParseState) {
    if let Some((text, timestamp)) = state.pending_user_message.take() {
        append_user_message(
            &mut state.messages,
            &mut state.content_parts,
            &mut state.first_user_message,
            text,
            timestamp,
        );
    }
}

fn append_user_message(
    messages: &mut Vec<Message>,
    content_parts: &mut Vec<String>,
    first_user_message: &mut Option<String>,
    text: String,
    timestamp: Option<String>,
) {
    if text.trim().is_empty() {
        return;
    }

    let trimmed = text.trim_start();
    if is_system_content(trimmed) {
        return;
    }

    if first_user_message.is_none() {
        *first_user_message = Some(text.clone());
    }

    content_parts.push(text.clone());
    messages.push(Message {
        timestamp,
        ..Message::user(text)
    });
}
