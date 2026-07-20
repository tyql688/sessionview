use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::models::{Message, Provider, SessionMeta, token_totals_from_messages};
use crate::provider::ParsedSession;
use crate::provider_utils::{parse_rfc3339_timestamp, project_name_from_path, session_title};
use crate::services::tail_reader::open_tail_reader;

mod lenient_json;
mod steps;
mod workspace;

use steps::AntigravityScanAccum;
use workspace::{find_workspace_by_display_content, load_history_workspaces};

#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    pub step_index: u64,
    pub source: String,
    #[serde(rename = "type")]
    pub step_type: String,
    pub status: String,
    pub created_at: String,
    pub content: Option<String>,
    pub thinking: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub args: Option<Value>,
}

pub fn parse_session_file(path: &Path) -> Option<ParsedSession> {
    let conversation_id = path
        .parent() // logs/
        .and_then(|p| p.parent()) // .system_generated/
        .and_then(|p| p.parent()) // {conversation_id}/
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())?
        .to_string();

    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut accum = AntigravityScanAccum::new();

    scan_antigravity_lines(reader, path, &conversation_id, &mut accum);

    if accum.messages.is_empty() {
        return None;
    }

    let history_workspaces = load_history_workspaces();
    let mut project_path = history_workspaces
        .get(&conversation_id)
        .cloned()
        .or_else(|| {
            accum
                .first_user_msg
                .as_ref()
                .and_then(|msg| find_workspace_by_display_content(msg))
        })
        .or_else(|| accum.invoke_workspace.clone())
        .unwrap_or_default();

    if project_path.is_empty() {
        let known_workspaces: Vec<String> = history_workspaces.values().cloned().collect();
        for p in &accum.candidate_paths {
            for ws in &known_workspaces {
                if p.starts_with(ws) {
                    project_path = ws.clone();
                    break;
                }
            }
            if !project_path.is_empty() {
                break;
            }
        }
    }

    let project_name = if project_path.is_empty() {
        "Unknown Project".to_string()
    } else {
        project_name_from_path(&project_path)
    };

    let (file_size_bytes, source_mtime) = std::fs::metadata(path)
        .map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(crate::provider::system_time_to_epoch_seconds)
                .unwrap_or(0);
            (m.len(), mtime)
        })
        .unwrap_or((0, 0));
    let message_count = accum.messages.len() as u32;

    let mut content_text = String::new();
    for msg in &accum.messages {
        content_text.push_str(&msg.content);
        content_text.push(' ');
    }

    let created_at = parse_rfc3339_timestamp(accum.first_timestamp.as_deref());
    let updated_at = parse_rfc3339_timestamp(accum.last_timestamp.as_deref());

    let totals = token_totals_from_messages(&accum.messages);

    let is_sidechain = accum.parent_from_send.is_some();

    let meta = SessionMeta {
        id: conversation_id,
        provider: Provider::Antigravity,
        title: session_title(accum.first_user_msg.as_deref()),
        project_path,
        project_name,
        created_at,
        updated_at,
        message_count,
        file_size_bytes,
        source_path: path.to_string_lossy().to_string(),
        is_sidechain,
        variant_name: None,
        model: accum.current_model,
        cc_version: None,
        git_branch: None,
        parent_id: accum.parent_from_send,
        input_tokens: totals.input_tokens,
        output_tokens: totals.output_tokens,
        cache_read_tokens: totals.cache_read_tokens,
        cache_write_tokens: totals.cache_write_tokens,
    };

    Some(ParsedSession {
        meta,
        messages: accum.messages,
        content_text,
        parse_warning_count: accum.parse_warning_count,
        child_session_ids: accum.child_session_ids,
        usage_events: Vec::new(),
        source_mtime,
    })
}

/// Read JSONL lines from `reader`, parse each as `Step`, and dispatch
/// into `accum`. Shared between full-file and tail-only parsing.
fn scan_antigravity_lines<R: BufRead>(
    reader: R,
    path: &Path,
    conversation_id: &str,
    accum: &mut AntigravityScanAccum,
) {
    let stats = crate::provider_utils::for_each_jsonl_record(reader, path, |_, step: Step| {
        accum.process_step(&step, conversation_id);
        std::ops::ControlFlow::Continue(())
    });
    accum.parse_warning_count += stats.read_error_count + stats.parse_error_count;
}

/// Tail-only parse result. Mirrors `ClaudeTailResult` / `CodexTailResult`:
/// just the trailing messages + the parse-warning count needed by
/// `try_tail_fast_path` to build a `SessionMessagesWindow`.
pub struct AntigravityTailResult {
    pub messages: Vec<Message>,
    pub parse_warning_count: u32,
}

/// Parse only the tail of an Antigravity transcript — the last
/// `target_messages` (or so) emitted messages — by mmap'ing the file
/// and seeking the BufReader past the byte offset of the first line we
/// care about.
///
/// Trade-offs vs the full-file parser:
/// - **Tool merging is best-effort at the boundary.** A tool-result step
///   in the tail whose matching tool_call was earlier in the file
///   surfaces as a standalone (unmerged) tool message. The background
///   full-parse promote replaces the cache with the merged version
///   once it completes.
/// - **No project_path / parent_id derivation.** The caller already has
///   `SessionMeta` from the DB; this function returns only the message
///   slice + parse warnings. INVOKE_SUBAGENT steps almost always live
///   near the top of the parent file, so the tail wouldn't see them
///   anyway.
/// - **Token estimates are undercounted near the boundary** because
///   `context_chars` starts at 0 instead of including everything before
///   the tail window. Acceptable for display; the indexer's full parse
///   computes authoritative totals.
pub fn parse_session_tail(path: &Path, target_messages: usize) -> Option<AntigravityTailResult> {
    let conversation_id = path
        .parent() // logs/
        .and_then(|p| p.parent()) // .system_generated/
        .and_then(|p| p.parent()) // {conversation_id}/
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())?
        .to_string();

    // Antigravity steps emit ~1-3 messages on average (USER_INPUT = 1,
    // PLANNER_RESPONSE can produce thinking + assistant + N tool calls).
    // Over-scan generously so the tool_call ↔ tool_result pairing has a
    // good chance of landing fully inside the parsed range.
    let safety_buffer = target_messages / 2 + 100;
    let scan_lines = target_messages.saturating_add(safety_buffer);
    let (reader, _window) = open_tail_reader(path, scan_lines, "Antigravity")?;

    let mut accum = AntigravityScanAccum::new();
    scan_antigravity_lines(reader, path, &conversation_id, &mut accum);

    if accum.messages.is_empty() {
        log::debug!(
            "Antigravity tail parse produced no messages for '{}'; falling back to full parse",
            path.display()
        );
        return None;
    }

    // Trim to exactly `target_messages` — we over-scan for tool-pair
    // merging at the boundary, but the caller asked for a specific window.
    let len = accum.messages.len();
    if len > target_messages {
        accum.messages.drain(0..(len - target_messages));
    }

    Some(AntigravityTailResult {
        messages: accum.messages,
        parse_warning_count: accum.parse_warning_count,
    })
}

#[cfg(test)]
mod tests;
