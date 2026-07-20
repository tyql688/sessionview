use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use super::{
    CancelFlagGuard, LoadRequest, build_session_turn_outline, session_window_bounds,
    subagent_meta_title,
};
use crate::commands::LoadToken;
use crate::models::{Message, MessageRole};

fn request(id: &str, seq: u64) -> LoadRequest<'_> {
    LoadRequest {
        id: Some(id),
        seq: Some(seq),
    }
}

fn is_tripped(guard: &CancelFlagGuard<'_>) -> bool {
    guard.flag().load(Ordering::Relaxed) != 0
}

#[test]
fn guard_newer_seq_supersedes_registered_older_load() {
    let tokens = Mutex::new(HashMap::<String, LoadToken>::new());

    let older = CancelFlagGuard::new(&tokens, "s1", request("s1:open:1", 1));
    let newer = CancelFlagGuard::new(&tokens, "s1", request("s1:open:2", 2));

    assert!(is_tripped(&older), "older in-flight load must be canceled");
    assert!(!is_tripped(&newer), "newer load must keep running");
}

#[test]
fn guard_stale_seq_registering_late_yields_instead_of_canceling_newer() {
    let tokens = Mutex::new(HashMap::<String, LoadToken>::new());

    // Task scheduling inversion: the newer request (seq 2) registers first,
    // then the older request's blocking task (seq 1) starts late.
    let newer = CancelFlagGuard::new(&tokens, "s1", request("s1:open:2", 2));
    let stale = CancelFlagGuard::new(&tokens, "s1", request("s1:open:1", 1));

    assert!(!is_tripped(&newer), "current load must NOT be canceled");
    assert!(
        is_tripped(&stale),
        "late stale load must start pre-canceled"
    );

    // The stale guard never owned the map entry: dropping it must leave the
    // newer token in place so explicit cancel-by-request-id still finds it.
    drop(stale);
    let map = tokens.lock().unwrap();
    assert_eq!(
        map.get("s1").and_then(|t| t.request_id.as_deref()),
        Some("s1:open:2")
    );
}

#[test]
fn guard_without_seq_keeps_replace_semantics_and_drop_cleans_own_entry() {
    let tokens = Mutex::new(HashMap::<String, LoadToken>::new());

    let first = CancelFlagGuard::new(&tokens, "s1", LoadRequest::default());
    let second = CancelFlagGuard::new(&tokens, "s1", LoadRequest::default());
    assert!(is_tripped(&first), "seq-less loads keep replace-previous");
    assert!(!is_tripped(&second));

    drop(first); // replaced entry: must not remove the second's token
    assert!(tokens.lock().unwrap().contains_key("s1"));
    drop(second);
    assert!(tokens.lock().unwrap().is_empty());
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
fn build_session_turn_outline_pairs_user_with_first_assistant_reply() {
    let messages = vec![
        Message::new(MessageRole::System, "ignored"),
        Message::new(MessageRole::User, " first   question "),
        Message::new(MessageRole::Assistant, " first reply "),
        Message::new(MessageRole::Assistant, "ignored follow-up"),
        Message::new(MessageRole::Tool, "ignored tool"),
        Message::new(MessageRole::User, "second question"),
    ];

    let result = build_session_turn_outline(&messages);
    let outline = &result.turns;

    // Session-wide renderable counts ride along on the same parse.
    assert_eq!(result.role_counts.user, 2);
    // Empty-string tool fields must read as absent, mirroring the TS side.
    let empty_tool = Message {
        tool_input: Some(String::new()),
        tool_name: Some(String::new()),
        ..Message::new(MessageRole::Tool, "")
    };
    let empty_result = build_session_turn_outline(&[empty_tool]);
    assert_eq!(empty_result.role_counts.tool, 0);
    assert_eq!(result.role_counts.assistant, 2);
    assert_eq!(result.role_counts.tool, 1);
    assert_eq!(result.role_counts.system, 1);

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
