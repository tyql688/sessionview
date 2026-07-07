use super::*;
use serde_json::json;

const PARENT_A: &str = "11111111-1111-4111-a111-111111111111";
const CHILD_A: &str = "22222222-2222-4222-a222-222222222222";
const CHILD_B: &str = "33333333-3333-4333-a333-333333333333";

#[test]
fn extract_uploaded_image_paths_returns_empty_when_no_metadata_block() {
    assert!(extract_uploaded_image_paths("<USER_REQUEST>just text</USER_REQUEST>").is_empty());
}

#[test]
fn extract_uploaded_image_paths_returns_empty_when_metadata_has_no_uploads() {
    let content = r#"<USER_REQUEST>hi</USER_REQUEST>
<ADDITIONAL_METADATA>
The current local time is: 2026-05-20T12:00:00+08:00.
</ADDITIONAL_METADATA>"#;
    assert!(extract_uploaded_image_paths(content).is_empty());
}

#[test]
fn extract_uploaded_image_paths_parses_single_upload() {
    let content = r#"<USER_REQUEST>这些都是什么</USER_REQUEST>
<ADDITIONAL_METADATA>
The current local time is: 2026-05-20T12:00:00+08:00.

The user has uploaded 1 image(s):
- /tmp/brain/conv-1/uploaded_media_111.png
You can embed this image in an artifact if you need the USER to review it.
</ADDITIONAL_METADATA>"#;
    let paths = extract_uploaded_image_paths(content);
    assert_eq!(paths, vec!["/tmp/brain/conv-1/uploaded_media_111.png"]);
}

#[test]
fn extract_uploaded_image_paths_parses_multiple_uploads_and_stops_at_prose() {
    let content = r#"<ADDITIONAL_METADATA>
The user has uploaded 2 image(s):
- /tmp/a.png
- /tmp/b.jpg
You can embed these images in an artifact.
</ADDITIONAL_METADATA>"#;
    let paths = extract_uploaded_image_paths(content);
    assert_eq!(paths, vec!["/tmp/a.png", "/tmp/b.jpg"]);
}

#[test]
fn extract_uploaded_image_paths_handles_missing_closing_tag() {
    // Truncated transcript — the open tag exists but the closing one
    // doesn't. Should not panic; should still collect upload lines.
    let content = "<ADDITIONAL_METADATA>\nThe user has uploaded 1 image(s):\n- /tmp/x.png";
    assert_eq!(
        extract_uploaded_image_paths(content),
        vec!["/tmp/x.png".to_string()]
    );
}

#[test]
fn parse_invoke_subagent_extracts_all_conversation_ids_and_workspace() {
    let content = format!(
        r#"Created the following subagents:
{{
  "conversationId": "{CHILD_A}",
  "logAbsoluteUri": "file:///root/.gemini/antigravity-cli/brain/{CHILD_A}/.system_generated/logs/transcript.jsonl",
  "workspaceUris": [
"file:///tmp/projects/example"
  ]
}}
{{
  "conversationId": "{CHILD_B}",
  "workspaceUris": [
"file:///tmp/projects/example"
  ]
}}"#
    );
    let info = parse_invoke_subagent_content(&content);
    assert_eq!(
        info.conversation_ids,
        vec![CHILD_A.to_string(), CHILD_B.to_string()]
    );
    assert_eq!(info.workspace.as_deref(), Some("/tmp/projects/example"));
}

#[test]
fn parse_invoke_subagent_dedupes_repeats() {
    let content = r#"{"conversationId": "a"} {"conversationId": "a"} {"conversationId": "b"}"#;
    let info = parse_invoke_subagent_content(content);
    assert_eq!(
        info.conversation_ids,
        vec!["a".to_string(), "b".to_string()]
    );
}

#[test]
fn parse_invoke_subagent_ignores_prose_outside_json_blocks() {
    // The old string-scan implementation would have picked up the
    // "conversationId" that appears inside the prose-only error sentence.
    let content = format!(
        r#"Failed: conversationId is required but was not supplied.
But this real block IS valid:
{{ "conversationId": "{CHILD_A}", "workspaceUris": ["file:///tmp/ok"] }}"#
    );
    let info = parse_invoke_subagent_content(&content);
    assert_eq!(info.conversation_ids, vec![CHILD_A.to_string()]);
}

#[test]
fn parse_invoke_subagent_tolerates_braces_inside_string_values() {
    let content = r#"{ "conversationId": "abc", "note": "value with { brace } inside" }"#;
    let info = parse_invoke_subagent_content(content);
    assert_eq!(info.conversation_ids, vec!["abc".to_string()]);
}

#[test]
fn parse_invoke_subagent_skips_unterminated_block() {
    // Truncated transcript: opening `{` never closes — should not panic
    // and should not extract a partial UUID.
    let content = r#"{ "conversationId": "abc", "next": "still going..."#;
    let info = parse_invoke_subagent_content(content);
    assert!(info.conversation_ids.is_empty());
}

#[test]
fn parse_manage_subagents_extracts_active_child_id_and_prompt() {
    let content = format!(
        r#"Created At: 2026-06-10T08:42:58Z
Completed At: 2026-06-10T08:42:58Z
You have 1 active subagent(s):
{{
  "spec": {{
"typeName": "agy_tool_analyzer",
"role": "Agy Tool Analyzer",
"initialPrompt": "Inspect the provider using only view_file and list_dir",
"inherit": true
  }},
  "result": {{
"conversationId": "{CHILD_A}",
"logAbsoluteUri": "file:///root/.gemini/antigravity-cli/brain/{CHILD_A}/.system_generated/logs/transcript.jsonl"
  }}
}}"#
    );

    let info = parse_manage_subagents_content(&content);

    assert_eq!(info.conversation_ids, vec![CHILD_A.to_string()]);
    assert_eq!(
        info.prompts,
        vec!["Inspect the provider using only view_file and list_dir".to_string()]
    );
}

#[test]
fn manage_subagents_result_does_not_duplicate_known_invoke_child() {
    let mut accum = AntigravityScanAccum::new();
    accum.child_session_ids.push(CHILD_A.to_string());
    accum.messages.push(Message {
        tool_name: Some("Agent".to_string()),
        tool_metadata: Some(build_tool_metadata(ToolCallFacts {
            provider: Provider::Antigravity,
            raw_name: "manage_subagents",
            input: Some(&json!({ "Action": "list" })),
            call_id: None,
            assistant_id: None,
        })),
        tool_input: Some(json!({ "Action": "list" }).to_string()),
        ..Message::new(MessageRole::Tool, String::new())
    });
    accum.pending_tool_indices.push_back(0);
    let step = Step {
        step_index: 10,
        source: "MODEL".to_string(),
        step_type: "GENERIC".to_string(),
        status: "DONE".to_string(),
        created_at: "2026-06-10T08:42:58Z".to_string(),
        content: Some(format!(
            r#"{{
  "spec": {{
"initialPrompt": "Known child"
  }},
  "result": {{
"conversationId": "{CHILD_A}"
  }}
}}"#
        )),
        thinking: None,
        tool_calls: None,
    };

    accum.enrich_pending_tool(&step, PARENT_A, None);

    let structured = accum.messages[0]
        .tool_metadata
        .as_ref()
        .and_then(|metadata| metadata.structured.as_ref());
    assert!(
        structured.is_none(),
        "known manage_subagents child should not create duplicate Open metadata"
    );
}

#[test]
fn recipient_strips_doubly_quoted_value() {
    // Antigravity send_message wraps the parent uuid in literal "" inside
    // the JSON string value: `"Recipient":"\"<uuid>\""`.
    let tc = ToolCall {
        name: "send_message".into(),
        args: Some(json!({
            "Recipient": format!("\"{PARENT_A}\""),
            "Message": "ok",
        })),
    };
    assert_eq!(recipient_from_send_message(&tc).as_deref(), Some(PARENT_A));
}

#[test]
fn recipient_accepts_bare_value() {
    let tc = ToolCall {
        name: "send_message".into(),
        args: Some(json!({ "Recipient": "abc-123" })),
    };
    assert_eq!(recipient_from_send_message(&tc).as_deref(), Some("abc-123"));
}

#[test]
fn recipient_ignores_other_tools() {
    let tc = ToolCall {
        name: "run_shell_command".into(),
        args: Some(json!({ "Recipient": "abc" })),
    };
    assert_eq!(recipient_from_send_message(&tc), None);
}
