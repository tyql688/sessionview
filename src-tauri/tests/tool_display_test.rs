//! Golden tests for tool display rendering in the HTML exporter.
//! Fixtures shared with frontend vitest tests.

use serde::Deserialize;
use serde_json::{json, Value};

use cc_session_lib::models::{
    Message, MessageRole, Provider, SessionDetail, SessionMeta, TokenUsage, ToolMetadata,
};
use cc_session_lib::tool_metadata::{
    build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

#[derive(Deserialize)]
struct GoldenCase {
    tool_name: String,
    tool_input: String,
    expected_keywords: Vec<String>,
}

#[test]
fn test_render_tool_detail_golden() {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tool_display/golden.json");
    let data = std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("failed to read fixture: {e}"));
    let cases: Vec<GoldenCase> =
        serde_json::from_str(&data).unwrap_or_else(|e| panic!("failed to parse fixture: {e}"));

    for case in &cases {
        let html = cc_session_lib::exporter_test_helpers::render_tool_detail_pub(
            &case.tool_name,
            &case.tool_input,
        );
        for keyword in &case.expected_keywords {
            assert!(
                html.contains(keyword) || html.contains(&html_escape(keyword)),
                "tool={}: expected keyword '{}' not found in output:\n{}",
                case.tool_name,
                keyword,
                html
            );
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn test_session(messages: Vec<Message>) -> SessionDetail {
    SessionDetail {
        meta: SessionMeta {
            id: "tool-html-test".to_string(),
            provider: Provider::Claude,
            title: "Tool HTML Test".to_string(),
            project_path: "/tmp/project".to_string(),
            project_name: "project".to_string(),
            created_at: 1_766_000_000,
            updated_at: 1_766_000_000,
            message_count: messages.len() as u32,
            file_size_bytes: 1,
            source_path: "/tmp/session.jsonl".to_string(),
            is_sidechain: false,
            variant_name: None,
            model: None,
            cc_version: None,
            git_branch: None,
            parent_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
        messages,
        parse_warning_count: 0,
    }
}

fn tool_message(name: &str, input: Option<String>, metadata: Option<ToolMetadata>) -> Message {
    tool_message_with_content(
        name,
        "raw output that should be hidden for structured diffs",
        input,
        metadata,
    )
}

fn tool_message_with_content(
    name: &str,
    content: &str,
    input: Option<String>,
    metadata: Option<ToolMetadata>,
) -> Message {
    Message {
        role: MessageRole::Tool,
        content: content.to_string(),
        timestamp: None,
        tool_name: Some(name.to_string()),
        tool_input: input,
        tool_metadata: metadata,
        token_usage: None,
        model: None,
        usage_hash: None,
    }
}

fn assistant_message(content: &str) -> Message {
    Message {
        role: MessageRole::Assistant,
        content: content.to_string(),
        timestamp: Some("2026-04-11T02:25:16.628Z".to_string()),
        tool_name: None,
        tool_input: None,
        tool_metadata: None,
        token_usage: Some(TokenUsage {
            input_tokens: 1,
            output_tokens: 2,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 4,
        }),
        model: Some("claude-opus-4-6".to_string()),
        usage_hash: Some("msg:req".to_string()),
    }
}

fn metadata(raw_name: &str, input: Option<Value>, result: Value) -> ToolMetadata {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name,
        input: input.as_ref(),
        call_id: None,
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&result),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );
    metadata
}

#[test]
fn test_render_session_html_uses_tool_metadata() {
    let detail = test_session(vec![
        tool_message(
            "Edit",
            Some(
                json!({
                    "file_path": "/tmp/project/src/app.py",
                    "old_string": "old",
                    "new_string": "new"
                })
                .to_string(),
            ),
            Some(metadata(
                "Edit",
                Some(json!({
                    "file_path": "/tmp/project/src/app.py",
                    "old_string": "old",
                    "new_string": "new"
                })),
                json!({
                    "filePath": "/tmp/project/src/app.py",
                    "oldString": "old",
                    "newString": "new"
                }),
            )),
        ),
        tool_message(
            "mcp__server__browser_snapshot",
            Some(json!({}).to_string()),
            Some(metadata(
                "mcp__server__browser_snapshot",
                Some(json!({})),
                json!({
                    "result": {
                        "Ok": {
                            "content": [{ "type": "text", "text": "snapshot" }]
                        }
                    }
                }),
            )),
        ),
        tool_message(
            "Edit",
            None,
            Some(metadata(
                "Edit",
                None,
                json!({
                    "filePath": "/tmp/project/src/patch.rs",
                    "structuredPatch": [{
                        "oldStart": 7,
                        "oldLines": 2,
                        "newStart": 7,
                        "newLines": 2,
                        "lines": [" context", "-old", "+new"]
                    }]
                }),
            )),
        ),
        tool_message_with_content(
            "TaskUpdate",
            "task status raw output",
            None,
            Some(metadata(
                "TaskUpdate",
                None,
                json!({
                    "taskId": "11",
                    "statusChange": {
                        "from": "in_progress",
                        "to": "completed"
                    }
                }),
            )),
        ),
    ]);

    let html = cc_session_lib::exporter_test_helpers::render_session_html_pub(&detail);
    assert!(html.contains("tool-line-diff"));
    assert!(html.contains("tool-diff-line remove"));
    assert!(html.contains("tool-diff-line add"));
    assert!(html.contains("@@ -7,2 +7,2 @@"));
    assert!(html.contains("browser snapshot"));
    assert!(html.contains("server"));
    assert!(html.contains("in_progress → completed"));
    assert_eq!(
        html.matches("raw output that should be hidden").count(),
        1,
        "structured file_patch output should appear only for the MCP sample, not the Edit diff"
    );
}

#[test]
fn test_render_session_html_skips_usage_only_assistant_placeholders() {
    let detail = test_session(vec![
        assistant_message(""),
        assistant_message("Visible reply"),
    ]);

    let html = cc_session_lib::exporter_test_helpers::render_session_html_pub(&detail);

    assert_eq!(html.matches("msg-assistant").count(), 1);
    assert!(html.contains("Visible reply"));
    assert!(!html.contains(r#"<div class="msg-body"></div>"#));
}

#[test]
fn test_render_session_markdown_skips_usage_only_assistant_placeholders() {
    let detail = test_session(vec![
        assistant_message(""),
        assistant_message("Visible reply"),
    ]);

    let markdown = cc_session_lib::exporter_test_helpers::render_session_markdown_pub(&detail);

    assert_eq!(markdown.matches("### Assistant").count(), 1);
    assert!(markdown.contains("Visible reply"));
}

#[test]
fn test_render_tool_detail_shortens_home_paths_in_patch_headers() {
    let input = json!({
        "patch": "*** Begin Patch\n*** Update File: /Users/alice/project/src/app.ts\n@@\n-old\n+new\n*** End Patch\n"
    })
    .to_string();
    let html = cc_session_lib::exporter_test_helpers::render_tool_detail_pub("Edit", &input);

    assert!(html.contains("*** Update File: ~/project/src/app.ts"));
    assert!(!html.contains("/Users/alice"));
}
