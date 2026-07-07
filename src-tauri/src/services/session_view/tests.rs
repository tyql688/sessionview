use super::{build_session_turn_outline, session_window_bounds, subagent_meta_title};
use crate::models::{Message, MessageRole};

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
