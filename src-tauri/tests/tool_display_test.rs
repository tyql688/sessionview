//! Export rendering regression tests.

use sessionview_lib::models::{
    Message, MessageRole, Provider, SessionDetail, SessionMeta, TokenUsage,
};

fn test_session(messages: Vec<Message>) -> SessionDetail {
    SessionDetail {
        meta: SessionMeta {
            id: "export-test".to_string(),
            provider: Provider::Claude,
            title: "Export Test".to_string(),
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

fn assistant_message(content: &str) -> Message {
    Message {
        role: MessageRole::Assistant,
        message_kind: None,
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

#[test]
fn render_session_markdown_skips_usage_only_assistant_placeholders() {
    let detail = test_session(vec![
        assistant_message(""),
        assistant_message("Visible reply"),
    ]);

    let markdown = sessionview_lib::exporter_test_helpers::render_session_markdown_pub(&detail);

    assert_eq!(markdown.matches("### Assistant").count(), 1);
    assert!(markdown.contains("Visible reply"));
}
