use super::{
    attach_call_metadata, build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};
use crate::models::Provider;
use serde_json::json;

#[test]
fn maps_common_tool_aliases_to_canonical_names() {
    for (raw, canonical) in [
        ("Shell", "Bash"),
        ("exec_command", "Bash"),
        ("ReadFile", "Read"),
        ("apply_patch", "Edit"),
        ("ApplyPatch", "Edit"),
        ("EditNotebook", "Edit"),
        ("delete", "Delete"),
        ("update_plan", "Plan"),
        ("ExitPlanMode", "Plan"),
        ("ScheduleWakeup", "ScheduleWakeup"),
        ("Monitor", "Bash"),
        ("image_generation_call", "ImageGeneration"),
        ("dynamic_tool_call", "DynamicTool"),
        ("load_workspace_dependencies", "DynamicTool"),
        ("write_stdin", "Bash"),
        ("request_user_input", "AskUserQuestion"),
        ("question", "AskUserQuestion"),
        ("SemanticSearch", "Grep"),
        ("read_mcp_resource", "ListMcpResourcesTool"),
        ("list_mcp_resources", "ListMcpResourcesTool"),
        ("list_mcp_resource_templates", "ListMcpResourcesTool"),
        ("Subagent", "Agent"),
        ("subagent", "Agent"),
        ("AgentSwarm", "Agent"),
        ("spawn_agent", "Agent"),
        ("send_message", "SendMessage"),
        ("SendMessage", "SendMessage"),
        ("followup_task", "FollowupTask"),
        ("list_agents", "ListAgents"),
        ("TaskCreate", "TaskCreate"),
        ("TaskUpdate", "TaskUpdate"),
        ("Workflow", "Workflow"),
        ("StructuredOutput", "StructuredOutput"),
        ("request_permissions", "RequestPermissions"),
        ("todowrite", "Plan"),
        ("TodoList", "Plan"),
        ("FetchURL", "WebFetch"),
        ("WebFetch", "WebFetch"),
        ("WebSearch", "WebSearch"),
        ("ReadMediaFile", "ReadMediaFile"),
        ("view_image", "ReadMediaFile"),
        ("TaskList", "TaskList"),
        ("TaskOutput", "TaskOutput"),
        ("TaskStop", "TaskStop"),
        ("CronCreate", "CronCreate"),
        ("CronList", "CronList"),
        ("CronDelete", "CronDelete"),
        ("CreateGoal", "CreateGoal"),
        ("GetGoal", "GetGoal"),
        ("SetGoalBudget", "SetGoalBudget"),
        ("UpdateGoal", "UpdateGoal"),
        ("create_goal", "CreateGoal"),
        ("get_goal", "GetGoal"),
        ("set_goal_budget", "SetGoalBudget"),
        ("update_goal", "UpdateGoal"),
        ("webfetch", "WebFetch"),
        ("websearch", "WebSearch"),
        ("codesearch", "ToolSearch"),
        ("ToolSearch", "ToolSearch"),
        ("js", "JavaScript"),
        ("get_app_state", "ComputerUse"),
        ("click", "ComputerUse"),
        ("skill", "Skill"),
        ("Skill", "Skill"),
        ("list", "Glob"),
        ("ls", "Glob"),
        ("find", "Glob"),
        ("sql", "SQL"),
    ] {
        let metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Claude,
            raw_name: raw,
            input: None,
            call_id: None,
            assistant_id: None,
        });
        assert_eq!(metadata.canonical_name, canonical);
    }
}

#[test]
fn sql_tool_uses_database_category() {
    let metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "sql",
        input: None,
        call_id: None,
        assistant_id: None,
    });
    assert_eq!(metadata.canonical_name, "SQL");
    assert_eq!(metadata.category, "database");
}

#[test]
fn promotes_snake_case_ids_and_nested_output_path() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Codex,
        raw_name: "spawn_agent",
        input: None,
        call_id: Some("call_1"),
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({
                "agent_id": "agent-123",
                "task_id": "task-456",
                "metadata": {
                    "outputPath": "/tmp/tool-output.txt"
                }
            })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );

    assert_eq!(metadata.result_kind.as_deref(), Some("persisted_output"));
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("agentId"))
            .and_then(|value| value.as_str()),
        Some("agent-123")
    );
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("taskId"))
            .and_then(|value| value.as_str()),
        Some("task-456")
    );
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("persistedOutputPath"))
            .and_then(|value| value.as_str()),
        Some("/tmp/tool-output.txt")
    );
}

#[test]
fn promotes_new_thread_id_to_agent_id_alias() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Codex,
        raw_name: "spawn_agent",
        input: None,
        call_id: Some("call_x"),
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({
                "new_thread_id": "019dae0e-8a30-76f2-92cc-e81cfcf0d125",
                "new_agent_nickname": "Hume",
            })),
            is_error: Some(false),
            status: Some("success"),
            artifact_path: None,
        },
    );
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("agentId"))
            .and_then(|value| value.as_str()),
        Some("019dae0e-8a30-76f2-92cc-e81cfcf0d125"),
        "new_thread_id must populate agentId when agent_id is absent (codex >=0.123)"
    );
}

#[test]
fn preserves_call_metadata_when_result_enriches_structured() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Kimi,
        raw_name: "Bash",
        input: Some(&json!({ "command": "pwd" })),
        call_id: Some("tc_1"),
        assistant_id: None,
    });
    attach_call_metadata(
        &mut metadata,
        Some("Run pwd"),
        Some(&json!({
            "kind": "bash",
            "cwd": "/Users/alice/project",
            "command": "pwd"
        })),
        [
            ("kimi_uuid", "uuid-1".to_string()),
            ("turn_id", "turn-1".to_string()),
            ("step_uuid", "step-1".to_string()),
        ],
    );
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({ "output": "hello" })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );

    let structured = metadata.structured.as_ref().expect("structured");
    assert_eq!(
        structured.get("output").and_then(|value| value.as_str()),
        Some("hello")
    );
    assert_eq!(
        structured
            .get("callDescription")
            .and_then(|value| value.as_str()),
        Some("Run pwd")
    );
    assert_eq!(
        structured
            .get("callDisplay")
            .and_then(|value| value.get("cwd"))
            .and_then(|value| value.as_str()),
        Some("/Users/alice/project")
    );
    assert_eq!(
        metadata.ids.get("kimi_uuid").map(String::as_str),
        Some("uuid-1")
    );
    assert_eq!(
        metadata.ids.get("turn_id").map(String::as_str),
        Some("turn-1")
    );
    assert_eq!(
        metadata.ids.get("step_uuid").map(String::as_str),
        Some("step-1")
    );
}

#[test]
fn agent_input_summary_covers_codex_spawn_wait_send_close() {
    fn summary(raw: &str, input: serde_json::Value) -> Option<String> {
        let metadata = build_tool_metadata(ToolCallFacts {
            provider: Provider::Codex,
            raw_name: raw,
            input: Some(&input),
            call_id: None,
            assistant_id: None,
        });
        metadata.summary
    }

    // spawn_agent: falls back to `message` when description/prompt absent
    assert_eq!(
        summary(
            "spawn_agent",
            json!({ "message": "你负责实现 1211 角色配置", "task_name": "worker" })
        )
        .as_deref(),
        Some("你负责实现 1211 角色配置")
    );
    // wait_agent: 0 targets → no summary
    assert_eq!(
        summary("wait_agent", json!({ "targets": [], "timeout_ms": 10000 })),
        None
    );
    // wait_agent: 1 target → compact target id
    assert_eq!(
        summary(
            "wait_agent",
            json!({ "targets": ["019dae0e-8a30-76f2-92cc-e81cfcf0d125"] })
        )
        .as_deref(),
        Some("019dae0e-8a30-76f2-92cc-e81cfcf0d125")
    );
    // wait_agent: multiple targets → "N agents"
    assert_eq!(
        summary(
            "wait_agent",
            json!({ "targets": ["a", "b", "c"], "timeout_ms": 10000 })
        )
        .as_deref(),
        Some("3 agents")
    );
    // send_input / close_agent / read_agent: target id
    for raw in ["send_input", "close_agent", "read_agent"] {
        assert_eq!(
            summary(raw, json!({ "target": "019dae0e-8a30" })).as_deref(),
            Some("019dae0e-8a30"),
            "{raw} summary must come from input.target"
        );
    }
}

#[test]
fn summarizes_new_tool_aliases_and_preserves_media_fields() {
    let wakeup = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "ScheduleWakeup",
        input: Some(&json!({
            "delaySeconds": 60,
            "reason": "wait for service startup"
        })),
        call_id: None,
        assistant_id: None,
    });
    assert_eq!(wakeup.category, "cron");
    assert_eq!(
        wakeup.summary.as_deref(),
        Some("60s · wait for service startup")
    );

    let image = build_tool_metadata(ToolCallFacts {
        provider: Provider::Codex,
        raw_name: "image_generation_call",
        input: Some(&json!({ "revised_prompt": "make an icon" })),
        call_id: Some("ig_1"),
        assistant_id: None,
    });
    assert_eq!(image.category, "media");
    assert_eq!(image.summary.as_deref(), Some("make an icon"));

    let mut dynamic = build_tool_metadata(ToolCallFacts {
        provider: Provider::Codex,
        raw_name: "load_workspace_dependencies",
        input: Some(&json!({ "tool": "load_workspace_dependencies" })),
        call_id: Some("call_1"),
        assistant_id: None,
    });
    assert_eq!(dynamic.category, "tool");
    assert_eq!(
        dynamic.summary.as_deref(),
        Some("load_workspace_dependencies")
    );
    enrich_tool_metadata(
        &mut dynamic,
        ToolResultFacts {
            raw_result: Some(&json!({
                "success": true,
                "base64": "long image payload",
                "data": { "message": "keep structured data" },
                "image": "data:image/png;base64,long image payload",
                "content": "ok"
            })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );
    assert_eq!(
        dynamic
            .structured
            .as_ref()
            .and_then(|value| value.get("base64"))
            .and_then(|value| value.as_str()),
        Some("long image payload")
    );
    assert_eq!(
        dynamic
            .structured
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(|value| value.get("message"))
            .and_then(|value| value.as_str()),
        Some("keep structured data")
    );
    assert_eq!(
        dynamic
            .structured
            .as_ref()
            .and_then(|value| value.get("image"))
            .and_then(|value| value.as_str()),
        Some("data:image/png;base64,long image payload")
    );
}

#[test]
fn summarizes_recent_claude_and_codex_tool_names() {
    fn metadata(
        provider: Provider,
        raw: &str,
        input: serde_json::Value,
    ) -> crate::models::ToolMetadata {
        build_tool_metadata(ToolCallFacts {
            provider,
            raw_name: raw,
            input: Some(&input),
            call_id: None,
            assistant_id: None,
        })
    }

    let task = metadata(
        Provider::Claude,
        "TaskCreate",
        json!({ "subject": "index new sessions", "description": "scan provider logs" }),
    );
    assert_eq!(task.category, "task");
    assert_eq!(task.display_name, "task create");
    assert_eq!(task.summary.as_deref(), Some("index new sessions"));

    let structured = metadata(
        Provider::Claude,
        "StructuredOutput",
        json!({ "finding_id": "P1", "analysis": "new tool was not classified" }),
    );
    assert_eq!(structured.category, "tool");
    assert_eq!(structured.display_name, "structured output");
    assert_eq!(structured.summary.as_deref(), Some("P1"));

    let workflow = metadata(
        Provider::Claude,
        "Workflow",
        json!({ "script": "cargo test --package ccsession" }),
    );
    assert_eq!(workflow.category, "tool");
    assert_eq!(workflow.display_name, "workflow");
    assert_eq!(
        workflow.summary.as_deref(),
        Some("cargo test --package ccsession")
    );

    let node = metadata(
        Provider::Codex,
        "js",
        json!({ "title": "Inspect payload shape", "code": "await inspect()" }),
    );
    assert_eq!(node.category, "tool");
    assert_eq!(node.display_name, "node repl");
    assert_eq!(node.summary.as_deref(), Some("Inspect payload shape"));

    let computer = metadata(
        Provider::Codex,
        "press_key",
        json!({ "app": "Codex", "key": "Return" }),
    );
    assert_eq!(computer.category, "tool");
    assert_eq!(computer.display_name, "press key");
    assert_eq!(computer.summary.as_deref(), Some("Codex · key Return"));

    let goal = metadata(
        Provider::Codex,
        "create_goal",
        json!({ "objective": "finish refactor" }),
    );
    assert_eq!(goal.category, "goal");
    assert_eq!(goal.display_name, "create goal");
    assert_eq!(goal.summary.as_deref(), Some("finish refactor"));
}

#[test]
fn enriches_recent_tool_result_shapes() {
    let mut send_message = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "SendMessage",
        input: Some(&json!({ "message": "notify parent" })),
        call_id: Some("toolu_send"),
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut send_message,
        ToolResultFacts {
            raw_result: Some(&json!("sent")),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );
    assert_eq!(send_message.result_kind.as_deref(), Some("tool_output"));
    assert_eq!(
        send_message
            .structured
            .as_ref()
            .and_then(|value| value.get("output"))
            .and_then(|value| value.as_str()),
        Some("sent")
    );

    let mut task_list = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "TaskList",
        input: Some(&json!({})),
        call_id: Some("toolu_tasks"),
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut task_list,
        ToolResultFacts {
            raw_result: Some(&json!({ "tasks": [{ "id": "1", "subject": "scan tools" }] })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );
    assert_eq!(task_list.result_kind.as_deref(), Some("task_status"));

    let mut web_search = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "WebSearch",
        input: Some(&json!({ "query": "ccsession tools" })),
        call_id: Some("toolu_web"),
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut web_search,
        ToolResultFacts {
            raw_result: Some(&json!({
                "query": "ccsession tools",
                "searchCount": 1,
                "results": [{ "title": "result" }]
            })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );
    assert_eq!(web_search.result_kind.as_deref(), Some("web_result"));

    let mut skill = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "Skill",
        input: Some(&json!({ "skill": "imagegen" })),
        call_id: Some("toolu_skill"),
        assistant_id: None,
    });
    enrich_tool_metadata(
        &mut skill,
        ToolResultFacts {
            raw_result: Some(&json!({ "commandName": "imagegen", "success": true })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );
    assert_eq!(skill.result_kind.as_deref(), Some("tool_output"));
}

#[test]
fn summarizes_kimi_declared_tools() {
    fn metadata(raw: &str, input: serde_json::Value) -> crate::models::ToolMetadata {
        build_tool_metadata(ToolCallFacts {
            provider: Provider::Kimi,
            raw_name: raw,
            input: Some(&input),
            call_id: None,
            assistant_id: None,
        })
    }

    let read_media = metadata("ReadMediaFile", json!({ "path": "/Users/alice/video.mp4" }));
    assert_eq!(read_media.category, "media");
    assert_eq!(read_media.summary.as_deref(), Some("~/video.mp4"));

    let task_output = metadata(
        "TaskOutput",
        json!({ "task_id": "task-1234567890", "block": true }),
    );
    assert_eq!(task_output.category, "task");
    assert_eq!(
        task_output.summary.as_deref(),
        Some("task-1234567890 · wait")
    );

    let cron_create = metadata(
        "CronCreate",
        json!({ "cron": "*/5 * * * *", "prompt": "check build" }),
    );
    assert_eq!(cron_create.category, "cron");
    assert_eq!(
        cron_create.summary.as_deref(),
        Some("*/5 * * * * · check build")
    );

    let goal = metadata("SetGoalBudget", json!({ "value": 3, "unit": "turns" }));
    assert_eq!(goal.category, "goal");
    assert_eq!(goal.summary.as_deref(), Some("3 · turns"));

    let ask_user = metadata(
        "AskUserQuestion",
        json!({
            "questions": [
                { "id": "choice", "question": "Pick one", "header": "Choice", "options": [] }
            ],
            "background": true
        }),
    );
    assert_eq!(ask_user.category, "interaction");
    assert_eq!(
        ask_user.summary.as_deref(),
        Some("1 question(s) · background")
    );
}

#[test]
fn preserves_large_structured_results() {
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Claude,
        raw_name: "Edit",
        input: None,
        call_id: Some("toolu_1"),
        assistant_id: Some("assistant-1"),
    });
    enrich_tool_metadata(
        &mut metadata,
        ToolResultFacts {
            raw_result: Some(&json!({
                "filePath": "/repo/src/main.rs",
                "originalFile": "very large",
                "oldString": "old",
                "newString": "new",
                "structuredPatch": [{
                    "oldStart": 1,
                    "oldLines": 1,
                    "newStart": 1,
                    "newLines": 1,
                    "lines": ["-old", "+new"]
                }]
            })),
            is_error: Some(false),
            status: None,
            artifact_path: None,
        },
    );

    assert_eq!(metadata.category, "file");
    assert_eq!(metadata.result_kind.as_deref(), Some("file_patch"));
    assert_eq!(
        metadata.ids.get("tool_use_id").map(String::as_str),
        Some("toolu_1")
    );
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("originalFile"))
            .and_then(|value| value.as_str()),
        Some("very large")
    );
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("structuredPatch"))
            .and_then(|value| value.get(0))
            .and_then(|value| value.get("lines"))
            .and_then(|value| value.get(0))
            .and_then(|value| value.as_str()),
        Some("-old")
    );
}
