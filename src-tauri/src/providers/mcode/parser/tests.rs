use super::*;
use std::io::Write;

fn temp_messages(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("messages.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    (dir, path)
}

#[test]
fn parses_user_assistant_tool_assistant_full_wire() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"user","content":[{"type":"text","text":"list /tmp"}],"timestamp":1787049058794}}
{"message_id":"m2","turn_id":"t1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"run ls"},{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls -la /tmp"}}],"api":"anthropic-messages","provider":"minimax","model":"MiniMax-M3","usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"toolUse","timestamp":1787049058887}}
{"message_id":"m3","turn_id":"t1","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","content":[{"type":"text","text":"file1\nfile2"}],"isError":false,"timestamp":1787049060860}}
{"message_id":"m4","turn_id":"t1","message":{"role":"assistant","content":[{"type":"text","text":"here you go"}],"model":"MiniMax-M3","usage":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":3},"stopReason":"endTurn","timestamp":1787049060900}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    // Expected layout: user, [thinking], tool(call+result merged),
    // assistant — 4 message rows. The toolResult wire entry is folded
    // into the matching toolCall row rather than emitted separately.
    assert_eq!(parsed.messages.len(), 4);
    assert_eq!(parsed.parse_warning_count, 0);
    assert_eq!(parsed.first_assistant_model.as_deref(), Some("MiniMax-M3"));

    let thinking = &parsed.messages[1];
    assert_eq!(thinking.role, MessageRole::System);
    assert!(thinking.content.starts_with("[thinking]\n"));

    let tool_call = &parsed.messages[2];
    assert_eq!(tool_call.role, MessageRole::Tool);
    assert_eq!(tool_call.tool_name.as_deref(), Some("Bash"));
    assert!(tool_call.tool_input.as_deref().unwrap().contains("ls -la"));
    assert!(tool_call.tool_metadata.is_some());
    // The wire's toolResult row was folded into this row.
    assert_eq!(tool_call.content, "file1\nfile2");
    let metadata = tool_call.tool_metadata.as_ref().unwrap();
    assert_eq!(metadata.status.as_deref(), Some("success"));

    let final_assistant = &parsed.messages[3];
    assert_eq!(final_assistant.role, MessageRole::Assistant);
    assert_eq!(final_assistant.content, "here you go");
    assert_eq!(
        final_assistant.token_usage.as_ref().unwrap().input_tokens,
        1
    );
}

#[test]
fn normalizes_stringified_tool_arguments() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"read","arguments":"{\"path\":\"/etc/hosts\"}"}],"timestamp":1}}
{"message_id":"m2","turn_id":"t1","message":{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"127.0.0.1 localhost"}],"isError":false,"timestamp":2}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    // 1 tool call + result merged = 1 row.
    assert_eq!(parsed.messages.len(), 1);
    let tool = &parsed.messages[0];
    let input = tool.tool_input.as_deref().unwrap();
    assert!(
        input.contains("/etc/hosts"),
        "raw stringified args preserved: {input}"
    );
}

#[test]
fn skips_malformed_lines_and_keeps_the_rest() {
    let jsonl = "not json\n{\"message_id\":\"m1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1}}\n";
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.parse_warning_count, 1);
}

#[test]
fn deduplicates_repeated_message_id() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"text","text":"a"}],"timestamp":1}}
{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"text","text":"a-dup"}],"timestamp":2}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.messages.len(), 1, "duplicate message_id dropped");
    assert_eq!(parsed.messages[0].content, "a");
    assert_eq!(
        parsed.parse_warning_count, 0,
        "stream-reconciliation duplicates are expected, not warnings"
    );
}

#[test]
fn orphan_tool_result_becomes_standalone_tool_message() {
    // Result arrives without a matching call (e.g. session truncated).
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"toolResult","toolCallId":"unknown","toolName":"bash","content":[{"type":"text","text":"orphan"}],"isError":false,"timestamp":1}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].role, MessageRole::Tool);
    assert_eq!(parsed.messages[0].tool_name.as_deref(), Some("Bash"));
    assert_eq!(parsed.messages[0].content, "orphan");
    assert_eq!(
        parsed.messages[0]
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.status.as_deref()),
        Some("success")
    );
    assert_eq!(parsed.parse_warning_count, 1);
}

#[test]
fn emits_assistant_parts_in_wire_order() {
    // Wire order is thinking, then prose, then the tool call.
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"look first"},{"type":"text","text":"here is /tmp:"},{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls"}}],"model":"MiniMax-M3","usage":{"input":10,"output":5,"cacheRead":2,"cacheWrite":0},"timestamp":1000}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.messages.len(), 3);
    assert_eq!(parsed.messages[0].role, MessageRole::System);
    assert!(parsed.messages[0].content.starts_with("[thinking]\n"));
    assert_eq!(parsed.messages[1].role, MessageRole::Assistant);
    assert_eq!(parsed.messages[1].content, "here is /tmp:");
    assert_eq!(
        parsed.messages[1]
            .token_usage
            .as_ref()
            .unwrap()
            .input_tokens,
        10
    );
    assert_eq!(parsed.messages[2].role, MessageRole::Tool);
    assert_eq!(parsed.usage_events.len(), 1);
    assert_eq!(parsed.usage_events[0].model, "MiniMax-M3");
    assert_eq!(parsed.usage_events[0].input_tokens, 10);
    assert_eq!(parsed.usage_events[0].cache_read_input_tokens, 2);
    assert_eq!(parsed.usage_events[0].usage_hash.as_deref(), Some("m1"));
}

#[test]
fn uses_canonical_text_range_for_user_prompt() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>\nagent: Mavis\n</system-reminder>\n\nlist /tmp"}],"canonicalTextRange":{"startOffset":51,"endOffset":60},"timestamp":1}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].role, MessageRole::User);
    assert_eq!(parsed.messages[0].content, "list /tmp");
    assert!(
        !parsed.messages[0].content.contains("<system-reminder>"),
        "injected reminder must not ride along on the user bubble"
    );
}

#[test]
fn drops_reminder_only_user_turn() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>only</system-reminder>"}],"canonicalTextRange":{"startOffset":39,"endOffset":39},"timestamp":1}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert!(parsed.messages.is_empty());
}

#[test]
fn embeds_user_image_as_data_uri_marker() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>x</system-reminder>see this"},{"type":"image","data":"AAAA","mimeType":"image/jpeg"}],"canonicalTextRange":{"startOffset":36,"endOffset":44},"timestamp":1}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(
        parsed.messages[0].content,
        "see this\n[Image: source: data:image/jpeg;base64,AAAA]"
    );
    assert_eq!(parsed.parse_warning_count, 0);
}

#[test]
fn malformed_image_keeps_valid_sibling_text() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"user","content":[{"type":"text","text":"keep me"},{"type":"image","data":"AAAA"},{"type":"image","mimeType":"image/png"}],"timestamp":1}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].content, "keep me");
    assert_eq!(parsed.parse_warning_count, 2);
}

#[test]
fn unknown_content_warns_without_dropping_known_text() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"text","text":"keep me"},{"type":"futureBlock","value":"synthetic"}],"model":"MiniMax-M3","timestamp":1000}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].content, "keep me");
    assert_eq!(parsed.parse_warning_count, 1);
}

#[test]
fn unknown_tool_result_content_is_preserved_as_raw_output() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"toolResult","toolCallId":"missing","toolName":"read","content":[{"type":"futureResult","value":"synthetic"}],"isError":false,"timestamp":1}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].role, MessageRole::Tool);
    assert!(parsed.messages[0].content.contains("futureResult"));
    assert_eq!(
        parsed.parse_warning_count, 2,
        "unknown block plus orphan result"
    );
}

#[test]
fn usage_without_model_or_timestamp_preserves_totals_and_warns() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"text","text":"answer"}],"usage":{"input":7,"output":3,"cacheRead":2,"cacheWrite":1}}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.usage_events.len(), 1);
    assert_eq!(parsed.usage_events[0].input_tokens, 7);
    assert!(parsed.usage_events[0].model.is_empty());
    assert!(parsed.usage_events[0].timestamp.is_empty());
    assert_eq!(parsed.parse_warning_count, 2);
}

#[test]
fn slice_char_range_handles_multibyte() {
    let text = "你好世界";
    assert_eq!(slice_char_range(text, 0, 2), "你好");
    assert_eq!(slice_char_range(text, 2, 4), "世界");
    assert_eq!(slice_char_range(text, 4, 4), "");
    assert_eq!(slice_char_range(text, 3, 99), "界");
}

#[test]
fn task_result_exposes_child_session_id() {
    let jsonl = r#"{"message_id":"m1","turn_id":"t1","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"task","arguments":{"description":"Inspect workspace","prompt":"list files","agent_name":"explore"}}],"model":"MiniMax-M3","timestamp":1}}
{"message_id":"m2","turn_id":"t1","message":{"role":"toolResult","toolCallId":"c1","toolName":"task","content":[{"type":"text","text":"<task_result task_id=\"bg_1\" session_id=\"mvs_child1\">\nrun_status: succeeded\nfinal_text:\nok\n</task_result>"}],"isError":false,"details":{"agent_name":"explore","status":"succeeded","task_id":"bg_1","sub_session_id":"mvs_child1","resolved_agent_name":"explore"},"timestamp":2}}
"#;
    let (_dir, path) = temp_messages(jsonl);
    let parsed = parse_messages_file(&path).expect("parse ok");
    assert_eq!(parsed.child_session_ids, vec!["mvs_child1".to_string()]);
    assert_eq!(parsed.messages.len(), 1);
    let tool = &parsed.messages[0];
    assert_eq!(tool.tool_name.as_deref(), Some("Agent"));
    let structured = tool
        .tool_metadata
        .as_ref()
        .and_then(|m| m.structured.as_ref())
        .expect("structured");
    assert_eq!(
        structured.get("agentId").and_then(|v| v.as_str()),
        Some("mvs_child1")
    );
    assert_eq!(
        structured.get("sub_session_id").and_then(|v| v.as_str()),
        Some("mvs_child1")
    );
}
