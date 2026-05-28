//! Parse a Cursor CLI `agent-transcripts/<sessionId>/<sessionId>.jsonl`
//! into a `ParsedSession`. The wire format is one JSON object per line:
//!
//! ```json
//! {"role":"user|assistant","message":{"content":[<parts>]}}
//! ```
//!
//! Each part is `{"type":"text", "text":"…"}` or
//! `{"type":"tool_use", "name":"…", "id":"…", "input":{…}}`. Tool
//! results don't appear as standalone parts — Cursor folds them into
//! the next assistant turn's text content. There is no per-line
//! timestamp; we fall back to the file's filesystem mtime/ctime.
//!
//! Subagent transcripts live next door at
//! `<sessionId>/subagents/<subagentId>.jsonl`; they share the same
//! line shape, and we link them by directory structure rather than an
//! embedded parent id.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::models::{Message, MessageRole, Provider, SessionMeta};
use crate::provider::ParsedSession;
use crate::provider_utils::{
    project_name_from_path, session_title, truncate_to_bytes, FTS_CONTENT_LIMIT,
};
use crate::tool_metadata::{build_tool_metadata, ToolCallFacts};

use super::tools::{
    extract_text_from_content, extract_think_content, normalise_user_text, parse_content_array,
    remap_tool_args, strip_redacted, strip_think_tags,
};

// ---------------------------------------------------------------------------
// Per-line walk
// ---------------------------------------------------------------------------

/// Iterate each line of a JSONL transcript, handing the role + raw
/// `message.content` value to `handler`. Returns the count of lines
/// that failed to parse — the caller threads this into
/// `ParsedSession.parse_warning_count` so the UI can show a ⚠ badge.
fn for_each_entry(
    content: &str,
    source_label: &str,
    mut handler: impl FnMut(&str, Option<&Value>),
) -> u32 {
    let mut warnings = 0u32;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(error) => {
                log::warn!(
                    "skipping malformed Cursor transcript JSONL in '{source_label}': {error}",
                );
                warnings = warnings.saturating_add(1);
                continue;
            }
        };
        let role = entry.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content_val = entry.get("message").and_then(|m| m.get("content"));
        handler(role, content_val);
    }
    warnings
}

/// Walk the transcript and build the in-order message list the UI
/// renders. Returns `(messages, parse_warning_count)`.
pub(crate) fn parse_messages(content: &str, source_label: &str) -> (Vec<Message>, u32) {
    let mut messages = Vec::new();
    let warnings = for_each_entry(content, source_label, |role, content_val| match role {
        "user" => {
            let raw = extract_text_from_content(content_val);
            let clean = normalise_user_text(&raw);
            if clean.is_empty() {
                return;
            }
            messages.push(Message {
                role: MessageRole::User,
                content: clean,
                timestamp: None,
                tool_name: None,
                tool_input: None,
                token_usage: None,
                model: None,
                usage_hash: None,
                tool_metadata: None,
            });
        }
        "assistant" => {
            let raw = extract_text_from_content(content_val);
            let cleaned = strip_redacted(&raw);

            if let Some(thinking) = extract_think_content(&cleaned) {
                messages.push(Message {
                    role: MessageRole::System,
                    content: format!("[thinking]\n{thinking}"),
                    timestamp: None,
                    tool_name: None,
                    tool_input: None,
                    token_usage: None,
                    model: None,
                    usage_hash: None,
                    tool_metadata: None,
                });
            }

            let visible = strip_think_tags(&cleaned);
            if !visible.is_empty() {
                messages.push(Message {
                    role: MessageRole::Assistant,
                    content: visible,
                    timestamp: None,
                    tool_name: None,
                    tool_input: None,
                    token_usage: None,
                    model: None,
                    usage_hash: None,
                    tool_metadata: None,
                });
            }

            for part in parse_content_array(content_val) {
                if part.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                    continue;
                }
                push_tool_call(&mut messages, &part);
            }
        }
        _ => {}
    });
    (messages, warnings)
}

fn push_tool_call(messages: &mut Vec<Message>, part: &Value) {
    let raw_name = part.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
    let args = part.get("input");
    let call_id = part
        .get("id")
        .or_else(|| part.get("tool_use_id"))
        .and_then(|v| v.as_str());
    let metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Cursor,
        raw_name,
        input: args,
        call_id,
        assistant_id: None,
    });
    let display_name = metadata.canonical_name.clone();
    let tool_input = args.and_then(|a| remap_tool_args(&display_name, a));
    messages.push(Message {
        role: MessageRole::Tool,
        content: String::new(),
        timestamp: None,
        tool_name: Some(display_name),
        tool_input,
        token_usage: None,
        model: None,
        usage_hash: None,
        tool_metadata: Some(metadata),
    });
}

// ---------------------------------------------------------------------------
// Path → identity helpers
// ---------------------------------------------------------------------------

/// Determine whether `path` is a subagent transcript (lives under
/// `…/<parentId>/subagents/<subagentId>.jsonl`). Returns the parent
/// session id when so.
pub(crate) fn parent_id_for_subagent(path: &Path) -> Option<String> {
    let subagents_dir = path.parent()?;
    if subagents_dir.file_name().and_then(|n| n.to_str()) != Some("subagents") {
        return None;
    }
    Some(
        subagents_dir
            .parent()?
            .file_name()?
            .to_string_lossy()
            .to_string(),
    )
}

/// Decode the Cursor projects dir name back into a filesystem path. The
/// CLI sanitises `/` to `-`, so `Users-john-Documents-proj` maps to
/// `/Users/john/Documents/proj`. We try the simple dash → slash swap
/// first, then greedy reconstruction against the local filesystem so
/// project names that themselves contain dashes still resolve.
pub(crate) fn decode_project_key(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    let direct = format!("/{}", key.replace('-', "/"));
    if Path::new(&direct).is_dir() {
        return direct;
    }
    let segments: Vec<&str> = key.split('-').collect();
    let mut path = String::from("/");
    let mut i = 0;
    while i < segments.len() {
        let mut consumed = 0usize;
        for end in (i + 1..=segments.len()).rev() {
            let part = segments[i..end].join("-");
            let candidate = if path == "/" {
                format!("/{part}")
            } else {
                format!("{path}/{part}")
            };
            if Path::new(&candidate).exists() {
                path = candidate;
                consumed = end - i;
                break;
            }
        }
        if consumed == 0 {
            if path == "/" {
                path = format!("/{}", segments[i]);
            } else {
                path = format!("{}/{}", path, segments[i]);
            }
            i += 1;
        } else {
            i += consumed;
        }
    }
    path
}

/// Fall back to decoding the project key in the transcript path
/// (`~/.cursor/projects/<KEY>/agent-transcripts/<id>/<id>.jsonl`).
fn project_path_from_transcript_path(path: &Path) -> String {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir.file_name().and_then(|n| n.to_str()) == Some("agent-transcripts") {
            if let Some(project_key_dir) = dir.parent() {
                let key = project_key_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                return decode_project_key(key);
            }
        }
        current = dir.parent();
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Subagent title: match against parent's Task/Subagent tool_use
// ---------------------------------------------------------------------------

/// Open the parent transcript and pull out the description of the
/// Task / Subagent tool call whose `prompt` matches this subagent's
/// first user message. Returns `(description, ordering_index)` so the
/// caller can sort siblings by spawn order.
pub(crate) fn subagent_description(
    parent_path: &Path,
    subagent_first_user: &str,
) -> Option<(String, usize)> {
    let content = match std::fs::read_to_string(parent_path) {
        Ok(c) => c,
        Err(error) => {
            log::warn!(
                "failed to read Cursor parent transcript '{}': {error}",
                parent_path.display()
            );
            return None;
        }
    };

    let mut candidates: Vec<(String, String)> = Vec::new();
    let _ = for_each_entry(
        &content,
        &parent_path.display().to_string(),
        |role, content_val| {
            if role != "assistant" {
                return;
            }
            for part in parse_content_array(content_val) {
                if part.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                    continue;
                }
                let name = part.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if name != "Task" && name != "Subagent" {
                    continue;
                }
                let Some(input) = part.get("input") else {
                    continue;
                };
                let description = input
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                if !description.is_empty() && !prompt.is_empty() {
                    candidates.push((prompt.to_string(), description.to_string()));
                }
            }
        },
    );

    for (idx, (prompt, desc)) in candidates.iter().enumerate() {
        if prompt == subagent_first_user
            || subagent_first_user.starts_with(prompt.as_str())
            || prompt.starts_with(subagent_first_user)
        {
            return Some((desc.clone(), idx));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Full parse entry point
// ---------------------------------------------------------------------------

/// Parse `path` into a `ParsedSession`. Returns None for non-recoverable
/// errors (open/stat failure, no messages survived). Project path is
/// resolved from `Workspace Path:` in the transcript first, then by
/// decoding the parent project-key directory.
///
/// `provider_project_path_override` lets callers (e.g. scan_all) inject
/// a workspace path they recovered from `~/.cursor/chats/.../store.db`,
/// which is more reliable than the sanitised dir name when the project
/// path itself contained dashes.
pub(crate) fn parse_session(
    path: &Path,
    provider_project_path_override: Option<String>,
) -> Option<ParsedSession> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(error) => {
            log::warn!(
                "failed to read Cursor transcript '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let source_label = path.display().to_string();
    let (messages, parse_warning_count) = parse_messages(&content, &source_label);
    if messages.is_empty() {
        return None;
    }

    let file_id = path.file_stem()?.to_string_lossy().to_string();
    let parent_id = parent_id_for_subagent(path);
    let is_sidechain = parent_id.is_some();

    let first_user = messages
        .iter()
        .find(|m| m.role == MessageRole::User && !m.content.is_empty())
        .map(|m| m.content.clone());

    // Title: subagents try to match the parent's Task/Subagent
    // description so each child shows up with a distinct, meaningful
    // label. created_at gets a small per-task offset so siblings sort
    // in spawn order under the parent.
    let (title, task_index) = if let Some(parent_session) = parent_id.as_deref() {
        let parent_transcript = path
            .parent()
            .and_then(|p| p.parent())
            .map(|d| d.join(format!("{parent_session}.jsonl")));
        match parent_transcript
            .as_deref()
            .and_then(|pp| subagent_description(pp, first_user.as_deref().unwrap_or("")))
        {
            Some((desc, idx)) => (desc, idx as i64),
            None => (session_title(first_user.as_deref()), 0),
        }
    } else {
        (session_title(first_user.as_deref()), 0)
    };

    let project_path = provider_project_path_override
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| project_path_from_transcript_path(path));
    let project_name = project_name_from_path(&project_path);

    let file_meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(error) => {
            log::warn!(
                "failed to stat Cursor transcript '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let file_size = file_meta.len();
    let created_at = file_meta
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64 + task_index)
        .or_else(|| {
            file_meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64 + task_index)
        })?;
    let updated_at = file_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(created_at);
    let source_mtime = updated_at;

    let content_text = build_fts_content(&messages);
    let message_count = messages.len() as u32;

    Some(ParsedSession {
        meta: SessionMeta {
            id: file_id,
            provider: Provider::Cursor,
            title,
            project_path,
            project_name,
            created_at,
            updated_at,
            message_count,
            file_size_bytes: file_size,
            source_path: path.to_string_lossy().to_string(),
            is_sidechain,
            variant_name: None,
            model: None,
            cc_version: None,
            git_branch: None,
            parent_id,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
        messages,
        content_text,
        parse_warning_count,
        child_session_ids: Vec::new(),
        usage_events: Vec::new(),
        source_mtime,
    })
}

fn build_fts_content(messages: &[Message]) -> String {
    let parts: Vec<String> = messages
        .iter()
        .filter(|m| {
            matches!(m.role, MessageRole::User | MessageRole::Assistant) && !m.content.is_empty()
        })
        .map(|m| m.content.clone())
        .collect();
    truncate_to_bytes(&parts.join("\n"), FTS_CONTENT_LIMIT)
}

/// Subagent transcripts under `<sessionId>/subagents/` produce one
/// candidate per file. mod.rs uses this to inherit the parent's
/// workspace path before handing off to the indexer.
pub(crate) fn subagent_paths_under(session_dir: &Path) -> Vec<PathBuf> {
    let subagents_dir = session_dir.join("subagents");
    if !subagents_dir.is_dir() {
        return Vec::new();
    }
    match std::fs::read_dir(&subagents_dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
            .collect(),
        Err(error) => {
            log::warn!(
                "failed to read Cursor subagents dir '{}': {error}",
                subagents_dir.display()
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_transcript(dir: &Path, project_key: &str, sid: &str, body: &str) -> PathBuf {
        let transcript_dir = dir
            .join(".cursor")
            .join("projects")
            .join(project_key)
            .join("agent-transcripts")
            .join(sid);
        std::fs::create_dir_all(&transcript_dir).unwrap();
        let path = transcript_dir.join(format!("{sid}.jsonl"));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn parses_user_assistant_and_tool_use_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>hello</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","id":"t1","name":"Read","input":{"path":"/tmp/a"}}]}}"#;
        let path = write_transcript(dir.path(), "TestProj", "s1", body);
        let parsed = parse_session(&path, None).expect("parses");
        assert_eq!(parsed.meta.id, "s1");
        assert_eq!(parsed.messages.len(), 3);
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content, "hello");
        assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
        assert_eq!(parsed.messages[2].role, MessageRole::Tool);
        assert_eq!(parsed.messages[2].tool_name.as_deref(), Some("Read"));
        // Edit / Read remap canonicalises `path` to `file_path` so the
        // frontend's existing renderer picks it up.
        let input: Value =
            serde_json::from_str(parsed.messages[2].tool_input.as_ref().unwrap()).unwrap();
        assert_eq!(input["file_path"], "/tmp/a");
    }

    #[test]
    fn extracts_thinking_block_to_system_role() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>do it</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"<think>reason here</think>\nresult"}]}}"#;
        let path = write_transcript(dir.path(), "TestProj", "s2", body);
        let parsed = parse_session(&path, None).expect("parses");
        let think = parsed
            .messages
            .iter()
            .find(|m| m.role == MessageRole::System)
            .expect("thinking");
        assert!(think.content.starts_with("[thinking]"));
        assert!(think.content.contains("reason here"));
    }

    #[test]
    fn image_files_block_becomes_marker() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"{"role":"user","message":{"content":[{"type":"text","text":"[Image]\n<image_files>\n1. /tmp/x.png\n</image_files>\n<user_query>see image</user_query>"}]}}"#;
        let path = write_transcript(dir.path(), "TestProj", "s3", body);
        let parsed = parse_session(&path, None).expect("parses");
        // user_query takes precedence in normalise_user_text. The image
        // marker is part of FTS content only when the prompt doesn't
        // wrap it in <user_query>. Confirm the parser doesn't crash and
        // surfaces the prompt text.
        assert_eq!(parsed.messages[0].role, MessageRole::User);
        assert_eq!(parsed.messages[0].content, "see image");
    }

    #[test]
    fn subagent_inherits_parent_path_marker() {
        let dir = tempfile::tempdir().unwrap();
        // Parent
        let parent_body = r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>spawn</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"tool_use","name":"Task","id":"t1","input":{"prompt":"explore","description":"Find files"}}]}}"#;
        let parent_path = write_transcript(dir.path(), "TestProj", "parent_sid", parent_body);
        // Subagent file under parent's subagents/
        let sub_dir = parent_path.parent().unwrap().join("subagents");
        std::fs::create_dir_all(&sub_dir).unwrap();
        let sub_path = sub_dir.join("agent-0.jsonl");
        std::fs::write(
            &sub_path,
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>explore</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#,
        )
        .unwrap();
        let parsed = parse_session(&sub_path, None).expect("parses");
        assert!(parsed.meta.is_sidechain);
        assert_eq!(parsed.meta.parent_id.as_deref(), Some("parent_sid"));
        // Title inherited from the parent's Task description.
        assert_eq!(parsed.meta.title, "Find files");
    }
}
