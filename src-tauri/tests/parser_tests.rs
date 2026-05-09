use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use cc_session_lib::models::{Message, MessageRole, Provider};
use cc_session_lib::provider::SessionProvider;
use cc_session_lib::providers::claude::ClaudeProvider;
use cc_session_lib::providers::codex::CodexProvider;
use cc_session_lib::providers::gemini::GeminiProvider;
use cc_session_lib::providers::kimi::KimiProvider;
use cc_session_lib::providers::opencode::OpenCodeProvider;
use cc_session_lib::providers::qwen::parser as qwen_parser;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn parse_temp_claude_jsonl(content: &str) -> cc_session_lib::provider::ParsedSession {
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

    use cc_session_lib::models::MessageRole;
    let system_msgs: Vec<&cc_session_lib::models::Message> = session
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

    let system_contents: Vec<&str> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::System)
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        system_contents.contains(&"[local_command] /model"),
        "slash command should be visible as a system message: {:?}",
        system_contents
    );
    assert!(
        system_contents.contains(&"[local_command] Kept model as Opus 4.6"),
        "local command output should be visible and ANSI-stripped: {:?}",
        system_contents
    );
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
    assert_eq!(usage.input_tokens, 120);
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
    assert!(session
        .messages
        .iter()
        .any(|m| m.role == MessageRole::Assistant
            && m.content.contains("Module A handles authentication")));
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
// ---------------------------------------------------------------------------

#[test]
fn kimi_parses_message_count() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let path = fixtures_dir()
        .join("kimi")
        .join("abc123def456")
        .join("session-uuid-0001")
        .join("wire.jsonl");
    let project_map = HashMap::new();
    let session = provider
        .parse_session_file(&path, &project_map)
        .expect("kimi fixture must parse");

    // Expected messages:
    //  1. User: "List files in the current directory"
    //  2. System (thinking): "[thinking]\nThe user wants to list files..."
    //  3. Tool (Bash): Shell call, content = merged output
    //  4. Assistant: "Here are the files..." (token_usage attached)
    assert_eq!(
        session.messages.len(),
        4,
        "expected 4 messages, got: {:#?}",
        session.messages
    );
}

#[test]
fn kimi_user_message_role_and_content() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let path = fixtures_dir()
        .join("kimi")
        .join("abc123def456")
        .join("session-uuid-0001")
        .join("wire.jsonl");
    let project_map = HashMap::new();
    let session = provider
        .parse_session_file(&path, &project_map)
        .expect("kimi fixture must parse");

    let first = &session.messages[0];
    assert_eq!(first.role, MessageRole::User);
    assert!(
        first.content.contains("List files"),
        "unexpected content: {}",
        first.content
    );
}

#[test]
fn kimi_thinking_emitted_as_system_role() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let path = fixtures_dir()
        .join("kimi")
        .join("abc123def456")
        .join("session-uuid-0001")
        .join("wire.jsonl");
    let project_map = HashMap::new();
    let session = provider
        .parse_session_file(&path, &project_map)
        .expect("kimi fixture must parse");

    let thinking = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::System)
        .expect("expected a thinking (System) message");

    assert!(
        thinking.content.starts_with("[thinking]\n"),
        "thinking message must start with [thinking]\\n, got: {}",
        thinking.content
    );
}

#[test]
fn kimi_shell_tool_mapped_to_bash() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let path = fixtures_dir()
        .join("kimi")
        .join("abc123def456")
        .join("session-uuid-0001")
        .join("wire.jsonl");
    let project_map = HashMap::new();
    let session = provider
        .parse_session_file(&path, &project_map)
        .expect("kimi fixture must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    assert_eq!(
        tool_msg.tool_name.as_deref(),
        Some("Bash"),
        "Shell must map to Bash, got: {:?}",
        tool_msg.tool_name
    );
}

#[test]
fn kimi_tool_result_merged_into_tool_call() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let path = fixtures_dir()
        .join("kimi")
        .join("abc123def456")
        .join("session-uuid-0001")
        .join("wire.jsonl");
    let project_map = HashMap::new();
    let session = provider
        .parse_session_file(&path, &project_map)
        .expect("kimi fixture must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    assert!(
        tool_msg.content.contains("main.rs"),
        "tool result must be merged, got: {}",
        tool_msg.content
    );
}

#[test]
fn kimi_tool_call_has_structured_tool_metadata() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let path = fixtures_dir()
        .join("kimi")
        .join("abc123def456")
        .join("session-uuid-0001")
        .join("wire.jsonl");
    let project_map = HashMap::new();
    let session = provider
        .parse_session_file(&path, &project_map)
        .expect("kimi fixture must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");
    let metadata = tool_msg
        .tool_metadata
        .as_ref()
        .expect("Kimi tool metadata must be present");

    assert_eq!(metadata.raw_name, "Shell");
    assert_eq!(metadata.canonical_name, "Bash");
    assert_eq!(metadata.category, "shell");
    assert_eq!(metadata.summary.as_deref(), Some("ls -la"));
    assert_eq!(metadata.status.as_deref(), Some("success"));
    assert_eq!(metadata.result_kind.as_deref(), Some("terminal_output"));
}

#[test]
fn kimi_token_usage_from_status_update() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let path = fixtures_dir()
        .join("kimi")
        .join("abc123def456")
        .join("session-uuid-0001")
        .join("wire.jsonl");
    let project_map = HashMap::new();
    let session = provider
        .parse_session_file(&path, &project_map)
        .expect("kimi fixture must parse");

    // StatusUpdate attaches to last assistant or tool message
    let last_with_usage = session
        .messages
        .iter()
        .rev()
        .find(|m| m.token_usage.is_some())
        .expect("expected at least one message with token_usage");

    let usage = last_with_usage.token_usage.as_ref().unwrap();
    // input_tokens = input_other(80) + input_cache_read(10) + input_cache_creation(5) = 95
    assert_eq!(usage.input_tokens, 95);
    assert_eq!(usage.output_tokens, 35);
    assert_eq!(usage.cache_read_input_tokens, 10);
    assert_eq!(usage.cache_creation_input_tokens, 5);
}

#[test]
fn kimi_session_id_from_parent_directory() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let path = fixtures_dir()
        .join("kimi")
        .join("abc123def456")
        .join("session-uuid-0001")
        .join("wire.jsonl");
    let project_map = HashMap::new();
    let session = provider
        .parse_session_file(&path, &project_map)
        .expect("kimi fixture must parse");

    // Session ID = parent directory name (the session UUID dir)
    assert_eq!(session.meta.id, "session-uuid-0001");
}

// ---------------------------------------------------------------------------
// Kimi subagent tests (extracted from SubagentEvent in parent wire.jsonl)
// ---------------------------------------------------------------------------

fn kimi_parent_with_subagents() -> Vec<cc_session_lib::provider::ParsedSession> {
    let provider = KimiProvider::new().expect("home dir must be available");
    let path = fixtures_dir()
        .join("kimi")
        .join("abc123def456")
        .join("session-uuid-0001")
        .join("wire.jsonl");
    let project_map = HashMap::new();
    provider.parse_session_with_subagents(&path, &project_map)
}

#[test]
fn kimi_subagent_extracted_from_parent() {
    let sessions = kimi_parent_with_subagents();
    assert_eq!(
        sessions.len(),
        2,
        "expected 2 sessions (parent + 1 subagent), got {}",
        sessions.len()
    );
}

#[test]
fn kimi_subagent_is_sidechain() {
    let sessions = kimi_parent_with_subagents();
    let sub = sessions
        .iter()
        .find(|s| s.meta.is_sidechain)
        .expect("expected a sidechain session");

    assert_eq!(sub.meta.id, "a1b2c3d4e");
    assert_eq!(
        sub.meta.parent_id.as_deref(),
        Some("session-uuid-0001"),
        "parent_id must be the parent session UUID"
    );
}

#[test]
fn kimi_subagent_title_from_meta() {
    let sessions = kimi_parent_with_subagents();
    let sub = sessions
        .iter()
        .find(|s| s.meta.is_sidechain)
        .expect("expected a sidechain session");

    // When meta.json exists, title comes from description.
    // Without meta.json, falls back to first user message.
    assert_eq!(
        sub.meta.title, "Analyze the project structure of this repo",
        "subagent title must fall back to first user message when meta.json is absent"
    );
}

#[test]
fn kimi_subagent_messages_parsed() {
    let sessions = kimi_parent_with_subagents();
    let sub = sessions
        .iter()
        .find(|s| s.meta.is_sidechain)
        .expect("expected a sidechain session");

    // Expected: User, System(thinking), Tool(Bash), Assistant
    assert_eq!(
        sub.messages.len(),
        4,
        "expected 4 messages in subagent, got: {:#?}",
        sub.messages
    );
    assert_eq!(sub.messages[0].role, MessageRole::User);
    assert_eq!(sub.messages[1].role, MessageRole::System); // thinking
    assert_eq!(sub.messages[2].role, MessageRole::Tool);
    assert_eq!(sub.messages[3].role, MessageRole::Assistant);
}

#[test]
fn kimi_toolcallpart_appends_to_empty_arguments() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("session-001");
    fs::create_dir(&session_dir).unwrap();
    let wire = session_dir.join("wire.jsonl");

    let lines = [
        r#"{"timestamp":1735725600.0,"message":{"type":"TurnBegin","payload":{"user_input":[{"type":"text","text":"list files"}]}}}"#,
        // ToolCall with empty arguments
        r#"{"timestamp":1735725601.0,"message":{"type":"ToolCall","payload":{"id":"call_001","function":{"name":"Shell","arguments":""}}}}"#,
        // ToolCallPart supplies the actual arguments
        r#"{"timestamp":1735725602.0,"message":{"type":"ToolCallPart","payload":{"arguments_part":"{\"command\":\"ls -la\"}"}}}"#,
        r#"{"timestamp":1735725603.0,"message":{"type":"ToolResult","payload":{"tool_call_id":"call_001","return_value":{"output":"total 16","message":"Command executed successfully."}}}}"#,
        r#"{"timestamp":1735725604.0,"message":{"type":"ContentPart","payload":{"type":"text","text":"Here are the files"}}}"#,
    ];
    fs::write(&wire, lines.join("\n")).unwrap();

    let session = provider
        .parse_session_file(&wire, &HashMap::new())
        .expect("must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    assert_eq!(
        tool_msg.tool_input.as_deref(),
        Some(r#"{"command":"ls -la"}"#),
        "ToolCallPart must append to empty arguments"
    );

    // Shell tool should show raw output, not generic success message
    assert_eq!(tool_msg.content, "total 16");
}

#[test]
fn kimi_toolcallpart_appends_to_truncated_arguments() {
    let provider = KimiProvider::new().expect("home dir must be available");
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("session-002");
    fs::create_dir(&session_dir).unwrap();
    let wire = session_dir.join("wire.jsonl");

    let lines = [
        r#"{"timestamp":1735725600.0,"message":{"type":"TurnBegin","payload":{"user_input":[{"type":"text","text":"read file"}]}}}"#,
        // ToolCall with truncated JSON (missing closing quote and brace)
        r#"{"timestamp":1735725601.0,"message":{"type":"ToolCall","payload":{"id":"call_002","function":{"name":"ReadFile","arguments":"{\"path\":\"/tmp/test"}}}}"#,
        // ToolCallPart supplies the missing suffix
        r#"{"timestamp":1735725602.0,"message":{"type":"ToolCallPart","payload":{"arguments_part":".txt\"}"}}}"#,
        r#"{"timestamp":1735725603.0,"message":{"type":"ToolResult","payload":{"tool_call_id":"call_002","return_value":{"output":"hello","message":"1 line read"}}}}"#,
        r#"{"timestamp":1735725604.0,"message":{"type":"ContentPart","payload":{"type":"text","text":"Done"}}}"#,
    ];
    fs::write(&wire, lines.join("\n")).unwrap();

    let session = provider
        .parse_session_file(&wire, &HashMap::new())
        .expect("must parse");

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    assert_eq!(
        tool_msg.tool_input.as_deref(),
        Some(r#"{"path":"/tmp/test.txt"}"#),
        "ToolCallPart must append to truncated arguments"
    );
}

// ---------------------------------------------------------------------------
// Gemini chat parser tests
// ---------------------------------------------------------------------------

fn gemini_fixture_path() -> PathBuf {
    fixtures_dir().join("gemini").join("session-test.json")
}

fn gemini_jsonl_fixture_path() -> PathBuf {
    fixtures_dir()
        .join("gemini")
        .join("session-jsonl-test.jsonl")
}

fn gemini_jsonl_rich_fixture_path() -> PathBuf {
    fixtures_dir()
        .join("gemini")
        .join("session-jsonl-rich-test.jsonl")
}

fn gemini_jsonl_subagent_fixture_path() -> PathBuf {
    fixtures_dir()
        .join("gemini")
        .join("chats")
        .join("gemini-parent-001")
        .join("gemini-child-001.jsonl")
}

fn gemini_parsed_session() -> cc_session_lib::provider::ParsedSession {
    let provider = GeminiProvider::new().expect("home dir must be available");
    let path = gemini_fixture_path();
    let project_map = HashMap::new();
    let sessions = provider.parse_chat_file_for_test(&path, &project_map);
    assert!(
        !sessions.is_empty(),
        "gemini fixture must parse at least one session"
    );
    sessions.into_iter().next().unwrap()
}

#[test]
fn gemini_parses_message_count() {
    let session = gemini_parsed_session();

    // Expected messages:
    //  1. User: "List the files in the current directory"
    //  2. Assistant: "I'll run a shell command to list the files for you."
    //  3. Tool (Bash): Shell call with merged result (token_usage attached here)
    //  4. User: "Thanks, that looks good!"
    assert_eq!(
        session.messages.len(),
        4,
        "expected 4 messages, got: {:#?}",
        session.messages
    );
}

#[test]
fn gemini_first_user_message() {
    let session = gemini_parsed_session();

    let first = &session.messages[0];
    assert_eq!(first.role, MessageRole::User);
    assert!(
        first.content.contains("List the files"),
        "unexpected content: {}",
        first.content
    );
}

#[test]
fn gemini_tool_call_parsed() {
    let session = gemini_parsed_session();

    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    // Shell displayName maps to canonical "Bash"
    assert_eq!(
        tool_msg.tool_name.as_deref(),
        Some("Bash"),
        "Shell must map to Bash, got: {:?}",
        tool_msg.tool_name
    );

    // tool_input must contain the command key (Bash remapping)
    let input = tool_msg
        .tool_input
        .as_ref()
        .expect("Bash tool must have tool_input");
    assert!(
        input.contains("\"command\""),
        "tool_input must use 'command' key, got: {}",
        input
    );
    assert!(
        input.contains("ls -la"),
        "tool_input must contain the shell command, got: {}",
        input
    );

    // Tool result must be merged into content
    assert!(
        tool_msg.content.contains("main.rs"),
        "tool result must be merged into content, got: {}",
        tool_msg.content
    );
}

#[test]
fn gemini_tool_call_has_structured_tool_metadata() {
    let session = gemini_parsed_session();
    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");
    let metadata = tool_msg
        .tool_metadata
        .as_ref()
        .expect("Gemini tool metadata must be present");

    assert_eq!(metadata.raw_name, "run_shell_command");
    assert_eq!(metadata.canonical_name, "Bash");
    assert_eq!(metadata.category, "shell");
    assert_eq!(metadata.summary.as_deref(), Some("ls -la"));
    assert_eq!(metadata.status.as_deref(), Some("success"));
    assert_eq!(metadata.result_kind.as_deref(), Some("terminal_output"));
}

#[test]
fn gemini_token_usage() {
    let session = gemini_parsed_session();

    // Token usage is on the model message's last tool call (the Bash tool message)
    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    let usage = tool_msg
        .token_usage
        .as_ref()
        .expect("last tool message must carry token_usage from the model turn");
    assert_eq!(usage.input_tokens, 150);
    assert_eq!(usage.output_tokens, 45);
    assert_eq!(usage.cache_read_input_tokens, 20);
}

#[test]
fn gemini_jsonl_chat_parser_uses_latest_message_record() {
    let provider = GeminiProvider::new().expect("home dir must be available");
    let path = gemini_jsonl_fixture_path();
    let sessions = provider.parse_chat_file_for_test(&path, &HashMap::new());
    let session = sessions
        .first()
        .expect("Gemini JSONL fixture must parse one session");

    assert_eq!(session.meta.id, "gemini-jsonl-test-001");
    assert_eq!(
        session.meta.model.as_deref(),
        Some("gemini-3-flash-preview")
    );
    assert_eq!(session.meta.updated_at, 1777461344);
    assert_eq!(
        session.messages.len(),
        4,
        "expected user, thinking, tool, assistant messages, got: {:#?}",
        session.messages
    );

    let thinking_count = session
        .messages
        .iter()
        .filter(|message| message.content.starts_with("[thinking]"))
        .count();
    assert_eq!(
        thinking_count, 1,
        "duplicate JSONL records with the same id must be replaced"
    );

    let tool_msg = session
        .messages
        .iter()
        .find(|message| message.role == MessageRole::Tool)
        .expect("expected Gemini JSONL tool message");
    assert_eq!(tool_msg.tool_name.as_deref(), Some("Read"));
    assert_eq!(
        tool_msg
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.status.as_deref()),
        Some("success")
    );
    assert!(
        tool_msg.content.contains("ccsession"),
        "tool result must be preserved"
    );
    assert!(
        tool_msg.token_usage.is_some(),
        "last tool call from the Gemini turn must carry token usage"
    );
}

#[test]
fn gemini_jsonl_chat_parser_preserves_new_content_parts_and_warning() {
    let provider = GeminiProvider::new().expect("home dir must be available");
    let path = gemini_jsonl_rich_fixture_path();
    let sessions = provider.parse_chat_file_for_test(&path, &HashMap::new());
    let session = sessions
        .first()
        .expect("Gemini rich JSONL fixture must parse one session");

    let user = session
        .messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .expect("expected user message");
    assert!(
        user.content
            .contains("[Image: source: /tmp/ccsession-gemini-probe/red32.png]"),
        "fileData image must be preserved, got: {}",
        user.content
    );
    assert!(
        user.content
            .contains("[Image: source: data:image/png;base64,"),
        "inlineData image must be preserved, got: {}",
        user.content
    );

    let warning = session
        .messages
        .iter()
        .find(|message| message.content.starts_with("[warning]"))
        .expect("expected warning message");
    assert!(
        warning.content.contains("Tool output was truncated"),
        "warning content must be preserved"
    );
}

#[test]
fn gemini_jsonl_agent_tool_keeps_child_agent_id() {
    let provider = GeminiProvider::new().expect("home dir must be available");
    let path = gemini_jsonl_rich_fixture_path();
    let sessions = provider.parse_chat_file_for_test(&path, &HashMap::new());
    let session = sessions
        .first()
        .expect("Gemini rich JSONL fixture must parse one session");

    let tool = session
        .messages
        .iter()
        .find(|message| message.tool_name.as_deref() == Some("Agent"))
        .expect("expected Gemini invoke_agent tool");
    let metadata = tool
        .tool_metadata
        .as_ref()
        .expect("Agent tool metadata must exist");
    assert_eq!(metadata.result_kind.as_deref(), Some("agent_summary"));
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("agentId"))
            .and_then(|value| value.as_str()),
        Some("gemini-child-001")
    );
}

#[test]
fn gemini_jsonl_subagent_file_parses_as_sidechain() {
    let provider = GeminiProvider::new().expect("home dir must be available");
    let path = gemini_jsonl_subagent_fixture_path();
    let sessions = provider.parse_chat_file_for_test(&path, &HashMap::new());
    let session = sessions
        .first()
        .expect("Gemini subagent JSONL fixture must parse one session");

    assert_eq!(session.meta.id, "gemini-child-001");
    assert!(session.meta.is_sidechain);
    assert_eq!(session.meta.parent_id.as_deref(), Some("gemini-parent-001"));
    assert!(
        session.meta.title.contains("Child summary"),
        "subagent title should use summary when no user prompt exists, got: {}",
        session.meta.title
    );
    assert!(
        session
            .messages
            .iter()
            .any(|message| message.tool_name.as_deref() == Some("Read")),
        "subagent tool messages must be parsed"
    );
}

// ---------------------------------------------------------------------------
// OpenCode parser tests
// ---------------------------------------------------------------------------

/// Create a temporary SQLite database matching the OpenCode schema.
/// Returns the `TempDir` (must be kept alive for the test) and the DB path.
fn create_opencode_test_db() -> (tempfile::TempDir, PathBuf) {
    use rusqlite::{params, Connection};

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
    use rusqlite::{params, Connection};

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
            r#"{"role":"assistant","time":{"created":1705321260000},"tokens":{"input":50,"output":100,"cache":{"read":10,"write":5}}}"#,
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
    assert_eq!(usage.output_tokens, 100);
    assert_eq!(usage.cache_read_input_tokens, 10);
    assert_eq!(usage.cache_creation_input_tokens, 5);
}

// ---------------------------------------------------------------------------
// Qwen parser tests
// ---------------------------------------------------------------------------

fn qwen_fixture() -> cc_session_lib::provider::ParsedSession {
    let path = fixtures_dir().join("qwen_session.jsonl");
    qwen_parser::parse_session_file(&path).expect("qwen fixture must parse")
}

#[test]
fn qwen_parses_message_count() {
    let session = qwen_fixture();
    // 2 user + 3 assistant text + 1 thinking + 2 tool calls + 1 user-with-image = 9
    // System records (slash_command, ui_telemetry) skipped; empty thought text skipped
    assert_eq!(session.meta.message_count, 9);
}

#[test]
fn qwen_session_metadata() {
    let session = qwen_fixture();
    assert_eq!(session.meta.id, "qwen_session");
    assert_eq!(session.meta.provider, Provider::Qwen);
    assert_eq!(session.meta.project_path, "/Users/test/myproject");
    assert_eq!(session.meta.project_name, "myproject");
    assert_eq!(session.meta.cc_version.as_deref(), Some("0.14.0"));
    assert_eq!(session.meta.git_branch.as_deref(), Some("main"));
    assert_eq!(session.meta.model.as_deref(), Some("qwen-coder"));
    assert!(!session.meta.is_sidechain);
}

#[test]
fn qwen_title_from_first_user_message() {
    let session = qwen_fixture();
    assert_eq!(session.meta.title, "Search for TODO comments");
}

#[test]
fn qwen_thinking_emitted_as_system_role() {
    let session = qwen_fixture();
    let thinking_msgs: Vec<_> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::System && m.content.starts_with("[thinking]"))
        .collect();
    assert!(
        !thinking_msgs.is_empty(),
        "expected at least one thinking message"
    );
    assert!(thinking_msgs[0].content.contains("Let me search"));
}

#[test]
fn qwen_tool_call_mapped_to_canonical_name() {
    let session = qwen_fixture();
    let tool_names: Vec<_> = session
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .filter_map(|m| m.tool_name.as_deref())
        .collect();
    assert!(
        tool_names.contains(&"Grep"),
        "expected Grep tool, got {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"Edit"),
        "expected Edit tool, got {:?}",
        tool_names
    );
}

#[test]
fn qwen_tool_result_merged_by_call_id() {
    let session = qwen_fixture();
    let grep_msg = session
        .messages
        .iter()
        .find(|m| m.tool_name.as_deref() == Some("Grep"))
        .expect("Grep tool message must exist");
    assert!(
        grep_msg.content.contains("TODO"),
        "tool result should be merged into tool call message"
    );
}

#[test]
fn qwen_tool_call_has_structured_tool_metadata() {
    let session = qwen_fixture();
    let grep_msg = session
        .messages
        .iter()
        .find(|m| m.tool_name.as_deref() == Some("Grep"))
        .expect("Grep tool message must exist");
    let metadata = grep_msg
        .tool_metadata
        .as_ref()
        .expect("Qwen tool metadata must be present");

    assert_eq!(metadata.raw_name, "grep_search");
    assert_eq!(metadata.canonical_name, "Grep");
    assert_eq!(metadata.category, "search");
    assert_eq!(metadata.summary.as_deref(), Some("/TODO/ src/"));
    assert_eq!(metadata.status.as_deref(), Some("success"));
}

#[test]
fn qwen_tool_call_without_args_omits_null_tool_input() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("qwen-no-args.jsonl");
    fs::write(
        &file,
        concat!(
            r#"{"uuid":"uuid-001","parentUuid":null,"sessionId":"test-qwen-no-args","timestamp":"2026-04-03T10:00:00.000Z","type":"user","message":{"role":"user","parts":[{"text":"List files"}]},"cwd":"/tmp/project","version":"0.14.0"}"#,
            "\n",
            r#"{"uuid":"uuid-002","parentUuid":"uuid-001","sessionId":"test-qwen-no-args","timestamp":"2026-04-03T10:00:01.000Z","type":"assistant","model":"qwen-coder","message":{"role":"model","parts":[{"functionCall":{"id":"call_no_args","name":"list_directory"}}]},"cwd":"/tmp/project","version":"0.14.0"}"#,
            "\n"
        ),
    )
    .unwrap();

    let session = qwen_parser::parse_session_file(&file).expect("qwen no-args fixture must parse");
    let tool_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("expected a Tool message");

    assert_eq!(tool_msg.tool_name.as_deref(), Some("Glob"));
    assert!(
        tool_msg.tool_input.is_none(),
        "missing Qwen functionCall args must not display as JSON null"
    );
    assert!(
        tool_msg
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.summary.as_deref())
            .is_none(),
        "missing Qwen functionCall args must not produce a bogus metadata summary"
    );
}

#[test]
fn qwen_image_marker_in_user_message() {
    let session = qwen_fixture();
    let img_msg = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::User && m.content.contains("[Image:"))
        .expect("expected user message with image marker");
    assert!(img_msg.content.contains("data:image/png;base64,"));
}

#[test]
fn qwen_token_usage_on_assistant() {
    let session = qwen_fixture();
    let assistant_with_usage = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Assistant && m.token_usage.is_some())
        .expect("expected assistant message with token usage");
    let usage = assistant_with_usage.token_usage.as_ref().unwrap();
    assert!(usage.input_tokens > 0);
    assert!(usage.output_tokens > 0);
}

#[test]
fn qwen_system_records_skipped() {
    let session = qwen_fixture();
    // No system message should contain slash_command or ui_telemetry content
    for msg in &session.messages {
        if msg.role == MessageRole::System {
            assert!(
                msg.content.starts_with("[thinking]"),
                "system message should only be thinking, got: {}",
                &msg.content[..msg.content.len().min(50)]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Qwen real data smoke test (ignored — requires local Qwen data)
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn qwen_real_data_smoke_test() {
    let home = dirs::home_dir().expect("home dir");
    let projects_dir = home.join(".qwen/projects");
    if !projects_dir.exists() {
        eprintln!("Skipping: no qwen data at {}", projects_dir.display());
        return;
    }
    let mut parsed = 0;
    // Walk all project directories to find chats
    for project_entry in std::fs::read_dir(&projects_dir).unwrap().flatten() {
        let chats_dir = project_entry.path().join("chats");
        if !chats_dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&chats_dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                let Some(s) = qwen_parser::parse_session_file(&path) else {
                    eprintln!(
                        "  skipping {} — no user/assistant/tool messages",
                        path.file_name().unwrap().to_string_lossy()
                    );
                    continue;
                };
                assert!(s.meta.message_count > 0, "empty session {}", path.display());
                assert!(!s.meta.title.is_empty(), "no title for {}", path.display());
                assert_eq!(s.meta.provider, Provider::Qwen);
                eprintln!(
                    "  {} — {} msgs, model={:?}, title='{}'",
                    path.file_name().unwrap().to_string_lossy(),
                    s.meta.message_count,
                    s.meta.model,
                    s.meta.title
                );
                parsed += 1;
            }
        }
    }
    assert!(parsed > 0, "no qwen session files found");
    eprintln!("Parsed {} real Qwen session files successfully", parsed);
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
) -> &'a cc_session_lib::models::ToolMetadata {
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
        .parse_session_file(&required_env_path("CCSESSION_REAL_CODEX_JSONL"))
        .expect("real Codex transcript must parse");
    assert_real_tool_metadata("Codex", &codex.messages, "Bash", true);

    let gemini_provider = GeminiProvider::new().expect("home dir must be available");
    let gemini_sessions = gemini_provider.parse_chat_file_for_test(
        &required_env_path("CCSESSION_REAL_GEMINI_JSON"),
        &HashMap::new(),
    );
    let gemini = gemini_sessions
        .first()
        .expect("real Gemini transcript must parse");
    assert_real_tool_metadata("Gemini", &gemini.messages, "Read", true);

    let kimi_provider = KimiProvider::new().expect("home dir must be available");
    let kimi = kimi_provider
        .parse_session_file(
            &required_env_path("CCSESSION_REAL_KIMI_WIRE"),
            &HashMap::new(),
        )
        .expect("real Kimi transcript must parse");
    assert_real_tool_metadata("Kimi", &kimi.messages, "Read", true);

    let qwen = qwen_parser::parse_session_file(&required_env_path("CCSESSION_REAL_QWEN_JSONL"))
        .expect("real Qwen transcript must parse");
    assert_real_tool_metadata("Qwen", &qwen.messages, "Read", true);

    let opencode_db = std::env::var_os("CCSESSION_REAL_OPENCODE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("home dir must be available")
                .join(".local/share/opencode/opencode.db")
        });
    let opencode_session =
        std::env::var("CCSESSION_REAL_OPENCODE_SESSION").expect("real OpenCode session id");
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
        .parse_session_file(&required_env_path("CCSESSION_ROUND2_CODEX_JSONL"))
        .expect("round2 Codex transcript must parse");
    assert_real_tool_metadata("Codex round2", &codex.messages, "Bash", true);

    let gemini_provider = GeminiProvider::new().expect("home dir must be available");
    let gemini_sessions = gemini_provider.parse_chat_file_for_test(
        &required_env_path("CCSESSION_ROUND2_GEMINI_JSON"),
        &HashMap::new(),
    );
    let gemini = gemini_sessions
        .first()
        .expect("round2 Gemini transcript must parse");
    assert_has_tool_metadata("Gemini round2", &gemini.messages, "Glob");
    assert_has_tool_metadata("Gemini round2", &gemini.messages, "Read");
    assert_has_tool_metadata("Gemini round2", &gemini.messages, "Grep");
    assert_eq!(
        assert_has_tool_metadata("Gemini round2", &gemini.messages, "Edit")
            .result_kind
            .as_deref(),
        Some("file_patch")
    );

    let qwen = qwen_parser::parse_session_file(&required_env_path("CCSESSION_ROUND2_QWEN_JSONL"))
        .expect("round2 Qwen transcript must parse");
    assert_has_tool_metadata("Qwen round2", &qwen.messages, "Read");
    assert_has_tool_metadata("Qwen round2", &qwen.messages, "Grep");
    assert_eq!(
        assert_has_tool_metadata("Qwen round2", &qwen.messages, "Edit")
            .result_kind
            .as_deref(),
        Some("file_patch")
    );

    let kimi_provider = KimiProvider::new().expect("home dir must be available");
    let kimi = kimi_provider
        .parse_session_file(
            &required_env_path("CCSESSION_ROUND2_KIMI_WIRE"),
            &HashMap::new(),
        )
        .expect("round2 Kimi transcript must parse");
    assert_has_tool_metadata("Kimi round2", &kimi.messages, "Read");
    assert_has_tool_metadata("Kimi round2", &kimi.messages, "Bash");
    assert_eq!(
        assert_has_tool_metadata("Kimi round2", &kimi.messages, "Edit")
            .result_kind
            .as_deref(),
        Some("file_patch")
    );

    let opencode_db = std::env::var_os("CCSESSION_ROUND2_OPENCODE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("home dir must be available")
                .join(".local/share/opencode/opencode.db")
        });
    let opencode_session =
        std::env::var("CCSESSION_ROUND2_OPENCODE_SESSION").expect("round2 OpenCode session id");
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
