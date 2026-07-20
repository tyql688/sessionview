// Test code: clippy's allow-*-in-tests only covers `#[cfg(test)]` modules.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use sessionview_lib::models::{Message, MessageKind, MessageRole, Provider};
use sessionview_lib::provider::{SessionProvider, SourceState};
use sessionview_lib::providers::antigravity::AntigravityProvider;
use sessionview_lib::providers::claude::ClaudeProvider;
use sessionview_lib::providers::codex::CodexProvider;
use sessionview_lib::providers::grok::GrokProvider;
use sessionview_lib::providers::kimi::KimiProvider;
use sessionview_lib::providers::opencode::OpenCodeProvider;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn parse_temp_claude_jsonl(content: &str) -> sessionview_lib::provider::ParsedSession {
    let dir = TempDir::new().expect("temp dir must be created");
    let path = dir.path().join("claude-temp.jsonl");
    fs::write(&path, content).expect("temp claude fixture must be written");
    ClaudeProvider::new()
        .expect("home dir must be available")
        .parse_session(&path)
        .expect("temp claude fixture must parse")
}

// ---------------------------------------------------------------------------
// Claude parser tests
// ---------------------------------------------------------------------------

#[test]
fn claude_parses_message_count() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    // Expected messages:
    //  1. User: "Hello, can you help me debug..."
    //  2. System (thinking): "[thinking]\nLet me think..."
    //  3. Assistant: "Sure! I'd be happy..."
    //  4. User: "Here is my function..."
    //  5. Assistant: "I'll read the file..."
    //  6. Tool (Read): content = file contents (merged from tool_result)
    //  7. Assistant: "Your function looks correct!"
    //  8. System: "[turn_duration] 20.0s, 6 messages"
    //  9. System: "[microcompact_boundary] 27k tokens saved 2k"
    assert_eq!(
        session.messages.len(),
        9,
        "expected 9 messages, got: {:#?}",
        session.messages
    );
}

#[test]
fn claude_first_user_message_role_and_content() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    let first = &session.messages[0];
    assert_eq!(first.role, MessageRole::User);
    assert!(
        first.content.contains("debug this Rust code"),
        "unexpected content: {}",
        first.content
    );
}

#[test]
fn claude_thinking_emitted_as_system_role() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    let thinking = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::System)
        .expect("expected a system (thinking) message");

    assert!(
        thinking.content.starts_with("[thinking]\n"),
        "thinking message must start with [thinking]\\n, got: {}",
        thinking.content
    );
    assert!(
        thinking.content.contains("Let me think"),
        "unexpected thinking content: {}",
        thinking.content
    );
}

#[test]
fn claude_tool_use_creates_tool_message() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    assert_eq!(
        tool_msg.tool_name.as_deref(),
        Some("Read"),
        "expected tool_name 'Read', got: {:?}",
        tool_msg.tool_name
    );
    // tool_result should have been merged into the tool_use message
    assert!(
        tool_msg.content.contains("fn add"),
        "tool message content should include merged result, got: {}",
        tool_msg.content
    );
}

#[test]
fn claude_token_usage_attached_to_last_assistant_message() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    let last_assistant = session
        .messages
        .iter()
        .rfind(|m| m.role == MessageRole::Assistant)
        .expect("expected at least one assistant message");

    let usage = last_assistant
        .token_usage
        .as_ref()
        .expect("last assistant message must have token_usage");
    assert_eq!(usage.input_tokens, 300);
    assert_eq!(usage.output_tokens, 40);
    assert_eq!(usage.cache_read_input_tokens, 20);
}

#[test]
fn claude_project_path_extracted_from_cwd() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    assert_eq!(session.meta.project_path, "/home/user/my-project");
    assert_eq!(session.meta.project_name, "my-project");
}

#[test]
fn claude_session_title_from_first_user_message() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    assert!(
        session.meta.title.contains("debug this Rust code"),
        "title should derive from first user message, got: {}",
        session.meta.title
    );
}

#[test]
fn claude_extracts_model() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    assert_eq!(
        session.meta.model.as_deref(),
        Some("claude-sonnet-4-5-20250514"),
        "model should be extracted from first assistant message"
    );
}

#[test]
fn claude_extracts_version_and_branch() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    assert_eq!(
        session.meta.cc_version.as_deref(),
        Some("2.1.87"),
        "cc_version should be extracted"
    );
    assert_eq!(
        session.meta.git_branch.as_deref(),
        Some("main"),
        "git_branch should be extracted"
    );
}

#[test]
fn claude_parses_system_subtypes() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude fixture must parse");

    use sessionview_lib::models::MessageRole;
    let system_msgs: Vec<&sessionview_lib::models::Message> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::System && !m.content.starts_with("[thinking]"))
        .collect();

    assert_eq!(system_msgs.len(), 2, "expected 2 system subtype messages");
    assert!(
        system_msgs[0].content.contains("[turn_duration]"),
        "first system msg should be turn_duration: {}",
        system_msgs[0].content
    );
    assert!(
        system_msgs[1].content.contains("[microcompact_boundary]"),
        "second system msg should be microcompact_boundary: {}",
        system_msgs[1].content
    );
}

#[test]
fn claude_normalizes_new_image_source_marker_format() {
    let provider = ClaudeProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("claude_new_image_source_session.jsonl");
    let session = provider
        .parse_session(&path)
        .expect("claude image fixture must parse");

    let first_user = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .expect("expected user message");

    assert!(
        first_user
            .content
            .contains("[Image: source: /Users/test/.claude/image-cache/example-session/1.png]"),
        "new marker format should be normalized for frontend rendering, got: {}",
        first_user.content
    );
    assert!(
        !first_user.content.contains("[Image source:"),
        "raw new marker should not leak into parsed output: {}",
        first_user.content
    );
    assert_eq!(
        session
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .count(),
        1,
        "attachment records between placeholder and isMeta should not split the user image message"
    );
}

#[test]
fn claude_image_block_without_text_marker_merges_meta_source() {
    let session = parse_temp_claude_jsonl(concat!(
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"screenshot attached"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc123"}}]},"timestamp":"2026-05-01T10:00:00.000Z","cwd":"/tmp/demo","sessionId":"claude-image-block","uuid":"u1","isSidechain":false}"#,
        "\n",
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Image source: /Users/test/.claude/image-cache/claude-image-block/1.png]"}]},"isMeta":true,"timestamp":"2026-05-01T10:00:00.100Z","cwd":"/tmp/demo","sessionId":"claude-image-block","uuid":"u2","isSidechain":false}"#,
        "\n"
    ));

    let user = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .expect("expected user message");
    assert!(
        user.content
            .contains("[Image: source: /Users/test/.claude/image-cache/claude-image-block/1.png]"),
        "image block should be replaced with meta source, got: {}",
        user.content
    );
}

#[test]
fn claude_image_block_without_meta_source_keeps_placeholder() {
    let session = parse_temp_claude_jsonl(concat!(
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"screenshot attached"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc123"}}]},"timestamp":"2026-05-01T10:00:00.000Z","cwd":"/tmp/demo","sessionId":"claude-image-block","uuid":"u1","isSidechain":false}"#,
        "\n"
    ));

    let user = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .expect("expected user message");
    assert!(
        user.content.contains("[Image]"),
        "image block without cache source should still be visible, got: {}",
        user.content
    );
}

#[test]
fn claude_displays_local_command_and_informational_system_messages() {
    let session = parse_temp_claude_jsonl(concat!(
        r#"{"type":"user","message":{"role":"user","content":"<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args></command-args>"},"timestamp":"2026-05-01T10:00:00.000Z","cwd":"/tmp/demo","sessionId":"claude-system","uuid":"u1","isSidechain":false}"#,
        "\n",
        r#"{"type":"system","subtype":"local_command","content":"<local-command-stdout>Kept model as \u001b[1mOpus 4.6\u001b[22m</local-command-stdout>","level":"info","timestamp":"2026-05-01T10:00:01.000Z","cwd":"/tmp/demo","sessionId":"claude-system","uuid":"s1","isSidechain":false}"#,
        "\n",
        r#"{"type":"system","subtype":"informational","content":"Auto mode lets Claude handle permission prompts automatically.","level":"warning","timestamp":"2026-05-01T10:00:02.000Z","cwd":"/tmp/demo","sessionId":"claude-system","uuid":"s2","isSidechain":false}"#,
        "\n"
    ));

    let user_contents: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        user_contents.contains(&"/model"),
        "slash command should be visible as a user command message: {:?}",
        user_contents
    );
    assert!(
        session.messages.iter().any(|m| {
            m.role == MessageRole::User && m.message_kind == Some(MessageKind::CommandInput)
        }),
        "slash command should be tagged as command input"
    );

    let assistant_contents: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        assistant_contents.contains(&"Kept model as Opus 4.6"),
        "local command output should be visible and ANSI-stripped: {:?}",
        assistant_contents
    );
    assert!(
        session.messages.iter().any(|m| {
            m.role == MessageRole::Assistant && m.message_kind == Some(MessageKind::CommandOutput)
        }),
        "local command output should be tagged as command output"
    );

    let system_contents: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::System)
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        system_contents.contains(
            &"[informational] Auto mode lets Claude handle permission prompts automatically."
        ),
        "informational system message should be visible: {:?}",
        system_contents
    );
}

// ---------------------------------------------------------------------------
// Codex parser tests
// ---------------------------------------------------------------------------

#[test]
fn codex_parses_message_count() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex fixture must parse");

    // Expected messages:
    //  1. User: "Write a hello world program"
    //  2. Assistant: "I'll create a hello world program..."  (token_usage attached)
    //  3. Tool (Bash): exec_command, content = merged output
    //  4. Assistant: "The hello world program is ready..." (token_usage attached)
    assert_eq!(
        session.messages.len(),
        4,
        "expected 4 messages, got: {:#?}",
        session.messages
    );
}

#[test]
fn codex_session_id_from_meta() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex fixture must parse");

    assert_eq!(session.meta.id, "codex-session-abc123");
}

#[test]
fn codex_exec_command_mapped_to_bash() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex fixture must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    assert_eq!(
        tool_msg.tool_name.as_deref(),
        Some("Bash"),
        "exec_command must map to Bash, got: {:?}",
        tool_msg.tool_name
    );
}

#[test]
fn codex_exec_command_args_remapped_to_command_key() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex fixture must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    let input = tool_msg
        .tool_input
        .as_ref()
        .expect("Bash tool must have tool_input");
    // exec_command {"cmd":"..."} must be remapped to {"command":"..."}
    assert!(
        input.contains("\"command\""),
        "tool_input must use 'command' key, got: {}",
        input
    );
    assert!(
        input.contains("cat hello.py"),
        "tool_input must contain the command, got: {}",
        input
    );
}

#[test]
fn codex_function_call_output_merged_into_tool_message() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex fixture must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    assert!(
        tool_msg.content.contains("Hello, World!"),
        "tool output must be merged into tool message, got: {}",
        tool_msg.content
    );
}

#[test]
fn codex_exec_command_has_structured_tool_metadata() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex fixture must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");
    let metadata = tool_msg
        .tool_metadata
        .as_ref()
        .expect("Codex tool metadata must be present");

    assert_eq!(metadata.raw_name, "exec_command");
    assert_eq!(metadata.canonical_name, "Bash");
    assert_eq!(metadata.category, "shell");
    assert_eq!(metadata.summary.as_deref(), Some("cat hello.py"));
    assert_eq!(metadata.status.as_deref(), Some("success"));
    assert_eq!(metadata.result_kind.as_deref(), Some("terminal_output"));
}

#[test]
fn codex_web_search_call_has_structured_tool_metadata() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex-web-search.jsonl");
    fs::write(
        &file,
        concat!(
            r#"{"timestamp":"2026-04-11T10:00:00Z","type":"session_meta","payload":{"id":"codex-web","cwd":"/tmp/project"}}"#,
            "\n",
            r#"{"timestamp":"2026-04-11T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Search docs"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-04-11T10:00:02Z","type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{"type":"search","query":"rust notify kqueue","queries":["rust notify kqueue"]}}}"#,
            "\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider::new().expect("home dir must be available");
    let session = provider
        .parse_session_file(&file)
        .expect("codex web search fixture must parse");
    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");
    let metadata = tool_msg
        .tool_metadata
        .as_ref()
        .expect("Codex web search metadata must be present");

    assert_eq!(metadata.raw_name, "web_search_call");
    assert_eq!(metadata.canonical_name, "WebSearch");
    assert_eq!(metadata.category, "web");
    assert_eq!(metadata.summary.as_deref(), Some("rust notify kqueue"));
    assert_eq!(metadata.status.as_deref(), Some("completed"));
}

#[test]
fn codex_token_usage_attached_to_assistant_message() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex fixture must parse");

    let last_assistant = session
        .messages
        .iter()
        .rfind(|m| m.role == MessageRole::Assistant)
        .expect("expected at least one assistant message");

    let usage = last_assistant
        .token_usage
        .as_ref()
        .expect("last assistant message must have token_usage");
    assert_eq!(usage.input_tokens, 110);
    assert_eq!(usage.output_tokens, 25);
    assert_eq!(usage.cache_read_input_tokens, 10);
}

#[test]
fn codex_project_path_from_session_meta() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex fixture must parse");

    assert_eq!(session.meta.project_path, "/home/user/my-project");
}

#[test]
fn codex_subagent_detected() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_subagent.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex subagent fixture must parse");

    assert!(session.meta.is_sidechain);
    assert_eq!(session.meta.parent_id.as_deref(), Some("codex-parent-001"));
    assert_eq!(session.meta.title, "Faraday");
    assert_eq!(session.meta.id, "codex-sub-001");
}

#[test]
fn codex_subagent_v2_skips_sanitized_fork_context() {
    // Codex 0.122+ rollouts: a second session_meta starts the sanitized forked
    // parent history, and the legacy "newly spawned agent" marker was removed
    // (upstream #16709). The parser must switch back on the first
    // subagent-owned `task_started` whose `started_at` matches the subagent's
    // session timestamp.
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_subagent_v2.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex subagent v2 fixture must parse");

    assert!(session.meta.is_sidechain);
    assert_eq!(
        session.meta.parent_id.as_deref(),
        Some("codex-parent-v2-001")
    );
    assert_eq!(session.meta.id, "codex-sub-v2-001");
    assert_eq!(session.meta.title, "Hume");

    let user_messages: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        user_messages,
        vec!["Investigate module A and return a summary."],
        "parent's forked user message must be skipped; only subagent's own turn survives"
    );
    assert!(
        session
            .messages
            .iter()
            .any(|m| m.role == MessageRole::Assistant
                && m.content.contains("Module A handles authentication"))
    );
}

#[test]
fn codex_user_message_event_merges_placeholder_with_embedded_image_source() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_local_image_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex local image fixture must parse");

    let first = session
        .messages
        .first()
        .expect("expected a parsed user message");
    assert_eq!(first.role, MessageRole::User);
    assert!(
        first.content.contains("[Image: source: /tmp/replay.png]"),
        "expected local image source without embedded base64, got: {}",
        first.content
    );
    assert!(
        !first.content.contains("data:image/png;base64"),
        "embedded base64 image data must not be stored in message content, got: {}",
        first.content
    );
    assert!(
        first.content.contains("literal '[Image #1]'"),
        "quoted placeholder text must remain literal, got: {}",
        first.content
    );
}

#[test]
fn codex_user_message_event_falls_back_to_windows_local_image_path() {
    let provider = CodexProvider::new().expect("home dir must be available");
    let path = fixtures_dir().join("codex_windows_local_image_session.jsonl");
    let session = provider
        .parse_session_file(&path)
        .expect("codex windows local image fixture must parse");

    let first = session
        .messages
        .first()
        .expect("expected a parsed user message");
    assert_eq!(first.role, MessageRole::User);
    assert!(
        first.content.contains(
            "[Image: source: C:\\\\Users\\\\Alice\\\\AppData\\\\Local\\\\Temp\\\\codex-clipboard.png]",
        ),
        "expected windows local image path fallback, got: {}",
        first.content
    );
}

// ---------------------------------------------------------------------------
// Kimi parser tests
//
// kimi-code 0.1.1+ stores each session under
// `~/.kimi-code/sessions/wd_*/<session_dir>/agents/<agent>/wire.jsonl`,
// with two wire formats coexisting: the migrated legacy protocol (only
// `context.append_message` lines) and the native event-stream protocol
// (per-line `time`, `context.append_loop_event`, `usage.record`). The
// tests below pin behaviour for both via the on-disk fixtures under
// `tests/fixtures/kimi/`.
// ---------------------------------------------------------------------------

fn kimi_fixture_provider() -> KimiProvider {
    KimiProvider::with_root(fixtures_dir().join("kimi"))
}

const NATIVE_BASIC_ID: &str = "session_11111111-1111-4111-a111-111111111111";
const NATIVE_SUBAGENT_PARENT_ID: &str = "session_22222222-2222-4222-a222-222222222222";
const LEGACY_MIGRATED_ID: &str = "ses_33333333-3333-4333-a333-333333333333";

fn parse_fixture_session(session_id: &str) -> sessionview_lib::provider::ParsedSession {
    let provider = kimi_fixture_provider();
    let sessions = provider.scan_all().expect("kimi fixture scan must succeed");
    sessions
        .into_iter()
        .find(|s| s.meta.id == session_id)
        .unwrap_or_else(|| panic!("session {session_id} missing from fixture scan"))
}

#[test]
fn kimi_scans_all_fixture_sessions() {
    let provider = kimi_fixture_provider();
    let sessions = provider.scan_all().expect("scan_all");
    // 3 main agents + 1 subagent = 4 parsed sessions.
    assert_eq!(sessions.len(), 4, "scan should find every wire.jsonl");
    let mut ids: Vec<String> = sessions.iter().map(|s| s.meta.id.clone()).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            LEGACY_MIGRATED_ID.to_string(),
            NATIVE_BASIC_ID.to_string(),
            NATIVE_SUBAGENT_PARENT_ID.to_string(),
            format!("{NATIVE_SUBAGENT_PARENT_ID}:agent-0"),
        ]
    );
    for s in &sessions {
        assert_eq!(s.meta.provider, Provider::Kimi);
    }
}

#[test]
fn kimi_native_session_pulls_title_from_state_json() {
    let s = parse_fixture_session(NATIVE_BASIC_ID);
    assert_eq!(s.meta.title, "Format B basic with tool");
    // state.json's `createdAt` and the first per-line `time` match,
    // both encode the same epoch ms — make sure first_time wins.
    assert_eq!(s.meta.created_at, 1779701829);
}

#[test]
fn kimi_native_session_uses_session_index_for_project() {
    let s = parse_fixture_session(NATIVE_BASIC_ID);
    assert_eq!(s.meta.project_path, "/work/demo-project");
    assert_eq!(s.meta.project_name, "demo-project");
}

#[test]
fn kimi_native_session_emits_thinking_as_system_role() {
    let s = parse_fixture_session(NATIVE_BASIC_ID);
    let think = s
        .messages
        .iter()
        .find(|m| m.role == MessageRole::System)
        .expect("thinking message must be present");
    assert!(think.content.starts_with("[thinking]"));
    assert!(think.content.contains("README.md"));
}

#[test]
fn kimi_native_session_merges_tool_call_and_result() {
    let s = parse_fixture_session(NATIVE_BASIC_ID);
    let tool = s
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("tool message must be present");
    assert_eq!(tool.tool_name.as_deref(), Some("Read"));
    assert_eq!(
        tool.content,
        "# Demo

This is a demo project."
    );
    let input: serde_json::Value = serde_json::from_str(tool.tool_input.as_ref().unwrap()).unwrap();
    assert_eq!(input["path"], "README.md");
}

#[test]
fn kimi_native_session_attaches_usage_to_assistant_text() {
    let s = parse_fixture_session(NATIVE_BASIC_ID);
    // The final assistant text comes after step.end; usage.record carries
    // the canonical model alias and disjoint token components.
    let last_assistant = s
        .messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::Assistant)
        .expect("assistant text");
    let usage = last_assistant
        .token_usage
        .as_ref()
        .expect("usage must attach to last assistant message");
    assert_eq!(usage.output_tokens, 40);
    assert_eq!(usage.cache_read_input_tokens, 2048);
    assert_eq!(usage.input_tokens, 120);
    assert_eq!(
        last_assistant.model.as_deref(),
        Some("kimi-code/kimi-for-coding")
    );
}

#[test]
fn kimi_subagent_is_separate_session_with_parent_link() {
    let parent = parse_fixture_session(NATIVE_SUBAGENT_PARENT_ID);
    assert!(!parent.meta.is_sidechain);
    assert!(parent.meta.parent_id.is_none());

    let child_id = format!("{NATIVE_SUBAGENT_PARENT_ID}:agent-0");
    let child = parse_fixture_session(&child_id);
    assert!(child.meta.is_sidechain);
    assert_eq!(
        child.meta.parent_id.as_deref(),
        Some(NATIVE_SUBAGENT_PARENT_ID)
    );
    // Subagents inherit the parent's workdir via session_index.jsonl since
    // their dir doesn't appear there directly.
    assert_eq!(child.meta.project_path, "/work/demo-project");
    // Title must NOT inherit state.json's title (which is shared with the
    // parent). The parser pulls the short `description` from the parent's
    // Agent tool.call so the tree shows what the subtask is about rather
    // than the full prompt or the `<git-context>` blob kimi-code injects.
    assert_ne!(child.meta.title, parent.meta.title);
    assert_eq!(child.meta.title, "Find toml");
}

#[test]
fn kimi_migrated_session_format_a_basic_parse() {
    let s = parse_fixture_session(LEGACY_MIGRATED_ID);
    assert_eq!(s.meta.title, "Migrated legacy");
    // user + assistant thinking + tool merged + assistant final = 4
    assert_eq!(s.messages.len(), 4);
    assert_eq!(s.messages[0].role, MessageRole::User);
    assert_eq!(s.messages[1].role, MessageRole::System);
    assert!(s.messages[1].content.starts_with("[thinking]"));
}

#[test]
fn kimi_migrated_session_shell_canonicalised_and_result_merged() {
    let s = parse_fixture_session(LEGACY_MIGRATED_ID);
    let tool = s
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("tool message");
    // Shell → canonical Bash.
    assert_eq!(tool.tool_name.as_deref(), Some("Bash"));
    // The system wrapper is stripped because real content sits alongside it.
    assert_eq!(
        tool.content,
        "README.md
src"
    );
    let input: serde_json::Value = serde_json::from_str(tool.tool_input.as_ref().unwrap()).unwrap();
    assert_eq!(input["command"], "ls -1");
}

#[test]
fn kimi_migrated_session_inherits_metadata_created_at_when_lines_lack_time() {
    let s = parse_fixture_session(LEGACY_MIGRATED_ID);
    // Migrated wire lines have no `time` field, so every message must
    // inherit the metadata.created_at timestamp (epoch ms 1777623844612
    // → secs 1777623844 → ISO 2026-05-01T...).
    assert_eq!(s.meta.created_at, 1_777_623_844);
    for m in &s.messages {
        let ts = m
            .timestamp
            .as_deref()
            .expect("each message has a timestamp");
        assert!(
            ts.starts_with("2026-05-01"),
            "expected 2026-05-01 timestamp, got {ts}"
        );
    }
}

#[test]
fn kimi_load_messages_round_trips_through_source_path() {
    let provider = kimi_fixture_provider();
    let s = parse_fixture_session(NATIVE_BASIC_ID);
    let loaded = provider
        .load_messages(&s.meta.id, &s.meta.source_path)
        .expect("load_messages");
    assert_eq!(loaded.messages.len(), s.messages.len());
}

#[test]
fn kimi_resume_command_strips_subagent_suffix() {
    use sessionview_lib::provider::ProviderDescriptor;
    let descriptor = &sessionview_lib::providers::kimi::Descriptor;
    let parent_cmd = descriptor
        .resume_command(NATIVE_BASIC_ID, None)
        .expect("parent resume command");
    assert_eq!(parent_cmd, format!("kimi --session {NATIVE_BASIC_ID}"));
    let sub_cmd = descriptor
        .resume_command(&format!("{NATIVE_SUBAGENT_PARENT_ID}:agent-0"), None)
        .expect("subagent resume command");
    // Kimi has no resume target for a subagent — resume the parent.
    assert_eq!(
        sub_cmd,
        format!("kimi --session {NATIVE_SUBAGENT_PARENT_ID}")
    );
}

#[test]
fn kimi_parse_session_tail_returns_only_last_n_messages() {
    let s = parse_fixture_session(NATIVE_BASIC_ID);
    let path = std::path::PathBuf::from(&s.meta.source_path);
    let tail =
        sessionview_lib::providers::kimi::parser::parse_session_tail(&path, 2).expect("tail parse");
    assert!(tail.messages.len() <= 2);
    // The tail must end with the assistant's final text (last emitted).
    let last = tail.messages.last().expect("non-empty tail");
    assert_eq!(last.role, MessageRole::Assistant);
}

// ---------------------------------------------------------------------------
// Antigravity parser tests
// ---------------------------------------------------------------------------

#[test]
fn test_antigravity_model_extraction() {
    let tmp = TempDir::new().unwrap();
    let conv_dir = tmp.path().join("conv-123");
    let logs_dir = conv_dir.join(".system_generated").join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let transcript_path = logs_dir.join("transcript.jsonl");

    let lines = [
        r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-05-20T04:23:57Z","content":"<USER_REQUEST>\nhi\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-05-20T12:23:57+08:00.\n</ADDITIONAL_METADATA>\n<USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection` from None to Gemini 3.5 Flash (High). No need to comment on this change if the user doesn't ask about it.\n</USER_SETTINGS_CHANGE>"}"#,
        r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-05-20T04:24:00Z","content":"Hello! How can I help you?"}"#,
    ];
    fs::write(&transcript_path, lines.join("\n")).unwrap();

    let parsed =
        sessionview_lib::providers::antigravity::parser::parse_session_file(&transcript_path)
            .expect("must parse");

    assert_eq!(parsed.meta.model.as_deref(), Some("gemini-3.5-flash"));
    assert_eq!(parsed.messages.len(), 2);
    assert_eq!(parsed.messages[0].role, MessageRole::User);
    assert_eq!(
        parsed.messages[0].model.as_deref(),
        Some("gemini-3.5-flash")
    );
    assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
    assert_eq!(
        parsed.messages[1].model.as_deref(),
        Some("gemini-3.5-flash")
    );

    let usage = parsed.messages[1]
        .token_usage
        .as_ref()
        .expect("should have token usage");
    assert!(usage.input_tokens > 0);
    assert!(usage.output_tokens > 0);
    assert_eq!(parsed.meta.input_tokens, usage.input_tokens as u64);
    assert_eq!(parsed.meta.output_tokens, usage.output_tokens as u64);
}

fn antigravity_transcript_fixture_path() -> PathBuf {
    fixtures_dir()
        .join("antigravity")
        .join("conv-123")
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl")
}

#[test]
fn antigravity_tool_calls_map_to_canonical_names_and_decode_args() {
    let path = antigravity_transcript_fixture_path();
    let parsed = sessionview_lib::providers::antigravity::parser::parse_session_file(&path)
        .expect("antigravity fixture must parse");

    let tools: Vec<&Message> = parsed
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .collect();
    assert_eq!(tools.len(), 2, "expected two tool messages, got {tools:#?}");

    // run_command → Bash, with CommandLine decoded from its JSON-string wrapper.
    let bash = tools[0];
    assert_eq!(bash.tool_name.as_deref(), Some("Bash"));
    let bash_meta = bash.tool_metadata.as_ref().unwrap();
    assert_eq!(bash_meta.raw_name, "run_command");
    assert_eq!(bash_meta.canonical_name, "Bash");
    assert_eq!(
        bash_meta.summary.as_deref(),
        Some("ls -la"),
        "Bash summary should be the decoded CommandLine, not the JSON-quoted blob"
    );
    let bash_input = bash.tool_input.as_ref().expect("tool_input populated");
    assert!(
        bash_input.contains("\"CommandLine\":\"ls -la\""),
        "persisted tool_input should hold the decoded value, got: {bash_input}"
    );
    assert!(
        !bash_input.contains(r#"\"ls -la\""#),
        "double-encoded value should not survive into tool_input: {bash_input}"
    );
    assert!(
        bash.content.contains("main.rs"),
        "tool result must be merged into the tool message, got: {}",
        bash.content
    );

    // view_file → Read, with AbsolutePath used as the summary.
    let read = tools[1];
    assert_eq!(read.tool_name.as_deref(), Some("Read"));
    let read_meta = read.tool_metadata.as_ref().unwrap();
    assert_eq!(read_meta.raw_name, "view_file");
    assert_eq!(read_meta.canonical_name, "Read");
    assert_eq!(
        read_meta.summary.as_deref(),
        Some("/tmp/project/main.rs"),
        "Read summary should fall back to AbsolutePath for antigravity"
    );

    assert_eq!(parsed.meta.id, "conv-123");
    assert_eq!(parsed.meta.provider, Provider::Antigravity);
}

#[test]
fn antigravity_user_message_includes_uploaded_image_markers() {
    let tmp = TempDir::new().unwrap();
    let conv_dir = tmp.path().join("conv-img");
    let logs_dir = conv_dir.join(".system_generated").join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let transcript_path = logs_dir.join("transcript.jsonl");

    // USER_INPUT step with an uploaded image listed in ADDITIONAL_METADATA.
    // The image path is the canonical antigravity layout:
    // ~/.gemini/antigravity-cli/brain/{conv_id}/uploaded_media_{ts}.png
    let img_path = "/tmp/brain/conv-img/uploaded_media_111.png";
    let user_content = format!(
        r#"<USER_REQUEST>\n这些是什么\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-05-20T12:00:00+08:00.\n\nThe user has uploaded 1 image(s):\n- {img_path}\nYou can embed this image in an artifact if you need the USER to review it.\n</ADDITIONAL_METADATA>"#
    );
    let line = format!(
        r#"{{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-05-20T04:00:00Z","content":"{}"}}"#,
        user_content.replace('"', "\\\"")
    );
    fs::write(&transcript_path, line).unwrap();

    let parsed =
        sessionview_lib::providers::antigravity::parser::parse_session_file(&transcript_path)
            .expect("antigravity image transcript must parse");
    let user_msg = parsed
        .messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .expect("user message");
    assert!(
        user_msg
            .content
            .contains(&format!("[Image: source: {img_path}]")),
        "user message should carry image marker, got: {}",
        user_msg.content
    );
    // Bare text from <USER_REQUEST> still survives alongside the marker.
    assert!(
        user_msg.content.contains("这些是什么"),
        "user message must keep the original text, got: {}",
        user_msg.content
    );
}

#[test]
fn antigravity_parse_session_tail_returns_only_last_n_messages() {
    let tmp = TempDir::new().unwrap();
    let conv_dir = tmp.path().join("conv-tail");
    let logs_dir = conv_dir.join(".system_generated").join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let transcript_path = logs_dir.join("transcript.jsonl");

    // Each PLANNER_RESPONSE step produces 1 message (assistant content).
    // 200 steps → 200 messages, then ask for the last 20.
    let mut content = String::new();
    for i in 0..200 {
        let ts = format!("2026-05-20T04:{:02}:{:02}Z", (i / 60) % 60, i % 60);
        content.push_str(&format!(
            r#"{{"step_index":{i},"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"{ts}","content":"msg-{i}"}}
"#
        ));
    }
    fs::write(&transcript_path, content).unwrap();

    let tail =
        sessionview_lib::providers::antigravity::parser::parse_session_tail(&transcript_path, 20)
            .expect("tail parse");
    assert_eq!(tail.messages.len(), 20);
    let first = tail.messages.first().expect("first").content.clone();
    let last = tail.messages.last().expect("last").content.clone();
    assert!(
        first.contains("msg-180"),
        "first tail message should be msg-180, got {first:?}"
    );
    assert!(
        last.contains("msg-199"),
        "last tail message should be msg-199, got {last:?}"
    );
}

#[test]
fn antigravity_parse_session_tail_returns_full_file_when_smaller_than_window() {
    let tmp = TempDir::new().unwrap();
    let conv_dir = tmp.path().join("conv-small");
    let logs_dir = conv_dir.join(".system_generated").join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let transcript_path = logs_dir.join("transcript.jsonl");

    let mut content = String::new();
    for i in 0..5 {
        content.push_str(&format!(
            r#"{{"step_index":{i},"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-05-20T04:00:0{i}Z","content":"only-{i}"}}
"#
        ));
    }
    fs::write(&transcript_path, content).unwrap();

    let tail =
        sessionview_lib::providers::antigravity::parser::parse_session_tail(&transcript_path, 100)
            .expect("tail parse");
    assert_eq!(
        tail.messages.len(),
        5,
        "tail must return all messages when file is smaller than requested window"
    );
}

#[test]
#[ignore]
fn antigravity_real_local_sessions_smoke() {
    let home = std::env::var("HOME").expect("HOME must be set");
    let brain = PathBuf::from(&home)
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain");
    if !brain.is_dir() {
        eprintln!("skip: {} does not exist", brain.display());
        return;
    }

    let provider = AntigravityProvider::new().expect("home dir must be available");
    let sessions = provider.scan_all().expect("scan_all must succeed");
    assert!(!sessions.is_empty(), "no antigravity sessions parsed");

    // Print parent → children map so the smoke run is self-describing.
    for s in &sessions {
        if !s.child_session_ids.is_empty() {
            eprintln!(
                "parent {} ({}) → children {:?}",
                s.meta.id, s.meta.project_name, s.child_session_ids
            );
        }
    }
    for s in &sessions {
        eprintln!(
            "  id={}  parent={:?}  is_sidechain={}  project={:?}",
            s.meta.id, s.meta.parent_id, s.meta.is_sidechain, s.meta.project_name
        );
    }

    // Surface what canonical names each session's tools mapped to. Anything
    // landing under the raw agy name (run_command / view_file / ...) means
    // the canonical_tool_name table is missing an alias.
    use std::collections::BTreeMap;
    let mut canonical_counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for s in &sessions {
        for msg in &s.messages {
            if let Some(md) = &msg.tool_metadata {
                *canonical_counts
                    .entry(md.raw_name.clone())
                    .or_default()
                    .entry(md.canonical_name.clone())
                    .or_default() += 1;
            }
        }
    }
    eprintln!("  tool name → canonical map:");
    for (raw, canon) in &canonical_counts {
        eprintln!("    {raw} → {canon:?}");
    }
    for (raw, canon_map) in &canonical_counts {
        for canon in canon_map.keys() {
            assert_ne!(
                raw, canon,
                "tool '{raw}' fell through to its raw name — needs a canonical alias",
            );
        }
    }

    // Every parent that declared subagents must also emit those conversationIds
    // on the invoke_subagent tool message's structured metadata, otherwise the
    // UI's "Open" button has nothing to navigate to.
    for parent in &sessions {
        if parent.child_session_ids.is_empty() {
            continue;
        }
        let mut all_agent_child_ids = Vec::new();
        for agent_tool in parent.messages.iter().filter(|m| {
            m.tool_metadata
                .as_ref()
                .is_some_and(|md| md.raw_name == "invoke_subagent")
        }) {
            let structured = agent_tool
                .tool_metadata
                .as_ref()
                .and_then(|md| md.structured.as_ref())
                .unwrap_or_else(|| {
                    panic!(
                        "parent {} invoke_subagent tool has no structured metadata",
                        parent.meta.id
                    )
                });
            let ids = structured
                .get("childConversationIds")
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| {
                    panic!(
                        "parent {} invoke_subagent structured.childConversationIds missing",
                        parent.meta.id
                    )
                });
            eprintln!(
                "  invoke_subagent childConversationIds for {} = {:?}",
                parent.meta.id, ids
            );
            all_agent_child_ids.extend(ids.iter().filter_map(|v| v.as_str()).map(str::to_string));

            // childPrompts must be a same-length parallel array so the UI can
            // label each Open button. Empty strings are allowed (no Prompt
            // declared) but the array length must match.
            let prompts = structured
                .get("childPrompts")
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| {
                    panic!(
                        "parent {} invoke_subagent structured.childPrompts missing",
                        parent.meta.id
                    )
                });
            assert_eq!(
                prompts.len(),
                ids.len(),
                "parent {} childPrompts length must match childConversationIds",
                parent.meta.id
            );
        }
        assert!(
            !all_agent_child_ids.is_empty(),
            "parent {} has no invoke_subagent tool metadata",
            parent.meta.id
        );
        for child_id in &parent.child_session_ids {
            assert!(
                all_agent_child_ids.iter().any(|id| id == child_id),
                "Agent tool metadata for {} missing child {child_id}",
                parent.meta.id
            );
        }
    }

    // Every declared child id must point at a session we actually parsed,
    // and that session must end up flagged as a sidechain with parent_id set.
    use std::collections::HashMap;
    let by_id: HashMap<&str, &sessionview_lib::provider::ParsedSession> =
        sessions.iter().map(|s| (s.meta.id.as_str(), s)).collect();

    for parent in &sessions {
        for child_id in &parent.child_session_ids {
            let Some(child) = by_id.get(child_id.as_str()) else {
                continue;
            };
            assert!(
                child.meta.is_sidechain,
                "child {} of {} not flagged as sidechain",
                child_id, parent.meta.id
            );
            assert_eq!(
                child.meta.parent_id.as_deref(),
                Some(parent.meta.id.as_str()),
                "child {} should point at parent {}",
                child_id,
                parent.meta.id
            );
            assert!(
                !child.meta.project_path.is_empty(),
                "child {} should inherit project_path from parent",
                child_id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// OpenCode parser tests
// ---------------------------------------------------------------------------

/// Create a temporary SQLite database matching the OpenCode schema.
/// Returns the `TempDir` (must be kept alive for the test) and the DB path.
fn create_opencode_test_db() -> (tempfile::TempDir, PathBuf) {
    use rusqlite::{Connection, params};

    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("opencode.db");

    let conn = Connection::open(&db_path).expect("open db");

    conn.execute_batch(
        "CREATE TABLE project (
            id           TEXT    PRIMARY KEY,
            name         TEXT    NOT NULL,
            worktree     TEXT    NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL
         );
         CREATE TABLE session (
            id           TEXT    PRIMARY KEY,
            title        TEXT    NOT NULL,
            directory    TEXT    NOT NULL,
            project_id   TEXT,
            parent_id    TEXT,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL
         );
         CREATE TABLE message (
            id           TEXT    PRIMARY KEY,
            session_id   TEXT    NOT NULL,
            data         TEXT    NOT NULL,
            time_created INTEGER NOT NULL
         );
         CREATE TABLE part (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            message_id   TEXT    NOT NULL,
            session_id   TEXT    NOT NULL,
            data         TEXT    NOT NULL,
            time_created INTEGER NOT NULL
         );",
    )
    .expect("create tables");

    // project
    conn.execute(
        "INSERT INTO project (id, name, worktree, time_created, time_updated)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            "proj-001",
            "my-opencode-project",
            "/home/user/my-opencode-project",
            1705321200000_i64,
            1705321200000_i64,
        ],
    )
    .expect("insert project");

    // session
    // time_created = 1705321200000 ms → epoch s = 1705321200
    // time_updated = 1705324800000 ms → epoch s = 1705324800
    conn.execute(
        "INSERT INTO session
             (id, title, directory, project_id, parent_id, time_created, time_updated)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "session-oc-0001",
            "Test OpenCode Session",
            "/home/user/my-opencode-project",
            "proj-001",
            Option::<String>::None,
            1705321200000_i64,
            1705324800000_i64,
        ],
    )
    .expect("insert session");

    // message 1 — user
    conn.execute(
        "INSERT INTO message (id, session_id, data, time_created) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg-001",
            "session-oc-0001",
            r#"{"role":"user","time":{"created":1705321200000}}"#,
            1705321200000_i64,
        ],
    )
    .expect("insert msg-001");

    // message 2 — assistant (text + tool part, carries token usage)
    conn.execute(
        "INSERT INTO message (id, session_id, data, time_created) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg-002",
            "session-oc-0001",
            r#"{"role":"assistant","time":{"created":1705321260000},"tokens":{"input":50,"output":100,"cache":{"read":10,"write":5}}}"#,
            1705321260000_i64,
        ],
    )
    .expect("insert msg-002");

    // message 3 — second user
    conn.execute(
        "INSERT INTO message (id, session_id, data, time_created) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg-003",
            "session-oc-0001",
            r#"{"role":"user","time":{"created":1705321320000}}"#,
            1705321320000_i64,
        ],
    )
    .expect("insert msg-003");

    // part — user text (msg-001)
    conn.execute(
        "INSERT INTO part (message_id, session_id, data, time_created) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg-001",
            "session-oc-0001",
            r#"{"type":"text","text":"List files in the project directory"}"#,
            1705321200000_i64,
        ],
    )
    .expect("insert part user text");

    // part — assistant text (msg-002)
    conn.execute(
        "INSERT INTO part (message_id, session_id, data, time_created) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg-002",
            "session-oc-0001",
            r#"{"type":"text","text":"Sure, let me list the files for you."}"#,
            1705321260000_i64,
        ],
    )
    .expect("insert part assistant text");

    // part — tool call (msg-002)
    conn.execute(
        "INSERT INTO part (message_id, session_id, data, time_created) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg-002",
            "session-oc-0001",
            r#"{"type":"tool","tool":"Bash","state":{"status":"completed","input":"{\"command\":\"ls -la\"}","output":"total 8\ndrwxr-xr-x main.rs\ndrwxr-xr-x lib.rs","time":{"start":1705321265000}}}"#,
            1705321265000_i64,
        ],
    )
    .expect("insert part tool");

    // part — user text (msg-003)
    conn.execute(
        "INSERT INTO part (message_id, session_id, data, time_created) VALUES (?1, ?2, ?3, ?4)",
        params![
            "msg-003",
            "session-oc-0001",
            r#"{"type":"text","text":"Thanks, that looks good!"}"#,
            1705321320000_i64,
        ],
    )
    .expect("insert part user text 2");

    (dir, db_path)
}

#[test]
fn opencode_parses_session_meta() {
    let (_dir, db_path) = create_opencode_test_db();
    let provider = OpenCodeProvider::with_db_path(db_path);

    let sessions = provider.scan_all().expect("scan_all must succeed");
    assert_eq!(sessions.len(), 1, "expected exactly 1 session");

    let meta = &sessions[0].meta;
    assert_eq!(meta.id, "session-oc-0001");
    assert_eq!(meta.title, "Test OpenCode Session");
    assert_eq!(meta.project_path, "/home/user/my-opencode-project");
    // time_created ms → epoch seconds
    assert_eq!(meta.created_at, 1705321200);
    // time_updated ms → epoch seconds
    assert_eq!(meta.updated_at, 1705324800);
    assert!(meta.file_size_bytes > 0);
    assert!(
        sessions[0].source_mtime > 0,
        "OpenCode source mtime must be recorded for incremental polling"
    );
}

#[test]
fn opencode_incremental_scan_skips_unchanged_database() {
    let (_dir, db_path) = create_opencode_test_db();
    let provider = OpenCodeProvider::with_db_path(db_path.clone());

    let sessions = provider.scan_all().expect("scan_all must succeed");
    assert_eq!(sessions.len(), 1);
    let known = std::collections::HashMap::from([(
        db_path.to_string_lossy().to_string(),
        SourceState {
            size: sessions[0].meta.file_size_bytes,
            mtime: sessions[0].source_mtime,
            title: None,
        },
    )]);

    let outcome = provider
        .scan_incremental(&known)
        .expect("scan_incremental must succeed");

    assert!(outcome.parsed.is_empty());
    assert_eq!(
        outcome.unchanged_source_paths,
        vec![db_path.to_string_lossy().to_string()]
    );
}

#[test]
fn opencode_incremental_scan_ignores_empty_wal_mtime() {
    let (_dir, db_path) = create_opencode_test_db();
    let provider = OpenCodeProvider::with_db_path(db_path.clone());

    let sessions = provider.scan_all().expect("scan_all must succeed");
    let known = std::collections::HashMap::from([(
        db_path.to_string_lossy().to_string(),
        SourceState {
            size: sessions[0].meta.file_size_bytes,
            mtime: sessions[0].source_mtime,
            title: None,
        },
    )]);

    fs::write(format!("{}-wal", db_path.to_string_lossy()), b"")
        .expect("empty WAL marker must be written");

    let outcome = provider
        .scan_incremental(&known)
        .expect("scan_incremental must succeed");

    assert!(outcome.parsed.is_empty());
    assert_eq!(
        outcome.unchanged_source_paths,
        vec![db_path.to_string_lossy().to_string()]
    );
}

#[test]
fn opencode_incremental_scan_reparses_changed_database() {
    let (_dir, db_path) = create_opencode_test_db();
    let provider = OpenCodeProvider::with_db_path(db_path.clone());

    let sessions = provider.scan_all().expect("initial scan must succeed");
    let known = std::collections::HashMap::from([(
        db_path.to_string_lossy().to_string(),
        SourceState {
            size: sessions[0].meta.file_size_bytes,
            mtime: sessions[0].source_mtime,
            title: None,
        },
    )]);

    {
        use rusqlite::{Connection, params};
        let conn = Connection::open(&db_path).expect("open test db");
        conn.execute(
            "INSERT INTO session
                 (id, title, directory, project_id, parent_id, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "session-oc-0002",
                "Changed OpenCode Session",
                "/home/user/my-opencode-project",
                "proj-001",
                Option::<String>::None,
                1705324900000_i64,
                1705324900000_i64,
            ],
        )
        .expect("insert changed session");
    }

    let outcome = provider
        .scan_incremental(&known)
        .expect("changed scan must succeed");

    assert_eq!(outcome.parsed.len(), 2);
    assert!(outcome.unchanged_source_paths.is_empty());
}

/// The mechanism the incremental fix actually exists for: a real OpenCode write
/// lands in the `-wal` file first (the main `opencode.db` is untouched until a
/// checkpoint). A held-open writer with autocheckpoint disabled reproduces that,
/// so `scan_incremental` must detect the WAL-only growth via the combined
/// (main + non-empty-WAL) `(size, mtime)`. The other incremental tests run
/// against a rollback-journal DB and never exercise this path.
#[test]
fn opencode_incremental_scan_detects_wal_only_append() {
    use rusqlite::{Connection, params};

    let (_dir, db_path) = create_opencode_test_db();
    let provider = OpenCodeProvider::with_db_path(db_path.clone());

    // Switch the DB to WAL mode and keep the writer open with autocheckpoint
    // disabled, so appended rows stay in `<db>-wal` and are never folded back
    // into the main file for the duration of the test.
    let writer = Connection::open(&db_path).expect("open writer");
    writer
        .pragma_update(None, "journal_mode", "wal")
        .expect("enable WAL");
    writer
        .pragma_update(None, "wal_autocheckpoint", 0)
        .expect("disable autocheckpoint");

    let insert_session = |conn: &Connection, id: &str, ts: i64| {
        conn.execute(
            "INSERT INTO session
                 (id, title, directory, project_id, parent_id, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                "WAL session",
                "/home/user/my-opencode-project",
                "proj-001",
                Option::<String>::None,
                ts,
                ts,
            ],
        )
        .expect("insert session");
    };

    // Establish a non-empty WAL baseline, so freshness is computed against a
    // populated `-wal` exactly like a long-running OpenCode process.
    insert_session(&writer, "session-oc-wal-base", 1705324900000);
    let wal_path = PathBuf::from(format!("{}-wal", db_path.to_string_lossy()));
    assert!(
        fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0) > 0,
        "writer must leave a non-empty WAL"
    );

    let baseline = provider.scan_all().expect("baseline scan must succeed");
    assert_eq!(baseline.len(), 2, "original + WAL-baseline session");
    let known = std::collections::HashMap::from([(
        db_path.to_string_lossy().to_string(),
        SourceState {
            size: baseline[0].meta.file_size_bytes,
            mtime: baseline[0].source_mtime,
            title: None,
        },
    )]);

    // No write since the baseline → unchanged, even though the WAL is non-empty
    // and a writer is still attached.
    let unchanged = provider
        .scan_incremental(&known)
        .expect("scan_incremental must succeed");
    assert!(
        unchanged.parsed.is_empty(),
        "a stable non-empty WAL must read as unchanged"
    );
    assert_eq!(
        unchanged.unchanged_source_paths,
        vec![db_path.to_string_lossy().to_string()]
    );

    // A fresh append lands only in the WAL (no checkpoint) → must be detected.
    insert_session(&writer, "session-oc-wal-append", 1705325000000);
    let changed = provider
        .scan_incremental(&known)
        .expect("scan_incremental must succeed");
    assert_eq!(
        changed.parsed.len(),
        3,
        "WAL-only append must be detected and reparsed"
    );
    assert!(changed.unchanged_source_paths.is_empty());

    drop(writer);
}

#[test]
fn opencode_parses_message_count() {
    let (_dir, db_path) = create_opencode_test_db();
    let provider = OpenCodeProvider::with_db_path(db_path.clone());

    let sessions = provider.scan_all().expect("scan_all must succeed");
    // 3 rows exist in the message table
    assert_eq!(
        sessions[0].meta.message_count, 3,
        "expected 3 DB message rows in meta"
    );

    // load_messages expands them into parsed Message structs:
    //  1. User: "List files..."
    //  2. Assistant: "Sure, let me list..."
    //  3. Tool (Bash): ls output
    //  4. User: "Thanks..."
    let messages = provider
        .load_messages("session-oc-0001", &db_path.to_string_lossy())
        .expect("load_messages must succeed");

    assert_eq!(
        messages.len(),
        4,
        "expected 4 parsed messages, got: {:#?}",
        messages
    );
}

#[test]
fn opencode_tool_message_parsed() {
    let (_dir, db_path) = create_opencode_test_db();
    let provider = OpenCodeProvider::with_db_path(db_path.clone());

    let messages = provider
        .load_messages("session-oc-0001", &db_path.to_string_lossy())
        .expect("load_messages must succeed");

    let tool_msg = messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    assert_eq!(
        tool_msg.tool_name.as_deref(),
        Some("Bash"),
        "tool_name must be 'Bash', got: {:?}",
        tool_msg.tool_name
    );

    let input = tool_msg
        .tool_input
        .as_ref()
        .expect("tool message must have tool_input");
    assert!(
        input.contains("ls -la"),
        "tool_input must contain the command, got: {}",
        input
    );
    assert!(
        tool_msg.content.contains("main.rs"),
        "tool output must contain ls result, got: {}",
        tool_msg.content
    );
}

#[test]
fn opencode_tool_part_has_structured_tool_metadata() {
    let (_dir, db_path) = create_opencode_test_db();
    let provider = OpenCodeProvider::with_db_path(db_path.clone());

    let messages = provider
        .load_messages("session-oc-0001", &db_path.to_string_lossy())
        .expect("load_messages must succeed");

    let tool_msg = messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");
    let metadata = tool_msg
        .tool_metadata
        .as_ref()
        .expect("OpenCode tool metadata must be present");

    assert_eq!(metadata.raw_name, "Bash");
    assert_eq!(metadata.canonical_name, "Bash");
    assert_eq!(metadata.category, "shell");
    assert_eq!(metadata.summary.as_deref(), Some("ls -la"));
    assert_eq!(metadata.status.as_deref(), Some("completed"));
    assert_eq!(metadata.result_kind.as_deref(), Some("terminal_output"));
}

#[test]
fn opencode_token_usage() {
    // Build a minimal DB where the assistant message has ONLY a tool part
    // (no text). In that case the provider attaches token_usage to the last
    // tool message, which is the only way to observe it in this code path.
    use rusqlite::{Connection, params};

    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("opencode.db");
    let conn = Connection::open(&db_path).expect("open db");

    conn.execute_batch(
        "CREATE TABLE project (
             id TEXT PRIMARY KEY, name TEXT NOT NULL, worktree TEXT NOT NULL,
             time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
         );
         CREATE TABLE session (
             id TEXT PRIMARY KEY, title TEXT NOT NULL, directory TEXT NOT NULL,
             project_id TEXT, parent_id TEXT,
             time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
         );
         CREATE TABLE message (
             id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
             data TEXT NOT NULL, time_created INTEGER NOT NULL
         );
         CREATE TABLE part (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             message_id TEXT NOT NULL, session_id TEXT NOT NULL,
             data TEXT NOT NULL, time_created INTEGER NOT NULL
         );",
    )
    .expect("create tables");

    conn.execute(
        "INSERT INTO project (id, name, worktree, time_created, time_updated)
         VALUES ('p1','proj','/proj',1705321200000,1705321200000)",
        [],
    )
    .expect("insert project");
    conn.execute(
        "INSERT INTO session (id, title, directory, project_id, parent_id, time_created, time_updated)
         VALUES ('s1','Token Test','/proj','p1',NULL,1705321200000,1705321200000)",
        [],
    )
    .expect("insert session");

    // user message
    conn.execute(
        "INSERT INTO message (id, session_id, data, time_created) VALUES (?1,?2,?3,?4)",
        params![
            "m1",
            "s1",
            r#"{"role":"user","time":{"created":1705321200000}}"#,
            1705321200000_i64
        ],
    )
    .expect("insert user msg");

    // assistant message — carries token usage, has NO text part
    conn.execute(
        "INSERT INTO message (id, session_id, data, time_created) VALUES (?1,?2,?3,?4)",
        params![
            "m2", "s1",
            r#"{"role":"assistant","time":{"created":1705321260000},"tokens":{"input":50,"output":100,"reasoning":25,"cache":{"read":10,"write":5}}}"#,
            1705321260000_i64
        ],
    )
    .expect("insert assistant msg");

    // user text part
    conn.execute(
        "INSERT INTO part (message_id, session_id, data, time_created) VALUES (?1,?2,?3,?4)",
        params![
            "m1",
            "s1",
            r#"{"type":"text","text":"Hello"}"#,
            1705321200000_i64
        ],
    )
    .expect("insert user part");

    // assistant has only a tool part (no text part → token_usage goes onto tool msg)
    conn.execute(
        "INSERT INTO part (message_id, session_id, data, time_created) VALUES (?1,?2,?3,?4)",
        params![
            "m2", "s1",
            r#"{"type":"tool","tool":"Bash","state":{"status":"completed","input":"{\"command\":\"echo hi\"}","output":"hi"}}"#,
            1705321260000_i64
        ],
    )
    .expect("insert tool part");

    drop(conn);

    let provider = OpenCodeProvider::with_db_path(db_path.clone());
    let indexed = provider.scan_all().expect("scan_all must succeed");
    assert_eq!(
        indexed[0].messages[0]
            .token_usage
            .as_ref()
            .expect("indexed usage")
            .output_tokens,
        125
    );
    let messages = provider
        .load_messages("s1", &db_path.to_string_lossy())
        .expect("load_messages must succeed");

    // The assistant turn has no text parts, so token_usage lands on the tool message.
    let tool_msg = messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    let usage = tool_msg
        .token_usage
        .as_ref()
        .expect("tool message must carry token_usage when assistant has no text parts");
    assert_eq!(usage.input_tokens, 50);
    assert_eq!(usage.output_tokens, 125);
    assert_eq!(usage.cache_read_input_tokens, 10);
    assert_eq!(usage.cache_creation_input_tokens, 5);
}

// ---------------------------------------------------------------------------
// Real generated tool metadata smoke test
// ---------------------------------------------------------------------------

fn required_env_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must point to real CLI-generated data"))
}

fn assert_real_tool_metadata(
    provider: &str,
    messages: &[Message],
    canonical_name: &str,
    expect_marker_in_parsed_content: bool,
) {
    if expect_marker_in_parsed_content {
        assert!(
            messages
                .iter()
                .any(|message| message.content.contains("real-provider-tool-metadata")),
            "{provider} real transcript must contain the marker value"
        );
    }
    let tool = messages
        .iter()
        .find(|message| {
            message.role == MessageRole::Tool
                && message
                    .tool_metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.canonical_name == canonical_name)
        })
        .unwrap_or_else(|| {
            panic!(
                "{provider} real transcript must contain {canonical_name} tool metadata, got: {messages:#?}"
            )
        });
    let metadata = tool.tool_metadata.as_ref().unwrap();
    assert_eq!(metadata.canonical_name, canonical_name);
    assert!(
        !metadata.raw_name.is_empty(),
        "{provider} metadata must preserve the raw provider tool name"
    );
}

fn assert_has_tool_metadata<'a>(
    provider: &str,
    messages: &'a [Message],
    canonical_name: &str,
) -> &'a sessionview_lib::models::ToolMetadata {
    messages
        .iter()
        .find_map(|message| {
            (message.role == MessageRole::Tool)
                .then_some(message.tool_metadata.as_ref())
                .flatten()
                .filter(|metadata| metadata.canonical_name == canonical_name)
        })
        .unwrap_or_else(|| {
            panic!(
                "{provider} real transcript must contain {canonical_name} tool metadata, got: {messages:#?}"
            )
        })
}

#[test]
#[ignore]
fn real_generated_tool_metadata_smoke_test() {
    let codex_provider = CodexProvider::new().expect("home dir must be available");
    let codex = codex_provider
        .parse_session_file(&required_env_path("SESSIONVIEW_REAL_CODEX_JSONL"))
        .expect("real Codex transcript must parse");
    assert_real_tool_metadata("Codex", &codex.messages, "Bash", true);

    let antigravity = sessionview_lib::providers::antigravity::parser::parse_session_file(
        &required_env_path("SESSIONVIEW_REAL_ANTIGRAVITY_JSONL"),
    )
    .expect("real Antigravity transcript must parse");
    assert_real_tool_metadata("Antigravity", &antigravity.messages, "Read", true);

    let kimi_path = required_env_path("SESSIONVIEW_REAL_KIMI_WIRE");
    let kimi_provider = KimiProvider::new().expect("home dir must be available");
    let kimi_loaded = kimi_provider
        .load_messages("", &kimi_path.to_string_lossy())
        .expect("real Kimi transcript must parse");
    assert_real_tool_metadata("Kimi", &kimi_loaded.messages, "Read", true);

    let opencode_db = std::env::var_os("SESSIONVIEW_REAL_OPENCODE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("home dir must be available")
                .join(".local/share/opencode/opencode.db")
        });
    let opencode_session =
        std::env::var("SESSIONVIEW_REAL_OPENCODE_SESSION").expect("real OpenCode session id");
    let opencode_provider = OpenCodeProvider::with_db_path(opencode_db.clone());
    let opencode_messages = opencode_provider
        .load_messages(&opencode_session, &opencode_db.to_string_lossy())
        .expect("real OpenCode DB session must load messages");
    assert_real_tool_metadata("OpenCode", &opencode_messages, "Read", true);
}

#[test]
#[ignore]
fn round2_generated_tool_metadata_smoke_test() {
    let codex_provider = CodexProvider::new().expect("home dir must be available");
    let codex = codex_provider
        .parse_session_file(&required_env_path("SESSIONVIEW_ROUND2_CODEX_JSONL"))
        .expect("round2 Codex transcript must parse");
    assert_real_tool_metadata("Codex round2", &codex.messages, "Bash", true);

    let antigravity = sessionview_lib::providers::antigravity::parser::parse_session_file(
        &required_env_path("SESSIONVIEW_ROUND2_ANTIGRAVITY_JSONL"),
    )
    .expect("round2 Antigravity transcript must parse");
    assert_has_tool_metadata("Antigravity round2", &antigravity.messages, "Read");

    let kimi_path = required_env_path("SESSIONVIEW_ROUND2_KIMI_WIRE");
    let kimi_provider = KimiProvider::new().expect("home dir must be available");
    let kimi_loaded = kimi_provider
        .load_messages("", &kimi_path.to_string_lossy())
        .expect("round2 Kimi transcript must parse");
    assert_has_tool_metadata("Kimi round2", &kimi_loaded.messages, "Read");
    assert_has_tool_metadata("Kimi round2", &kimi_loaded.messages, "Bash");
    assert_eq!(
        assert_has_tool_metadata("Kimi round2", &kimi_loaded.messages, "Edit")
            .result_kind
            .as_deref(),
        Some("file_patch")
    );

    let opencode_db = std::env::var_os("SESSIONVIEW_ROUND2_OPENCODE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("home dir must be available")
                .join(".local/share/opencode/opencode.db")
        });
    let opencode_session =
        std::env::var("SESSIONVIEW_ROUND2_OPENCODE_SESSION").expect("round2 OpenCode session id");
    let opencode_provider = OpenCodeProvider::with_db_path(opencode_db.clone());
    let opencode_messages = opencode_provider
        .load_messages(&opencode_session, &opencode_db.to_string_lossy())
        .expect("round2 OpenCode DB session must load messages");
    assert_has_tool_metadata("OpenCode round2", &opencode_messages, "Glob");
    assert_has_tool_metadata("OpenCode round2", &opencode_messages, "Read");
    assert_has_tool_metadata("OpenCode round2", &opencode_messages, "Grep");
    assert_eq!(
        assert_has_tool_metadata("OpenCode round2", &opencode_messages, "Edit")
            .result_kind
            .as_deref(),
        Some("file_patch")
    );
}

// ---------------------------------------------------------------------------
// Grok parser tests (fixtures under tests/fixtures/grok/)
// ---------------------------------------------------------------------------

fn grok_fixture_provider() -> GrokProvider {
    GrokProvider::with_root(fixtures_dir().join("grok"))
}

const GROK_BASIC_ID: &str = "01900000-0000-7000-8000-000000000001";

fn parse_grok_fixture_session(session_id: &str) -> sessionview_lib::provider::ParsedSession {
    let provider = grok_fixture_provider();
    let sessions = provider.scan_all().expect("grok fixture scan must succeed");
    sessions
        .into_iter()
        .find(|s| s.meta.id == session_id)
        .unwrap_or_else(|| panic!("session {session_id} missing from grok fixture scan"))
}

#[test]
fn grok_scans_fixture_session_with_summary_metadata() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    assert_eq!(session.meta.provider, Provider::Grok);
    assert_eq!(session.meta.title, "Demo project check");
    assert_eq!(session.meta.project_path, "/tmp/demo-project");
    assert_eq!(session.meta.project_name, "demo-project");
    assert_eq!(session.meta.model.as_deref(), Some("grok-4.5"));
    assert!(!session.meta.is_sidechain);
    assert!(session.meta.created_at > 0);
    assert!(session.meta.updated_at >= session.meta.created_at);
    assert_eq!(session.parse_warning_count, 0);
}

#[test]
fn grok_skips_synthetic_context_and_strips_user_query_wrapper() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    let users: Vec<&Message> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .collect();
    // <user_info> preamble and system reminder are CLI-injected, not user
    // content — prompt_index-bearing entries survive, plus compaction-
    // preserved prompts recognized by their <user_query> wrapper.
    assert_eq!(users.len(), 3);
    assert_eq!(users[0].content, "check the demo files");
    assert_eq!(users[2].content, "legacy prompt preserved by compaction");
    assert!(
        users[2].timestamp.is_none(),
        "index-less prompts have no anchor"
    );
    // Timestamp anchored from updates.jsonl user_message_chunk (epoch 1782892830).
    assert_eq!(
        users[0].timestamp.as_deref(),
        Some("2026-07-01T08:00:30+00:00")
    );
}

#[test]
fn grok_reasoning_summary_emitted_as_thinking_system_message() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    let thinking = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::System && m.content.starts_with("[thinking]\n"))
        .expect("reasoning summary must surface as [thinking] system message");
    assert!(thinking.content.contains("list the directory"));
}

#[test]
fn grok_tool_calls_merge_results_and_map_canonical_names() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    let bash = session
        .messages
        .iter()
        .find(|m| m.tool_name.as_deref() == Some("Bash"))
        .expect("run_terminal_command must canonicalize to Bash");
    assert!(bash.content.contains("README.md"), "result must be merged");
    assert_eq!(
        bash.timestamp.as_deref(),
        Some("2026-07-01T08:01:00+00:00"),
        "tool timestamp anchored from updates.jsonl tool_call"
    );
    let read = session
        .messages
        .iter()
        .find(|m| m.tool_name.as_deref() == Some("Read"))
        .expect("read_file must canonicalize to Read");
    assert!(read.content.contains("# Demo Project"));
}

#[test]
fn grok_usage_events_from_turn_completed_model_usage() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    assert_eq!(session.usage_events.len(), 1);
    let event = &session.usage_events[0];
    assert_eq!(event.model, "grok-4.5");
    assert_eq!(event.input_tokens, 13312 - 11264);
    assert_eq!(event.output_tokens, 106);
    assert_eq!(event.cache_read_input_tokens, 11264);

    let provider = grok_fixture_provider();
    let rows = provider.compute_token_stats(&session, None, None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].input_tokens, 13312 - 11264);
    assert_eq!(rows[0].output_tokens, 106);
    assert_eq!(rows[0].cache_read_tokens, 11264);
    assert_eq!(rows[0].turn_count, 1);
}

#[test]
fn grok_load_messages_totals_come_from_usage_events() {
    let provider = grok_fixture_provider();
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    let loaded = provider
        .load_messages(GROK_BASIC_ID, &session.meta.source_path)
        .expect("grok fixture must load messages");
    assert_eq!(loaded.token_totals.input_tokens, 13312 - 11264);
    assert_eq!(loaded.token_totals.output_tokens, 106);
    assert_eq!(loaded.token_totals.cache_read_tokens, 11264);
}

#[test]
fn grok_scan_incremental_short_circuits_unchanged_and_reparses_on_title_change() {
    use std::collections::HashMap;

    let provider = grok_fixture_provider();

    let fresh_state_of = |session: &sessionview_lib::provider::ParsedSession| SourceState {
        size: session.meta.file_size_bytes,
        mtime: {
            let metadata =
                fs::metadata(&session.meta.source_path).expect("fixture chat file must stat");
            let modified = metadata.modified().expect("fixture mtime must exist");
            modified
                .duration_since(std::time::UNIX_EPOCH)
                .expect("mtime after epoch")
                .as_secs() as i64
        },
        title: Some(session.meta.title.clone()),
    };

    // All fixture sessions unchanged + matching titles → full short-circuit.
    let sessions = provider.scan_all().expect("grok fixture scan");
    let mut known = HashMap::new();
    for session in &sessions {
        known.insert(session.meta.source_path.clone(), fresh_state_of(session));
    }
    let outcome = provider.scan_incremental(&known).expect("incremental scan");
    assert!(outcome.parsed.is_empty());
    assert_eq!(outcome.unchanged_source_paths.len(), sessions.len());

    // Unchanged file but stored title disagrees with summary.json → that
    // session (and only that session) is promoted to a re-parse.
    let parent_path = sessions
        .iter()
        .find(|s| s.meta.id == GROK_BASIC_ID)
        .expect("parent fixture present")
        .meta
        .source_path
        .clone();
    let mut stale = known.clone();
    stale.get_mut(&parent_path).expect("parent state").title = Some("old stored title".to_string());
    let outcome = provider.scan_incremental(&stale).expect("incremental scan");
    assert_eq!(outcome.parsed.len(), 1);
    assert_eq!(outcome.parsed[0].meta.id, GROK_BASIC_ID);
    assert_eq!(outcome.unchanged_source_paths.len(), sessions.len() - 1);
}

const GROK_CHILD_ID: &str = "01900000-0000-7000-8000-000000000002";

#[test]
fn grok_subagent_child_links_to_parent_and_uses_description_title() {
    let child = parse_grok_fixture_session(GROK_CHILD_ID);
    assert!(child.meta.is_sidechain);
    assert_eq!(child.meta.parent_id.as_deref(), Some(GROK_BASIC_ID));
    // Parent-side meta.json description beats the child's generated_title.
    assert_eq!(child.meta.title, "Demo child task");
}

#[test]
fn grok_parent_surfaces_child_session_ids_and_agent_id_metadata() {
    let parent = parse_grok_fixture_session(GROK_BASIC_ID);
    assert_eq!(parent.child_session_ids, vec![GROK_CHILD_ID.to_string()]);

    let spawn = parent
        .messages
        .iter()
        .find(|m| m.tool_name.as_deref() == Some("Agent"))
        .expect("spawn_subagent must canonicalize to Agent");
    assert!(spawn.content.contains("Subagent started"));
    let structured = spawn
        .tool_metadata
        .as_ref()
        .and_then(|m| m.structured.as_ref())
        .expect("spawn result must carry structured metadata");
    assert_eq!(
        structured.get("agentId").and_then(|v| v.as_str()),
        Some(GROK_CHILD_ID),
        "subagent_id line must be promoted to structured.agentId"
    );
}

#[test]
fn grok_turn_usage_attached_to_final_assistant_message() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    let closing = session
        .messages
        .iter()
        .find(|m| m.content.contains("single README.md"))
        .expect("turn-closing assistant message present");
    let usage = closing
        .token_usage
        .as_ref()
        .expect("turn totals must attach to the turn's final assistant message");
    // inputTokens includes cachedReadTokens — stored disjoint.
    assert_eq!(usage.input_tokens, 13312 - 11264);
    assert_eq!(usage.output_tokens, 106);
    assert_eq!(usage.cache_read_input_tokens, 11264);
    assert_eq!(usage.cache_creation_input_tokens, 0);
    // Turn-end timestamp lands on the message that had none.
    assert_eq!(
        closing.timestamp.as_deref(),
        Some("2026-07-01T08:02:00+00:00")
    );
    // Mid-turn assistant messages carry no usage.
    let mid_turn = session
        .messages
        .iter()
        .find(|m| m.content == "Let me check the files.")
        .expect("mid-turn assistant message present");
    assert!(mid_turn.token_usage.is_none());
}

#[test]
fn grok_image_prompt_pairs_saved_asset_paths() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    let image_prompt = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .nth(1)
        .expect("image prompt present");
    assert_eq!(
        image_prompt.content,
        "[Image: source: /tmp/fake-home/.grok/sessions/%2Ftmp%2Fdemo-project/01900000-0000-7000-8000-000000000001/assets/image-00000000-0000-0000-0000-000000000001.png]",
        "image block must pair with the <image_files> saved path; the CLI-generated list text is dropped"
    );
}

#[test]
fn grok_task_output_result_suppresses_duplicate_raw_output() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    // The spawn tool's Agent result keeps its kind; TaskOutput-style results
    // (get_command_or_subagent_output) mark terminal_output so the UI hides
    // the raw copy that the result detail already shows. Covered here via
    // metadata on a synthetic enrich to avoid inflating the fixture.
    use sessionview_lib::tool_metadata::{
        ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
    };
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Grok,
        raw_name: "get_command_or_subagent_output",
        input: None,
        call_id: Some("call-1"),
        assistant_id: None,
    });
    assert_eq!(metadata.canonical_name, "TaskOutput");
    let result = serde_json::json!({"toolCallId": "call-1", "output": "=== Task done ==="});
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&result),
            is_error: None,
            status: None,
            artifact_path: None,
        },
    );
    assert_eq!(metadata.result_kind.as_deref(), Some("terminal_output"));
    let _ = session;
}

#[test]
fn grok_non_terminal_results_stay_out_of_structured_output() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    // Glob (list-like) results render as the raw output block only —
    // structured.output would duplicate the same text in the detail lines.
    let read = session
        .messages
        .iter()
        .find(|m| m.tool_name.as_deref() == Some("Read"))
        .expect("Read tool message present");
    let structured = read
        .tool_metadata
        .as_ref()
        .and_then(|m| m.structured.as_ref());
    assert!(
        structured.and_then(|s| s.get("output")).is_none(),
        "non-terminal tools must not mirror raw output into structured"
    );
    // Bash keeps output in structured: its terminal_output kind suppresses
    // the raw copy, so the detail line is the only render.
    let bash = session
        .messages
        .iter()
        .find(|m| m.tool_name.as_deref() == Some("Bash"))
        .expect("Bash tool message present");
    let bash_meta = bash.tool_metadata.as_ref().expect("bash metadata");
    assert_eq!(bash_meta.result_kind.as_deref(), Some("terminal_output"));
    assert!(
        bash_meta
            .structured
            .as_ref()
            .and_then(|s| s.get("output"))
            .is_some()
    );
}

#[test]
fn grok_compaction_summary_surfaces_and_reinjected_context_is_skipped() {
    let session = parse_grok_fixture_session(GROK_BASIC_ID);
    let compaction = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::System && m.content.starts_with("[Compaction] "))
        .expect("compaction summary must surface as a system message");
    assert!(compaction.content.contains("scaffolded the demo project"));
    // The compaction_meta re-injection of <user_info> stays hidden.
    assert!(
        !session
            .messages
            .iter()
            .any(|m| m.content.contains("<user_info>"))
    );
}

const GROK_COMPACTED_ID: &str = "01900000-0000-7000-8000-000000000003";

#[test]
fn grok_compacted_session_reconstructs_history_from_updates() {
    let session = parse_grok_fixture_session(GROK_COMPACTED_ID);
    let contents: Vec<&str> = session
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect();

    // Pre-compaction turns rebuilt from updates.jsonl, in order, ahead of
    // the surviving transcript.
    let hello = contents
        .iter()
        .position(|c| *c == "hello")
        .expect("reconstructed prompt 0");
    let listing = contents
        .iter()
        .position(|c| c.contains("demo project contains README.md"))
        .expect("reconstructed assistant answer");
    let compaction_note = contents
        .iter()
        .position(|c| c.starts_with("[Compaction] "))
        .expect("compaction summary note");
    let tail = contents
        .iter()
        .position(|c| *c == "what next?")
        .expect("surviving prompt");
    assert!(hello < listing && listing < compaction_note && compaction_note < tail);

    // Reconstructed tool call: canonical name + result from rawOutput variant.
    let glob = session
        .messages
        .iter()
        .find(|m| m.tool_name.as_deref() == Some("Glob"))
        .expect("reconstructed list_dir call");
    assert!(glob.content.contains("README.md"));
    assert_eq!(
        glob.timestamp.as_deref(),
        Some("2026-07-02T08:01:50+00:00"),
        "tool timestamp from updates wrapper"
    );

    // The verbatim prompt grok preserved in the compacted transcript
    // ("list the demo files") must not appear twice.
    assert_eq!(
        contents
            .iter()
            .filter(|c| **c == "list the demo files")
            .count(),
        1,
        "preserved verbatim prompt must not duplicate the reconstruction"
    );

    // Reconstructed turns keep their usage.
    let greeting = session
        .messages
        .iter()
        .find(|m| m.content == "Hi! How can I help?")
        .expect("reconstructed greeting");
    let usage = greeting.token_usage.as_ref().expect("turn usage attached");
    assert_eq!(usage.input_tokens, 1000 - 800);
    assert_eq!(usage.output_tokens, 20);
    assert_eq!(usage.cache_read_input_tokens, 800);

    // All three turns land in usage_events for the stats layer.
    assert_eq!(session.usage_events.len(), 3);
}

#[test]
fn grok_session_parses_without_summary_json() {
    // The id and cwd can be recovered from the path when summary.json is
    // unavailable (dir name / percent-encoded parent dir).
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp
        .path()
        .join("sessions")
        .join("%2Ftmp%2Fdemo-project")
        .join("01900000-0000-7000-8000-00000000000f");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("chat_history.jsonl"),
        concat!(
            "{\"type\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"<user_query>\\nrestored prompt\\n</user_query>\"}],\"prompt_index\":0}\n",
            "{\"type\":\"assistant\",\"content\":\"Restored answer.\",\"model_id\":\"grok-4.5\"}\n",
        ),
    )
    .unwrap();

    let provider = GrokProvider::with_root(tmp.path().to_path_buf());
    let sessions = provider.scan_all().expect("scan session");
    assert_eq!(sessions.len(), 1);
    let meta = &sessions[0].meta;
    assert_eq!(meta.id, "01900000-0000-7000-8000-00000000000f");
    assert_eq!(meta.project_path, "/tmp/demo-project");
    assert_eq!(meta.title, "restored prompt");
    assert!(meta.created_at > 0);
}
