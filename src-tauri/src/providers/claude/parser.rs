use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::services::tail_reader::open_tail_reader;

use crate::models::{Message, Provider};
use crate::provider::ParsedSession;
use crate::provider_utils::{
    parse_rfc3339_timestamp, project_name_from_path, session_title, subagents_ancestor,
};

mod content;
mod handlers;
mod text_clean;

use content::dedup_hash_from_entry;
use handlers::{
    flush_pending, flush_pending_tool_results, handle_assistant_message, handle_mode,
    handle_pr_link, handle_summary, handle_system_message, handle_user_message,
    preserves_pending_user_message,
};

// Re-export the one public content helper so existing
// `parser::resolve_persisted_outputs` callers keep compiling.
pub use content::resolve_persisted_outputs;

/// Shared mutable state threaded through the per-message-type handlers.
struct ParseState {
    messages: Vec<Message>,
    content_parts: Vec<String>,
    first_user_message: Option<String>,
    pending_user_message: Option<(String, Option<String>)>,
    tool_use_id_map: crate::provider_utils::ToolCallPairer,
    assistant_tool_indices_by_uuid: HashMap<String, Vec<usize>>,
    pending_tool_results_by_use_id: HashMap<String, PendingToolResult>,
    /// Count of per-line parse warnings: malformed JSONL lines or JSON fields
    /// the parser had to skip to keep the rest of the session usable. File-
    /// level failures are surfaced through `load_messages` as `ProviderError::Parse`
    /// instead; this only tracks recoverable, line-scoped issues.
    parse_warning_count: u32,
}

struct PendingToolResult {
    result_text: String,
    is_raw: bool,
    result_item: Value,
    top_level_result: Option<Value>,
    timestamp: Option<String>,
    source_tool_assistant_uuid: Option<String>,
}

// `ParseState`/`PendingToolResult`/`ScanAccum` stay module-private here. The
// `handlers` and `content` submodules reach them (and their fields) via
// `use super::{...}` — a child module may access private items of an ancestor.

/// Per-scan accumulator shared between the full-file and tail-only
/// entry points. Bundles the cross-line state the dispatch loop walks
/// (parsed messages + the various "first occurrence" metadata fields)
/// so we don't have to duplicate the body of the for-loop in both
/// callers.
struct ScanAccum {
    state: ParseState,
    summary_text: Option<String>,
    custom_title: Option<String>,
    ai_title: Option<String>,
    agent_name: Option<String>,
    first_timestamp: Option<String>,
    last_timestamp: Option<String>,
    cwd: Option<String>,
    is_sidechain: bool,
    model: Option<String>,
    cc_version: Option<String>,
    git_branch: Option<String>,
    /// Last `mode` value seen — emitted once per change as a System
    /// `[mode] <value>` event so plan/accept_edits/normal transitions
    /// are visible in the timeline without spamming an entry per turn.
    /// Seeded with `"normal"` so the implicit-default starting mode
    /// doesn't generate a noisy `[mode] normal` leader on every session
    /// (Claude Code emits `mode: normal` on essentially every turn).
    last_mode: Option<String>,
    processed_hashes: HashSet<String>,
}

impl ScanAccum {
    fn new() -> Self {
        Self {
            state: ParseState {
                messages: Vec::new(),
                content_parts: Vec::new(),
                first_user_message: None,
                pending_user_message: None,
                tool_use_id_map: crate::provider_utils::ToolCallPairer::default(),
                assistant_tool_indices_by_uuid: HashMap::new(),
                pending_tool_results_by_use_id: HashMap::new(),
                parse_warning_count: 0,
            },
            summary_text: None,
            custom_title: None,
            ai_title: None,
            agent_name: None,
            first_timestamp: None,
            last_timestamp: None,
            cwd: None,
            is_sidechain: false,
            model: None,
            cc_version: None,
            git_branch: None,
            last_mode: Some("normal".to_string()),
            processed_hashes: HashSet::new(),
        }
    }
}

/// Returned by `scan_jsonl_lines` so the caller can distinguish "parse
/// finished" from "user navigated away mid-parse".
enum ScanOutcome {
    Completed,
    Canceled,
}

/// Walk a JSONL stream line by line, dispatching each record to the
/// matching per-type handler and folding the result into `accum`. The
/// caller owns I/O setup (file open + optional seek) and final
/// `flush_pending*` calls so both the full-file and tail-only parsers
/// can reuse the same dispatch logic.
fn scan_jsonl_lines<R: BufRead>(reader: R, path: &Path, accum: &mut ScanAccum) -> ScanOutcome {
    let stats =
        crate::provider_utils::for_each_jsonl_record(reader, path, |line_no, entry: Value| {
            // Cooperative cancellation: bail out fast when the user navigated
            // away mid-load. Checked every 1024 lines so the polling cost is
            // negligible for normal-size sessions.
            if line_no.is_multiple_of(1024) && crate::services::load_cancel::is_canceled() {
                log::debug!(
                    "Claude parse canceled at line {line_no} of '{}'",
                    path.display()
                );
                return ControlFlow::Break(());
            }

            if let Some(dedup_hash) = dedup_hash_from_entry(&entry)
                && !accum.processed_hashes.insert(dedup_hash)
            {
                return ControlFlow::Continue(());
            }

            let line_type = match entry.get("type").and_then(|t| t.as_str()) {
                Some(t) => t.to_string(),
                None => return ControlFlow::Continue(()),
            };

            if accum.cwd.is_none()
                && let Some(c) = entry.get("cwd").and_then(|c| c.as_str())
                && !c.is_empty()
            {
                accum.cwd = Some(c.to_string());
            }

            if !accum.is_sidechain
                && entry
                    .get("isSidechain")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            {
                accum.is_sidechain = true;
            }

            if accum.cc_version.is_none()
                && let Some(v) = entry.get("version").and_then(|v| v.as_str())
                && !v.is_empty()
            {
                accum.cc_version = Some(v.to_string());
            }

            if accum.git_branch.is_none()
                && let Some(b) = entry.get("gitBranch").and_then(|b| b.as_str())
                && !b.is_empty()
                && b != "HEAD"
            {
                accum.git_branch = Some(b.to_string());
            }

            if let Some(ts) = entry.get("timestamp").and_then(|t| t.as_str()) {
                if accum.first_timestamp.is_none() {
                    accum.first_timestamp = Some(ts.to_string());
                }
                accum.last_timestamp = Some(ts.to_string());
            }

            let timestamp = entry
                .get("timestamp")
                .and_then(|t| t.as_str())
                .map(std::string::ToString::to_string);

            match line_type.as_str() {
                "user" => {
                    handle_user_message(&entry, &mut accum.state, timestamp);
                }
                "assistant" => {
                    if accum.model.is_none()
                        && let Some(m) = entry
                            .get("message")
                            .and_then(|msg| msg.get("model"))
                            .and_then(|m| m.as_str())
                        && !m.is_empty()
                    {
                        accum.model = Some(m.to_string());
                    }
                    handle_assistant_message(&entry, &mut accum.state, timestamp);
                }
                "summary" => {
                    handle_summary(&entry, &mut accum.summary_text, &mut accum.state);
                }
                "system" => {
                    handle_system_message(&entry, &mut accum.state, timestamp);
                }
                // Current Claude Code writes `customTitle` / `aiTitle`;
                // `title` is kept as a legacy fallback key.
                "custom-title" => {
                    flush_pending(&mut accum.state);
                    if let Some(t) = entry
                        .get("customTitle")
                        .or_else(|| entry.get("title"))
                        .and_then(|t| t.as_str())
                        && !t.trim().is_empty()
                    {
                        accum.custom_title = Some(t.to_string());
                    }
                }
                "ai-title" => {
                    flush_pending(&mut accum.state);
                    if let Some(t) = entry
                        .get("aiTitle")
                        .or_else(|| entry.get("title"))
                        .and_then(|t| t.as_str())
                        && !t.trim().is_empty()
                    {
                        accum.ai_title = Some(t.to_string());
                    }
                }
                "agent-name" => {
                    if let Some(name) = entry
                        .get("agentName")
                        .or_else(|| entry.get("name"))
                        .or_else(|| entry.get("title"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                    {
                        accum.agent_name = Some(name.to_string());
                    }
                }
                "pr-link" => {
                    flush_pending(&mut accum.state);
                    handle_pr_link(&entry, &mut accum.state, timestamp);
                }
                "mode" => {
                    handle_mode(&entry, accum, timestamp);
                }
                _ => {
                    if !preserves_pending_user_message(line_type.as_str()) {
                        flush_pending(&mut accum.state);
                    }
                }
            }
            ControlFlow::Continue(())
        });

    accum.state.parse_warning_count = accum
        .state
        .parse_warning_count
        .saturating_add(stats.parse_error_count);
    if stats.stopped_early {
        ScanOutcome::Canceled
    } else {
        ScanOutcome::Completed
    }
}

/// Extract parent session ID from subagent path.
/// Plain subagent: .../{parent_session_id}/subagents/agent-{agentId}.jsonl
/// Workflow agent: .../{parent_session_id}/subagents/workflows/wf_{id}/agent-{agentId}.jsonl
fn parent_id_from_path(path: &Path) -> Option<String> {
    let session_dir = subagents_ancestor(path)?.parent()?; // {parent_session_id}/
    Some(session_dir.file_name()?.to_str()?.to_string())
}

pub fn parse_session_file(path: &PathBuf) -> Option<ParsedSession> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "failed to open Claude session '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            log::warn!(
                "failed to read Claude session metadata '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let file_size = metadata.len();

    let reader = BufReader::new(file);
    let mut accum = ScanAccum::new();
    let parent_id = parent_id_from_path(path);
    let subagent_title = parent_id.as_ref().and_then(|_| {
        let meta_path = path.with_extension("meta.json");
        if !meta_path.exists() {
            return None;
        }
        let meta_content = match fs::read_to_string(&meta_path) {
            Ok(content) => content,
            Err(error) => {
                log::warn!(
                    "failed to read Claude subagent meta '{}': {error}",
                    meta_path.display()
                );
                return None;
            }
        };
        let meta_json: Value = match serde_json::from_str(&meta_content) {
            Ok(json) => json,
            Err(error) => {
                log::warn!(
                    "failed to parse Claude subagent meta '{}': {error}",
                    meta_path.display()
                );
                return None;
            }
        };
        meta_json
            .get("description")
            .or_else(|| meta_json.get("agentType"))
            .and_then(|d| d.as_str())
            .map(|s| s.to_string())
    });
    match scan_jsonl_lines(reader, path, &mut accum) {
        ScanOutcome::Completed => {}
        ScanOutcome::Canceled => return None,
    }

    flush_pending(&mut accum.state);
    flush_pending_tool_results(&mut accum.state);

    // Hoist accumulator fields out so the rest of the function reads
    // exactly like the pre-refactor code did.
    let state = accum.state;
    let summary_text = accum.summary_text;
    let custom_title = accum.custom_title;
    let ai_title = accum.ai_title;
    let agent_name = accum.agent_name;
    let first_timestamp = accum.first_timestamp;
    let last_timestamp = accum.last_timestamp;
    let cwd = accum.cwd;
    // Subagent files detected by path are always sidechains.
    let is_sidechain = accum.is_sidechain || parent_id.is_some();
    let model = accum.model;
    let cc_version = accum.cc_version;
    let git_branch = accum.git_branch;

    if state.messages.is_empty() {
        return None;
    }

    let session_id = path.file_stem()?.to_string_lossy().to_string();

    // Subagents inherit project_path from parent session's cwd (first entry in parent JSONL).
    // Their own cwd may differ (e.g. subagent ran in src-tauri/ subfolder).
    // We derive the parent's project path from the file system path instead.
    let project_path = if let Some(parent_id) = parent_id.as_ref() {
        // Path: .../{project_dir}/{parent_id}/subagents/.../agent-xxx.jsonl
        // Parent JSONL: .../{project_dir}/{parent_id}.jsonl
        // We need the project_dir's cwd, which we can't get here.
        // But the parent session's project_path is stored by its own cwd.
        // Best effort: walk up to the project directory and read the parent session's cwd.
        subagents_ancestor(path)
            .and_then(|p| p.parent()) // {parent_id}/
            .and_then(|p| p.parent()) // {project_dir}/
            .and_then(|project_dir| {
                // Read parent session to find first line with cwd
                // (first line may be file-history-snapshot without cwd)
                let parent_jsonl = project_dir.join(format!("{parent_id}.jsonl"));
                let file = match std::fs::File::open(&parent_jsonl) {
                    Ok(file) => file,
                    Err(error) => {
                        log::warn!(
                            "failed to open Claude parent transcript '{}': {error}",
                            parent_jsonl.display()
                        );
                        return None;
                    }
                };
                let reader = std::io::BufReader::new(file);
                use std::io::BufRead;
                for line in reader.lines().take(10) {
                    let line = match line {
                        Ok(line) => line,
                        Err(error) => {
                            log::warn!(
                                "failed to read Claude parent transcript line from '{}': {error}",
                                parent_jsonl.display()
                            );
                            continue;
                        }
                    };
                    match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(entry) => {
                            if let Some(c) = entry.get("cwd").and_then(|c| c.as_str())
                                && !c.is_empty()
                            {
                                return Some(c.to_string());
                            }
                        }
                        Err(error) => {
                            log::warn!(
                                "failed to parse Claude parent transcript line in '{}': {error}",
                                parent_jsonl.display()
                            );
                        }
                    }
                }
                None
            })
            .or(cwd)
            .unwrap_or_default()
    } else {
        cwd.unwrap_or_default()
    };
    let project_name = project_name_from_path(&project_path);

    let created_at = parse_rfc3339_timestamp(first_timestamp.as_deref());

    let updated_at = parse_rfc3339_timestamp(last_timestamp.as_deref());

    let content_text = state.content_parts.join("\n");

    let title = custom_title
        .or(ai_title)
        .or(subagent_title)
        .or(agent_name)
        .unwrap_or_else(|| {
            session_title(
                state
                    .first_user_message
                    .as_deref()
                    .or(summary_text.as_deref()),
            )
        });

    let meta = crate::models::SessionMeta {
        id: session_id,
        provider: Provider::Claude,
        title,
        project_path,
        project_name,
        created_at,
        updated_at,
        message_count: state.messages.len() as u32,
        file_size_bytes: file_size,
        source_path: path.to_string_lossy().to_string(),
        is_sidechain,
        variant_name: None,
        model,
        cc_version,
        git_branch,
        parent_id,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };

    let source_mtime = metadata
        .modified()
        .ok()
        .and_then(crate::provider::system_time_to_epoch_seconds)
        .unwrap_or(0);

    Some(ParsedSession {
        meta,
        messages: state.messages,
        content_text,
        parse_warning_count: state.parse_warning_count,
        child_session_ids: Vec::new(),
        usage_events: Vec::new(),
        source_mtime,
    })
}

/// Tail-only parse result. Carries the most recent N messages plus the
/// metadata bits the caller needs to assemble a `SessionMessagesWindow`
/// without re-parsing the whole file.
pub struct ClaudeTailResult {
    pub messages: Vec<Message>,
    pub parse_warning_count: u32,
    pub last_timestamp: Option<String>,
}

/// Parse only the tail of a Claude session file — the last
/// `target_messages` (or so) emitted messages — by mmap'ing the file
/// and seeking the BufReader past the byte offset of the first line
/// we care about.
///
/// Trade-offs vs the full-file parser:
/// - **Cross-line tool merging is best-effort.** A `tool_result` line
///   in the tail whose matching `tool_use` was earlier in the file
///   surfaces as a standalone (unmerged) tool message. The background
///   full-parse promote pass replaces the cache with the merged version
///   once it completes, so the imperfect tail is short-lived.
/// - **No title / project_path / metadata computation.** The caller
///   already has `SessionMeta` from the DB; this function returns only
///   the message slice + parse warnings.
/// - **No cancellation check inside the byte-offset scan.** mmap walk
///   is O(small constant) — usually a few hundred KB of trailing bytes
///   — so there's nothing meaningful to cancel before the dispatch
///   loop is entered.
pub(crate) fn parse_session_tail(path: &Path, target_messages: usize) -> Option<ClaudeTailResult> {
    // Pull a small extra buffer above the requested window so a tool
    // call/result pair that happens to span the cut boundary has a
    // reasonable chance of landing fully inside the parsed range.
    let safety_buffer = target_messages / 4 + 50;
    let scan_lines = target_messages.saturating_add(safety_buffer);
    let (reader, _window) = open_tail_reader(path, scan_lines, "Claude")?;

    let mut accum = ScanAccum::new();
    match scan_jsonl_lines(reader, path, &mut accum) {
        ScanOutcome::Completed => {}
        ScanOutcome::Canceled => return None,
    }

    flush_pending(&mut accum.state);
    flush_pending_tool_results(&mut accum.state);

    if accum.state.messages.is_empty() {
        // Returning None makes `try_claude_tail_fast_path` fall back to
        // the full-file parse. Most often this happens for very small
        // sessions where the tail window covered only metadata lines
        // (summary / custom-title), so the full parse is cheap anyway.
        log::debug!(
            "Claude tail parse produced no messages for '{}'; falling back to full parse",
            path.display()
        );
        return None;
    }

    // Trim to exactly `target_messages` — we deliberately over-scan so
    // tool merging at the boundary works, but the caller asked for
    // a specific window size.
    let len = accum.state.messages.len();
    if len > target_messages {
        accum.state.messages.drain(0..(len - target_messages));
    }

    Some(ClaudeTailResult {
        messages: accum.state.messages,
        parse_warning_count: accum.state.parse_warning_count,
        last_timestamp: accum.last_timestamp,
    })
}

#[cfg(test)]
mod tests;
