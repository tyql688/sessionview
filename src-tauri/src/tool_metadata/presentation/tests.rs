use serde_json::json;

use super::*;
use crate::models::{Provider, ToolResultMode};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
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
            raw_output: None,
        },
    );

    let presentation = metadata.presentation.unwrap();
    assert_eq!(presentation.icon, "💻");
    assert_eq!(presentation.result_mode, ToolResultMode::Terminal);
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
    assert!(
        presentation
            .result_detail
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .all(|line| line.label != "stdout" && line.label != "output")
    );
}

#[test]
fn raw_verdict_is_owned_by_facts_not_by_provider_writes() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Kimi,
        raw_name: "FutureTool",
        input: None,
        call_id: None,
        assistant_id: None,
    });
    let facts = |raw_output| ToolResultFacts {
        raw_result: None,
        is_error: None,
        status: None,
        artifact_path: None,
        raw_output,
    };

    enrich_tool_metadata(&mut metadata, facts(Some(true)));
    assert_eq!(
        metadata.presentation.as_ref().unwrap().result_mode,
        ToolResultMode::Raw
    );

    // A follow-up with no statement about the body keeps the verdict.
    enrich_tool_metadata(&mut metadata, facts(None));
    assert_eq!(
        metadata.presentation.as_ref().unwrap().result_mode,
        ToolResultMode::Raw
    );

    // A later readable body demotes it back to the default mode.
    enrich_tool_metadata(&mut metadata, facts(Some(false)));
    assert_eq!(
        metadata.presentation.as_ref().unwrap().result_mode,
        ToolResultMode::Output
    );
}

#[test]
fn result_detail_media_extracts_image_sources_from_structured() {
    let enriched = |raw_result: serde_json::Value| {
        let mut metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Codex,
            raw_name: "screenshot",
            input: None,
            call_id: None,
            assistant_id: None,
        });
        enrich_tool_metadata(
            &mut metadata,
            ToolResultFacts {
                raw_result: Some(&raw_result),
                is_error: None,
                status: None,
                artifact_path: None,
                raw_output: Some(false),
            },
        );
        metadata
            .presentation
            .unwrap()
            .result_detail
            .map(|detail| detail.media)
            .unwrap_or_default()
    };

    // Codex-style input_image parts; unrelated fields are never scanned.
    assert_eq!(
        enriched(json!({
            "invocation": { "image_url": "data:image/png;base64,INPUT" },
            "output": [
                { "type": "input_text", "text": "captured" },
                { "type": "input_image", "image_url": "data:image/png;base64,RESULT" }
            ]
        })),
        vec!["data:image/png;base64,RESULT".to_string()]
    );

    // MCP image data under result.Ok.content becomes a data URI.
    assert_eq!(
        enriched(json!({
            "result": {
                "Ok": { "content": [{ "type": "image", "mimeType": "image/png", "data": "AAAA" }] }
            }
        })),
        vec!["data:image/png;base64,AAAA".to_string()]
    );

    // A JSON-encoded part array is recognized; nested unrelated JSON is not.
    assert_eq!(
        enriched(json!({ "output": "[{\"image_url\":\"data:image/png;base64,AAAA\"}]" })),
        vec!["data:image/png;base64,AAAA".to_string()]
    );
    assert_eq!(
        enriched(json!({
            "metadata": { "output": [{ "type": "input_image", "image_url": "data:image/png;base64,HIDDEN" }] }
        })),
        Vec::<String>::new()
    );
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
            raw_output: None,
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
fn scalar_result_does_not_create_a_second_output_body() {
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
            raw_output: None,
        },
    );

    let presentation = metadata.presentation.as_ref().unwrap();
    assert_eq!(presentation.result_detail, None);
    assert_eq!(presentation.result_mode, ToolResultMode::Output);
}

#[test]
fn unknown_results_do_not_guess_a_lossy_preferred_field() {
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
            raw_output: None,
        },
    );

    let presentation = metadata.presentation.as_ref().unwrap();
    assert_eq!(presentation.result_detail, None);
    assert_eq!(presentation.result_mode, ToolResultMode::Output);
    assert_eq!(
        metadata.structured,
        Some(json!({ "message": "done", "count": 3 }))
    );
}

#[test]
fn builds_presentation_for_recent_tool_families() {
    fn presentation(raw_name: &str, result: Value) -> crate::models::ToolPresentation {
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
                raw_output: None,
            },
        );
        metadata.presentation.unwrap()
    }

    let agent = presentation(
        "spawn_agent",
        json!({ "agentId": "agent-1", "nickname": "worker" }),
    )
    .result_detail
    .unwrap()
    .lines;
    assert!(
        agent
            .iter()
            .any(|line| line.label == "agent" && line.value == "agent-1")
    );

    let task = presentation(
        "TaskOutput",
        json!({ "task": { "task_id": "task-1", "output": "done" } }),
    );
    assert_eq!(task.result_mode, ToolResultMode::Output);
    assert!(
        task.result_detail
            .unwrap()
            .lines
            .iter()
            .all(|line| line.label != "output")
    );

    let web_search = presentation(
        "WebSearch",
        json!({ "query": "tools", "searchCount": 1, "results": [{ "title": "hit" }] }),
    )
    .result_detail
    .unwrap()
    .lines;
    assert!(
        web_search
            .iter()
            .any(|line| line.label == "results" && line.value == "1")
    );

    let web_fetch = presentation(
        "WebFetch",
        json!({ "url": "https://example.com", "code": 200 }),
    )
    .result_detail
    .unwrap()
    .lines;
    assert!(
        web_fetch
            .iter()
            .any(|line| line.label == "code" && line.value == "200")
    );

    let question = presentation(
        "AskUserQuestion",
        json!({ "questions": [{ "question": "Ship?" }], "answers": { "ship": "yes" } }),
    )
    .result_detail
    .unwrap()
    .lines;
    assert!(
        question
            .iter()
            .any(|line| line.label == "answers" && line.value == "ship: yes")
    );

    let schedule = presentation(
        "ScheduleWakeup",
        json!({ "scheduledFor": "2026-06-14T12:00:00Z" }),
    )
    .result_detail
    .unwrap()
    .lines;
    assert!(schedule.iter().any(|line| line.label == "scheduledFor"));

    let skill = presentation(
        "Skill",
        json!({ "commandName": "imagegen", "success": true }),
    )
    .result_detail
    .unwrap()
    .lines;
    assert!(
        skill
            .iter()
            .any(|line| line.label == "command" && line.value == "imagegen")
    );

    let workflow = presentation(
        "Workflow",
        json!({ "workflowName": "audit", "summary": "ok" }),
    )
    .result_detail
    .unwrap()
    .lines;
    assert!(
        workflow
            .iter()
            .any(|line| line.label == "workflowName" && line.value == "audit")
    );
    assert!(workflow.iter().all(|line| line.label != "summary"));

    let mcp = presentation(
        "mcp__server__do_thing",
        json!({ "result": { "Ok": { "content": [{ "text": "mcp ok" }] } } }),
    );
    assert_eq!(mcp.result_detail, None);
    assert_eq!(mcp.result_mode, ToolResultMode::Output);
}

#[test]
fn ordinary_kimi_read_hides_internal_result_fields() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Kimi,
        raw_name: "Read",
        input: Some(&json!({ "path": "/tmp/output.log" })),
        call_id: None,
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({
                "note": "<system>8 lines read.</system>",
                "output": "line 1\nline 2",
                "callDescription": "Reading /tmp/output.log"
            })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
            raw_output: None,
        },
    );

    let presentation = metadata.presentation.unwrap();
    assert_eq!(presentation.result_detail, None);
    assert_eq!(presentation.result_mode, ToolResultMode::Output);
    let serialized = serde_json::to_value(presentation).unwrap();
    assert_eq!(serialized.get("resultMode"), Some(&json!("output")));
    assert!(serialized.get("output").is_none());
    assert!(serialized.get("rawOutputPolicy").is_none());
}

#[test]
fn file_mutation_content_is_not_promoted_to_common_output() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "Write",
        input: Some(&json!({ "file_path": "/tmp/example.rs", "content": "fn main() {}" })),
        call_id: None,
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({
                "type": "create",
                "filePath": "/tmp/example.rs",
                "content": "fn main() {}"
            })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
            raw_output: None,
        },
    );

    let presentation = metadata.presentation.unwrap();
    assert_eq!(presentation.result_mode, ToolResultMode::Diff);
    assert!(
        presentation
            .result_detail
            .and_then(|detail| detail.diff)
            .is_some()
    );
}

#[test]
fn structured_stream_alias_does_not_create_a_second_output_body() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Codex,
        raw_name: "javascript",
        input: Some(&json!({ "code": "1 + 1" })),
        call_id: None,
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({ "aggregatedOutput": "2" })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
            raw_output: None,
        },
    );

    let presentation = metadata.presentation.unwrap();
    assert_eq!(presentation.result_mode, ToolResultMode::Output);
    assert_eq!(presentation.result_detail, None);
}
