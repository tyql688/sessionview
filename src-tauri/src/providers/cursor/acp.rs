//! Cursor's ACP (Agent Client Protocol) sessions.
//!
//! When an IDE or third-party editor (Zed, etc.) talks to
//! `cursor-agent acp`, the session is stored under
//! `~/.cursor/acp-sessions/<sessionId>/` instead of the per-project
//! `~/.cursor/projects/<key>/agent-transcripts/<id>/<id>.jsonl` layout
//! the standalone CLI writes. ACP sessions have:
//!
//! * `meta.json` — `{ schemaVersion, cwd, title }` (no model field;
//!   that lives in `store.db` meta like the chats/ store).
//! * `store.db` — the SAME content-addressed blob layout as
//!   `~/.cursor/chats/.../store.db`, except every chat message is
//!   reachable from the latest root protobuf blob: there is NO
//!   accompanying JSONL transcript on disk.
//!
//! So we reconstruct the transcript by recursively walking the
//! root blob's protobuf hash references and collecting any JSON
//! envelope with `role` in `{user, assistant, tool}`. The blob
//! payload shape mirrors the JSONL records the standalone CLI emits
//! but uses slightly different field names (`tool-call` vs
//! `tool_use`, `toolName`/`args` vs `name`/`input`, etc).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::models::{Message, MessageRole, Provider};
use crate::provider_utils::ToolCallPairer;
use crate::tool_metadata::{ToolCallFacts, build_tool_metadata, set_tool_result_raw};

use super::store_db::{
    default_cursor_image_cache_dir, read_blob, read_meta_value, scan_pb_hash_refs,
    write_image_to_cache,
};
use super::tools::{
    extract_text_from_content, normalise_user_text, remap_tool_args, strip_redacted,
};

/// Side-channel metadata pulled from `meta.json` next to the
/// session's `store.db`.
pub(crate) struct AcpSessionMeta {
    pub cwd: Option<String>,
    pub title: Option<String>,
}

/// schemaVersion the parser was written against. Anything else gets a
/// warning so format drifts surface in logs.
const SUPPORTED_SCHEMA_VERSION: i64 = 1;

/// Load `meta.json`. Failures degrade gracefully — the rest of the
/// session still parses, the UI just shows untitled / no-project.
pub(crate) fn load_meta_json(session_dir: &Path) -> AcpSessionMeta {
    let path = session_dir.join("meta.json");
    let mut meta = AcpSessionMeta {
        cwd: None,
        title: None,
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "failed to read Cursor ACP meta.json '{}': {error}",
                    path.display()
                );
            }
            return meta;
        }
    };
    match serde_json::from_str::<Value>(&content) {
        Ok(value) => {
            if let Some(schema) = value.get("schemaVersion").and_then(|v| v.as_i64())
                && schema != SUPPORTED_SCHEMA_VERSION
            {
                log::warn!(
                    "Cursor ACP meta.json '{}' has schemaVersion {schema}, expected {SUPPORTED_SCHEMA_VERSION} — parsing may miss new fields",
                    path.display()
                );
            }
            meta.cwd = value
                .get("cwd")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            meta.title = value
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
        Err(error) => {
            log::warn!(
                "failed to parse Cursor ACP meta.json '{}': {error}",
                path.display()
            );
        }
    }
    meta
}

/// Output of one ACP store.db parse pass. Wraps messages with the
/// per-turn model harvested from `providerOptions.cursor.modelName` —
/// `meta.lastUsedModel` is missing for ACP, so this is the only path
/// to surface a real model name in the session badge.
pub(crate) struct AcpParseResult {
    pub messages: Vec<Message>,
    pub warnings: u32,
    pub model: Option<String>,
}

/// Reconstruct the message list from an ACP `store.db`, using the
/// default image cache directory. Most production callers use this.
pub(crate) fn parse_acp_transcript(store_db: &Path) -> AcpParseResult {
    let cache_dir = default_cursor_image_cache_dir();
    if cache_dir.is_none() {
        log::warn!(
            "Cursor ACP image cache directory unresolvable (dirs::data_local_dir() returned None) — tool-result images will not be cached for '{}'",
            store_db.display()
        );
    }
    parse_acp_transcript_with_cache_dir(store_db, cache_dir.as_deref())
}

/// Same as `parse_acp_transcript` but accepts an explicit
/// `cache_dir` for tests that need to inject a tempdir. Passing
/// `None` disables image extraction; production should always pass
/// `default_cursor_image_cache_dir()`.
pub(crate) fn parse_acp_transcript_with_cache_dir(
    store_db: &Path,
    cache_dir: Option<&Path>,
) -> AcpParseResult {
    let empty = AcpParseResult {
        messages: Vec::new(),
        warnings: 0,
        model: None,
    };
    let conn = match Connection::open(store_db) {
        Ok(c) => c,
        Err(error) => {
            log::warn!(
                "failed to open Cursor ACP store.db '{}': {error}",
                store_db.display()
            );
            return empty;
        }
    };

    let Some(meta_value) = read_meta_value(&conn, store_db) else {
        return empty;
    };
    let Some(root_id) = meta_value
        .get("latestRootBlobId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return empty;
    };

    let mut visited: HashSet<String> = HashSet::new();
    let mut envelopes: Vec<Value> = Vec::new();
    let mut warnings: u32 = 0;
    walk_blob(&conn, &root_id, &mut visited, &mut envelopes, &mut warnings);

    let session_id = store_db
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut messages = Vec::new();
    let mut pairer = ToolCallPairer::default();
    let mut last_model: Option<String> = None;
    for envelope in envelopes {
        translate_envelope(
            envelope,
            &mut messages,
            &mut pairer,
            &session_id,
            cache_dir,
            &mut last_model,
        );
    }
    AcpParseResult {
        messages,
        warnings,
        model: last_model,
    }
}

/// Recursively unfold the protobuf DAG: a non-JSON blob is treated as
/// a list of `0A 20 <32 bytes>` length-delimited hash references and
/// followed; a JSON blob is collected for translation. Cycles are
/// guarded by `visited`.
fn walk_blob(
    conn: &Connection,
    blob_id: &str,
    visited: &mut HashSet<String>,
    envelopes: &mut Vec<Value>,
    warnings: &mut u32,
) {
    if !visited.insert(blob_id.to_string()) {
        return;
    }
    let Some(bytes) = read_blob(conn, blob_id) else {
        return;
    };
    if bytes.first() == Some(&b'{') {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => envelopes.push(value),
            Err(error) => {
                log::warn!("skipping malformed Cursor ACP blob '{blob_id}': {error}");
                *warnings = warnings.saturating_add(1);
            }
        }
        return;
    }
    for child in scan_pb_hash_refs(&bytes) {
        walk_blob(conn, &child, visited, envelopes, warnings);
    }
}

/// Convert one ACP JSON envelope into transcript messages. Mirrors
/// parser::parse_messages but reads the ACP field shape directly:
///
/// * `role=user` → MessageRole::User with `<user_query>` stripped and
///   `<image_files>` rewritten the same way the JSONL parser does.
/// * `role=assistant` → emit `[thinking]` (System) from any `reasoning`
///   part, Assistant text from `text` parts, then one Tool message per
///   `tool-call` part. Model name is harvested from any part's
///   `providerOptions.cursor.modelName` since ACP meta has no global
///   `lastUsedModel`.
/// * `role=tool` → merge `tool-result.result` into the matching tool
///   message by `toolCallId`. Base64 images in `experimental_content`
///   are written to the shared cache and surfaced as
///   `[Image: source: …]` lines on the tool result body.
fn translate_envelope(
    envelope: Value,
    messages: &mut Vec<Message>,
    pairer: &mut ToolCallPairer,
    session_id: &str,
    cache_dir: Option<&Path>,
    last_model: &mut Option<String>,
) {
    let role = envelope.get("role").and_then(|v| v.as_str()).unwrap_or("");
    match role {
        "user" => {
            let raw = extract_text_from_content(envelope.get("content"));
            let text = normalise_user_text(&raw);
            if text.is_empty() {
                return;
            }
            messages.push(Message {
                role: MessageRole::User,
                message_kind: None,
                content: text,
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
            let Some(content) = envelope.get("content").and_then(|v| v.as_array()) else {
                return;
            };

            // Harvest model from any part — Cursor stamps the active
            // model on every reasoning/tool-call part via
            // `providerOptions.cursor.modelName`. Take the latest.
            if let Some(model) = harvest_model(content) {
                *last_model = Some(model);
            }

            // Emit each `reasoning` part as its own [thinking] System
            // message (preserves order with assistant text + tool calls).
            for part in content {
                if part.get("type").and_then(|v| v.as_str()) != Some("reasoning") {
                    continue;
                }
                let text = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let cleaned = strip_redacted(text);
                let trimmed = cleaned.trim();
                if trimmed.is_empty() {
                    continue;
                }
                messages.push(Message {
                    role: MessageRole::System,
                    message_kind: None,
                    content: format!("[thinking]\n{trimmed}"),
                    timestamp: None,
                    tool_name: None,
                    tool_input: None,
                    token_usage: None,
                    model: last_model.clone(),
                    usage_hash: None,
                    tool_metadata: None,
                });
            }

            // Collect text parts in order, strip [REDACTED], emit as one
            // assistant message.
            let mut combined_text = String::new();
            for part in content {
                if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                    let raw = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    if !raw.is_empty() {
                        if !combined_text.is_empty() {
                            combined_text.push('\n');
                        }
                        combined_text.push_str(raw);
                    }
                }
            }
            let visible = strip_redacted(&combined_text);
            if !visible.is_empty() {
                messages.push(Message {
                    role: MessageRole::Assistant,
                    message_kind: None,
                    content: visible,
                    timestamp: None,
                    tool_name: None,
                    tool_input: None,
                    token_usage: None,
                    model: last_model.clone(),
                    usage_hash: None,
                    tool_metadata: None,
                });
            }
            // Then: one Tool message per tool-call, in order.
            for part in content {
                if part.get("type").and_then(|v| v.as_str()) != Some("tool-call") {
                    continue;
                }
                push_tool_call_acp(part, messages, pairer, last_model);
            }
        }
        "tool" => {
            let Some(content) = envelope.get("content").and_then(|v| v.as_array()) else {
                return;
            };
            for part in content {
                if part.get("type").and_then(|v| v.as_str()) != Some("tool-result") {
                    continue;
                }
                merge_tool_result_acp(part, messages, pairer, session_id, cache_dir);
            }
        }
        // Intentionally dropped — system framing that the user never
        // typed and that the assistant didn't author. We don't surface
        // it but we also don't warn about it.
        "system" => {}
        "" => {
            log::warn!("Cursor ACP envelope missing role — skipped");
        }
        other => {
            log::warn!(
                "Cursor ACP envelope with unrecognised role '{other}' — skipped (new ACP role?)"
            );
        }
    }
}

/// Find the **last** non-empty `providerOptions.cursor.modelName` in
/// the envelope's parts. Cursor stamps the active model on each
/// `reasoning` / `tool-call` / sometimes `text` part; on a single
/// turn they should agree, but if a model switch happens mid-turn
/// the trailing part is what actually answered, so we prefer it.
fn harvest_model(content: &[Value]) -> Option<String> {
    let mut latest: Option<String> = None;
    for part in content {
        if let Some(name) = part
            .get("providerOptions")
            .and_then(|p| p.get("cursor"))
            .and_then(|c| c.get("modelName"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            latest = Some(name.to_string());
        }
    }
    latest
}

fn push_tool_call_acp(
    part: &Value,
    messages: &mut Vec<Message>,
    pairer: &mut ToolCallPairer,
    last_model: &Option<String>,
) {
    let raw_name = part
        .get("toolName")
        .and_then(|v| v.as_str())
        .unwrap_or("tool");
    let args = part.get("args");
    let call_id = part.get("toolCallId").and_then(|v| v.as_str());
    let metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Cursor,
        raw_name,
        input: args,
        call_id,
        assistant_id: None,
    });
    let display_name = metadata.canonical_name.clone();
    let tool_input = args.and_then(|a| remap_tool_args(&display_name, a));
    pairer.register(call_id, messages.len());
    messages.push(Message {
        role: MessageRole::Tool,
        message_kind: None,
        content: String::new(),
        timestamp: None,
        tool_name: Some(display_name),
        tool_input,
        token_usage: None,
        model: last_model.clone(),
        usage_hash: None,
        tool_metadata: Some(metadata),
    });
}

fn merge_tool_result_acp(
    part: &Value,
    messages: &mut Vec<Message>,
    pairer: &ToolCallPairer,
    session_id: &str,
    cache_dir: Option<&Path>,
) {
    let call_id = part.get("toolCallId").and_then(|v| v.as_str());
    let experimental_content = part.get("experimental_content").and_then(Value::as_array);
    let (mut body, mut is_raw) = match part.get("result") {
        Some(Value::String(result)) => (result.clone(), false),
        Some(Value::Null) | None => (String::new(), false),
        Some(result) => (result.to_string(), true),
    };
    if body.is_empty()
        && !is_raw
        && let Some(parts) = experimental_content
    {
        let mut texts = Vec::new();
        let mut unsupported = false;
        for item in parts {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => match item.get("text").and_then(Value::as_str) {
                    Some(text) if !text.is_empty() => texts.push(text.to_string()),
                    Some(_) => {}
                    None => unsupported = true,
                },
                Some("image")
                    if item.get("data").and_then(Value::as_str).is_some()
                        && item.get("mimeType").and_then(Value::as_str).is_some() => {}
                _ => unsupported = true,
            }
        }
        if unsupported {
            body = Value::Array(parts.clone()).to_string();
            is_raw = true;
        } else {
            body = texts.join("\n");
        }
    }

    // Cache any base64-encoded images in experimental_content and
    // append `[Image: source: <path>]` markers to the body so the
    // frontend renders them like every other provider's images.
    // When cache_dir is None (data_local_dir unresolvable, or test
    // intentionally disabled it) we skip the write — the parent
    // function logged the warning once at startup, no need to repeat
    // it per image.
    if !is_raw && let (Some(cache_dir), Some(arr)) = (cache_dir, experimental_content) {
        for item in arr {
            if item.get("type").and_then(|v| v.as_str()) != Some("image") {
                continue;
            }
            let Some(data) = item.get("data").and_then(|v| v.as_str()) else {
                continue;
            };
            let bytes = match BASE64.decode(data) {
                Ok(b) => b,
                Err(error) => {
                    log::warn!(
                        "Cursor ACP tool-result image base64 decode failed (session {session_id}): {error}"
                    );
                    continue;
                }
            };
            let mime = item.get("mimeType").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(path) = write_image_to_cache(cache_dir, session_id, &bytes, mime) {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(&format!("[Image: source: {}]", path.display()));
            }
        }
    }

    if let Some(msg) = pairer.message_mut(call_id, messages) {
        msg.content = body;
        if let Some(metadata) = msg.tool_metadata.as_mut() {
            set_tool_result_raw(metadata, is_raw);
        }
        return;
    }
    messages.push(Message {
        role: MessageRole::Tool,
        message_kind: None,
        content: body,
        timestamp: None,
        tool_name: None,
        tool_input: None,
        token_usage: None,
        model: None,
        usage_hash: None,
        tool_metadata: None,
    });
}

/// Walk an ACP `~/.cursor/acp-sessions/` root and return the
/// per-session paths the provider treats as source files. Each entry
/// is `<session_dir>/store.db`.
pub(crate) fn collect_acp_sessions(home_dir: &Path) -> Vec<PathBuf> {
    let acp_root = home_dir.join(".cursor").join("acp-sessions");
    let Ok(entries) = std::fs::read_dir(&acp_root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let store = path.join("store.db");
            if store.is_file() { Some(store) } else { None }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ToolResultMode;
    use serde_json::json;

    struct TranslateContext {
        messages: Vec<Message>,
        pairer: ToolCallPairer,
        last_model: Option<String>,
        cache_dir: Option<PathBuf>,
    }

    impl TranslateContext {
        fn new() -> Self {
            Self {
                messages: Vec::new(),
                pairer: ToolCallPairer::default(),
                last_model: None,
                cache_dir: None,
            }
        }

        fn with_cache_dir(mut self, dir: PathBuf) -> Self {
            self.cache_dir = Some(dir);
            self
        }

        fn push(&mut self, env: Value) {
            translate_envelope(
                env,
                &mut self.messages,
                &mut self.pairer,
                "test-session",
                self.cache_dir.as_deref(),
                &mut self.last_model,
            );
        }
    }

    fn assistant_envelope(parts: Vec<Value>) -> Value {
        json!({"role": "assistant", "content": parts})
    }

    #[test]
    fn translate_user_strips_user_query_wrapper() {
        let mut ctx = TranslateContext::new();
        ctx.push(json!({
            "role": "user",
            "content": [{"type": "text", "text": "<user_query>\nhi there\n</user_query>"}],
        }));
        assert_eq!(ctx.messages.len(), 1);
        assert_eq!(ctx.messages[0].role, MessageRole::User);
        assert_eq!(ctx.messages[0].content, "hi there");
    }

    #[test]
    fn translate_assistant_skips_redacted_reasoning_and_emits_text_plus_tool_call() {
        let mut ctx = TranslateContext::new();
        ctx.push(assistant_envelope(vec![
            json!({"type": "redacted-reasoning", "data": "x", "providerOptions": {}}),
            json!({"type": "text", "text": "looking..."}),
            json!({
                "type": "tool-call",
                "toolCallId": "tool_1",
                "toolName": "Glob",
                "args": {"glob_pattern": "**/*.rs", "target_directory": "/src"}
            }),
        ]));
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[0].role, MessageRole::Assistant);
        assert_eq!(ctx.messages[0].content, "looking...");
        assert_eq!(ctx.messages[1].role, MessageRole::Tool);
        assert_eq!(ctx.messages[1].tool_name.as_deref(), Some("Glob"));
        let input: Value =
            serde_json::from_str(ctx.messages[1].tool_input.as_ref().unwrap()).unwrap();
        // Glob remap canonicalises `glob_pattern` → `pattern`.
        assert_eq!(input["pattern"], "**/*.rs");
        assert_eq!(input["path"], "/src");
    }

    #[test]
    fn tool_result_merges_into_call_by_id() {
        let mut ctx = TranslateContext::new();
        ctx.push(assistant_envelope(vec![
            json!({"type": "text", "text": "reading"}),
            json!({
                "type": "tool-call",
                "toolCallId": "tool_2",
                "toolName": "Read",
                "args": {"path": "/tmp/a"}
            }),
        ]));
        ctx.push(json!({
            "role": "tool",
            "content": [{
                "type": "tool-result",
                "toolCallId": "tool_2",
                "toolName": "Read",
                "result": "file contents here"
            }]
        }));
        let tool = ctx
            .messages
            .iter()
            .find(|m| m.role == MessageRole::Tool)
            .expect("tool message");
        assert_eq!(tool.content, "file contents here");
    }

    #[test]
    fn tool_result_preserves_unknown_experimental_content_as_raw() {
        let mut ctx = TranslateContext::new();
        ctx.push(assistant_envelope(vec![json!({
            "type": "tool-call",
            "toolCallId": "tool_future",
            "toolName": "Read",
            "args": {"path": "/tmp/future"}
        })]));
        ctx.push(json!({
            "role": "tool",
            "content": [{
                "type": "tool-result",
                "toolCallId": "tool_future",
                "experimental_content": [
                    {"type": "future_content", "payload": {"keep": true}}
                ]
            }]
        }));
        let tool = ctx
            .messages
            .iter()
            .find(|message| message.role == MessageRole::Tool)
            .expect("tool message");
        assert_eq!(
            tool.tool_metadata
                .as_ref()
                .and_then(|metadata| metadata.presentation.as_ref())
                .map(|presentation| presentation.result_mode),
            Some(ToolResultMode::Raw)
        );
        let raw: Value = serde_json::from_str(&tool.content).expect("raw content array");
        assert_eq!(raw[0]["type"], "future_content");
        assert_eq!(raw[0]["payload"]["keep"], true);
    }

    #[test]
    fn translate_assistant_emits_reasoning_as_thinking_system_message() {
        let mut ctx = TranslateContext::new();
        ctx.push(assistant_envelope(vec![
            json!({
                "type": "reasoning",
                "text": "**Pondering the request**\n\nWeighing options…",
                "signature": "sig",
                "providerOptions": {"cursor": {"modelName": "gpt-5.2-low"}}
            }),
            json!({"type": "text", "text": "here is my answer"}),
        ]));
        assert_eq!(ctx.last_model.as_deref(), Some("gpt-5.2-low"));
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[0].role, MessageRole::System);
        assert!(ctx.messages[0].content.starts_with("[thinking]"));
        assert!(ctx.messages[0].content.contains("Pondering the request"));
        assert_eq!(ctx.messages[0].model.as_deref(), Some("gpt-5.2-low"));
        assert_eq!(ctx.messages[1].role, MessageRole::Assistant);
        assert_eq!(ctx.messages[1].model.as_deref(), Some("gpt-5.2-low"));
    }

    #[test]
    fn tool_result_base64_image_appended_as_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cache_dir = tmp.path().join("images");
        let mut ctx = TranslateContext::new().with_cache_dir(cache_dir.clone());
        ctx.push(assistant_envelope(vec![json!({
            "type": "tool-call",
            "toolCallId": "tool_img",
            "toolName": "Shell",
            "args": {"command": "screencapture"}
        })]));
        // 1×1 transparent PNG, base64-encoded.
        let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
        ctx.push(json!({
            "role": "tool",
            "content": [{
                "type": "tool-result",
                "toolCallId": "tool_img",
                "result": "captured",
                "experimental_content": [
                    {"type": "text", "text": "captured"},
                    {"type": "image", "mimeType": "image/png", "data": png_b64}
                ]
            }]
        }));
        let tool = ctx
            .messages
            .iter()
            .find(|m| m.role == MessageRole::Tool)
            .expect("tool message");

        // The body must include BOTH the text result AND the image marker
        // pointing at a real file inside our injected cache dir.
        assert!(
            tool.content.contains("captured"),
            "tool body must keep the text result, got {:?}",
            tool.content
        );
        let marker_prefix = format!("[Image: source: {}", cache_dir.display());
        assert!(
            tool.content.contains(&marker_prefix),
            "tool body must contain an image marker pointing into the injected cache dir; got {:?}",
            tool.content
        );

        // And the cached file must actually exist on disk with the
        // image bytes (deterministic name: cursor-{session}-{sha256}.png).
        let cached_files: Vec<_> = std::fs::read_dir(&cache_dir)
            .expect("cache dir exists")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        assert_eq!(
            cached_files.len(),
            1,
            "exactly one image file should be in the cache; got {cached_files:?}"
        );
        let cached = &cached_files[0];
        assert!(
            cached
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("cursor-test-session-") && n.ends_with(".png"))
        );
        let bytes = std::fs::read(cached).expect("read cached file");
        assert!(
            bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
            "PNG magic bytes"
        );
    }

    #[test]
    fn tool_result_skips_image_when_no_cache_dir_injected() {
        // Tempdir absent → cache_dir = None → image data quietly skipped
        // and the body is only the text result. This is the production
        // fallback when dirs::data_local_dir() returns None.
        let mut ctx = TranslateContext::new();
        ctx.push(assistant_envelope(vec![json!({
            "type": "tool-call",
            "toolCallId": "tool_skip",
            "toolName": "Shell",
            "args": {"command": "x"}
        })]));
        let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
        ctx.push(json!({
            "role": "tool",
            "content": [{
                "type": "tool-result",
                "toolCallId": "tool_skip",
                "result": "ok",
                "experimental_content": [
                    {"type": "image", "mimeType": "image/png", "data": png_b64}
                ]
            }]
        }));
        let tool = ctx
            .messages
            .iter()
            .find(|m| m.role == MessageRole::Tool)
            .expect("tool message");
        assert_eq!(tool.content, "ok");
        assert!(!tool.content.contains("[Image:"));
    }
}
