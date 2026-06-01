use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::services::tail_reader::tail_byte_offset;

use crate::models::{Message, MessageRole, Provider, TokenUsage};
use crate::provider::ParsedSession;
use crate::provider_utils::{
    is_system_content, parse_rfc3339_timestamp, project_name_from_path, session_title,
    truncate_to_bytes, FTS_CONTENT_LIMIT,
};
use crate::tool_metadata::{
    build_tool_metadata, canonical_tool_name, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

use super::images::{
    contains_image_placeholder_without_source, contains_image_source, count_image_markers,
    merge_image_placeholders_with_sources, normalize_image_source_segments,
};

/// Shared mutable state threaded through the per-message-type handlers.
struct ParseState {
    messages: Vec<Message>,
    content_parts: Vec<String>,
    first_user_message: Option<String>,
    pending_user_message: Option<(String, Option<String>)>,
    tool_use_id_map: HashMap<String, usize>,
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
    result_item: Value,
    top_level_result: Option<Value>,
    timestamp: Option<String>,
    source_tool_assistant_uuid: Option<String>,
}

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
                tool_use_id_map: HashMap::new(),
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
    let mut line_index: usize = 0;
    for line in reader.lines() {
        // Cooperative cancellation: bail out fast when the user navigated
        // away mid-load. Checked every 1024 lines so the polling cost is
        // negligible for normal-size sessions.
        line_index = line_index.wrapping_add(1);
        if line_index.is_multiple_of(1024) && crate::services::load_cancel::is_canceled() {
            log::debug!(
                "Claude parse canceled at line {} of '{}'",
                line_index,
                path.display()
            );
            return ScanOutcome::Canceled;
        }

        let line = match line {
            Ok(l) => l,
            Err(error) => {
                log::warn!(
                    "failed to read Claude session line from '{}': {}",
                    path.display(),
                    error
                );
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let entry: Value = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("skipping malformed JSONL in '{}': {}", path.display(), e);
                accum.state.parse_warning_count = accum.state.parse_warning_count.saturating_add(1);
                continue;
            }
        };

        if let Some(dedup_hash) = dedup_hash_from_entry(&entry) {
            if !accum.processed_hashes.insert(dedup_hash) {
                continue;
            }
        }

        let line_type = match entry.get("type").and_then(|t| t.as_str()) {
            Some(t) => t.to_string(),
            None => continue,
        };

        if accum.cwd.is_none() {
            if let Some(c) = entry.get("cwd").and_then(|c| c.as_str()) {
                if !c.is_empty() {
                    accum.cwd = Some(c.to_string());
                }
            }
        }

        if !accum.is_sidechain
            && entry
                .get("isSidechain")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        {
            accum.is_sidechain = true;
        }

        if accum.cc_version.is_none() {
            if let Some(v) = entry.get("version").and_then(|v| v.as_str()) {
                if !v.is_empty() {
                    accum.cc_version = Some(v.to_string());
                }
            }
        }

        if accum.git_branch.is_none() {
            if let Some(b) = entry.get("gitBranch").and_then(|b| b.as_str()) {
                if !b.is_empty() && b != "HEAD" {
                    accum.git_branch = Some(b.to_string());
                }
            }
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
                if accum.model.is_none() {
                    if let Some(m) = entry
                        .get("message")
                        .and_then(|msg| msg.get("model"))
                        .and_then(|m| m.as_str())
                    {
                        if !m.is_empty() {
                            accum.model = Some(m.to_string());
                        }
                    }
                }
                handle_assistant_message(&entry, &mut accum.state, timestamp);
            }
            "summary" => {
                handle_summary(&entry, &mut accum.summary_text, &mut accum.state);
                continue;
            }
            "system" => {
                handle_system_message(&entry, &mut accum.state, timestamp);
            }
            "custom-title" => {
                flush_pending(&mut accum.state);
                if let Some(t) = entry.get("title").and_then(|t| t.as_str()) {
                    if !t.trim().is_empty() {
                        accum.custom_title = Some(t.to_string());
                    }
                }
                continue;
            }
            "ai-title" => {
                flush_pending(&mut accum.state);
                if let Some(t) = entry.get("title").and_then(|t| t.as_str()) {
                    if !t.trim().is_empty() {
                        accum.ai_title = Some(t.to_string());
                    }
                }
                continue;
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
                continue;
            }
            "pr-link" => {
                flush_pending(&mut accum.state);
                handle_pr_link(&entry, &mut accum.state, timestamp);
                continue;
            }
            "mode" => {
                handle_mode(&entry, accum, timestamp);
                continue;
            }
            _ => {
                if preserves_pending_user_message(line_type.as_str()) {
                    continue;
                }
                flush_pending(&mut accum.state);
                continue;
            }
        }
    }

    ScanOutcome::Completed
}

fn preserves_pending_user_message(line_type: &str) -> bool {
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
fn handle_mode(entry: &Value, accum: &mut ScanAccum, timestamp: Option<String>) {
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

/// Extract parent session ID from subagent path.
/// Path pattern: .../{parent_session_id}/subagents/agent-{agentId}.jsonl
fn parent_id_from_path(path: &Path) -> Option<String> {
    let parent = path.parent()?; // subagents/
    if parent.file_name()?.to_str()? != "subagents" {
        return None;
    }
    let session_dir = parent.parent()?; // {parent_session_id}/
    Some(session_dir.file_name()?.to_str()?.to_string())
}

pub fn parse_session_file(path: &PathBuf) -> Option<ParsedSession> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "failed to open Claude session '{}': {}",
                path.display(),
                error
            );
            return None;
        }
    };
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            log::warn!(
                "failed to read Claude session metadata '{}': {}",
                path.display(),
                error
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
                    "failed to read Claude subagent meta '{}': {}",
                    meta_path.display(),
                    error
                );
                return None;
            }
        };
        let meta_json: Value = match serde_json::from_str(&meta_content) {
            Ok(json) => json,
            Err(error) => {
                log::warn!(
                    "failed to parse Claude subagent meta '{}': {}",
                    meta_path.display(),
                    error
                );
                return None;
            }
        };
        meta_json
            .get("description")
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
        // Path: .../{project_dir}/{parent_id}/subagents/agent-xxx.jsonl
        // Parent JSONL: .../{project_dir}/{parent_id}.jsonl
        // We need the project_dir's cwd, which we can't get here.
        // But the parent session's project_path is stored by its own cwd.
        // Best effort: walk up to the project directory and read the parent session's cwd.
        path.parent() // subagents/
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
                            "failed to open Claude parent transcript '{}': {}",
                            parent_jsonl.display(),
                            error
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
                                "failed to read Claude parent transcript line from '{}': {}",
                                parent_jsonl.display(),
                                error
                            );
                            continue;
                        }
                    };
                    match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(entry) => {
                            if let Some(c) = entry.get("cwd").and_then(|c| c.as_str()) {
                                if !c.is_empty() {
                                    return Some(c.to_string());
                                }
                            }
                        }
                        Err(error) => {
                            log::warn!(
                                "failed to parse Claude parent transcript line in '{}': {}",
                                parent_jsonl.display(),
                                error
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

    let full_content = state.content_parts.join("\n");
    let content_text = truncate_to_bytes(&full_content, FTS_CONTENT_LIMIT);

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
pub fn parse_session_tail(path: &Path, target_messages: usize) -> Option<ClaudeTailResult> {
    // Pull a small extra buffer above the requested window so a tool
    // call/result pair that happens to span the cut boundary has a
    // reasonable chance of landing fully inside the parsed range.
    let safety_buffer = target_messages / 4 + 50;
    let scan_lines = target_messages.saturating_add(safety_buffer);
    let window = match tail_byte_offset(path, scan_lines) {
        Ok(w) => w,
        Err(error) => {
            log::warn!(
                "failed to locate Claude session tail in '{}': {}",
                path.display(),
                error
            );
            return None;
        }
    };

    let file = match File::open(path) {
        Ok(f) => f,
        Err(error) => {
            log::warn!(
                "failed to open Claude session for tail parse '{}': {}",
                path.display(),
                error
            );
            return None;
        }
    };
    let mut reader = BufReader::new(file);
    if window.start_offset > 0 {
        if let Err(error) = reader.seek(SeekFrom::Start(window.start_offset)) {
            log::warn!(
                "failed to seek Claude session for tail parse '{}': {}",
                path.display(),
                error
            );
            return None;
        }
    }

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

fn unique_hash_from_entry(entry: &Value) -> Option<String> {
    let message_id = entry
        .get("message")
        .and_then(|message| message.get("id"))
        .and_then(|id| id.as_str())?;
    let request_id = entry.get("requestId").and_then(|id| id.as_str())?;
    Some(format!("{message_id}:{request_id}"))
}

fn dedup_hash_from_entry(entry: &Value) -> Option<String> {
    let base = unique_hash_from_entry(entry)?;
    // Hash the content rather than storing its full serialization in the dedup
    // set — keeps `processed_hashes` small for sessions with large messages.
    let content_hash = match entry.get("message").and_then(|m| m.get("content")) {
        // `to_string` on an in-memory Value never fails in practice; if it ever
        // did, skip dedup for this entry (returns None) instead of panicking.
        Some(content) => {
            let serialized = serde_json::to_string(content).ok()?;
            let mut hasher = DefaultHasher::new();
            serialized.hash(&mut hasher);
            hasher.finish()
        }
        None => 0,
    };
    Some(format!("{base}:{content_hash:x}"))
}

/// Heuristic fallback used only when a tool_result reaches us with no
/// matching `tool_use` (truncated transcript, mid-stream crash). The
/// authoritative source for tool identity is the `tool_use` line's
/// `name` field; this exists so we still show *something* useful when
/// that line is missing.
///
/// Logging policy: warn only when we land at the no-signal-at-all
/// fallback (`UnknownToolResult`). Truncated transcripts where the
/// heuristic or `use_id` produces a usable name are an expected
/// degraded path and should not spam warnings — they get `debug!`.
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

fn standalone_tool_result_message(
    result_text: String,
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
        tool_result_facts(result_item, top_level_result),
    );

    Message {
        role: MessageRole::Tool,
        content: result_text,
        timestamp,
        tool_name: Some(canonical_name),
        tool_input: None,
        tool_metadata: Some(metadata),
        token_usage: None,
        model: None,
        usage_hash: None,
    }
}

fn flush_pending_tool_results(state: &mut ParseState) {
    let pending = std::mem::take(&mut state.pending_tool_results_by_use_id);
    for (use_id, result) in pending {
        state.messages.push(standalone_tool_result_message(
            result.result_text,
            &result.result_item,
            result.top_level_result.as_ref(),
            Some(&use_id),
            result.source_tool_assistant_uuid.as_deref(),
            result.timestamp,
        ));
    }
}

fn tool_result_facts<'a>(
    result_item: &'a Value,
    top_level_result: Option<&'a Value>,
) -> ToolResultFacts<'a> {
    // Async-Agent results carry a lifecycle marker
    // (`"async_launched"` / `"completed"` / …). Surface it so the UI
    // can distinguish "kicked off in background" from "finished",
    // instead of always showing the default success badge.
    let status = top_level_result
        .and_then(|v| v.get("status"))
        .and_then(|v| v.as_str());
    ToolResultFacts {
        raw_result: top_level_result,
        is_error: result_item.get("is_error").and_then(|v| v.as_bool()),
        status,
        artifact_path: top_level_result
            .and_then(|v| v.get("persistedOutputPath"))
            .and_then(|v| v.as_str()),
    }
}

/// Handle a "user" line, which may be a real user message or a tool_result turn.
fn handle_user_message(entry: &Value, state: &mut ParseState, timestamp: Option<String>) {
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
    if let Some(content) = format_local_command_text(&text) {
        flush_pending(state);
        append_system_message(state, content, timestamp);
        return;
    }
    let is_meta = entry
        .get("isMeta")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

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
            let result_text = extract_tool_result_content(result_item);
            if result_text.trim().is_empty() && top_level_result.is_none() {
                continue;
            }
            if !result_text.trim().is_empty() {
                state.content_parts.push(result_text.clone());
            }
            let use_id = result_item.get("tool_use_id").and_then(|i| i.as_str());
            if let Some(idx) = use_id.and_then(|id| state.tool_use_id_map.get(id)) {
                // Merge result into the existing tool_use message
                state.messages[*idx].content = result_text;
                if let Some(metadata) = state.messages[*idx].tool_metadata.as_mut() {
                    enrich_tool_metadata(
                        metadata,
                        tool_result_facts(result_item, top_level_result),
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
                state.messages[idx].content = result_text;
                if let Some(metadata) = state.messages[idx].tool_metadata.as_mut() {
                    enrich_tool_metadata(
                        metadata,
                        tool_result_facts(result_item, top_level_result),
                    );
                }
            } else {
                if let Some(use_id) = use_id {
                    state.pending_tool_results_by_use_id.insert(
                        use_id.to_string(),
                        PendingToolResult {
                            result_text,
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
                    result_text,
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
fn handle_assistant_message(entry: &Value, state: &mut ParseState, timestamp: Option<String>) {
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
                    if let Some(t) = item.get("thinking").and_then(|t| t.as_str()) {
                        if !t.trim().is_empty() {
                            // Emit thinking as a separate assistant message with marker
                            state.messages.push(Message {
                                role: MessageRole::System,
                                content: format!("[thinking]\n{t}"),
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
                "text" => {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        if !t.trim().is_empty() {
                            text_parts.push(t.to_string());
                        }
                    }
                }
                "tool_use" => {
                    // Flush accumulated text as assistant message
                    if !text_parts.is_empty() {
                        let text = text_parts.join("\n");
                        state.content_parts.push(text.clone());
                        state.messages.push(Message {
                            role: MessageRole::Assistant,
                            content: text,
                            timestamp: timestamp.clone(),
                            tool_name: None,
                            tool_input: None,
                            token_usage: None,
                            model: per_message_model.clone(),
                            usage_hash: None,
                            tool_metadata: None,
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
                        role: MessageRole::Tool,
                        content: String::new(),
                        timestamp: timestamp.clone(),
                        tool_name: Some(canonical_name),
                        tool_input: input,
                        tool_metadata: Some(metadata),
                        token_usage: None,
                        model: None,
                        usage_hash: None,
                    });
                    tool_indices.push(msg_idx);
                    // Record tool_use_id for merging results later
                    if let Some(id) = use_id {
                        state.tool_use_id_map.insert(id.to_string(), msg_idx);
                        if let Some(pending) = state.pending_tool_results_by_use_id.remove(id) {
                            state.messages[msg_idx].content = pending.result_text;
                            if let Some(metadata) = state.messages[msg_idx].tool_metadata.as_mut() {
                                enrich_tool_metadata(
                                    metadata,
                                    tool_result_facts(
                                        &pending.result_item,
                                        pending.top_level_result.as_ref(),
                                    ),
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(uuid) = entry.get("uuid").and_then(|u| u.as_str()) {
            if !tool_indices.is_empty() {
                state
                    .assistant_tool_indices_by_uuid
                    .insert(uuid.to_string(), tool_indices);
            }
        }
        // Flush remaining text
        if !text_parts.is_empty() {
            let text = text_parts.join("\n");
            state.content_parts.push(text.clone());
            state.messages.push(Message {
                role: MessageRole::Assistant,
                content: text,
                timestamp: timestamp.clone(),
                tool_name: None,
                tool_input: None,
                token_usage: None,
                model: per_message_model.clone(),
                usage_hash: None,
                tool_metadata: None,
            });
        }
    } else {
        // content is a plain string
        let text = extract_message_content(msg);
        if !text.trim().is_empty() {
            state.content_parts.push(text.clone());
            state.messages.push(Message {
                role: MessageRole::Assistant,
                content: text,
                timestamp: timestamp.clone(),
                tool_name: None,
                tool_input: None,
                token_usage: None,
                model: per_message_model.clone(),
                usage_hash: None,
                tool_metadata: None,
            });
        }
    }

    // Attach token usage + dedup hash to the last assistant/tool message of this turn.
    // When the turn produced only thinking (System) or empty content, insert a
    // minimal placeholder so the usage is never silently dropped.
    //
    // Tool messages (and the empty placeholder below) carry model=None by
    // design, so we always force the usage-bearing message's model and
    // timestamp to the assistant entry's values. Without this, usage attached
    // to a tool message is dropped later by `compute_token_stats_dedup`'s
    // "missing model" filter.
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
                role: MessageRole::Assistant,
                content: String::new(),
                timestamp: timestamp.clone(),
                tool_name: None,
                tool_input: None,
                token_usage: Some(usage),
                model: per_message_model.clone(),
                usage_hash: hash,
                tool_metadata: None,
            });
        }
    }
}

/// Handle a "summary" line: capture the first non-empty summary text.
fn handle_summary(entry: &Value, summary_text: &mut Option<String>, state: &mut ParseState) {
    if summary_text.is_none() {
        if let Some(s) = entry.get("summary").and_then(|s| s.as_str()) {
            if !s.trim().is_empty() {
                *summary_text = Some(s.to_string());
            }
        }
    }
    flush_pending(state);
}

/// Handle a "system" line: emit human-readable summaries of system subtypes.
fn handle_system_message(entry: &Value, state: &mut ParseState, timestamp: Option<String>) {
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
            let Some(content) = entry
                .get("content")
                .and_then(|v| v.as_str())
                .and_then(format_local_command_text)
            else {
                return;
            };
            content
        }
        "informational" => {
            let Some(content) = entry
                .get("content")
                .and_then(|v| v.as_str())
                .map(clean_system_text)
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
        role: MessageRole::System,
        content,
        timestamp,
        tool_name: None,
        tool_input: None,
        token_usage: None,
        model: None,
        usage_hash: None,
        tool_metadata: None,
    });
}

fn format_local_command_text(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("<command-name>")
        && !trimmed.starts_with("<command-message>")
        && !trimmed.starts_with("<local-command-stdout>")
        && !trimmed.starts_with("<local-command-stderr>")
    {
        return None;
    }

    if let Some(command) = extract_tag_text(raw, "command-name").filter(|s| !s.is_empty()) {
        let args = extract_tag_text(raw, "command-args").unwrap_or_default();
        let detail = [command, args]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        return Some(format!("[local_command] {detail}"));
    }

    let stdout = extract_tag_text(raw, "local-command-stdout")
        .or_else(|| extract_tag_text(raw, "local-command-stderr"))
        .map(|value| clean_system_text(&value))
        .filter(|s| !s.is_empty())?;
    Some(format!("[local_command] {stdout}"))
}

fn extract_tag_text(raw: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = raw.find(&open)? + open.len();
    let end = raw[start..].find(&close)? + start;
    Some(clean_system_text(&raw[start..end]))
}

fn clean_system_text(raw: &str) -> String {
    strip_ansi_codes(raw)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_ansi_codes(raw: &str) -> String {
    let mut cleaned = String::new();
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
            continue;
        }
        cleaned.push(ch);
    }

    cleaned
}

fn handle_pr_link(entry: &Value, state: &mut ParseState, timestamp: Option<String>) {
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
        role: MessageRole::System,
        content: format!("[pr_link] {label}: {pr_url}"),
        timestamp,
        tool_name: None,
        tool_input: None,
        token_usage: None,
        model: None,
        usage_hash: None,
        tool_metadata: None,
    });
}

/// Flush any pending user message that was waiting for an image-source merge.
fn flush_pending(state: &mut ParseState) {
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
        role: MessageRole::User,
        content: text,
        timestamp,
        tool_name: None,
        tool_input: None,
        token_usage: None,
        model: None,
        usage_hash: None,
        tool_metadata: None,
    });
}

/// Extract token usage from a message's `usage` field.
fn extract_token_usage(message: &Value) -> Option<TokenUsage> {
    let usage = message.get("usage")?;
    let input_tokens = usage
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let output_tokens = usage
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_creation_input_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_read_input_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    if input_tokens == 0 && output_tokens == 0 {
        return None;
    }
    Some(TokenUsage {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens,
        cache_read_input_tokens,
    })
}

/// Extract text content from a message object.
/// The `content` field can be a string or an array of typed blocks.
/// Handles both "text" and "tool_use" content blocks.
fn extract_message_content(message: &Value) -> String {
    let content = message.get("content");
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => {
            let mut parts = Vec::new();
            let mut image_block_count = 0usize;
            for item in arr {
                let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match item_type {
                    "text" => {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            parts.push(normalize_image_source_segments(text));
                        }
                    }
                    "tool_use" => {
                        let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                        let input = item
                            .get("input")
                            .map(std::string::ToString::to_string)
                            .unwrap_or_default();
                        let end = if input.len() > 200 {
                            input.floor_char_boundary(200)
                        } else {
                            input.len()
                        };
                        parts.push(format!("[Tool: {}] {}", name, &input[..end]));
                    }
                    "tool_result" => {
                        if let Some(text) = item.get("content").and_then(|c| c.as_str()) {
                            let end = if text.len() > 200 {
                                text.floor_char_boundary(200)
                            } else {
                                text.len()
                            };
                            parts.push(format!("[Result] {}", &text[..end]));
                        }
                    }
                    "image" => {
                        image_block_count += 1;
                    }
                    other => {
                        log::debug!(
                            "unknown Claude assistant content block type '{other}' — skipped"
                        );
                    }
                }
            }
            let marker_count = parts
                .iter()
                .map(|part| count_image_markers(part))
                .sum::<usize>();
            for _ in marker_count..image_block_count {
                parts.push("[Image]".to_string());
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

/// Check if a "user" message is actually a tool_result turn.
/// In the Anthropic API, tool results are sent as user-role messages
/// with content blocks of type "tool_result".
fn is_tool_result_message(message: &Value) -> bool {
    match message.get("content") {
        Some(Value::Array(arr)) if !arr.is_empty() => arr
            .iter()
            .all(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_result")),
        _ => false,
    }
}

/// Resolve `<persisted-output>` tags by reading the referenced external file.
/// Falls back to keeping the original content (with preview) if the file can't be read.
/// Only paths under `~/.claude/` are allowed to prevent arbitrary file reads.
pub fn resolve_persisted_outputs(content: &str) -> String {
    const TAG_START: &str = "<persisted-output>";
    const TAG_END: &str = "</persisted-output>";
    /// Defensive guard against pathological inputs (deeply nested or
    /// malformed tags). Any real Claude session has at most a handful
    /// per message; values above this likely indicate a corrupt file.
    const MAX_TAGS_PER_MESSAGE: usize = 1024;

    if !content.contains(TAG_START) {
        return content.to_string();
    }

    let mut result = String::new();
    let mut remaining = content;
    let mut iterations = 0usize;

    while let Some(start_pos) = remaining.find(TAG_START) {
        iterations += 1;
        if iterations > MAX_TAGS_PER_MESSAGE {
            log::warn!(
                "resolve_persisted_outputs: bailing after {MAX_TAGS_PER_MESSAGE} tags; \
                 returning remaining content unmodified"
            );
            result.push_str(remaining);
            return result;
        }

        // Add everything before the tag
        result.push_str(&remaining[..start_pos]);

        let after_tag_start = &remaining[start_pos + TAG_START.len()..];
        if let Some(end_pos) = after_tag_start.find(TAG_END) {
            let inner = &after_tag_start[..end_pos];

            // Extract file path from "Full output saved to: /path"
            let file_content = inner
                .lines()
                .find_map(|line| {
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix("Full output saved to: ") {
                        Some(rest.trim().to_string())
                    } else if trimmed.contains("saved to: ") {
                        trimmed
                            .split("saved to: ")
                            .nth(1)
                            .map(|p| p.trim().to_string())
                    } else {
                        None
                    }
                })
                .and_then(|path| {
                    let canonical = match std::fs::canonicalize(&path) {
                        Ok(canonical) => canonical,
                        Err(error) => {
                            log::warn!(
                                "failed to canonicalize Claude full-output path '{}': {}",
                                path,
                                error
                            );
                            return None;
                        }
                    };
                    let home = dirs::home_dir()?;
                    let allowed = [home.join(".claude"), home.join(".cc-mirror")];
                    if !allowed.iter().any(|base| {
                        std::fs::canonicalize(base)
                            .ok()
                            .is_some_and(|b| canonical.starts_with(&b))
                    }) {
                        return None;
                    }
                    match std::fs::read_to_string(&canonical) {
                        Ok(content) => Some(content),
                        Err(error) => {
                            log::warn!(
                                "failed to read Claude full-output file '{}': {}",
                                canonical.display(),
                                error
                            );
                            None
                        }
                    }
                });

            match file_content {
                Some(full) => result.push_str(&full),
                None => {
                    // Keep the original tag content as fallback
                    result.push_str(TAG_START);
                    result.push_str(inner);
                    result.push_str(TAG_END);
                }
            }

            remaining = &after_tag_start[end_pos + TAG_END.len()..];
        } else {
            // No closing tag found, keep everything as-is
            result.push_str(&remaining[start_pos..]);
            remaining = "";
        }
    }

    result.push_str(remaining);
    result
}

/// Extract text content from a single tool_result block.
/// The `content` field can be a string, an array of text blocks, or absent.
fn extract_tool_result_content(result: &Value) -> String {
    match result.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => {
            let mut parts = Vec::new();
            for item in arr {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            parts.push(t.to_string());
                        }
                    }
                    Some("image") => {
                        let source = item.get("source");
                        let source_type = source
                            .and_then(|s| s.get("type"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("base64");
                        match source_type {
                            "base64" => {
                                let data =
                                    source.and_then(|s| s.get("data")).and_then(|d| d.as_str());
                                let media = source
                                    .and_then(|s| s.get("media_type"))
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("image/png");
                                if let Some(b64) = data {
                                    parts.push(format!(
                                        "[Image: source: data:{};base64,{}]",
                                        media, b64
                                    ));
                                } else {
                                    parts.push("[Image]".to_string());
                                }
                            }
                            "url" => {
                                if let Some(url) =
                                    source.and_then(|s| s.get("url")).and_then(|u| u.as_str())
                                {
                                    parts.push(format!("[Image: source: {url}]"));
                                } else {
                                    parts.push("[Image]".to_string());
                                }
                            }
                            other => {
                                log::debug!(
                                    "unknown Claude tool_result image source.type '{other}'"
                                );
                                parts.push("[Image]".to_string());
                            }
                        }
                    }
                    Some("tool_reference") => {
                        let name = item
                            .get("tool_name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("?");
                        parts.push(format!("[Tool: {name}]"));
                    }
                    Some(other) => {
                        log::debug!(
                            "unknown Claude tool_result content block type '{other}' — skipped"
                        );
                    }
                    None => {}
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_session_file, parse_session_tail};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parse_session_file_counts_malformed_lines_without_aborting() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let good =
            r#"{"type":"user","timestamp":"2026-04-10T10:00:00Z","message":{"content":"hello"}}"#;
        let broken = r#"{ this is not valid json "#;
        let good2 =
            r#"{"type":"user","timestamp":"2026-04-10T10:00:05Z","message":{"content":"world"}}"#;
        fs::write(&file, format!("{good}\n{broken}\n{good2}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("file-level parse must succeed");
        assert_eq!(
            parsed.messages.len(),
            2,
            "both well-formed lines should produce messages"
        );
        assert_eq!(
            parsed.parse_warning_count, 1,
            "the single broken line should be counted as one parse warning"
        );
    }

    #[test]
    fn parse_session_file_deduplicates_same_message_request_pair() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let line = r#"{"type":"assistant","requestId":"req-1","timestamp":"2026-04-10T10:00:00Z","message":{"id":"msg-1","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20},"content":[{"type":"text","text":"hello"}]}}"#;
        fs::write(&file, format!("{line}\n{line}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        assert_eq!(parsed.messages.len(), 1);
        let usage = parsed.messages[0].token_usage.as_ref().expect("usage");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, 10);
        assert_eq!(usage.cache_read_input_tokens, 20);
    }

    #[test]
    fn parse_session_file_keeps_distinct_chunks_with_same_message_request_pair() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let thinking = r#"{"type":"assistant","requestId":"req-1","uuid":"assistant-thinking","timestamp":"2026-04-10T10:00:00Z","message":{"id":"msg-1","model":"claude-opus-4-6","role":"assistant","content":[{"type":"thinking","thinking":"I should inspect a file."}]}}"#;
        let tool_use = r#"{"type":"assistant","requestId":"req-1","uuid":"assistant-tool","timestamp":"2026-04-10T10:00:01Z","message":{"id":"msg-1","model":"claude-opus-4-6","role":"assistant","content":[{"type":"tool_use","id":"toolu_same_request","name":"Read","input":{"file_path":"/Users/alice/project/src/App.tsx"}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-04-10T10:00:02Z","sourceToolAssistantUUID":"assistant-tool","toolUseResult":{"type":"text","file":{"filePath":"/Users/alice/project/src/App.tsx","content":"export default App;","numLines":1,"startLine":1,"totalLines":1}},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_same_request","content":"1\texport default App;"}]}}"#;
        fs::write(&file, format!("{thinking}\n{tool_use}\n{result}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        assert!(
            parsed
                .messages
                .iter()
                .any(|message| message.content.starts_with("[thinking]")),
            "thinking chunk should be preserved"
        );
        let tool = parsed
            .messages
            .iter()
            .find(|message| message.role == crate::models::MessageRole::Tool)
            .expect("tool message");

        assert_eq!(tool.tool_name.as_deref(), Some("Read"));
        assert_eq!(tool.content, "1\texport default App;");
        assert_ne!(tool.tool_name.as_deref(), Some("toolu_same_request"));
    }

    #[test]
    fn parse_session_file_matches_tool_result_that_arrives_before_tool_use() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let result = r#"{"type":"user","timestamp":"2026-04-10T10:00:00Z","sourceToolAssistantUUID":"assistant-late","toolUseResult":"Error: File has not been read yet. Read it first before writing to it.","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_late","content":"<tool_use_error>File has not been read yet. Read it first before writing to it.</tool_use_error>"}]}}"#;
        let tool_use = r#"{"type":"assistant","requestId":"req-1","uuid":"assistant-late","timestamp":"2026-04-10T10:00:01Z","message":{"id":"msg-1","model":"claude-opus-4-6","role":"assistant","content":[{"type":"tool_use","id":"toolu_late","name":"Edit","input":{"file_path":"/Users/alice/project/src/App.tsx","old_string":"old","new_string":"new"}}]}}"#;
        fs::write(&file, format!("{result}\n{tool_use}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        let tool_messages = parsed
            .messages
            .iter()
            .filter(|message| message.role == crate::models::MessageRole::Tool)
            .collect::<Vec<_>>();

        assert_eq!(tool_messages.len(), 1);
        assert_eq!(tool_messages[0].tool_name.as_deref(), Some("Edit"));
        assert!(tool_messages[0]
            .content
            .contains("File has not been read yet"));
    }

    #[test]
    fn parse_session_file_adds_claude_tool_metadata() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let assistant = r#"{"type":"assistant","uuid":"assistant-1","timestamp":"2026-04-10T10:00:00Z","message":{"id":"msg-1","model":"claude-opus-4-6","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"ToolSearch","input":{"query":"select:TaskCreate","max_results":2}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-04-10T10:00:01Z","sourceToolAssistantUUID":"assistant-1","toolUseResult":{"matches":[{"tool_name":"TaskCreate"}],"query":"select:TaskCreate","total_deferred_tools":1},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"TaskCreate found"}]}]}}"#;
        fs::write(&file, format!("{assistant}\n{result}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        let tool = parsed
            .messages
            .iter()
            .find(|message| message.tool_name.as_deref() == Some("ToolSearch"))
            .expect("tool message");
        let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

        assert_eq!(metadata.raw_name, "ToolSearch");
        assert_eq!(metadata.category, "search");
        assert_eq!(metadata.summary.as_deref(), Some("select:TaskCreate"));
        assert_eq!(metadata.status.as_deref(), Some("success"));
        assert_eq!(metadata.result_kind.as_deref(), None);
        assert_eq!(tool.content, "TaskCreate found");
    }

    #[test]
    fn parse_session_file_keeps_model_and_timestamp_on_usage_attached_to_tool_message() {
        // A tool_use-only assistant turn has its usage attached to the Tool
        // message. Tool messages are emitted with model=None by design, so the
        // parser must backfill the entry's model/timestamp on the usage-bearing
        // message — otherwise `compute_token_stats_dedup` silently drops the
        // usage via its "missing model" filter.
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let line = r#"{"type":"assistant","requestId":"req-1","uuid":"assistant-1","timestamp":"2026-04-21T10:00:00Z","message":{"id":"msg-1","model":"claude-opus-4-7","role":"assistant","usage":{"input_tokens":12,"output_tokens":34,"cache_creation_input_tokens":5,"cache_read_input_tokens":7},"content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/tmp/x.txt"}}]}}"#;
        fs::write(&file, format!("{line}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        let usage_msg = parsed
            .messages
            .iter()
            .find(|m| m.token_usage.is_some())
            .expect("usage-bearing message");
        assert_eq!(usage_msg.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(usage_msg.timestamp.as_deref(), Some("2026-04-21T10:00:00Z"));
        assert_eq!(
            usage_msg.usage_hash.as_deref(),
            Some("msg-1:req-1"),
            "usage_hash must be msg:req for cross-file dedup"
        );
    }

    #[test]
    fn parse_session_file_keeps_model_and_timestamp_on_thinking_only_turn() {
        // A turn whose only content is `thinking` produces no Assistant/Tool
        // message (thinking is emitted as System). The fallback placeholder
        // for the usage must carry the entry's model/timestamp, not a guess
        // read from adjacent messages.
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let line = r#"{"type":"assistant","requestId":"req-2","uuid":"assistant-2","timestamp":"2026-04-21T10:05:00Z","message":{"id":"msg-2","model":"claude-opus-4-7","role":"assistant","usage":{"input_tokens":3,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"content":[{"type":"thinking","thinking":"reasoning only"}]}}"#;
        fs::write(&file, format!("{line}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        let usage_msg = parsed
            .messages
            .iter()
            .find(|m| m.token_usage.is_some())
            .expect("usage-bearing message");
        assert_eq!(usage_msg.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(usage_msg.timestamp.as_deref(), Some("2026-04-21T10:05:00Z"));
    }

    #[test]
    fn parse_session_file_recovers_unmatched_edit_tool_result() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let result = r#"{"type":"user","timestamp":"2026-04-10T10:00:01Z","toolUseResult":{"filePath":"/project/src/App.tsx","oldString":"old","newString":"new","originalFile":"very large","structuredPatch":[],"userModified":false,"replaceAll":false},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_missing","content":"The file /project/src/App.tsx has been updated successfully."}]}}"#;
        fs::write(&file, format!("{result}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        let tool = parsed
            .messages
            .iter()
            .find(|message| message.role == crate::models::MessageRole::Tool)
            .expect("tool result");
        let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

        assert_eq!(tool.tool_name.as_deref(), Some("Edit"));
        assert_eq!(metadata.raw_name, "Edit");
        assert_eq!(metadata.result_kind.as_deref(), Some("file_patch"));
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("originalFile"))
                .and_then(|value| value.as_str()),
            Some("<omitted>")
        );
    }

    #[test]
    fn parse_session_file_handles_new_claude_events_and_tool_aliases() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let agent_name = r#"{"type":"agent-name","agentName":"blush-task-polling-refactor"}"#;
        let attachment = r#"{"type":"attachment","timestamp":"2026-04-25T02:03:02Z","attachment":{"type":"skill_listing","content":"skill listing noise that should not render"}}"#;
        let assistant = r##"{"type":"assistant","uuid":"assistant-1","timestamp":"2026-04-25T02:03:03Z","message":{"id":"msg-1","model":"claude-opus-4-7","role":"assistant","content":[{"type":"tool_use","id":"toolu_wakeup","name":"ScheduleWakeup","input":{"delaySeconds":60,"reason":"wait for startup"}},{"type":"tool_use","id":"toolu_monitor","name":"Monitor","input":{"command":"tail -f app.log","description":"Watch startup logs"}},{"type":"tool_use","id":"toolu_plan","name":"ExitPlanMode","input":{"plan":"# Plan\nDo it"}}]}}"##;
        let away = r#"{"type":"system","subtype":"away_summary","timestamp":"2026-04-25T02:03:04Z","content":"Work is paused."}"#;
        let scheduled = r#"{"type":"system","subtype":"scheduled_task_fire","timestamp":"2026-04-25T02:03:05Z","content":"Claude resuming /loop wakeup"}"#;
        let pr = r#"{"type":"pr-link","timestamp":"2026-04-25T02:03:06Z","prUrl":"https://github.com/example/repo/pull/7","prNumber":7}"#;
        fs::write(
            &file,
            format!("{agent_name}\n{attachment}\n{assistant}\n{away}\n{scheduled}\n{pr}\n"),
        )
        .unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        assert_eq!(parsed.meta.title, "blush-task-polling-refactor");
        assert!(
            !parsed
                .messages
                .iter()
                .any(|message| message.content.contains("skill listing noise")),
            "attachment skill listings must stay hidden"
        );

        let wakeup = parsed
            .messages
            .iter()
            .find(|message| message.tool_name.as_deref() == Some("ScheduleWakeup"))
            .expect("ScheduleWakeup tool");
        let wakeup_metadata = wakeup.tool_metadata.as_ref().expect("metadata");
        assert_eq!(wakeup_metadata.category, "cron");
        assert_eq!(
            wakeup_metadata.summary.as_deref(),
            Some("60s · wait for startup")
        );

        let monitor = parsed
            .messages
            .iter()
            .find(|message| {
                message
                    .tool_metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.raw_name == "Monitor")
            })
            .expect("Monitor tool");
        assert_eq!(monitor.tool_name.as_deref(), Some("Bash"));
        assert_eq!(
            monitor
                .tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.summary.as_deref()),
            Some("Watch startup logs")
        );

        let exit_plan = parsed
            .messages
            .iter()
            .find(|message| {
                message
                    .tool_metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.raw_name == "ExitPlanMode")
            })
            .expect("ExitPlanMode tool");
        assert_eq!(exit_plan.tool_name.as_deref(), Some("Plan"));

        for marker in ["[away_summary]", "[scheduled_task_fire]", "[pr_link]"] {
            assert!(
                parsed
                    .messages
                    .iter()
                    .any(|message| message.content.contains(marker)),
                "{marker} should be visible as a system event"
            );
        }
    }

    #[test]
    fn parse_session_file_surfaces_mode_transitions_and_dedupes_them() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        // Five mode lines: normal → plan → plan (dup) → normal → accept_edits.
        // Expected emissions: [mode] plan, [mode] normal, [mode] accept_edits.
        //   - Leading `normal` is suppressed (it matches the default).
        //   - Duplicate `plan` is deduped.
        //   - Transition back to `normal` is still emitted.
        let lines = [
            r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
            r#"{"type":"user","timestamp":"2026-04-25T02:03:00Z","message":{"content":"hi"}}"#,
            r#"{"type":"mode","mode":"plan","sessionId":"s"}"#,
            r#"{"type":"mode","mode":"plan","sessionId":"s"}"#,
            r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
            r#"{"type":"mode","mode":"accept_edits","sessionId":"s"}"#,
        ];
        fs::write(&file, lines.join("\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        let mode_msgs: Vec<&str> = parsed
            .messages
            .iter()
            .filter_map(|m| {
                if m.content.starts_with("[mode]") {
                    Some(m.content.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            mode_msgs,
            vec!["[mode] plan", "[mode] normal", "[mode] accept_edits"]
        );
    }

    #[test]
    fn parse_session_file_does_not_emit_leading_mode_normal_for_default_state() {
        // The common case: session opens with `mode: normal` (the default).
        // We must NOT inject a [mode] normal System message at the top —
        // that would clutter every Claude session's timeline.
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let lines = [
            r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
            r#"{"type":"user","timestamp":"2026-04-25T02:03:00Z","message":{"content":"hi"}}"#,
            r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
        ];
        fs::write(&file, lines.join("\n")).unwrap();
        let parsed = parse_session_file(&file).expect("parsed");
        assert!(
            !parsed
                .messages
                .iter()
                .any(|m| m.content.starts_with("[mode]")),
            "no [mode] messages should appear when only `normal` was seen; got {:?}",
            parsed
                .messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parse_session_file_handles_tool_reference_inside_tool_result_content() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let assistant = r##"{"type":"assistant","uuid":"a1","timestamp":"2026-04-25T02:03:00Z","message":{"id":"msg-1","model":"claude-opus-4-7","role":"assistant","content":[{"type":"tool_use","id":"toolu_ref","name":"ToolSearch","input":{"query":"task"}}]}}"##;
        let tool_result = r##"{"type":"user","timestamp":"2026-04-25T02:03:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_ref","content":[{"type":"tool_reference","tool_name":"TaskCreate"},{"type":"tool_reference","tool_name":"TaskUpdate"}]}]},"toolUseResult":{"matches":[],"total_deferred_tools":2}}"##;
        fs::write(&file, format!("{assistant}\n{tool_result}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        let tool_msg = parsed
            .messages
            .iter()
            .find(|m| m.tool_name.as_deref() == Some("ToolSearch"))
            .expect("tool message");
        assert!(
            tool_msg.content.contains("[Tool: TaskCreate]"),
            "tool_reference parts must render as [Tool: <name>], got {:?}",
            tool_msg.content
        );
        assert!(tool_msg.content.contains("[Tool: TaskUpdate]"));
    }

    #[test]
    fn parse_session_file_surfaces_async_agent_status() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        let assistant = r##"{"type":"assistant","uuid":"a1","timestamp":"2026-04-25T02:03:00Z","message":{"id":"msg-1","model":"claude-opus-4-7","role":"assistant","content":[{"type":"tool_use","id":"toolu_a","name":"Task","input":{"description":"audit","prompt":"go","subagent_type":"general-purpose"}}]}}"##;
        let tool_result = r##"{"type":"user","timestamp":"2026-04-25T02:03:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_a","content":[{"type":"text","text":"launched"}]}]},"toolUseResult":{"agentId":"abc","isAsync":true,"status":"async_launched"}}"##;
        fs::write(&file, format!("{assistant}\n{tool_result}\n")).unwrap();

        let parsed = parse_session_file(&file).expect("parsed");
        let tool_msg = parsed
            .messages
            .iter()
            .find(|m| {
                m.tool_metadata
                    .as_ref()
                    .is_some_and(|md| md.raw_name == "Task")
            })
            .expect("Task tool message");
        let status = tool_msg
            .tool_metadata
            .as_ref()
            .and_then(|md| md.status.as_deref());
        assert_eq!(status, Some("async_launched"));
    }

    #[test]
    fn parse_session_tail_returns_only_the_last_n_messages() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");

        let mut content = String::new();
        for i in 0..200 {
            let ts = format!("2026-04-10T10:00:{:02}Z", i % 60);
            content.push_str(&format!(
                r#"{{"type":"user","timestamp":"{ts}","message":{{"content":"msg-{i}"}}}}"#
            ));
            content.push('\n');
        }
        fs::write(&file, content).unwrap();

        let tail = parse_session_tail(&file, 20).expect("tail parse");
        assert_eq!(tail.messages.len(), 20);
        // Tail must be the LAST 20 messages — msg-180 through msg-199.
        let first = tail.messages.first().expect("first").content.clone();
        let last = tail.messages.last().expect("last").content.clone();
        assert!(
            first.ends_with("msg-180"),
            "first tail message expected to contain msg-180, got {first:?}"
        );
        assert!(
            last.ends_with("msg-199"),
            "last tail message expected to contain msg-199, got {last:?}"
        );
    }

    #[test]
    fn parse_session_tail_falls_back_to_full_file_when_smaller_than_window() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");

        let mut content = String::new();
        for i in 0..5 {
            content.push_str(&format!(
                r#"{{"type":"user","timestamp":"2026-04-10T10:00:0{i}Z","message":{{"content":"only-{i}"}}}}"#
            ));
            content.push('\n');
        }
        fs::write(&file, content).unwrap();

        let tail = parse_session_tail(&file, 100).expect("tail parse");
        assert_eq!(
            tail.messages.len(),
            5,
            "tail must return all messages when the file is smaller than the requested window"
        );
    }
}
