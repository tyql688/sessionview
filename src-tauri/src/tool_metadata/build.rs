use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::names::{canonical_tool_name, display_tool_name, parse_mcp_tool_name, tool_category};
use super::result::{compact_json_value, normalized_status, result_kind_for_tool};
use super::summary::input_summary;
use crate::models::{Provider, ToolMetadata};

pub struct ToolCallFacts<'a> {
    pub provider: Provider,
    pub raw_name: &'a str,
    pub input: Option<&'a Value>,
    pub call_id: Option<&'a str>,
    pub assistant_id: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub struct ToolResultFacts<'a> {
    pub raw_result: Option<&'a Value>,
    pub is_error: Option<bool>,
    pub status: Option<&'a str>,
    pub artifact_path: Option<&'a str>,
}

pub fn build_tool_metadata(call: ToolCallFacts<'_>) -> ToolMetadata {
    let canonical_name = canonical_tool_name(call.provider, call.raw_name);
    let display_name = display_tool_name(call.raw_name, &canonical_name);
    let mut ids = BTreeMap::new();
    if let Some(id) = call.call_id {
        ids.insert("tool_use_id".to_string(), id.to_string());
    }
    if let Some(id) = call.assistant_id {
        ids.insert("assistant_id".to_string(), id.to_string());
    }

    ToolMetadata {
        raw_name: call.raw_name.to_string(),
        canonical_name: canonical_name.clone(),
        display_name,
        category: tool_category(&canonical_name, call.raw_name),
        summary: input_summary(&canonical_name, call.raw_name, call.input),
        status: None,
        ids,
        mcp: parse_mcp_tool_name(call.raw_name),
        result_kind: None,
        structured: None,
    }
}

pub fn enrich_tool_metadata(metadata: &mut ToolMetadata, result: ToolResultFacts<'_>) {
    metadata.status = normalized_status(ToolResultFacts { ..result });
    metadata.result_kind = result_kind_for_tool(&metadata.raw_name, result.raw_result)
        .or_else(|| metadata.result_kind.clone());
    metadata.structured = result
        .raw_result
        .map(|value| {
            let mut compact = compact_json_value(value, 0);
            normalize_structured_result(&mut compact);
            compact
        })
        .or_else(|| metadata.structured.clone());
    if let Some(path) = result.artifact_path {
        let mut structured = metadata
            .structured
            .take()
            .unwrap_or_else(|| Value::Object(Map::new()));
        if !structured.is_object() {
            structured = Value::Object(Map::new());
        }
        if let Value::Object(obj) = &mut structured {
            obj.insert(
                "persistedOutputPath".to_string(),
                Value::String(path.to_string()),
            );
        }
        metadata.structured = Some(structured);
    }
}

fn normalize_structured_result(value: &mut Value) {
    let Value::Object(obj) = value else {
        return;
    };

    promote_string_alias(obj, "agent_id", "agentId");
    // Codex v2 dropped `agent_id` from spawn_agent results (upstream #17005);
    // fall back to the `new_thread_id` carried by `collab_agent_spawn_end`.
    promote_string_alias(obj, "new_thread_id", "agentId");
    promote_string_alias(obj, "task_id", "taskId");

    if obj.contains_key("persistedOutputPath") {
        return;
    }
    let path = obj
        .get("outputPath")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            obj.get("metadata")
                .and_then(|v| v.as_object())
                .and_then(|metadata| metadata.get("outputPath"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    if let Some(path) = path {
        obj.insert("persistedOutputPath".to_string(), Value::String(path));
    }
}

fn promote_string_alias(obj: &mut Map<String, Value>, from: &str, to: &str) {
    if obj.contains_key(to) {
        return;
    }
    let Some(value) = obj.get(from).cloned() else {
        return;
    };
    obj.insert(to.to_string(), value);
}

#[cfg(test)]
mod tests {
    use super::{build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts};
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
            ("spawn_agent", "Agent"),
            ("send_message", "SendMessage"),
            ("followup_task", "FollowupTask"),
            ("list_agents", "ListAgents"),
            ("request_permissions", "RequestPermissions"),
            ("todowrite", "Plan"),
            ("webfetch", "WebFetch"),
            ("websearch", "WebSearch"),
            ("codesearch", "ToolSearch"),
            ("skill", "Skill"),
            ("list", "Glob"),
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
    fn summarizes_new_tool_aliases_and_omits_large_media_fields() {
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
            Some("<omitted>")
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
            Some("<omitted>")
        );
    }

    #[test]
    fn compacts_large_structured_results() {
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
            Some("<omitted>")
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
}
