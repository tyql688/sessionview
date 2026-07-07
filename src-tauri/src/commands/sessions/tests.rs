use super::{
    build_session_messages_window, build_session_turn_outline, canceled_error,
    session_window_bounds, subagent_meta_title, CANCEL_ERROR,
};
use crate::error::CommandError;
use crate::models::{Message, MessageRole, TokenTotals};

/// The cancel sentinel must reach the command boundary unchanged so
/// the frontend's `isLoadCanceledError` (`msg.includes(...)`) keeps
/// suppressing the toast on tab-switch races. This locks the exact
/// serialized text the frontend matches against.
#[test]
fn canceled_error_serializes_with_cancel_sentinel() {
    let command: CommandError = canceled_error().into();
    let serialized = format!("{:#}", command.0);
    assert_eq!(serialized, CANCEL_ERROR);
    assert!(serialized.contains("__cc_session_load_canceled__"));
}

#[test]
fn session_window_bounds_negative_offset_uses_tail_window() {
    assert_eq!(session_window_bounds(1_000, -300, 300), (700, 1_000));
    assert_eq!(session_window_bounds(1_000, -1, 200), (800, 1_000));
}

#[test]
fn session_window_bounds_clamps_to_total() {
    assert_eq!(session_window_bounds(20, 10, 100), (10, 20));
    assert_eq!(session_window_bounds(20, 30, 100), (20, 20));
    assert_eq!(session_window_bounds(0, -300, 300), (0, 0));
}

#[test]
fn build_session_messages_window_preserves_full_total() {
    let messages: Vec<Message> = (0..5)
        .map(|idx| Message::assistant(format!("message {idx}")))
        .collect();

    let window = build_session_messages_window(&messages, 2, TokenTotals::default(), -2, 2);

    assert_eq!(window.total, 5);
    assert_eq!(window.start, 3);
    assert_eq!(window.messages.len(), 2);
    assert_eq!(window.messages[0].content, "message 3");
    assert_eq!(window.parse_warning_count, 2);
}

#[test]
fn build_session_turn_outline_pairs_user_with_first_assistant_reply() {
    let messages = vec![
        Message::new(MessageRole::System, "ignored"),
        Message::new(MessageRole::User, " first   question "),
        Message::new(MessageRole::Assistant, " first reply "),
        Message::new(MessageRole::Assistant, "ignored follow-up"),
        Message::new(MessageRole::Tool, "ignored tool"),
        Message::new(MessageRole::User, "second question"),
    ];

    let outline = build_session_turn_outline(&messages);

    assert_eq!(outline.len(), 2);
    assert_eq!(outline[0].ordinal, 0);
    assert_eq!(outline[0].message_index, 1);
    assert_eq!(outline[0].user_text, "first question");
    assert_eq!(outline[0].reply_text, "first reply");
    assert_eq!(outline[1].ordinal, 1);
    assert_eq!(outline[1].message_index, 5);
    assert_eq!(outline[1].user_text, "second question");
    assert!(outline[1].reply_text.is_empty());
}

#[test]
fn subagent_meta_title_reads_agent_type_when_description_is_absent() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("agent-a1111111111111111.jsonl");
    std::fs::write(
        source.with_extension("meta.json"),
        r#"{"agentType":"ws_nte2_v2"}"#,
    )
    .unwrap();

    assert_eq!(
        subagent_meta_title(source.to_str().unwrap()),
        Some("ws_nte2_v2".to_string())
    );
}
