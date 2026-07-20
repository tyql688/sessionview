//! Kimi-code wire.jsonl parser.
//!
//! Handles two on-disk wire formats — both start with
//! `{"type":"metadata","protocol_version":"1.0",...}` and live under
//! `~/.kimi-code/sessions/wd_*/<session-dir>/agents/<name>/wire.jsonl`:
//!
//! * **Migrated** (from legacy kimi-cli protocol 1.9): only `metadata` +
//!   `context.append_message` lines. Messages carry `role` and structured
//!   `content[]`/`toolCalls[]` arrays. No per-line `time` field.
//! * **Native** (kimi-code 0.1.1+): events split into `metadata`,
//!   `config.update`, `turn.prompt`, `context.append_message` (user
//!   prompts only), `context.append_loop_event` (assistant
//!   `content.part` / `tool.call` / `tool.result` / step bookkeeping),
//!   `usage.record`. Each event-bearing line carries `"time"` in epoch
//!   milliseconds.
//!
//! The parser walks the file once, dispatching per-line by `type`, and
//! reuses a single accumulator so the message order matches on-disk
//! order regardless of which format the line uses.

mod dispatch;
mod index;
mod subagents;

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use crate::models::{Message, Provider, SessionMeta};
use crate::provider::{ParsedSession, token_totals_from_usage_events};
use crate::provider_utils::{NO_PROJECT, project_name_from_path, session_title};
use crate::services::tail_reader::open_tail_reader;

use dispatch::{ScanAccum, dispatch_line};
use index::{StateJson, split_session_path};
use subagents::collect_subagent_descriptions;

pub(crate) use index::SessionIndex;
pub use index::session_id_for_path;

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

/// `time` fields in the new wire format are epoch milliseconds.
/// `metadata.created_at` is also epoch milliseconds. We treat both
/// uniformly: convert to (epoch_seconds, rfc3339_string).
fn time_ms_to_parts(ms: i64) -> Option<(i64, String)> {
    crate::provider_utils::epoch_ms_to_rfc3339(ms).map(|rfc| (ms.div_euclid(1000), rfc))
}

fn scan_lines<R: BufRead>(reader: R, path: &Path, accum: &mut ScanAccum) {
    let stats = crate::provider_utils::for_each_jsonl_record(reader, path, |_, entry: Value| {
        dispatch_line(accum, &entry);
        std::ops::ControlFlow::Continue(())
    });
    accum.note_warnings(
        stats
            .read_error_count
            .saturating_add(stats.parse_error_count),
    );
}

// ---------------------------------------------------------------------------
// Full-file parse entry point
// ---------------------------------------------------------------------------

/// Parse one `<session>/agents/<name>/wire.jsonl`. Returns `None` only
/// for non-recoverable issues (file open failure, unrecognised layout)
/// — file-level recoverable problems return Some(...) with
/// `parse_warning_count > 0`.
pub(crate) fn parse_session(path: &Path, index: &SessionIndex) -> Option<ParsedSession> {
    let (session_dir, agent_name) = match split_session_path(path) {
        Some(parts) => parts,
        None => {
            log::warn!(
                "Kimi wire.jsonl path '{}' does not match <session_dir>/agents/<name>/wire.jsonl",
                path.display()
            );
            return None;
        }
    };

    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "failed to open Kimi wire.jsonl '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let file_meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(error) => {
            log::warn!(
                "failed to stat Kimi wire.jsonl '{}': {error}",
                path.display()
            );
            return None;
        }
    };

    let state = StateJson::load(&session_dir);
    let mut accum = ScanAccum::new();
    scan_lines(BufReader::new(file), path, &mut accum);
    let token_totals = token_totals_from_usage_events(&accum.usage_events);

    let parent_agent = state.agents.get(&agent_name).cloned().unwrap_or(None);
    let is_subagent = parent_agent.is_some();
    let parent_is_main = parent_agent.as_deref() == Some("main");

    let session_dir_name = session_dir.file_name()?.to_string_lossy().to_string();
    let session_id = if is_subagent {
        // No "official" global id for subagents — kimi-code identifies
        // them only by name local to a session. Combining parent dir + agent
        // name keeps the DB primary key globally unique while still being
        // resolvable back to the on-disk path. resume_command in mod.rs
        // strips the suffix before passing to `kimi --session`.
        format!("{session_dir_name}:{agent_name}")
    } else {
        session_dir_name.clone()
    };

    if accum.messages.is_empty() && accum.usage_events.is_empty() {
        log::debug!(
            "Kimi session '{}' parsed to no messages or usage — skipping",
            path.display()
        );
        return None;
    }

    // Title resolution:
    //   * Parent: state.json.title (kimi-code's own display label) →
    //     first user message heuristic.
    //   * Subagent: state.json is shared with the parent so its title
    //     is useless. The parent's `Agent` tool.call carries the short,
    //     intentional `description` the LLM chose for the subtask —
    //     prefer that. Fall back to the heuristic over the subagent's
    //     own first user message, which is typically a 1k+ char blob
    //     of `<git-context>` + environment prefixed to the real prompt.
    let title = if is_subagent {
        // Walk to the DIRECT parent agent (could be main or another
        // subagent) and scan its wire.jsonl for the Agent tool.call
        // that spawned us. Falls back to the first user message
        // (`<git-context>…` blob) heuristic when the description is
        // unavailable.
        let parent_agent_name = parent_agent.clone().unwrap_or_else(|| "main".to_string());
        let parent_wire = session_dir
            .join("agents")
            .join(&parent_agent_name)
            .join("wire.jsonl");
        state
            .swarm_items
            .get(&agent_name)
            .cloned()
            .or_else(|| {
                let descriptions = collect_subagent_descriptions(&parent_wire);
                descriptions.get(&agent_name).cloned()
            })
            .unwrap_or_else(|| session_title(accum.first_user_message.as_deref()))
    } else {
        state
            .title
            .clone()
            .unwrap_or_else(|| session_title(accum.first_user_message.as_deref()))
    };

    let project_path = index
        .lookup_workdir(&session_dir_name, &session_dir)
        .unwrap_or_else(|| NO_PROJECT.to_string());
    let project_name = project_name_from_path(&project_path);

    let state_created = state
        .created_at
        .as_deref()
        .and_then(crate::provider_utils::parse_rfc3339_epoch_seconds);
    let state_updated = state
        .updated_at
        .as_deref()
        .and_then(crate::provider_utils::parse_rfc3339_epoch_seconds);

    let Some(created_at) = accum.first_time_secs.or(state_created) else {
        log::warn!(
            "skipping Kimi session '{}': no usable timestamp found",
            path.display()
        );
        return None;
    };
    let updated_at = accum.last_time_secs.or(state_updated).unwrap_or(created_at);

    let content_text = accum.content_parts.join("\n");

    let parent_id = if is_subagent {
        // Direct parent: the agent named in state.json.parentAgentId.
        // If that's `main`, the parent is the top-level session; for
        // any other agent (e.g. `agent-0`), the parent itself is a
        // subagent and its id is `<session_dir>:<agent>`.
        if parent_is_main {
            Some(session_dir_name.clone())
        } else {
            parent_agent
                .as_deref()
                .map(|a| format!("{session_dir_name}:{a}"))
        }
    } else {
        None
    };

    let meta = SessionMeta {
        id: session_id,
        provider: Provider::Kimi,
        title,
        project_path,
        project_name,
        created_at,
        updated_at,
        message_count: accum.messages.len() as u32,
        file_size_bytes: file_meta.len(),
        source_path: path.to_string_lossy().to_string(),
        is_sidechain: is_subagent,
        variant_name: None,
        model: accum.current_model.clone(),
        cc_version: None,
        git_branch: None,
        parent_id,
        input_tokens: token_totals.input_tokens,
        output_tokens: token_totals.output_tokens,
        cache_read_tokens: token_totals.cache_read_tokens,
        cache_write_tokens: token_totals.cache_write_tokens,
    };

    let source_mtime = file_meta
        .modified()
        .ok()
        .and_then(crate::provider::system_time_to_epoch_seconds)
        .unwrap_or(0);

    Some(ParsedSession {
        meta,
        messages: accum.messages,
        content_text,
        parse_warning_count: accum.parse_warning_count,
        child_session_ids: Vec::new(),
        usage_events: accum.usage_events,
        source_mtime,
    })
}

// ---------------------------------------------------------------------------
// Tail parse — fast path for SessionView's negative-offset windows.
// ---------------------------------------------------------------------------

pub struct KimiTailResult {
    pub messages: Vec<Message>,
    pub parse_warning_count: u32,
}

/// Parse only the last ~target_messages worth of lines from a
/// `wire.jsonl`. Returns None if the file cannot be opened or the tail
/// produced zero messages (caller should fall through to full parse).
///
/// Trade-offs match the other tail parsers: tool.call/tool.result pairs
/// that straddle the boundary surface as standalone tool messages until
/// the background full-parse replaces the cache.
pub fn parse_session_tail(path: &Path, target_messages: usize) -> Option<KimiTailResult> {
    let safety_buffer = target_messages / 4 + 50;
    let mut scan_lines_count = target_messages.saturating_add(safety_buffer);
    let mut head_context = Vec::new();
    if let Ok(head_file) = File::open(path) {
        for line in BufReader::new(head_file)
            .lines()
            .take(4)
            .map_while(Result::ok)
        {
            if let Ok(entry) = serde_json::from_str::<Value>(&line)
                && matches!(
                    entry.get("type").and_then(Value::as_str),
                    Some("metadata" | "config.update")
                )
            {
                head_context.push(entry);
            }
        }
    }
    loop {
        let (reader, window) = open_tail_reader(path, scan_lines_count, "Kimi wire.jsonl")?;
        let mut accum = ScanAccum::new();
        accum.is_tail = true;
        if window.start_offset > 0 {
            for entry in &head_context {
                dispatch_line(&mut accum, entry);
            }
        }
        scan_lines(reader, path, &mut accum);
        if accum.cancel_without_snapshot {
            // The window opened mid-turn and that turn was cancelled; a wider
            // window captures the turn.prompt and rolls it back cleanly.
            if window.covers_whole_file {
                return None;
            }
            scan_lines_count = scan_lines_count.saturating_mul(2);
            continue;
        }
        if accum.messages.len() >= target_messages || window.covers_whole_file {
            if accum.messages.is_empty() {
                return None;
            }
            let drain = accum.messages.len().saturating_sub(target_messages);
            accum.messages.drain(..drain);
            return Some(KimiTailResult {
                messages: accum.messages,
                parse_warning_count: accum.parse_warning_count,
            });
        }
        scan_lines_count = scan_lines_count.saturating_mul(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MessageRole;
    use std::path::PathBuf;

    fn write_wire(dir: &Path, agent: &str, lines: &[&str]) -> PathBuf {
        let agent_dir = dir.join("agents").join(agent);
        std::fs::create_dir_all(&agent_dir).unwrap();
        let path = agent_dir.join("wire.jsonl");
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    fn write_state(dir: &Path, json: &str) {
        std::fs::write(dir.join("state.json"), json).unwrap();
    }

    #[test]
    fn parses_format_b_basic_session() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_demo_abc").join("session_aaaa");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{
                "createdAt": "2026-05-25T09:26:36.474Z",
                "updatedAt": "2026-05-25T09:26:40.000Z",
                "title": "Demo title",
                "agents": {
                    "main": { "type": "main", "parentAgentId": null }
                }
            }"#,
        );
        let path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"config.update","modelAlias":"kimi-code/kimi-for-coding","time":1779701196500}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"hi"}],"toolCalls":[]}}"#,
                r#"{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"think","think":"thinking..."}},"time":1779701200000}"#,
                r#"{"type":"context.append_loop_event","event":{"type":"content.part","part":{"type":"text","text":"Hello!"}},"time":1779701200500}"#,
                r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":10,"output":5,"inputCacheRead":100,"inputCacheCreation":0},"usageScope":"turn","time":1779701200600}"#,
            ],
        );
        let parsed = parse_session(&path, &SessionIndex::default()).expect("parses");
        assert_eq!(parsed.meta.title, "Demo title");
        assert!(!parsed.meta.is_sidechain);
        // user, thinking (System), assistant
        assert_eq!(parsed.messages.len(), 3);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[1].role, MessageRole::System);
        assert!(parsed.messages[1].content.starts_with("[thinking]"));
        assert_eq!(parsed.messages[2].role, MessageRole::Assistant);
        assert_eq!(parsed.messages[2].content, "Hello!");
        assert_eq!(
            parsed.messages[2].model.as_deref(),
            Some("kimi-code/kimi-for-coding")
        );
        let usage = parsed.messages[2].token_usage.as_ref().expect("usage");
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_input_tokens, 100);
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(parsed.usage_events.len(), 1);
        assert_eq!(parsed.meta.input_tokens, 10);
        assert_eq!(parsed.meta.output_tokens, 5);
        assert_eq!(parsed.meta.cache_read_tokens, 100);
    }

    #[test]
    fn parses_usage_only_agent_session() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_demo").join("session_usage");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{"title":"Usage only","agents":{"main":{"parentAgentId":null}}}"#,
        );
        let path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"usage.record","model":"kimi-test","usage":{"inputOther":10,"output":5,"inputCacheRead":20},"usageScope":"turn","time":1779701200600}"#,
            ],
        );

        let parsed = parse_session(&path, &SessionIndex::default()).unwrap();

        assert!(parsed.messages.is_empty());
        assert_eq!(parsed.usage_events.len(), 1);
        assert_eq!(parsed.meta.input_tokens, 10);
        assert_eq!(parsed.meta.output_tokens, 5);
        assert_eq!(parsed.meta.cache_read_tokens, 20);
    }

    #[test]
    fn parse_session_tail_expands_for_tool_dense_windows() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_demo").join("session_tools");
        std::fs::create_dir_all(&session_dir).unwrap();
        let mut lines = vec![
            r#"{"type":"metadata","protocol_version":"1.4","created_at":1779701196480}"#
                .to_string(),
            r#"{"type":"config.update","modelAlias":"kimi-test","time":1779701196490}"#.to_string(),
            r#"{"type":"turn.prompt","time":1779701196500}"#.to_string(),
        ];
        for index in 0..100 {
            lines.push(
                serde_json::json!({
                    "type": "context.append_loop_event",
                    "event": {
                        "type": "tool.call",
                        "toolCallId": format!("tc_{index}"),
                        "name": "Read",
                        "args": {"path": "file.rs"}
                    },
                    "time": 1779701196600i64 + index * 3
                })
                .to_string(),
            );
            lines.push(
                serde_json::json!({
                    "type": "context.append_loop_event",
                    "event": {
                        "type": "tool.result",
                        "toolCallId": format!("tc_{index}"),
                        "result": {"output": "ok"}
                    },
                    "time": 1779701196601i64 + index * 3
                })
                .to_string(),
            );
            lines.push(
                serde_json::json!({
                    "type": "context.append_loop_event",
                    "event": {
                        "type": "step.end",
                        "usage": {"inputOther": 1, "output": 1}
                    },
                    "time": 1779701196602i64 + index * 3
                })
                .to_string(),
            );
        }
        let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
        let path = write_wire(&session_dir, "main", &refs);

        let tail = parse_session_tail(&path, 80).unwrap();
        assert_eq!(tail.messages.len(), 80);
        assert_eq!(tail.parse_warning_count, 0);
    }

    #[test]
    fn parse_session_tail_cancel_without_prompt_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_demo").join("session_cancel");
        std::fs::create_dir_all(&session_dir).unwrap();
        let mut lines = vec![
            r#"{"type":"metadata","protocol_version":"1.4","created_at":1779701196480}"#
                .to_string(),
            r#"{"type":"turn.prompt","time":1779701196500}"#.to_string(),
        ];
        for index in 0..80 {
            lines.push(
                serde_json::json!({
                    "type": "context.append_loop_event",
                    "event": {
                        "type": "tool.call",
                        "toolCallId": format!("tc_{index}"),
                        "name": "Read",
                        "args": {"path": "file.rs"}
                    },
                    "time": 1779701196600i64 + index
                })
                .to_string(),
            );
        }
        lines.push(
            r#"{"type":"usage.record","model":"kimi-test","usage":{"inputOther":10,"output":5},"usageScope":"turn","time":1779701196800}"#
                .to_string(),
        );
        lines.push(r#"{"type":"turn.cancel","time":1779701196900}"#.to_string());
        let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
        let path = write_wire(&session_dir, "main", &refs);

        assert!(parse_session_tail(&path, 1).is_none());
        let parsed = parse_session(&path, &SessionIndex::default()).expect("usage is retained");
        assert!(parsed.messages.is_empty());
        assert_eq!(parsed.meta.input_tokens, 10);
    }

    #[test]
    fn parse_session_tail_widens_past_cancelled_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_demo").join("session_widen");
        std::fs::create_dir_all(&session_dir).unwrap();
        let mut lines = vec![
            r#"{"type":"metadata","protocol_version":"1.4","created_at":1779701196480}"#
                .to_string(),
            r#"{"type":"turn.prompt","time":1779701196500}"#.to_string(),
        ];
        // A cancelled turn dense enough that the initial tail window starts
        // inside it (after its turn.prompt, before its turn.cancel).
        for index in 0..100 {
            lines.push(
                serde_json::json!({
                    "type": "context.append_loop_event",
                    "event": {
                        "type": "tool.call",
                        "toolCallId": format!("tc_{index}"),
                        "name": "Read",
                        "args": {"path": "file.rs"}
                    },
                    "time": 1779701196600i64 + index
                })
                .to_string(),
            );
        }
        lines.push(r#"{"type":"turn.cancel","time":1779701196800}"#.to_string());
        lines.push(r#"{"type":"turn.prompt","time":1779701196900}"#.to_string());
        for index in 0..10 {
            lines.push(
                serde_json::json!({
                    "type": "context.append_loop_event",
                    "event": {
                        "type": "content.part",
                        "part": {"type": "text", "text": format!("answer {index}")}
                    },
                    "time": 1779701197000i64 + index
                })
                .to_string(),
            );
        }
        let refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
        let path = write_wire(&session_dir, "main", &refs);

        let tail = parse_session_tail(&path, 5).expect("widened window rolls the cancel back");
        assert_eq!(tail.messages.len(), 5);
        assert_eq!(tail.parse_warning_count, 0);
        assert!(
            tail.messages
                .iter()
                .all(|message| message.role == MessageRole::Assistant)
        );
    }

    #[test]
    fn pairs_format_b_tool_call_and_result() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_x_zz").join("session_bbbb");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{"agents":{"main":{"type":"main","parentAgentId":null}}}"#,
        );
        let path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"run command"}],"toolCalls":[]}}"#,
                r#"{"type":"context.append_loop_event","event":{"type":"tool.call","toolCallId":"tc_1","uuid":"uuid_1","turnId":"turn_1","step":2,"stepUuid":"step_1","name":"Shell","args":{"command":"pwd"},"description":"Run pwd","display":{"kind":"bash","cwd":"/Users/alice/project","command":"pwd"}},"time":1779701197000}"#,
                r#"{"type":"context.append_loop_event","event":{"type":"tool.result","toolCallId":"tc_1","result":{"output":"hello world"}},"time":1779701197500}"#,
            ],
        );
        let parsed = parse_session(&path, &SessionIndex::default()).expect("parses");
        // user + tool (call+result merged)
        assert_eq!(parsed.messages.len(), 2);
        let tool = &parsed.messages[1];
        assert_eq!(tool.role, MessageRole::Tool);
        assert_eq!(tool.tool_name.as_deref(), Some("Bash"));
        assert_eq!(tool.content, "hello world");
        let input: Value = serde_json::from_str(tool.tool_input.as_ref().unwrap()).unwrap();
        assert_eq!(input["command"], "pwd");
        let metadata = tool.tool_metadata.as_ref().expect("tool metadata");
        assert_eq!(metadata.result_kind.as_deref(), Some("terminal_output"));
        assert_eq!(
            metadata.ids.get("kimi_uuid").map(String::as_str),
            Some("uuid_1")
        );
        assert_eq!(
            metadata.ids.get("turn_id").map(String::as_str),
            Some("turn_1")
        );
        assert_eq!(metadata.ids.get("step").map(String::as_str), Some("2"));
        assert_eq!(
            metadata.ids.get("step_uuid").map(String::as_str),
            Some("step_1")
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("output"))
                .and_then(Value::as_str),
            Some("hello world")
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("callDescription"))
                .and_then(Value::as_str),
            Some("Run pwd")
        );
        assert_eq!(
            metadata
                .structured
                .as_ref()
                .and_then(|value| value.get("callDisplay"))
                .and_then(|value| value.get("cwd"))
                .and_then(Value::as_str),
            Some("/Users/alice/project")
        );
    }

    #[test]
    fn agent_swarm_exposes_completed_subagents_for_open_links() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_x_zz").join("session_swarm");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{
                "agents": {
                    "main": {"type":"main","parentAgentId":null},
                    "agent-2": {"type":"sub","parentAgentId":"main"},
                    "agent-3": {"type":"sub","parentAgentId":"main"},
                    "agent-4": {"type":"sub","parentAgentId":"main"},
                    "agent-5": {"type":"sub","parentAgentId":"main"}
                }
            }"#,
        );

        let output = r#"<agent_swarm_result>
<summary>completed: 3, failed: 1</summary>
<subagent agent_id="agent-2" item="src-tauri/src/providers/kimi/tools.rs" outcome="completed">done</subagent>
<subagent agent_id="agent-3" item="src-tauri/src/providers/kimi/parser/dispatch.rs" outcome="completed">done</subagent>
<subagent agent_id="agent-4" item="src-tauri/src/tool_metadata/names.rs" outcome="completed">done</subagent>
<subagent agent_id="agent-5" item="src-tauri/src/providers/kimi/parser/subagents.rs" outcome="failed">failed</subagent>
</agent_swarm_result>"#;
        let call_line = serde_json::json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "tool.call",
                "toolCallId": "swarm_1",
                "name": "AgentSwarm",
                "args": {
                    "description": "深度分析 kimi provider 工具映射",
                    "items": [
                        "src-tauri/src/providers/kimi/tools.rs",
                        "src-tauri/src/providers/kimi/parser/dispatch.rs",
                        "src-tauri/src/tool_metadata/names.rs",
                        "src-tauri/src/providers/kimi/parser/subagents.rs"
                    ],
                    "prompt_template": "请深度分析这个文件：{{item}}"
                }
            },
            "time": 1779701197000i64
        })
        .to_string();
        let result_line = serde_json::json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "tool.result",
                "toolCallId": "swarm_1",
                "result": {"output": output}
            },
            "time": 1779701197500i64
        })
        .to_string();
        let parent_path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"inspect"}],"toolCalls":[]}}"#,
                call_line.as_str(),
                result_line.as_str(),
            ],
        );
        let parsed = parse_session(&parent_path, &SessionIndex::default()).expect("parses");
        let tool = &parsed.messages[1];
        assert_eq!(tool.tool_name.as_deref(), Some("Agent"));

        let structured = tool
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.structured.as_ref())
            .expect("structured metadata");
        let ids: Vec<&str> = structured
            .get("childConversationIds")
            .and_then(Value::as_array)
            .expect("child ids")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(ids, vec!["agent-2", "agent-3", "agent-4"]);
        let prompts: Vec<&str> = structured
            .get("childPrompts")
            .and_then(Value::as_array)
            .expect("child prompts")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            prompts,
            vec![
                "src-tauri/src/providers/kimi/tools.rs",
                "src-tauri/src/providers/kimi/parser/dispatch.rs",
                "src-tauri/src/tool_metadata/names.rs",
            ]
        );

        let sub_path = write_wire(
            &session_dir,
            "agent-2",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701197600}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"long generated swarm prompt"}],"toolCalls":[]}}"#,
            ],
        );
        let sub = parse_session(&sub_path, &SessionIndex::default()).expect("sub parses");
        assert_eq!(sub.meta.title, "src-tauri/src/providers/kimi/tools.rs");
    }

    #[test]
    fn subagent_title_uses_state_swarm_item_when_parent_result_is_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_teli_hash").join("session_swarm_state");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{
                "agents": {
                    "main": {"type":"main","parentAgentId":null},
                    "agent-2": {
                        "type":"sub",
                        "parentAgentId":"main",
                        "swarmItem":"apps/desktop/src-tauri/tests/db_tests.rs, apps/desktop/src-tauri/tests/export_import_tests.rs, apps/desktop/src-tauri/tests/util_tests.rs"
                    }
                }
            }"#,
        );

        let sub_path = write_wire(
            &session_dir,
            "agent-2",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701197600}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"你正在 review Teli 项目的源代码。这个 prompt 很长，不应该成为 swarm 子代理标题。"}],"toolCalls":[]}}"#,
            ],
        );

        let sub = parse_session(&sub_path, &SessionIndex::default()).expect("sub parses");
        assert_eq!(
            sub.meta.title,
            "apps/desktop/src-tauri/tests/db_tests.rs, apps/desktop/src-tauri/tests/export_import_tests.rs, apps/desktop/src-tauri/tests/util_tests.rs"
        );
    }

    #[test]
    fn parses_format_a_migrated_session() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_y_yy").join("ses_cccc");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{
                "createdAt": "2026-05-01T08:24:04.612Z",
                "updatedAt": "2026-05-01T08:24:04.612Z",
                "title": "Migrated",
                "agents": {"main": {"type":"main","parentAgentId":null}}
            }"#,
        );
        let path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1777623844612}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"check files"}],"toolCalls":[]}}"#,
                r#"{"type":"context.append_message","message":{"role":"assistant","content":[{"type":"think","think":"let me look"}],"toolCalls":[{"type":"function","id":"tc_a","function":{"name":"Shell","arguments":"{\"command\":\"ls\"}"}}]}}"#,
                r#"{"type":"context.append_message","message":{"role":"tool","content":[{"type":"text","text":"file1\nfile2"}],"toolCalls":[],"toolCallId":"tc_a"}}"#,
            ],
        );
        let parsed = parse_session(&path, &SessionIndex::default()).expect("parses");
        // user + assistant thinking + tool (merged)
        assert_eq!(parsed.messages.len(), 3);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[1].role, MessageRole::System);
        let tool = &parsed.messages[2];
        assert_eq!(tool.role, MessageRole::Tool);
        // Shell → canonicalised to Bash
        assert_eq!(tool.tool_name.as_deref(), Some("Bash"));
        assert_eq!(tool.content, "file1\nfile2");
    }

    #[test]
    fn subagent_links_parent_via_state_json() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_z_zz").join("session_dddd");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{
                "agents": {
                    "main": {"type":"main","parentAgentId":null},
                    "agent-0": {"type":"sub","parentAgentId":"main"}
                }
            }"#,
        );
        // Both agents need at least one user message for the parser to keep them.
        write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"parent prompt"}],"toolCalls":[]}}"#,
            ],
        );
        let sub_path = write_wire(
            &session_dir,
            "agent-0",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196500}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"sub prompt"}],"toolCalls":[]}}"#,
            ],
        );
        let sub = parse_session(&sub_path, &SessionIndex::default()).expect("sub parses");
        assert!(sub.meta.is_sidechain);
        assert_eq!(sub.meta.id, "session_dddd:agent-0");
        assert_eq!(sub.meta.parent_id.as_deref(), Some("session_dddd"));
    }

    #[test]
    fn project_path_comes_from_session_index() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("wd_proj_hash").join("session_eeee");
        std::fs::create_dir_all(&session_dir).unwrap();
        write_state(
            &session_dir,
            r#"{"agents":{"main":{"type":"main","parentAgentId":null}}}"#,
        );
        let path = write_wire(
            &session_dir,
            "main",
            &[
                r#"{"type":"metadata","protocol_version":"1.0","created_at":1779701196480}"#,
                r#"{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"hi"}],"toolCalls":[]}}"#,
            ],
        );
        let mut index = SessionIndex::default();
        index
            .by_id
            .insert("session_eeee".to_string(), "/home/user/proj".to_string());
        let parsed = parse_session(&path, &index).expect("parses");
        assert_eq!(parsed.meta.project_path, "/home/user/proj");
        assert_eq!(parsed.meta.project_name, "proj");
    }

    #[test]
    fn session_id_for_path_strips_layout() {
        let p = Path::new("/home/u/.kimi-code/sessions/wd_x_yy/session_abc/agents/main/wire.jsonl");
        assert_eq!(session_id_for_path(p).as_deref(), Some("session_abc"));
        let p2 = Path::new("/home/u/.kimi-code/sessions/wd_x_yy/ses_abc/agents/agent-0/wire.jsonl");
        assert_eq!(session_id_for_path(p2).as_deref(), Some("ses_abc"));
        let bogus = Path::new("/etc/passwd");
        assert!(session_id_for_path(bogus).is_none());
    }
}
