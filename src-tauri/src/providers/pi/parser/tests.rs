use super::*;
use crate::models::{MessageRole, ToolResultMode};

#[test]
fn parse_session_header() {
    let json = r#"{"type":"session","version":3,"id":"test-uuid","timestamp":"2024-12-03T14:00:00.000Z","cwd":"/path/to/project"}"#;
    let entry: PiEntry = serde_json::from_str(json).unwrap();
    match entry {
        PiEntry::Session(header) => {
            assert_eq!(header.version, 3);
            assert_eq!(header.id, "test-uuid");
            assert_eq!(header.cwd, "/path/to/project");
        }
        _ => panic!("Expected session entry"),
    }
}

#[test]
fn parse_user_message() {
    let json = r#"{"type":"message","id":"a1b2c3d4","parentId":null,"timestamp":"2024-12-03T14:00:01.000Z","message":{"role":"user","content":"Hello","timestamp":1733236801000}}"#;
    let entry: PiEntry = serde_json::from_str(json).unwrap();
    match entry {
        PiEntry::Message(msg) => {
            assert_eq!(msg.base.id, "a1b2c3d4");
            match msg.message {
                PiAgentMessage::User(user) => match user.content {
                    PiContent::Text(text) => assert_eq!(text, "Hello"),
                    _ => panic!("Expected text content"),
                },
                _ => panic!("Expected user message"),
            }
        }
        _ => panic!("Expected message entry"),
    }
}

#[test]
fn parse_assistant_message_with_usage() {
    let json = r#"{"type":"message","id":"b2c3d4e5","parentId":"a1b2c3d4","timestamp":"2024-12-03T14:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Hi!"}],"provider":"anthropic","model":"claude-sonnet-4-5","usage":{"input":100,"output":50,"cacheRead":0,"cacheWrite":0,"totalTokens":150},"stopReason":"stop","timestamp":1733236802000}}"#;
    let entry: PiEntry = serde_json::from_str(json).unwrap();
    match entry {
        PiEntry::Message(msg) => match msg.message {
            PiAgentMessage::Assistant(assistant) => {
                assert_eq!(assistant.provider, Some("anthropic".to_string()));
                assert_eq!(assistant.model, Some("claude-sonnet-4-5".to_string()));
                let usage = assistant.usage.unwrap();
                assert_eq!(usage.input, 100);
                assert_eq!(usage.output, 50);
            }
            _ => panic!("Expected assistant message"),
        },
        _ => panic!("Expected message entry"),
    }
}

#[test]
fn extract_messages_splits_thinking_and_merges_multiple_tool_results() {
    let entries: Vec<PiEntry> = [
        r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:00:00.000Z","message":{"role":"user","content":"Inspect files","timestamp":1781074800000}}"#,
        r#"{"type":"message","id":"assistant-1","parentId":"user-1","timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Need to read files","thinkingSignature":"reasoning_content"},{"type":"text","text":"I will inspect these files."},{"type":"toolCall","id":"call-read","name":"read","arguments":{"path":"README.md"}},{"type":"toolCall","id":"call-bash","name":"bash","arguments":{"command":"pwd"}}],"provider":"pi-test","model":"mimo-test","usage":{"input":10,"output":5,"cacheRead":2,"cacheWrite":1,"totalTokens":18},"stopReason":"toolUse","timestamp":1781074801000}}"#,
        r#"{"type":"message","id":"result-1","parentId":"assistant-1","timestamp":"2026-06-10T07:00:02.000Z","message":{"role":"toolResult","toolCallId":"call-read","toolName":"read","content":[{"type":"text","text":"file body"}],"details":{},"isError":false,"timestamp":1781074802000}}"#,
        r#"{"type":"message","id":"result-2","parentId":"result-1","timestamp":"2026-06-10T07:00:03.000Z","message":{"role":"toolResult","toolCallId":"call-bash","toolName":"bash","content":[{"type":"text","text":"/tmp/project"}],"details":{"truncation":{"fullOutputPath":"/tmp/pi-output.log"}},"isError":false,"timestamp":1781074803000}}"#,
    ]
    .into_iter()
    .map(|json| serde_json::from_str(json).unwrap())
    .collect();

    let branch = build_active_branch(&entries);
    let messages = extract_messages(&entries, &branch);

    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[1].role, MessageRole::System);
    assert!(messages[1].content.starts_with("[thinking]\n"));
    assert_eq!(messages[2].role, MessageRole::Assistant);
    assert_eq!(messages[2].content, "I will inspect these files.");
    assert!(!messages[2].content.contains("[thinking]"));
    assert_eq!(messages[2].token_usage.as_ref().unwrap().input_tokens, 10);

    assert_eq!(messages[3].role, MessageRole::Tool);
    assert_eq!(messages[3].tool_name.as_deref(), Some("Read"));
    assert_eq!(messages[3].content, "file body");
    assert_eq!(
        messages[3]
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.ids.get("tool_use_id"))
            .map(String::as_str),
        Some("call-read")
    );
    assert_eq!(
        messages[3]
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.status.as_deref()),
        Some("success")
    );

    assert_eq!(messages[4].role, MessageRole::Tool);
    assert_eq!(messages[4].tool_name.as_deref(), Some("Bash"));
    assert_eq!(messages[4].content, "/tmp/project");
    assert_eq!(
        messages[4]
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.result_kind.as_deref()),
        Some("persisted_output")
    );
}

#[test]
fn extract_messages_keeps_tool_only_turn_as_tool_with_error_status() {
    let entries: Vec<PiEntry> = [
        r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:00:00.000Z","message":{"role":"user","content":"Edit file","timestamp":1781074800000}}"#,
        r#"{"type":"message","id":"assistant-1","parentId":"user-1","timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Need to edit"},{"type":"toolCall","id":"call-edit","name":"edit","arguments":{"path":"src/main.rs","oldText":"old","newText":"new"}}],"provider":"pi-test","model":"mimo-test","usage":{"input":4,"output":3,"cacheRead":0,"cacheWrite":0,"totalTokens":7},"stopReason":"toolUse","timestamp":1781074801000}}"#,
        r#"{"type":"message","id":"result-1","parentId":"assistant-1","timestamp":"2026-06-10T07:00:02.000Z","message":{"role":"toolResult","toolCallId":"call-edit","toolName":"edit","content":[{"type":"text","text":"replacement failed"}],"details":{"diff":"--- a\n+++ b"},"isError":true,"timestamp":1781074802000}}"#,
    ]
    .into_iter()
    .map(|json| serde_json::from_str(json).unwrap())
    .collect();

    let branch = build_active_branch(&entries);
    let messages = extract_messages(&entries, &branch);

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].role, MessageRole::System);
    assert_eq!(messages[2].role, MessageRole::Tool);
    assert_eq!(messages[2].tool_name.as_deref(), Some("Edit"));
    assert_eq!(messages[2].content, "replacement failed");
    assert!(messages[2].token_usage.is_some());
    assert_eq!(
        messages[2]
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.status.as_deref()),
        Some("error")
    );
}

#[test]
fn extract_messages_preserves_unknown_tool_result_blocks_as_raw() {
    let entries: Vec<PiEntry> = [
        r#"{"type":"message","id":"assistant-1","parentId":null,"timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-read","name":"read","arguments":{"path":"future.json"}}],"provider":"pi-test","model":"mimo-test","timestamp":1781074801000}}"#,
        r#"{"type":"message","id":"result-1","parentId":"assistant-1","timestamp":"2026-06-10T07:00:02.000Z","message":{"role":"toolResult","toolCallId":"call-read","toolName":"read","content":[{"type":"future_content","payload":{"keep":true}}],"details":{},"isError":false,"timestamp":1781074802000}}"#,
    ]
    .into_iter()
    .map(|json| serde_json::from_str(json).unwrap())
    .collect();

    let branch = build_active_branch(&entries);
    let messages = extract_messages(&entries, &branch);
    let tool = messages
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
    let raw: serde_json::Value = serde_json::from_str(&tool.content).expect("raw content array");
    assert_eq!(raw[0]["type"], "future_content");
    assert_eq!(raw[0]["payload"]["keep"], true);
}

#[test]
fn extract_messages_keeps_json_file_text_as_output() {
    let entries: Vec<PiEntry> = [
        r#"{"type":"message","id":"assistant-1","parentId":null,"timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-read","name":"read","arguments":{"path":"package.json"}}],"provider":"pi-test","model":"mimo-test","timestamp":1781074801000}}"#,
        r#"{"type":"message","id":"result-1","parentId":"assistant-1","timestamp":"2026-06-10T07:00:02.000Z","message":{"role":"toolResult","toolCallId":"call-read","toolName":"read","content":[{"type":"text","text":"{\"name\":\"sessionview\"}"}],"details":{},"isError":false,"timestamp":1781074802000}}"#,
    ]
    .into_iter()
    .map(|json| serde_json::from_str(json).unwrap())
    .collect();

    let branch = build_active_branch(&entries);
    let messages = extract_messages(&entries, &branch);
    let tool = messages
        .iter()
        .find(|message| message.role == MessageRole::Tool)
        .expect("tool message");
    assert_eq!(tool.content, r#"{"name":"sessionview"}"#);
    assert_eq!(
        tool.tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.presentation.as_ref())
            .map(|presentation| presentation.result_mode),
        Some(ToolResultMode::Output)
    );
}

#[test]
fn parse_session_file_stores_meta_timestamps_as_epoch_seconds() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"session","version":3,"id":"session-1","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"user","content":"Hello","timestamp":1781074801000}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&path).unwrap();

    assert_eq!(session.meta.created_at, 1_781_074_800);
    assert_eq!(session.meta.updated_at, 1_781_074_801);
    assert_eq!(
        session.messages[0].timestamp.as_deref(),
        Some("2026-06-10T07:00:01+00:00")
    );
}

#[test]
fn parse_session_file_counts_usage_outside_the_active_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"session","version":3,"id":"session-1","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"user","content":"Choose","timestamp":1781074801000}}"#,
            r#"{"type":"message","id":"discarded","parentId":"user-1","timestamp":"2026-06-10T07:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Discarded answer"}],"provider":"pi-test","model":"model-a","usage":{"input":10,"output":2,"cacheRead":3,"cacheWrite":1,"totalTokens":16},"stopReason":"stop","timestamp":1781074802000}}"#,
            r#"{"type":"message","id":"active","parentId":"user-1","timestamp":"2026-06-10T07:00:03.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Active answer"}],"provider":"pi-test","model":"model-a","usage":{"input":20,"output":4,"cacheRead":6,"cacheWrite":2,"totalTokens":32},"stopReason":"stop","timestamp":1781074803000}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&path).unwrap();
    let loaded = load_messages(&path).unwrap();

    assert_eq!(session.messages.len(), 2);
    assert_eq!(session.messages[1].content, "Active answer");
    assert_eq!(session.usage_events.len(), 2);
    assert_eq!(session.meta.input_tokens, 30);
    assert_eq!(session.meta.output_tokens, 6);
    assert_eq!(session.meta.cache_read_tokens, 9);
    assert_eq!(session.meta.cache_write_tokens, 3);
    assert_eq!(loaded.token_totals.input_tokens, 30);
    assert_eq!(loaded.messages[1].content, "Active answer");
}

#[test]
fn parse_session_file_skips_usage_without_model() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"session","version":3,"id":"session-1","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","id":"assistant-1","parentId":null,"timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Answer"}],"usage":{"input":10,"output":2,"cacheRead":3,"cacheWrite":1,"totalTokens":16},"stopReason":"stop","timestamp":1781074801000}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&path).unwrap();

    assert!(session.usage_events.is_empty());
    assert_eq!(session.parse_warning_count, 1);
    assert_eq!(session.meta.input_tokens, 0);
}

#[test]
fn parse_session_file_uses_pi_message_activity_for_updated_at() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"session","version":3,"id":"session-1","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:10:00.000Z","message":{"role":"user","content":"Hello","timestamp":1781074801000}}"#,
            r#"{"type":"message","id":"assistant-1","parentId":"user-1","timestamp":"2026-06-10T07:20:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Hi!"}],"provider":"pi-test","model":"mimo-test","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2},"stopReason":"stop","timestamp":1781074802000}}"#,
            r#"{"type":"message","id":"result-1","parentId":"assistant-1","timestamp":"2026-06-10T07:25:00.000Z","message":{"role":"toolResult","toolCallId":"call-read","toolName":"read","content":[{"type":"text","text":"file body"}],"details":{},"isError":false,"timestamp":1781076300000}}"#,
            r#"{"type":"model_change","id":"model-1","parentId":"result-1","timestamp":"2026-06-10T07:30:00.000Z","provider":"pi-test","modelId":"other-model"}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&path).unwrap();

    assert_eq!(session.meta.updated_at, 1_781_074_802);
}

#[test]
fn parse_session_file_resolves_parent_session_path_to_parent_id() {
    let tmp = tempfile::tempdir().unwrap();
    let parent_path = tmp.path().join("parent.jsonl");
    let child_path = tmp.path().join("child.jsonl");
    std::fs::write(
        &parent_path,
        [
            r#"{"type":"session","version":3,"id":"parent-session","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","id":"parent-user","parentId":null,"timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"user","content":"Parent","timestamp":1781074801000}}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    std::fs::write(
        &child_path,
        [
            format!(
                r#"{{"type":"session","version":3,"id":"child-session","timestamp":"2026-06-10T07:10:00.000Z","cwd":"/tmp/project","parentSession":{}}}"#,
                serde_json::to_string(parent_path.to_str().unwrap()).unwrap()
            ),
            r#"{"type":"message","id":"child-user","parentId":null,"timestamp":"2026-06-10T07:10:01.000Z","message":{"role":"user","content":"Child","timestamp":1781075401000}}"#.to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&child_path).unwrap();

    assert_eq!(session.meta.parent_id.as_deref(), Some("parent-session"));
    assert!(session.meta.is_sidechain);
}

#[test]
fn parse_session_file_treats_empty_session_info_as_cleared_title() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"session","version":3,"id":"session-1","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"user","content":"Fallback user title","timestamp":1781074801000}}"#,
            r#"{"type":"session_info","id":"info-1","parentId":"user-1","timestamp":"2026-06-10T07:00:02.000Z","name":"Custom title"}"#,
            r#"{"type":"session_info","id":"info-2","parentId":"info-1","timestamp":"2026-06-10T07:00:03.000Z","name":""}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&path).unwrap();

    assert_eq!(session.meta.title, "Fallback user title");
    assert_eq!(session.parse_warning_count, 0);
}

#[test]
fn parse_session_file_accepts_session_info_without_name() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"session","version":3,"id":"session-1","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"user","content":"Fallback user title","timestamp":1781074801000}}"#,
            r#"{"type":"session_info","id":"info-1","parentId":"user-1","timestamp":"2026-06-10T07:00:02.000Z"}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&path).unwrap();

    assert_eq!(session.meta.title, "Fallback user title");
    assert_eq!(session.parse_warning_count, 0);
}

#[test]
fn parse_session_file_accepts_label_without_label_text() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"session","version":3,"id":"session-1","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","id":"user-1","parentId":null,"timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"user","content":"Fallback user title","timestamp":1781074801000}}"#,
            r#"{"type":"label","id":"label-1","parentId":"user-1","timestamp":"2026-06-10T07:00:02.000Z","targetId":"user-1"}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&path).unwrap();

    assert_eq!(session.meta.title, "Fallback user title");
    assert_eq!(session.parse_warning_count, 0);
}

#[test]
fn parse_session_file_does_not_store_unresolved_parent_session_path() {
    let tmp = tempfile::tempdir().unwrap();
    let missing_parent_path = tmp.path().join("missing-parent.jsonl");
    let child_path = tmp.path().join("child.jsonl");
    std::fs::write(
        &child_path,
        [
            format!(
                r#"{{"type":"session","version":3,"id":"child-session","timestamp":"2026-06-10T07:10:00.000Z","cwd":"/tmp/project","parentSession":{}}}"#,
                serde_json::to_string(missing_parent_path.to_str().unwrap()).unwrap()
            ),
            r#"{"type":"message","id":"child-user","parentId":null,"timestamp":"2026-06-10T07:10:01.000Z","message":{"role":"user","content":"Child","timestamp":1781075401000}}"#.to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&child_path).unwrap();

    assert_eq!(session.meta.parent_id, None);
    assert!(!session.meta.is_sidechain);
}

#[test]
fn extract_messages_uses_pi_compaction_context_order() {
    let entries: Vec<PiEntry> = [
        r#"{"type":"message","id":"old-user","parentId":null,"timestamp":"2026-06-10T07:00:00.000Z","message":{"role":"user","content":"old prompt","timestamp":1781074800000}}"#,
        r#"{"type":"message","id":"old-assistant","parentId":"old-user","timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"old answer"}],"provider":"pi-test","model":"mimo-test","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2},"stopReason":"stop","timestamp":1781074801000}}"#,
        r#"{"type":"message","id":"kept-user","parentId":"old-assistant","timestamp":"2026-06-10T07:00:02.000Z","message":{"role":"user","content":"kept prompt","timestamp":1781074802000}}"#,
        r#"{"type":"compaction","id":"compact-1","parentId":"kept-user","timestamp":"2026-06-10T07:00:03.000Z","summary":"checkpoint","firstKeptEntryId":"kept-user","tokensBefore":100}"#,
        r#"{"type":"message","id":"after-user","parentId":"compact-1","timestamp":"2026-06-10T07:00:04.000Z","message":{"role":"user","content":"after compaction","timestamp":1781074804000}}"#,
    ]
    .into_iter()
    .map(|json| serde_json::from_str(json).unwrap())
    .collect();

    let active_branch = build_active_branch(&entries);
    let context_branch = build_context_branch(&entries, &active_branch, Path::new("session.jsonl"));
    let messages = extract_messages(&entries, &context_branch);

    assert_eq!(context_branch, ["compact-1", "kept-user", "after-user"]);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, MessageRole::System);
    assert_eq!(messages[0].content, "[Compaction] checkpoint");
    assert_eq!(messages[1].content, "kept prompt");
    assert_eq!(messages[2].content, "after compaction");
}

#[test]
fn parse_session_file_migrates_legacy_v1_linear_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("legacy.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"session","id":"legacy-session","timestamp":"2026-06-10T07:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","timestamp":"2026-06-10T07:00:01.000Z","message":{"role":"user","content":"old prompt","timestamp":1781074801000}}"#,
            r#"{"type":"compaction","timestamp":"2026-06-10T07:00:02.000Z","summary":"legacy checkpoint","firstKeptEntryIndex":1,"tokensBefore":12}"#,
            r#"{"type":"message","timestamp":"2026-06-10T07:00:03.000Z","message":{"role":"user","content":"after legacy compaction","timestamp":1781074803000}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let session = parse_session_file(&path).unwrap();

    assert_eq!(session.meta.id, "legacy-session");
    assert_eq!(session.messages.len(), 3);
    assert_eq!(
        session.messages[0].content,
        "[Compaction] legacy checkpoint"
    );
    assert_eq!(session.messages[1].content, "old prompt");
    assert_eq!(session.messages[2].content, "after legacy compaction");
}

#[test]
fn extract_project_name_test() {
    assert_eq!(extract_project_name("/path/to/project"), "project");
    assert_eq!(extract_project_name("/home/user/code"), "code");
    assert_eq!(extract_project_name("/"), "/");
}

#[test]
#[ignore = "requires local Pi session data"]
fn parse_real_local_session() {
    // Use real local Pi session data if available
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return,
    };
    let sessions_dir = home.join(".pi").join("agent").join("sessions");
    if !sessions_dir.exists() {
        return;
    }

    // Find first JSONL file
    let mut session_file = None;
    for entry in std::fs::read_dir(&sessions_dir).into_iter().flatten() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(&path).into_iter().flatten() {
            let file = match file {
                Ok(f) => f,
                Err(_) => continue,
            };
            let file_path = file.path();
            if file_path.extension().is_some_and(|ext| ext == "jsonl") {
                session_file = Some(file_path);
                break;
            }
        }
        if session_file.is_some() {
            break;
        }
    }

    let file_path = match session_file {
        Some(f) => f,
        None => return,
    };

    // Parse the session
    let result = parse_session_file(&file_path);
    assert!(
        result.is_some(),
        "Failed to parse real Pi session: {}",
        file_path.display()
    );

    let session = result.unwrap();

    // Verify basic structure
    assert_eq!(session.meta.provider, Provider::Pi);
    assert!(
        !session.meta.id.is_empty(),
        "Session ID should not be empty"
    );
    assert!(
        !session.meta.title.is_empty(),
        "Session title should not be empty"
    );
    assert!(
        !session.meta.project_path.is_empty(),
        "Project path should not be empty"
    );
    assert!(
        !session.meta.project_name.is_empty(),
        "Project name should not be empty"
    );
    assert!(
        session.meta.created_at > 0,
        "Created timestamp should be positive"
    );
    assert!(
        session.meta.updated_at > 0,
        "Updated timestamp should be positive"
    );
    assert!(
        session.meta.message_count > 0,
        "Message count should be positive"
    );
    assert!(
        session.meta.file_size_bytes > 0,
        "File size should be positive"
    );

    // Verify messages
    assert!(!session.messages.is_empty(), "Messages should not be empty");
    for msg in &session.messages {
        assert!(
            !msg.content.is_empty() || msg.tool_name.is_some(),
            "Message should have content or tool name"
        );
    }

    // Verify source path
    assert_eq!(session.meta.source_path, file_path.to_string_lossy());

    println!(
        "Parsed Pi session: id={}, title={}, messages={}, tokens={}",
        session.meta.id,
        session.meta.title,
        session.meta.message_count,
        session.meta.input_tokens + session.meta.output_tokens
    );
}
