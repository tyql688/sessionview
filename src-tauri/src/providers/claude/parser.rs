use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::services::tail_reader::open_tail_reader;

use crate::models::{Message, Provider};
use crate::provider::ParsedSession;
use crate::provider_utils::{
    parse_rfc3339_timestamp, project_name_from_path, session_title, truncate_to_bytes,
    FTS_CONTENT_LIMIT,
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
                "Claude parse canceled at line {line_index} of '{}'",
                path.display()
            );
            return ScanOutcome::Canceled;
        }

        let line = match line {
            Ok(l) => l,
            Err(error) => {
                log::warn!(
                    "failed to read Claude session line from '{}': {error}",
                    path.display()
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
                log::warn!("skipping malformed JSONL in '{}': {e}", path.display());
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
                            if let Some(c) = entry.get("cwd").and_then(|c| c.as_str()) {
                                if !c.is_empty() {
                                    return Some(c.to_string());
                                }
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
