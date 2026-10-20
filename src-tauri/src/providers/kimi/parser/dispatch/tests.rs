use super::*;
use crate::models::ToolResultMode;
use serde_json::{Value, json};

fn text_part(text: &str) -> Value {
    json!({
        "type": "context.append_loop_event",
        "event": {"type": "content.part", "part": {"type": "text", "text": text}},
        "time": 1001
    })
}

fn contents(accum: &ScanAccum) -> Vec<&str> {
    accum.messages.iter().map(|m| m.content.as_str()).collect()
}

#[test]
fn cancelled_turn_keeps_partial_output_and_usage() {
    let mut accum = ScanAccum::new();
    dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
    dispatch_line(&mut accum, &text_part("partial response..."));
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "tool.call", "toolCallId": "tc_1", "name": "Read", "args": {"path": "a.txt"}},
            "time": 1002
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "usage.record",
            "model": "kimi-test",
            "usage": {"inputOther": 10, "output": 5},
            "usageScope": "turn",
            "time": 1002
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({"type": "turn.cancel", "turnId": 0, "target": "active", "reason": "user_cancelled", "time": 1003}),
    );
    dispatch_line(
        &mut accum,
        &json!({"type": "turn.ended", "turnId": 0, "reason": "cancelled", "time": 1004}),
    );

    assert_eq!(
        contents(&accum),
        ["partial response...", "", "[turn_cancelled]"]
    );
    assert_eq!(accum.call_id_map.index_of(Some("tc_1")), Some(1));
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 10);
    assert_eq!(accum.parse_warning_count, 0);
}

#[test]
fn turn_prompt_without_cancel_keeps_content() {
    let mut accum = ScanAccum::new();
    dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_message",
            "message": {"role": "user", "content": [{"type": "text", "text": "query"}], "toolCalls": [], "origin": {"kind": "user"}},
            "time": 1001
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "content.part", "part": {"type": "text", "text": "answer"}},
            "time": 1002
        }),
    );
    // No cancel — content should be kept
    assert_eq!(accum.messages.len(), 2);
    assert_eq!(accum.messages[1].content, "answer");
}

#[test]
fn step_end_usage_fallback_when_no_usage_record() {
    let mut accum = ScanAccum::new();
    dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_message",
            "message": {"role": "user", "content": [{"type": "text", "text": "hi"}], "toolCalls": [], "origin": {"kind": "user"}},
            "time": 1001
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "content.part", "part": {"type": "text", "text": "Hello!"}},
            "time": 1002
        }),
    );
    // step.end with usage but no usage.record
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "step.end",
                "usage": {"model": "kimi-test", "inputOther": 100, "output": 50, "inputCacheRead": 200, "inputCacheCreation": 0}
            },
            "time": 1003
        }),
    );

    let usage = accum.messages[1]
        .token_usage
        .as_ref()
        .expect("usage attached");
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 100);
}

#[test]
fn turn_usage_overwrites_step_fallback_once() {
    let mut accum = ScanAccum::new();
    accum.push_assistant_text("answer", Some("2026-07-19T00:00:00Z".into()));
    accum.push_tool_call("Read", Some("call-1"), None, None, None);
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "step.end",
                "usage": {"inputOther": 10, "output": 5, "inputCacheRead": 20}
            }
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "usage.record",
            "usageScope": "turn",
            "model": "kimi-test",
            "usage": {"inputOther": 12, "output": 6, "inputCacheRead": 24},
            "time": 1001
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "usage.record",
            "usageScope": "session",
            "model": "kimi-test",
            "usage": {"inputOther": 120, "output": 60, "inputCacheRead": 240}
        }),
    );

    assert_eq!(
        accum
            .messages
            .iter()
            .filter(|message| message.token_usage.is_some())
            .count(),
        1
    );
    let usage = accum.messages[0].token_usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.cache_read_input_tokens, 24);
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 12);
}

#[test]
fn step_usage_accumulates_when_turn_record_never_arrives() {
    let mut accum = ScanAccum::new();
    accum.current_model = Some("kimi-test".into());
    accum.push_assistant_text("answer", Some("2026-07-19T00:00:00Z".into()));
    for step in 0..2 {
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {
                    "type": "step.end",
                    "usage": {"inputOther": 100, "output": 50, "inputCacheRead": 30}
                },
                "time": 1001 + step
            }),
        );
    }

    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 200);
    assert_eq!(accum.usage_events[0].output_tokens, 100);
    assert_eq!(accum.usage_events[0].cache_read_input_tokens, 60);
    let usage = accum.messages[0].token_usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, 200);
    assert_eq!(usage.output_tokens, 100);
}

#[test]
fn usage_record_before_content_stays_authoritative_and_attaches_later() {
    let mut accum = ScanAccum::new();
    dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
    dispatch_line(
        &mut accum,
        &json!({
            "type": "usage.record",
            "usageScope": "turn",
            "model": "kimi-test",
            "usage": {"inputOther": 12, "output": 6, "inputCacheRead": 24},
            "time": 1001
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "content.part", "part": {"type": "text", "text": "answer"}},
            "time": 1002
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "step.end",
                "usage": {"inputOther": 12, "output": 6, "inputCacheRead": 24}
            },
            "time": 1003
        }),
    );

    assert_eq!(accum.parse_warning_count, 0);
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 12);
    let usage = accum.messages[0].token_usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.cache_read_input_tokens, 24);
    assert_eq!(usage.output_tokens, 6);
}

#[test]
fn usage_before_content_stays_isolated_across_model_steps() {
    let mut accum = ScanAccum::new();
    dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));

    for (step, input, output) in [(0, 12, 6), (1, 20, 9)] {
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {"type": "step.begin", "step": step},
                "time": 1100 + step * 10
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "usage.record",
                "usageScope": "turn",
                "model": "kimi-test",
                "usage": {"inputOther": input, "output": output},
                "time": 1101 + step * 10
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {
                    "type": "content.part",
                    "part": {"type": "text", "text": format!("answer-{step}")}
                },
                "time": 1102 + step * 10
            }),
        );
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {
                    "type": "step.end",
                    "step": step,
                    "usage": {"inputOther": input, "output": output}
                },
                "time": 1103 + step * 10
            }),
        );
    }

    assert_eq!(accum.parse_warning_count, 0);
    assert_eq!(accum.usage_events.len(), 2);
    assert_eq!(accum.usage_events[0].input_tokens, 12);
    assert_eq!(accum.usage_events[1].input_tokens, 20);
    assert_eq!(accum.messages.len(), 2);
    assert_eq!(
        accum.messages[0].token_usage.as_ref().unwrap().input_tokens,
        12
    );
    assert_eq!(
        accum.messages[1].token_usage.as_ref().unwrap().input_tokens,
        20
    );
}

#[test]
fn later_step_fallback_survives_previous_authoritative_usage() {
    let mut accum = ScanAccum::new();
    accum.current_model = Some("kimi-test".into());

    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "step.begin", "step": 0},
            "time": 1000
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "usage.record",
            "usageScope": "turn",
            "model": "kimi-test",
            "usage": {"inputOther": 12, "output": 6},
            "time": 1001
        }),
    );
    accum.push_assistant_text("first", Some("2026-07-19T00:00:00Z".into()));
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "step.end", "step": 0, "usage": {"inputOther": 12, "output": 6}},
            "time": 1002
        }),
    );

    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "step.begin", "step": 1},
            "time": 1010
        }),
    );
    accum.push_assistant_text("second", Some("2026-07-19T00:00:01Z".into()));
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "step.end", "step": 1, "usage": {"inputOther": 20, "output": 9}},
            "time": 1011
        }),
    );

    assert_eq!(accum.parse_warning_count, 0);
    assert_eq!(accum.usage_events.len(), 2);
    assert_eq!(accum.usage_events[1].input_tokens, 20);
    let second_usage = accum.messages[1].token_usage.as_ref().unwrap();
    assert_eq!(second_usage.input_tokens, 20);
    assert_eq!(second_usage.output_tokens, 9);
}

#[test]
fn assistant_text_takes_over_usage_from_tool_owner() {
    let mut accum = ScanAccum::new();
    accum.current_model = Some("kimi-test".into());
    accum.push_tool_call(
        "Read",
        Some("call-1"),
        None,
        Some("2026-07-19T00:00:00Z".into()),
        None,
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "step.end",
                "usage": {"inputOther": 10, "output": 5}
            },
            "time": 1001
        }),
    );
    accum.push_assistant_text("answer", Some("2026-07-19T00:00:02Z".into()));
    dispatch_line(
        &mut accum,
        &json!({
            "type": "usage.record",
            "usageScope": "turn",
            "model": "kimi-test",
            "usage": {"inputOther": 12, "output": 6},
            "time": 1002
        }),
    );

    let carriers: Vec<usize> = accum
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| message.token_usage.is_some().then_some(index))
        .collect();
    assert_eq!(carriers, vec![1], "only the assistant text carries usage");
    let usage = accum.messages[1].token_usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 12);
}

#[test]
fn real_user_boundary_finalizes_step_usage_fallback() {
    let mut accum = ScanAccum::new();
    for (text, time) in [("first", 1000), ("second", 2000)] {
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_message",
                "message": {
                    "role": "user",
                    "content": [{"type": "text", "text": text}]
                },
                "time": time
            }),
        );
        accum.push_assistant_text("answer", Some("2026-07-19T00:00:00Z".into()));
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_loop_event",
                "event": {
                    "type": "step.end",
                    "usage": {"model": "kimi-test", "inputOther": 10, "output": 5}
                },
                "time": time + 1
            }),
        );
    }

    assert_eq!(accum.usage_events.len(), 2);
}

#[test]
fn usage_without_current_output_does_not_overwrite_previous_turn() {
    let mut accum = ScanAccum::new();
    accum.current_model = Some("kimi-test".into());
    accum.push_assistant_text("first", Some("2026-07-19T00:00:00Z".into()));
    dispatch_line(
        &mut accum,
        &json!({"type":"usage.record","usageScope":"turn","model":"kimi-test","usage":{"output":1},"time":1000}),
    );
    accum.push_user_text("next", Some("2026-07-19T00:00:01Z".into()));
    dispatch_line(
        &mut accum,
        &json!({"type":"usage.record","usageScope":"turn","model":"kimi-test","usage":{"output":2},"time":2000}),
    );

    assert_eq!(
        accum.messages[0]
            .token_usage
            .as_ref()
            .unwrap()
            .output_tokens,
        1
    );
    assert!(accum.messages[1].token_usage.is_none());
    assert_eq!(accum.usage_events.len(), 2);
    accum.finish_pending_usage();
    assert_eq!(accum.parse_warning_count, 1);
}

#[test]
fn background_task_origin_renders_status_instead_of_user() {
    let mut accum = ScanAccum::new();
    let notification = r#"<notification id="task:bash-demo1234:failed" category="task" type="task.failed" source_kind="background_task" source_id="bash-demo1234">
Title: Background process failed
Severity: warning
Synthetic test task failed.
</notification>"#;

    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_message",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": notification}],
                "toolCalls": [],
                "origin": {
                    "kind": "background_task",
                    "taskId": "bash-demo1234",
                    "status": "failed",
                    "notificationId": "task:bash-demo1234:failed"
                }
            },
            "time": 1779701196500i64
        }),
    );

    assert_eq!(accum.messages.len(), 1);
    assert_eq!(accum.messages[0].role, MessageRole::System);
    assert!(
        accum.messages[0]
            .content
            .starts_with("[task_status_error] failed · bash-demo1234\n")
    );
    assert!(accum.messages[0].content.contains(notification));
    assert_eq!(accum.first_user_message, None);
    assert_eq!(accum.content_parts, vec![notification]);

    // A real TaskOutput call remains a tool bubble. Runtime task status
    // normalization must not blur the tool/event boundary.
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "tool.call",
                "toolCallId": "tool-task-output",
                "name": "TaskOutput",
                "args": {"task_id": "bash-demo1234", "block": true}
            },
            "time": 1779701196600i64
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "tool.result",
                "toolCallId": "tool-task-output",
                "result": {"output": "status: failed"}
            },
            "time": 1779701196700i64
        }),
    );

    assert_eq!(accum.messages.len(), 2);
    assert_eq!(accum.messages[1].role, MessageRole::Tool);
    assert_eq!(accum.messages[1].tool_name.as_deref(), Some("TaskOutput"));
    assert_eq!(
        accum.messages[1]
            .tool_metadata
            .as_ref()
            .map(|metadata| metadata.canonical_name.as_str()),
        Some("TaskOutput")
    );
}

#[test]
fn subagent_system_trigger_renders_task_context_instead_of_user() {
    let mut accum = ScanAccum::new();

    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_message",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "Inspect parser behavior"}],
                "toolCalls": [],
                "origin": {"kind": "system_trigger", "name": "subagent"}
            },
            "time": 1779701196500i64
        }),
    );

    assert_eq!(accum.messages.len(), 1);
    assert_eq!(accum.messages[0].role, MessageRole::System);
    assert_eq!(
        accum.messages[0].content,
        "[subagent_task] Inspect parser behavior"
    );
    assert_eq!(
        accum.first_user_message.as_deref(),
        Some("Inspect parser behavior")
    );
}

#[test]
fn unknown_prompt_origin_renders_as_generic_context() {
    let mut accum = ScanAccum::new();

    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_message",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "runtime payload"}],
                "origin": {"kind": "future_runtime_event"}
            },
            "time": 1779701196500i64
        }),
    );

    assert_eq!(accum.messages.len(), 1);
    assert_eq!(accum.messages[0].role, MessageRole::System);
    assert_eq!(
        accum.messages[0].content,
        "[kimi_context] future_runtime_event\nruntime payload"
    );
    // Unknown kinds are future protocol, fully rendered — not a parse
    // warning; the text is still indexed for search.
    assert_eq!(accum.parse_warning_count, 0);
    assert_eq!(accum.content_parts, vec!["runtime payload"]);
    assert_eq!(accum.first_user_message, None);
}

#[test]
fn unsupported_native_tool_result_uses_the_exclusive_raw_mode() {
    let mut accum = ScanAccum::new();
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "tool.call",
                "toolCallId": "future-call",
                "name": "FutureTool",
                "args": {}
            },
            "time": 1779701196500i64
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {
                "type": "tool.result",
                "toolCallId": "future-call",
                "result": {
                    "output": [{"type": "future_media", "payload": "keep"}]
                }
            },
            "time": 1779701196600i64
        }),
    );

    let message = accum.messages.first().unwrap();
    assert_eq!(
        message.content,
        r#"[{"payload":"keep","type":"future_media"}]"#
    );
    assert_eq!(
        message
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.presentation.as_ref())
            .map(|presentation| presentation.result_mode),
        Some(ToolResultMode::Raw)
    );
}

#[test]
fn turn_steer_renders_user_text_and_notifications() {
    let mut accum = ScanAccum::new();
    dispatch_line(
        &mut accum,
        &json!({
            "type": "turn.steer",
            "input": [{"type": "text", "text": "also fix the tests"}],
            "time": 1779701196500i64
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "turn.steer",
            "input": [{"type": "text", "text": "<notification id=\"task:x:completed\">done</notification>"}],
            "time": 1779701196600i64
        }),
    );

    assert_eq!(accum.messages.len(), 2);
    assert_eq!(accum.messages[0].role, MessageRole::User);
    assert_eq!(accum.messages[0].content, "also fix the tests");
    assert_eq!(accum.messages[1].role, MessageRole::System);
    assert!(
        accum.messages[1]
            .content
            .starts_with("[kimi_context] steer\n")
    );
    assert_eq!(accum.parse_warning_count, 0);
}

#[test]
fn unknown_record_type_is_counted_not_silently_dropped() {
    let mut accum = ScanAccum::new();
    dispatch_line(
        &mut accum,
        &json!({"type": "future.event", "time": 1779701196500i64}),
    );
    assert!(accum.messages.is_empty());
    assert_eq!(accum.parse_warning_count, 1);
}

#[test]
fn current_profile_and_lifecycle_records_are_handled() {
    let mut accum = ScanAccum::new();
    dispatch_line(
        &mut accum,
        &json!({
            "type": "profile.bind",
            "modelAlias": "kimi-latest",
            "profileName": "reviewer",
            "time": 1779701196500i64
        }),
    );
    for event in [
        json!({"type": "runtime.set_binding", "workspaceId": "wd-test", "runtimeId": "local", "agentId": "main", "time": 1779701196501i64}),
        json!({"type": "turn.ended", "agentId": "main", "turnId": 0, "reason": "completed", "time": 1779701196502i64}),
        json!({"type": "token_counting.turn_recorded", "agentId": "main", "turnId": 0, "tokens": 100, "length": 200, "time": 1779701196503i64}),
        json!({"type": "token_counting.measured", "agentId": "main", "tokens": 100, "length": 200, "time": 1779701196504i64}),
        json!({"type": "prompt.accepted", "agentId": "main", "promptId": "prompt-1", "time": 1779701196505i64}),
        json!({"type": "plugin.session_start", "agentId": "main", "time": 1779701196506i64}),
    ] {
        dispatch_line(&mut accum, &event);
    }

    assert_eq!(accum.current_model.as_deref(), Some("kimi-latest"));
    assert_eq!(accum.current_profile.as_deref(), Some("reviewer"));
    assert!(accum.messages.is_empty());
    assert_eq!(accum.parse_warning_count, 0);
}

#[test]
fn abnormal_lifecycle_records_surface_status_details() {
    let mut accum = ScanAccum::new();
    for event in [
        json!({
            "type": "turn.step.retrying",
            "nextAttempt": 2,
            "maxAttempts": 5,
            "delayMs": 750,
            "errorName": "ProviderError",
            "errorMessage": "temporary upstream failure",
            "time": 1779701196501i64
        }),
        json!({
            "type": "turn.step.interrupted",
            "reason": "tool_cancelled",
            "message": "the active tool was stopped",
            "time": 1779701196502i64
        }),
        json!({
            "type": "turn.ended",
            "reason": "failed",
            "error": {"message": "request failed"},
            "time": 1779701196503i64
        }),
    ] {
        dispatch_line(&mut accum, &event);
    }

    assert_eq!(accum.parse_warning_count, 0);
    assert_eq!(accum.messages.len(), 3);
    assert!(accum.messages[0].content.contains("attempt 2/5"));
    assert!(
        accum.messages[0]
            .content
            .contains("temporary upstream failure")
    );
    assert!(accum.messages[1].content.contains("tool_cancelled"));
    assert!(
        accum.messages[1]
            .content
            .contains("the active tool was stopped")
    );
    assert_eq!(accum.messages[2].content, "[turn_failed]\nrequest failed");
}

#[test]
fn retry_status_accepts_fractional_millisecond_delays() {
    let mut accum = ScanAccum::new();
    dispatch_line(
        &mut accum,
        &json!({
            "type": "turn.step.retrying",
            "nextAttempt": 2,
            "maxAttempts": 5,
            "delayMs": 750.25,
            "errorName": "ProviderError",
            "time": 1779701196501i64
        }),
    );
    assert_eq!(accum.parse_warning_count, 0);
    assert_eq!(accum.messages.len(), 1);
    assert!(accum.messages[0].content.contains("after 750.25 ms"));
}

#[test]
fn retry_status_rejects_negative_and_non_numeric_delays() {
    for delay in [json!(-0.5), json!("750"), json!(null)] {
        let mut accum = ScanAccum::new();
        dispatch_line(
            &mut accum,
            &json!({
                "type": "turn.step.retrying",
                "nextAttempt": 2,
                "maxAttempts": 5,
                "delayMs": delay,
                "errorName": "ProviderError",
                "time": 1779701196501i64
            }),
        );
        assert_eq!(accum.parse_warning_count, 1);
        assert!(accum.messages.is_empty());
    }
}

#[test]
fn malformed_abnormal_lifecycle_record_remains_a_warning() {
    let mut accum = ScanAccum::new();

    dispatch_line(
        &mut accum,
        &json!({"type": "turn.ended", "time": 1779701196500i64}),
    );

    assert!(accum.messages.is_empty());
    assert_eq!(accum.parse_warning_count, 1);
}

#[test]
fn invalid_metadata_timestamp_is_reported() {
    let mut accum = ScanAccum::new();

    dispatch_line(
        &mut accum,
        &json!({"type": "metadata", "created_at": i64::MAX}),
    );

    assert_eq!(accum.parse_warning_count, 1);
    assert_eq!(accum.first_time_secs, None);
}

#[test]
fn session_scoped_usage_counts_without_annotating_messages() {
    let mut accum = ScanAccum::new();
    dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
    dispatch_line(&mut accum, &text_part("answer"));
    // A compaction call outside the turn: real spend, no owning message.
    dispatch_line(
        &mut accum,
        &json!({
            "type": "usage.record",
            "model": "kimi-test",
            "usage": {"inputOther": 700, "output": 30, "inputCacheRead": 0, "inputCacheCreation": 0},
            "usageScope": "session",
            "time": 1002
        }),
    );
    accum.finish_pending_usage();

    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 700);
    assert_eq!(accum.usage_events[0].turn_count, 0);
    assert!(accum.messages[0].token_usage.is_none());
    assert_eq!(accum.parse_warning_count, 0);
}

#[test]
fn scopeless_usage_records_defer_to_step_end() {
    let mut accum = ScanAccum::new();
    dispatch_line(
        &mut accum,
        &json!({"type": "config.update", "modelAlias": "kimi-test", "time": 999}),
    );
    dispatch_line(&mut accum, &json!({"type": "turn.prompt", "time": 1000}));
    dispatch_line(&mut accum, &text_part("answer"));
    dispatch_line(
        &mut accum,
        &json!({
            "type": "usage.record",
            "model": "kimi-test",
            "usage": {"inputOther": 10, "output": 5},
            "time": 1002
        }),
    );
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "step.end", "usage": {"inputOther": 10, "output": 5}},
            "time": 1003
        }),
    );

    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 10);
}

#[test]
fn undo_marks_the_transcript_and_keeps_history() {
    let mut accum = ScanAccum::new();
    dispatch_line(&mut accum, &text_part("first answer"));
    // What kimi-code 2.1.1 writes for `/undo` of one turn.
    for record in [
        json!({"type": "agent.switched", "agentId": "main", "branch": "b1", "reason": "undo", "base": {"branch": "main", "line": 36}, "turns": 1, "legacyUndoLine": 50, "time": 1002}),
        json!({"type": "context.undo", "agentId": "main", "count": 1, "time": 1002}),
        json!({"type": "context.undone", "agentId": "main", "turns": 1, "fromTurnId": 1, "time": 1002}),
        json!({"type": "token_counting.truncated", "agentId": "main", "time": 1002}),
    ] {
        dispatch_line(&mut accum, &record);
    }

    assert_eq!(
        contents(&accum),
        [
            "first answer",
            "[kimi_context] undo\nundid the last 1 turn(s); they stay above",
        ]
    );
    assert_eq!(accum.parse_warning_count, 0);
}

#[test]
fn journal_and_lifecycle_mirrors_are_silent() {
    let mut accum = ScanAccum::new();
    for record in [
        json!({"type": "agent.message.appended", "kind": "event", "message": {"message": {"role": "user", "content": [{"type": "text", "text": "hi"}]}, "meta": {"source": "input"}}, "time": 1000}),
        json!({"type": "agent.message.appended", "kind": "event", "message": {"message": {"role": "assistant", "content": [{"type": "text", "text": "hello"}], "toolCalls": []}, "meta": {"source": "llm"}}, "time": 1001}),
        json!({"type": "agent.turn.started", "kind": "event", "turnId": 0, "queueItemId": "q", "time": 1000}),
        json!({"type": "agent.turn.ended", "kind": "event", "turnId": 0, "outcome": "done", "time": 1001}),
        json!({"type": "subagent.spawned", "subagentId": "agent-1", "subagentName": "explore", "parentToolCallId": "tc", "parentAgentId": "main", "callerAgentId": "main", "description": "List files", "runInBackground": false, "time": 1001}),
        json!({"type": "subagent.started", "subagentId": "agent-1", "time": 1001}),
        json!({"type": "subagent.completed", "subagentId": "agent-1", "resultSummary": "ok", "usage": {"inputOther": 902, "output": 130, "inputCacheRead": 15360, "inputCacheCreation": 0}, "contextTokens": 8295, "time": 1002}),
        json!({"type": "file_history.checkpoint", "agentId": "main", "turnId": 0, "entries": {}, "time": 1002}),
        json!({"type": "file_history.tracked", "agentId": "main", "turnId": 0, "path": "a.rs", "entry": {"key": null, "version": 1}, "time": 1002}),
        json!({"type": "tower_mode.enter", "agentId": "main", "time": 1002}),
        json!({"type": "tower_mode.exit", "agentId": "main", "time": 1002}),
    ] {
        dispatch_line(&mut accum, &record);
    }

    assert!(accum.messages.is_empty());
    // A child's usage lives in its own wire; the parent's mirror adds none.
    assert!(accum.usage_events.is_empty());
    assert_eq!(accum.parse_warning_count, 0);
}

#[test]
fn salvaged_partial_output_of_an_interrupted_step_renders() {
    // On cancel kimi-code journals the interrupted step's partial output
    // and writes no `content.part` for it.
    let mut accum = ScanAccum::new();
    for record in [
        json!({"type": "turn.cancel", "agentId": "main", "turnId": 1, "target": "active", "reason": "user_cancelled", "time": 1000}),
        json!({"type": "context.append_loop_event", "agentId": "main", "event": {"type": "step.end", "turnId": "1", "step": 1, "finishReason": "interrupted"}, "time": 1001}),
        json!({"type": "turn.step.interrupted", "agentId": "main", "turnId": 1, "step": 1, "reason": "aborted", "time": 1001}),
        json!({"type": "agent.message.appended", "kind": "event", "message": {"message": {"role": "assistant", "content": [{"type": "think", "think": "The user wants an essay", "reasoningKey": "reasoning_content"}], "toolCalls": []}, "meta": {"model": {"provider": "agent-loop", "model": "agent-loop"}, "source": "salvaged", "usage": {"inputOther": 0, "output": 0, "inputCacheRead": 0, "inputCacheCreation": 0}}}, "time": 1002}),
        json!({"type": "turn.ended", "agentId": "main", "turnId": 1, "reason": "cancelled", "time": 1003}),
    ] {
        dispatch_line(&mut accum, &record);
    }

    assert_eq!(
        contents(&accum),
        [
            "[step_interrupted] aborted",
            "[thinking]\nThe user wants an essay",
            "[turn_cancelled]",
        ]
    );
    assert!(accum.usage_events.is_empty());
    assert_eq!(accum.parse_warning_count, 0);
}

#[test]
fn steering_renders_once() {
    // kimi-code writes the steering message to context, then `turn.steer`.
    let mut accum = ScanAccum::new();
    for record in [
        json!({"type": "context.append_message", "agentId": "main", "message": {"role": "user", "content": [{"type": "text", "text": "Also mention the year."}], "toolCalls": [], "id": "msg_steer", "origin": {"kind": "user", "inTurn": true}}, "time": 1000}),
        json!({"type": "turn.steer", "agentId": "main", "input": [{"type": "text", "text": "Also mention the year."}], "origin": {"kind": "user", "inTurn": true}, "messageId": "msg_steer", "promptIds": ["msg_steer"], "turnId": 4, "time": 1000}),
    ] {
        dispatch_line(&mut accum, &record);
    }

    assert_eq!(contents(&accum), ["Also mention the year."]);
}

#[test]
fn a_session_fork_counts_only_its_own_usage() {
    // `kimi fork` copies the source's records, then appends `forked`.
    let mut accum = ScanAccum::new();
    let usage = |input: u64, time: i64| json!({"type": "usage.record", "agentId": "main", "model": "kimi-code/k3", "usage": {"inputOther": input, "output": 1, "inputCacheRead": 0, "inputCacheCreation": 0}, "usageScope": "turn", "time": time});
    for record in [
        json!({"type": "turn.prompt", "agentId": "main", "turnId": 0, "time": 1000}),
        text_part("inherited answer"),
        usage(100, 1002),
        json!({"type": "forked", "agentId": "main", "time": 2000}),
        json!({"type": "turn.prompt", "agentId": "main", "turnId": 1, "time": 2001}),
        text_part("own answer"),
        usage(7, 2003),
    ] {
        dispatch_line(&mut accum, &record);
    }

    assert_eq!(contents(&accum), ["inherited answer", "own answer"]);
    assert_eq!(accum.usage_events.len(), 1);
    assert_eq!(accum.usage_events[0].input_tokens, 7);
    assert!(accum.messages[0].token_usage.is_none());
}

#[test]
fn migrated_tool_calls_accept_both_wire_shapes() {
    let mut accum = ScanAccum::new();
    for tool_call in [
        // Wire 1.0 nests the call under `function`.
        json!({"type": "function", "id": "tc_old", "function": {"name": "Read", "arguments": "{\"path\":\"a.rs\"}"}}),
        // Wire 1.1+ (and every forked agent) flattens it.
        json!({"type": "function", "id": "tc_new", "name": "Grep", "arguments": "{\"pattern\":\"fn\"}"}),
    ] {
        dispatch_line(
            &mut accum,
            &json!({
                "type": "context.append_message",
                "message": {"role": "assistant", "content": [], "toolCalls": [tool_call]},
                "time": 1000
            }),
        );
    }

    let names: Vec<Option<&str>> = accum
        .messages
        .iter()
        .map(|m| m.tool_name.as_deref())
        .collect();
    assert_eq!(names, [Some("Read"), Some("Grep")]);
    assert_eq!(
        accum.messages[0].tool_input.as_deref(),
        Some(r#"{"path":"a.rs"}"#)
    );
    assert_eq!(
        accum.messages[1].tool_input.as_deref(),
        Some(r#"{"pattern":"fn"}"#)
    );
}

#[test]
fn user_media_parts_render_and_unknown_parts_are_skipped_with_a_warning() {
    let mut accum = ScanAccum::new();
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_message",
            "message": {
                "role": "user",
                "origin": {"kind": "user"},
                "content": [
                    {"type": "text", "text": "listen and watch"},
                    {"type": "audio_url", "audioUrl": {"url": "/tmp/clip.wav"}},
                    {"type": "video_url", "videoUrl": {"url": "/tmp/clip.mp4"}},
                    {"type": "hologram", "data": {}}
                ],
                "toolCalls": []
            },
            "time": 1000
        }),
    );

    assert_eq!(
        contents(&accum),
        ["listen and watch\n[Audio: source: /tmp/clip.wav]\n[Video: source: /tmp/clip.mp4]"]
    );
    assert_eq!(accum.parse_warning_count, 1);
}

#[test]
fn assistant_media_parts_render() {
    let mut accum = ScanAccum::new();
    dispatch_line(
        &mut accum,
        &json!({
            "type": "context.append_loop_event",
            "event": {"type": "content.part", "part": {"type": "image_url", "imageUrl": {"url": "/tmp/chart.png"}}},
            "time": 1000
        }),
    );

    assert_eq!(contents(&accum), ["[Image: source: /tmp/chart.png]"]);
    assert_eq!(accum.parse_warning_count, 0);
}
