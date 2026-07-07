use serde_json::json;

use super::*;
use crate::models::{Provider, RawOutputPolicy, ToolLine};
use crate::tool_metadata::{
    build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

#[test]
fn builds_input_and_result_presentation_for_bash() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Codex,
        raw_name: "exec_command",
        input: Some(&json!({ "cmd": "cargo test" })),
        call_id: None,
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({ "stdout": "ok", "exitCode": 0 })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );

    let presentation = metadata.presentation.unwrap();
    assert_eq!(presentation.icon, "💻");
    assert_eq!(
        presentation.raw_output_policy,
        RawOutputPolicy::SuppressTerminal
    );
    assert_eq!(
        presentation
            .input_detail
            .as_ref()
            .unwrap()
            .lines
            .first()
            .unwrap()
            .value,
        "cargo test"
    );
    assert!(presentation
        .result_detail
        .as_ref()
        .unwrap()
        .lines
        .iter()
        .any(|line| line.label == "stdout" && line.value == "ok"));
}

#[test]
fn keeps_full_structured_output_and_patch_lines() {
    let large = "x".repeat(8_000);
    let patch_lines = (0..400)
        .map(|index| format!("+line {index}"))
        .collect::<Vec<_>>();
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "Edit",
        input: None,
        call_id: None,
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({
                "filePath": "/tmp/a.rs",
                "originalFile": large,
                "structuredPatch": [{
                    "oldStart": 1,
                    "oldLines": 0,
                    "newStart": 1,
                    "newLines": 400,
                    "lines": patch_lines
                }]
            })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );

    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("originalFile"))
            .and_then(Value::as_str)
            .map(str::len),
        Some(8_000)
    );
    let patch_diff = metadata
        .presentation
        .as_ref()
        .and_then(|presentation| presentation.result_detail.as_ref())
        .and_then(|detail| detail.patch_diff.as_ref())
        .unwrap();
    assert_eq!(patch_diff.len(), 401);
}

#[test]
fn wraps_scalar_results_as_output_detail() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "SendMessage",
        input: Some(&json!({ "message": "notify" })),
        call_id: None,
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!("sent")),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );

    assert!(metadata
        .presentation
        .as_ref()
        .and_then(|presentation| presentation.result_detail.as_ref())
        .unwrap()
        .lines
        .iter()
        .any(|line| line.label == "output" && line.value == "sent"));
}

#[test]
fn unknown_results_still_render_generic_lines() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "Frobnicate",
        input: Some(&json!({ "target": "thing" })),
        call_id: None,
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({ "message": "done", "count": 3 })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );

    let detail = metadata
        .presentation
        .as_ref()
        .and_then(|presentation| presentation.result_detail.as_ref())
        .unwrap();
    assert!(detail
        .lines
        .iter()
        .any(|line| line.label == "message" && line.value == "done"));
    assert!(detail
        .lines
        .iter()
        .any(|line| line.label == "count" && line.value == "3"));
}

#[test]
fn builds_presentation_for_recent_tool_families() {
    fn detail_lines(raw_name: &str, result: Value) -> Vec<ToolLine> {
        let mut metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Claude,
            raw_name,
            input: Some(&json!({})),
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
            .presentation
            .as_ref()
            .and_then(|presentation| presentation.result_detail.as_ref())
            .map(|detail| detail.lines.clone())
            .unwrap()
    }

    let agent = detail_lines(
        "spawn_agent",
        json!({ "agentId": "agent-1", "nickname": "worker" }),
    );
    assert!(agent
        .iter()
        .any(|line| line.label == "agent" && line.value == "agent-1"));

    let task = detail_lines(
        "TaskOutput",
        json!({ "task": { "task_id": "task-1", "output": "done" } }),
    );
    assert!(task
        .iter()
        .any(|line| line.label == "output" && line.value == "done"));

    let web_search = detail_lines(
        "WebSearch",
        json!({ "query": "tools", "searchCount": 1, "results": [{ "title": "hit" }] }),
    );
    assert!(web_search
        .iter()
        .any(|line| line.label == "results" && line.value == "1"));

    let web_fetch = detail_lines(
        "WebFetch",
        json!({ "url": "https://example.com", "code": 200 }),
    );
    assert!(web_fetch
        .iter()
        .any(|line| line.label == "code" && line.value == "200"));

    let question = detail_lines(
        "AskUserQuestion",
        json!({ "questions": [{ "question": "Ship?" }], "answers": { "ship": "yes" } }),
    );
    assert!(question
        .iter()
        .any(|line| line.label == "answers" && line.value == "ship: yes"));

    let schedule = detail_lines(
        "ScheduleWakeup",
        json!({ "scheduledFor": "2026-06-14T12:00:00Z" }),
    );
    assert!(schedule.iter().any(|line| line.label == "scheduledFor"));

    let skill = detail_lines(
        "Skill",
        json!({ "commandName": "imagegen", "success": true }),
    );
    assert!(skill
        .iter()
        .any(|line| line.label == "command" && line.value == "imagegen"));

    let workflow = detail_lines(
        "Workflow",
        json!({ "workflowName": "audit", "summary": "ok" }),
    );
    assert!(workflow
        .iter()
        .any(|line| line.label == "summary" && line.value == "ok"));

    let mcp = detail_lines(
        "mcp__server__do_thing",
        json!({ "result": { "Ok": { "content": [{ "text": "mcp ok" }] } } }),
    );
    assert!(mcp
        .iter()
        .any(|line| line.label == "server" && line.value == "server"));
    assert!(mcp
        .iter()
        .any(|line| line.label == "output" && line.value == "mcp ok"));
}
