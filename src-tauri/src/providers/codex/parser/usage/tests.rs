use serde_json::json;

use super::*;

fn usage(multiplier: u64) -> Value {
    json!({
        "input_tokens": 100 * multiplier,
        "cached_input_tokens": 40 * multiplier,
        "cache_write_input_tokens": 10 * multiplier,
        "output_tokens": 10 * multiplier,
        "reasoning_output_tokens": 2 * multiplier,
        "total_tokens": 110 * multiplier,
    })
}

fn snapshot(second: u8, last: Value, total: Value) -> CodexLine {
    CodexLine {
        ordinal: None,
        timestamp: Some(format!("2026-09-01T00:00:{second:02}Z")),
        line_type: "event_msg".to_string(),
        payload: Some(json!({
            "type": "token_count",
            "info": {"last_token_usage": last, "total_token_usage": total},
        })),
    }
}

fn response(id: &str) -> CodexLine {
    CodexLine {
        ordinal: None,
        timestamp: Some("2026-09-01T00:00:01Z".to_string()),
        line_type: "token_usage_record".to_string(),
        payload: Some(json!({
            "response_id": id, "turn_id": "turn-test", "usage": usage(1),
            "thread_token_usage": usage(1),
        })),
    }
}

fn scan(lines: Vec<CodexLine>) -> CodexScanAccum {
    let mut accum = CodexScanAccum::new();
    accum.current_model = Some("model-test".to_string());
    accum.begin_usage_turn(Some("turn-test"));
    for line in lines {
        accum.scan_line(&line, Path::new("fixture.jsonl"));
    }
    assert_eq!(accum.parse_warning_count, 0);
    accum
}

#[test]
fn repeated_snapshots_and_response_record_count_once_in_either_order() {
    for record_position in 0..=2 {
        let mut lines = vec![
            snapshot(2, usage(1), usage(1)),
            snapshot(3, usage(1), usage(1)),
        ];
        lines.insert(record_position, response("response-test"));
        lines.push(response("response-test"));
        let accum = scan(lines);
        assert_eq!(accum.usage_events.len(), 1);
        let event = &accum.usage_events[0];
        assert_eq!(
            event.usage_hash.as_deref(),
            Some("codex-response:response-test")
        );
        assert_eq!(event.input_tokens, 50);
        assert_eq!(event.cache_read_input_tokens, 40);
        assert_eq!(event.cache_creation_input_tokens, 10);
        assert_eq!(event.output_tokens, 10); // Reasoning is a subset of output.
    }
}

#[test]
fn equal_usage_at_same_timestamp_counts_twice_when_totals_advance() {
    let accum = scan(vec![
        snapshot(2, usage(1), usage(1)),
        snapshot(2, usage(1), usage(2)),
    ]);
    assert_eq!(accum.usage_events.len(), 2);
}

#[test]
fn legacy_without_totals_keeps_distinct_timestamp_responses() {
    let accum = scan(vec![
        snapshot(2, usage(1), Value::Null),
        snapshot(3, usage(1), Value::Null),
    ]);
    assert_eq!(accum.usage_events.len(), 2);
}

#[test]
fn changed_snapshot_after_reset_uses_last_response_usage() {
    let accum = scan(vec![
        snapshot(2, usage(1), usage(9)),
        snapshot(3, usage(1), usage(1)),
        snapshot(4, usage(1), usage(1)),
        snapshot(5, usage(1), usage(2)),
    ]);
    assert_eq!(accum.usage_events.len(), 3);
    assert!(
        accum
            .usage_events
            .iter()
            .all(|event| event.input_tokens == 50)
    );
}

#[test]
fn estimated_context_tokens_do_not_create_a_request() {
    let accum = scan(vec![snapshot(
        2,
        json!({"input_tokens": 0, "output_tokens": 0, "total_tokens": 900}),
        Value::Null,
    )]);
    assert!(accum.usage_events.is_empty());
}

#[test]
fn missing_required_usage_ordinal_is_a_warning() {
    let mut accum = CodexScanAccum::new();
    accum.usage_start_ordinal = Some(5);
    accum.current_model = Some("model-test".to_string());
    accum.scan_line(&snapshot(2, usage(1), usage(1)), Path::new("fixture.jsonl"));
    assert!(accum.usage_events.is_empty());
    assert_eq!(accum.parse_warning_count, 1);
}

#[test]
fn response_records_still_count_when_legacy_snapshot_does_not_advance() {
    let accum = scan(vec![
        response("response-first"),
        snapshot(2, usage(1), usage(1)),
        response("response-second"),
        snapshot(3, usage(1), usage(1)),
    ]);
    assert_eq!(accum.usage_events.len(), 2);
    assert!(
        accum
            .usage_events
            .iter()
            .all(|event| event.usage_hash.is_some())
    );
}

#[test]
fn equal_usage_for_a_remote_compaction_and_an_ordinary_request_counts_both() {
    for compaction_first in [false, true] {
        let marker = serde_json::from_value(json!({
            "type": "compacted", "payload": {"compaction_response_id": "compaction-test"},
        }))
        .unwrap();
        let mut request = response("compaction-test");
        request.payload.as_mut().unwrap()["thread_token_usage"] = usage(2);
        let ordinary = snapshot(2, usage(1), usage(1));
        let lines = if compaction_first {
            vec![request, marker, ordinary]
        } else {
            vec![ordinary, request, marker]
        };
        let accum = scan(lines);
        assert_eq!(accum.usage_events.len(), 2);
    }
}

fn child_meta() -> CodexLine {
    serde_json::from_value(json!({
        "timestamp": "2026-09-01T00:00:00Z", "type": "session_meta",
        "payload": {
            "id": "child-test",
            "source": {"subagent": {"thread_spawn": {"parent_thread_id": "parent-test"}}},
        },
    }))
    .unwrap()
}

#[test]
fn fresh_subagent_keeps_first_response_usage() {
    let accum = scan(vec![
        child_meta(),
        snapshot(2, usage(1), usage(1)),
        snapshot(3, usage(1), usage(2)),
    ]);
    assert!(accum.is_sidechain);
    assert_eq!(accum.usage_events.len(), 2);
}

#[test]
fn subagent_with_parent_metadata_still_skips_inherited_usage() {
    let parent_meta = serde_json::from_value(json!({
        "timestamp": "2026-09-01T00:00:00Z", "type": "session_meta",
        "payload": {"id": "parent-test"},
    }))
    .unwrap();
    let accum = scan(vec![
        child_meta(),
        parent_meta,
        snapshot(2, usage(9), usage(9)),
        snapshot(3, usage(1), usage(10)),
    ]);
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 50);
}

#[test]
fn own_task_start_ends_parent_replay_even_without_parent_usage() {
    let parent_meta = serde_json::from_value(json!({
        "timestamp": "2026-09-01T00:00:00Z", "type": "session_meta",
        "payload": {"id": "parent-test"},
    }))
    .unwrap();
    let started_at = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:01Z")
        .unwrap()
        .timestamp();
    let task_start = serde_json::from_value(json!({
        "timestamp": "2026-09-01T00:00:01Z", "type": "event_msg",
        "payload": {"type": "task_started", "turn_id": "child-turn", "started_at": started_at},
    }))
    .unwrap();
    let accum = scan(vec![
        child_meta(),
        parent_meta,
        task_start,
        snapshot(2, usage(1), usage(1)),
    ]);
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.current_turn_id.as_deref(), Some("child-turn"));
}

#[test]
fn inherited_usage_crossing_a_second_boundary_is_not_counted() {
    for cumulative_only in [false, true] {
        let parent = serde_json::from_value(json!({
            "type": "session_meta", "payload": {"id": "parent-test"},
        }))
        .unwrap();
        let mut first = snapshot(2, usage(9), usage(9));
        first.timestamp = Some("2026-09-01T00:00:02.995Z".to_string());
        let mut second = snapshot(3, usage(1), usage(10));
        second.timestamp = Some("2026-09-01T00:00:03.010Z".to_string());
        let mut own = snapshot(6, usage(1), usage(11));
        if cumulative_only {
            for entry in [&mut first, &mut second, &mut own] {
                entry.payload.as_mut().unwrap()["info"]
                    .as_object_mut()
                    .unwrap()
                    .remove("last_token_usage");
            }
        }
        let accum = scan(vec![child_meta(), parent, first, second, own]);
        assert_eq!(accum.usage_events.len(), 1);
        assert_eq!(accum.usage_events[0].input_tokens, 50);
        assert_eq!(accum.usage_events[0].output_tokens, 10);
    }
}

#[test]
fn own_task_boundary_counts_usage_inside_the_replay_burst() {
    let parent = serde_json::from_value(json!({
        "type": "session_meta", "payload": {"id": "parent-test"},
    }))
    .unwrap();
    let mut inherited = snapshot(2, usage(9), usage(9));
    inherited.timestamp = Some("2026-09-01T00:00:02.995Z".to_string());
    let start = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:03Z")
        .unwrap()
        .timestamp();
    let task = serde_json::from_value(json!({
        "timestamp": "2026-09-01T00:00:03.001Z", "type": "event_msg",
        "payload": {"type": "task_started", "turn_id": "child-turn", "started_at": start},
    }))
    .unwrap();
    let mut own = snapshot(3, usage(1), usage(10));
    own.timestamp = Some("2026-09-01T00:00:03.010Z".to_string());
    let accum = scan(vec![child_meta(), parent, inherited, task, own]);
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 50);
    assert_eq!(accum.current_turn_id.as_deref(), Some("child-turn"));
}

#[test]
fn unprovable_replay_timestamps_warn_and_skip_usage() {
    for timestamp in [None, Some("invalid"), Some("2026-09-01T00:00:01Z")] {
        let mut accum = CodexScanAccum::new();
        accum.current_model = Some("model-test".to_string());
        accum.replay_usage_skip = true;
        accum.replay_last_timestamp_ms = Some(
            chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:02Z")
                .unwrap()
                .timestamp_millis(),
        );
        let mut inherited = snapshot(2, usage(9), usage(9));
        inherited.timestamp = timestamp.map(str::to_string);
        accum.scan_line(&inherited, Path::new("fixture.jsonl"));
        assert!(accum.usage_events.is_empty());
        assert_eq!(accum.parse_warning_count, 1);
        accum.scan_line(
            &snapshot(6, usage(1), usage(10)),
            Path::new("fixture.jsonl"),
        );
        assert_eq!(accum.usage_events.len(), 1);
        assert_eq!(accum.usage_events[0].input_tokens, 50);
    }
}

#[test]
fn copied_parent_metadata_with_history_base_preserves_child_identity() {
    for base in [
        Value::Null,
        json!({"thread_id": "ancestor-test", "end_ordinal_exclusive": 10, "end_byte_offset": 100}),
    ] {
        let parent = serde_json::from_value(json!({
            "type": "session_meta", "payload": {"id": "parent-test", "history_base": base},
        }))
        .unwrap();
        let accum = scan(vec![child_meta(), parent]);
        assert_eq!(accum.session_id.as_deref(), Some("child-test"));
        assert!(accum.is_sidechain);
        assert!(accum.skipping_fork_context);
    }
}

#[test]
fn request_model_does_not_change_active_turn_model() {
    let mut request = response("request-model");
    request.payload.as_mut().unwrap()["model"] = json!("compaction-model");
    let accum = scan(vec![request, snapshot(3, usage(2), usage(2))]);
    assert_eq!(accum.usage_events.len(), 2);
    assert_eq!(accum.usage_events[0].model, "compaction-model");
    assert_eq!(accum.usage_events[1].model, "model-test");
}

#[test]
fn foreign_thread_response_is_not_child_usage() {
    let mut accum = CodexScanAccum::new();
    accum.owned_session_id = Some("child-test".to_string());
    accum.current_model = Some("model-test".to_string());
    accum.begin_usage_turn(Some("turn-test"));
    let mut inherited = response("inherited-response");
    inherited.payload.as_mut().unwrap()["thread_id"] = json!("parent-test");
    accum.scan_line(&inherited, Path::new("fixture.jsonl"));
    assert!(accum.usage_events.is_empty());
    let mut own = response("own-response");
    own.payload.as_mut().unwrap()["thread_id"] = json!("child-test");
    accum.scan_line(&own, Path::new("fixture.jsonl"));
    assert_eq!(accum.usage_events.len(), 1);
}

#[test]
fn trigger_turn_true_ends_replay_without_timestamp_heuristics() {
    let parent =
        serde_json::from_value(json!({"type": "session_meta", "payload": {"id": "parent-test"}}))
            .unwrap();
    let marker = |trigger| {
        serde_json::from_value(json!({
            "type": "inter_agent_communication_metadata", "payload": {"trigger_turn": trigger},
        }))
        .unwrap()
    };
    let accum = scan(vec![
        child_meta(),
        parent,
        snapshot(2, usage(9), usage(9)),
        marker(false),
        marker(true),
        snapshot(2, usage(1), usage(10)),
    ]);
    assert_eq!(accum.usage_events.len(), 1);
    assert!(!accum.skipping_fork_context);
    assert_eq!(accum.usage_events[0].input_tokens, 50);
}

#[test]
fn explicit_ordinal_boundary_excludes_inherited_usage() {
    let mut accum = CodexScanAccum::new();
    accum.current_model = Some("model-test".to_string());
    accum.usage_start_ordinal = Some(5);
    let mut parent = snapshot(2, usage(9), usage(9));
    parent.ordinal = Some(4);
    accum.scan_line(&parent, Path::new("fixture.jsonl"));
    assert!(accum.usage_events.is_empty());
    let mut own = snapshot(2, usage(1), usage(10));
    own.ordinal = Some(5);
    accum.scan_line(&own, Path::new("fixture.jsonl"));
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 50);
}
