use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::CodexProvider;
use crate::models::MessageRole;
use crate::provider::ParsedSession;

fn parse(rows: &[Value]) -> ParsedSession {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut lines = vec![
        json!({"type":"session_meta","payload":{"id":"session-test","cwd":"/tmp/project"}}),
        json!({"type":"turn_context","payload":{"turn_id":"turn-one","model":"gpt-5.4"}}),
    ];
    lines.extend_from_slice(rows);
    for row in &mut lines {
        row["timestamp"] = json!("2026-09-07T10:00:00Z");
    }
    fs::write(
        &path,
        lines
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    CodexProvider {
        home_dir: dir.path().to_path_buf(),
    }
    .parse_session_file(&path)
    .unwrap()
}

fn completed(item: Value) -> Value {
    json!({"type":"event_msg","payload":{"type":"item_completed","item":item}})
}

#[test]
fn completed_clock_sleep_preserves_duration_and_deduplicates() {
    let item = completed(
        json!({"type":"Extension","kind":"clock.sleep","id":"sleep-one","durationMs":1250}),
    );
    let parsed = parse(&[item.clone(), item]);
    assert_eq!(parsed.parse_warning_count, 0);
    assert_eq!(parsed.messages.len(), 1);
    let message = &parsed.messages[0];
    assert_eq!(message.role, MessageRole::Tool);
    assert_eq!(
        serde_json::from_str::<Value>(message.tool_input.as_deref().unwrap()).unwrap(),
        json!({"duration_ms":1250})
    );
    assert_eq!(
        message
            .tool_metadata
            .as_ref()
            .unwrap()
            .structured
            .as_ref()
            .unwrap()["durationMs"],
        1250
    );
}

#[test]
fn completed_web_search_preserves_actions_results_and_deduplicates() {
    for action in [
        json!({"type":"search","query":"sample query"}),
        json!({"type":"search","queries":["first query","second query"]}),
        json!({"type":"open_page","url":"https://example.com/page"}),
        json!({"type":"find_in_page","pattern":"sample phrase"}),
        json!({"type":"other"}),
    ] {
        let results = json!([{
            "type":"web_search_result","ref_id":"result-one","title":"Sample page",
            "url":"https://example.com/page","domain":"example.com","snippet":"Sample result"
        }]);
        let item = completed(json!({
            "type":"WebSearch","id":"web-one","query":"sample query",
            "action":action,"results":results
        }));
        let parsed = parse(&[item.clone(), item]);
        assert_eq!(parsed.parse_warning_count, 0);
        assert_eq!(parsed.messages.len(), 1);
        let message = &parsed.messages[0];
        assert_eq!(message.role, MessageRole::Tool);
        assert_eq!(message.tool_name.as_deref(), Some("WebSearch"));
        assert_eq!(message.content, "sample query");
        assert_eq!(
            serde_json::from_str::<Value>(message.tool_input.as_deref().unwrap()).unwrap(),
            action
        );
        let metadata = message.tool_metadata.as_ref().unwrap();
        assert_eq!(metadata.ids["tool_use_id"], "web-one");
        let structured = metadata.structured.as_ref().unwrap();
        assert_eq!(structured["action"], action);
        assert_eq!(structured["results"], results);
        assert!(parsed.content_text.contains("sample query"));
    }
}

#[test]
fn completed_web_search_merges_legacy_events_and_response_calls() {
    let item = completed(json!({
        "type":"WebSearch","id":"web-one","query":"sample query",
        "action":{"type":"search","query":"sample query"},"results":[]
    }));
    let event = json!({"type":"event_msg","payload":{
        "type":"web_search_end","call_id":"web-one","query":"sample query",
        "action":{"type":"search","query":"sample query"},"results":[]
    }});
    let extension = completed(json!({
        "type":"Extension","kind":"web.search","id":"web-one","query":"sample query",
        "action":{"type":"search","query":"sample query"},"results":[]
    }));
    let response = json!({"type":"response_item","payload":{
        "type":"web_search_call","id":"web-one","status":"completed",
        "action":{"type":"search","query":"sample query"}
    }});
    for rows in [
        vec![event.clone(), item.clone()],
        vec![item.clone(), event],
        vec![extension.clone(), item.clone()],
        vec![item.clone(), extension],
        vec![response, item],
    ] {
        let parsed = parse(&rows);
        assert_eq!(parsed.parse_warning_count, 0);
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].content, "sample query");
        assert_eq!(
            parsed.messages[0]
                .tool_metadata
                .as_ref()
                .unwrap()
                .structured
                .as_ref()
                .unwrap()["results"],
            json!([])
        );
    }
}

#[test]
fn image_generation_end_without_call_preserves_media_and_merges_completed_mirror() {
    let end = json!({"type":"event_msg","payload":{
        "type":"image_generation_end","call_id":"image-event","status":"completed",
        "revised_prompt":"a sample icon","saved_path":"/tmp/project/generated.png"
    }});
    let mirror = completed(json!({
        "type":"Extension","kind":"image_gen.generation","id":"image-event",
        "status":"completed","revisedPrompt":"a sample icon","savedPath":"/tmp/project/generated.png"
    }));
    for records in [
        vec![end.clone(), end.clone()],
        vec![end.clone(), mirror.clone()],
        vec![mirror, end],
    ] {
        let parsed = parse(&records);
        assert_eq!(parsed.parse_warning_count, 0);
        assert_eq!(parsed.messages.len(), 1);
        let message = &parsed.messages[0];
        assert_eq!(
            message.content,
            "[Image: source: /tmp/project/generated.png]"
        );
        assert_eq!(
            message.tool_metadata.as_ref().unwrap().status.as_deref(),
            Some("completed")
        );
        assert_eq!(
            serde_json::from_str::<Value>(message.tool_input.as_deref().unwrap()).unwrap(),
            json!({"revised_prompt":"a sample icon"})
        );
    }
}

#[test]
fn completed_nested_tools_preserve_input_output_failure_and_deduplicate() {
    let command = completed(
        json!({"type":"CommandExecution","id":"nested-command","command":["sh","-c","echo hello"],"cwd":"/tmp/project","status":"completed","stdout":"hello","stderr":"failed","exit_code":2,"duration":{"secs":1,"nanos":500000000}}),
    );
    let parsed = parse(&[
        json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"outer-call","input":"text(await tools.exec_command({cmd: 'echo hello'}));"}}),
        command.clone(),
        command,
        completed(
            json!({"type":"FileChange","id":"nested-edit","status":"completed","stdout":"Updated file","changes":{"/tmp/project/file.txt":{"type":"update","unified_diff":"@@ -1 +1 @@\n-old\n+new","move_path":null}}}),
        ),
        json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"outer-call","output":[{"type":"text","text":"outer result"}]}}),
    ]);
    assert_eq!(parsed.parse_warning_count, 0);
    assert_eq!(parsed.messages.len(), 3);
    let outer = &parsed.messages[0];
    assert_eq!(outer.tool_name.as_deref(), Some("CodeExecution"));
    assert_eq!(outer.content, "outer result");
    let command = &parsed.messages[1];
    let metadata = command.tool_metadata.as_ref().unwrap();
    assert_eq!(command.tool_name.as_deref(), Some("Bash"));
    assert_eq!(metadata.status.as_deref(), Some("error"));
    assert_eq!(
        metadata.structured.as_ref().unwrap()["durationSeconds"],
        1.5
    );
    assert_eq!(command.content, "hello");
    assert!(
        metadata
            .presentation
            .as_ref()
            .unwrap()
            .input_detail
            .as_ref()
            .unwrap()
            .lines[0]
            .value
            .contains("echo hello")
    );
    assert!(
        parsed.messages[2]
            .tool_metadata
            .as_ref()
            .unwrap()
            .structured
            .as_ref()
            .unwrap()["diff"]
            .as_str()
            .unwrap()
            .contains("+new")
    );
}

#[test]
fn completed_mcp_calls_preserve_direct_results_and_error_status() {
    let parsed = parse(&[
        completed(
            json!({"type":"McpToolCall","id":"mcp-one","server":"sample","tool":"lookup","arguments":{"query":"test"},"status":"completed","result":{"content":[{"type":"text","text":"found result"}],"isError":false}}),
        ),
        completed(
            json!({"type":"McpToolCall","id":"mcp-two","server":"sample","tool":"lookup","arguments":{},"status":"failed","result":{"content":[{"type":"text","text":"access denied"}],"isError":true}}),
        ),
        completed(
            json!({"type":"McpToolCall","id":"mcp-three","server":"sample","tool":"lookup","arguments":{},"status":"failed","error":{"message":"connection lost"}}),
        ),
    ]);
    assert_eq!(parsed.parse_warning_count, 0);
    assert_eq!(parsed.messages.len(), 3);
    assert_eq!(parsed.messages[0].content, "found result");
    assert_eq!(
        parsed.messages[0]
            .tool_metadata
            .as_ref()
            .unwrap()
            .mcp
            .as_ref()
            .unwrap()
            .server,
        "sample"
    );
    for message in &parsed.messages[1..] {
        assert_eq!(
            message.tool_metadata.as_ref().unwrap().status.as_deref(),
            Some("error")
        );
        assert!(!message.content.is_empty());
    }
}

#[test]
fn completed_media_dynamic_and_subagent_items_remain_visible() {
    let parsed = parse(&[
        completed(json!({"type":"ImageView","id":"view-one","path":"/tmp/project/preview.png"})),
        completed(
            json!({"type":"Extension","kind":"web.search","id":"web-one","query":"example","action":{"type":"search","query":"example"},"results":[]}),
        ),
        completed(
            json!({"type":"Extension","kind":"image_gen.generation","id":"image-one","status":"completed","revisedPrompt":"test image","savedPath":"/tmp/project/generated.png"}),
        ),
        completed(
            json!({"type":"DynamicToolCall","id":"dynamic-one","namespace":"functions","tool":"request_user_input_async","arguments":{"questions":[{"title":"Choose"}]},"success":true,"content_items":[{"type":"inputText","text":"requested"}]}),
        ),
        completed(
            json!({"type":"SubAgentActivity","id":"spawn-one","kind":"spawned","agent_thread_id":"child-test","agent_path":"/root/child"}),
        ),
        completed(
            json!({"type":"CollabAgentToolCall","id":"wait-one","tool":"wait","status":"completed","receiver_thread_ids":["child-test"],"agents_states":{}}),
        ),
    ]);
    assert_eq!(parsed.parse_warning_count, 0);
    assert_eq!(parsed.messages.len(), 6);
    assert_eq!(
        parsed.messages[0].content,
        "[Image: source: /tmp/project/preview.png]"
    );
    assert_eq!(
        parsed.messages[2].content,
        "[Image: source: /tmp/project/generated.png]"
    );
    assert_eq!(
        parsed.messages[3].tool_name.as_deref(),
        Some("AskUserQuestion")
    );
    assert_eq!(parsed.messages[3].content, "requested");
    assert_eq!(
        parsed.messages[4]
            .tool_metadata
            .as_ref()
            .unwrap()
            .structured
            .as_ref()
            .unwrap()["agentId"],
        "child-test"
    );
    assert_eq!(
        parsed.messages[5]
            .tool_metadata
            .as_ref()
            .unwrap()
            .structured
            .as_ref()
            .unwrap()["childConversationIds"],
        json!(["child-test"])
    );
}

#[test]
fn completed_assistant_mirrors_are_deduplicated_in_both_orders() {
    let item = completed(
        json!({"type":"AgentMessage","id":"assistant-one","content":[{"type":"text","text":"answer"}]}),
    );
    let response = json!({"type":"response_item","payload":{"type":"message","id":"assistant-one","role":"assistant","content":[{"type":"output_text","text":"answer"}]}});
    for rows in [[item.clone(), response.clone()], [response, item]] {
        let parsed = parse(&rows);
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].content, "answer");
        assert_eq!(parsed.messages[0].role, MessageRole::Assistant);
    }
}

#[test]
fn completed_function_output_mirrors_are_named_once_in_both_orders() {
    let item = completed(
        json!({"type":"FunctionCallOutput","id":"output-one","name":"automation_update","namespace":"codex_app","output":"created"}),
    );
    let response = json!({"type":"response_item","payload":{"type":"function_call_output","id":"output-one","call_id":"call-one","output":"created"}});
    for rows in [[item.clone(), response.clone()], [response, item]] {
        let parsed = parse(&rows);
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(
            parsed.messages[0].tool_name.as_deref(),
            Some("automation_update")
        );
        assert_eq!(parsed.messages[0].content, "created");
    }
}

#[test]
fn completed_reasoning_and_legacy_mirror_do_not_repeat_sections() {
    let parsed = parse(&[
        completed(
            json!({"type":"Reasoning","id":"reason-one","summary_text":[],"raw_content":["Think carefully"]}),
        ),
        json!({"type":"event_msg","payload":{"type":"agent_reasoning","text":"Think carefully"}}),
        completed(
            json!({"type":"Reasoning","id":"reason-two","summary_text":["Next step"],"raw_content":["Full internal text"]}),
        ),
    ]);
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(
        parsed.messages[0].content,
        "[thinking]\nThink carefully\n\nNext step"
    );
}

#[test]
fn malformed_and_unknown_completed_items_remain_warnings() {
    let parsed = parse(&[
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}),
        completed(json!({"type":"CommandExecution","id":"bad-command"})),
        completed(json!({"type":"McpToolCall","id":"bad-mcp","server":"sample"})),
        completed(json!({"type":"FutureTool","id":"unknown-item"})),
        completed(json!({"type":"FileChange"})),
        completed(json!({"type":"WebSearch","query":"sample query","results":[]})),
    ]);
    assert_eq!(parsed.parse_warning_count, 5);
    assert_eq!(parsed.messages.len(), 1);
}

#[test]
fn task_started_attributes_pre_context_usage_to_the_new_turn_and_model() {
    let usage =
        json!({"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"total_tokens":110});
    let parsed = parse(&[
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}),
        json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"gpt-5.5"}}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"turn-two"}}),
        json!({"type":"token_usage_record","payload":{"turn_id":"turn-two","response_id":"response-two","usage":usage}}),
        json!({"type":"turn_context","payload":{"turn_id":"turn-two","model":"gpt-5.5"}}),
        json!({"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":usage}}}),
    ]);
    assert_eq!(parsed.parse_warning_count, 0);
    assert_eq!(parsed.usage_events.len(), 1);
    assert_eq!(parsed.usage_events[0].model, "gpt-5.5");
    assert_eq!(
        parsed.usage_events[0].usage_hash.as_deref(),
        Some("codex-response:response-two")
    );
    assert_eq!(parsed.usage_events[0].input_tokens, 60);
}

#[test]
fn completed_and_legacy_command_results_merge_by_call_id() {
    let item = completed(
        json!({"type":"CommandExecution","id":"command-one","command":"echo hello","status":"completed","stdout":"hello","exit_code":0}),
    );
    let event = json!({"type":"event_msg","payload":{"type":"exec_command_end","call_id":"command-one","command":"echo hello","status":"completed","stdout":"hello","exit_code":0}});
    for rows in [[item.clone(), event.clone()], [event, item]] {
        let parsed = parse(&rows);
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].content, "hello");
        assert_eq!(parsed.parse_warning_count, 0);
    }
}

#[test]
fn goal_updates_retain_objective_status_and_usage() {
    let goal = json!({"objective":"Finish the report","status":"active","tokensUsed":50,"timeUsedSeconds":12});
    let parsed = parse(&[
        json!({"type":"event_msg","payload":{"type":"thread_goal_updated","threadId":"thread-test","goal":goal}}),
    ]);
    assert_eq!(parsed.messages.len(), 1);
    assert!(parsed.messages[0].content.contains("Finish the report"));
    assert_eq!(
        serde_json::from_str::<Value>(parsed.messages[0].content.strip_prefix("[goal]\n").unwrap())
            .unwrap(),
        goal
    );
}

#[test]
fn completed_file_additions_and_deletions_include_their_content() {
    let parsed = parse(&[completed(
        json!({"type":"FileChange","id":"files-one","status":"completed","changes":{
            "/tmp/project/added.txt":{"type":"add","content":"new line\nsecond line\n"},
            "/tmp/project/deleted.txt":{"type":"delete","content":"old line"}
        }}),
    )]);
    let metadata = parsed.messages[0].tool_metadata.as_ref().unwrap();
    let diff = metadata.structured.as_ref().unwrap()["diff"]
        .as_str()
        .unwrap();
    assert!(diff.contains("+new line\n+second line"));
    assert!(diff.contains("-old line\n\\ No newline at end of file"));
    assert_eq!(
        metadata.presentation.as_ref().unwrap().result_mode,
        crate::models::ToolResultMode::Diff
    );
}

#[test]
fn completed_command_uses_original_shell_text_for_copy_and_summary() {
    let command = "printf '%s\\n' 'hello world'";
    let argv = json!(["/bin/zsh", "-lc", command]);
    let parsed = parse(&[completed(
        json!({"type":"CommandExecution","id":"command-copy","command":argv,"parsed_cmd":[{"type":"unknown","cmd":command}],"status":"completed","stdout":"hello world\n","exit_code":0}),
    )]);
    let message = &parsed.messages[0];
    assert_eq!(
        serde_json::from_str::<Value>(message.tool_input.as_ref().unwrap()).unwrap()["command"],
        command
    );
    let metadata = message.tool_metadata.as_ref().unwrap();
    assert_eq!(metadata.summary.as_deref(), Some(command));
    assert_eq!(metadata.structured.as_ref().unwrap()["commandArgs"], argv);
}

#[test]
fn completed_compound_command_preserves_shell_operators_and_quotes() {
    let command = "echo 'hello world' && pwd";
    let parsed = parse(&[completed(
        json!({"type":"CommandExecution","id":"compound-copy","command":["/bin/zsh","-lc",command],"parsed_cmd":[{"type":"unknown","cmd":"echo 'hello world'"},{"type":"unknown","cmd":"pwd"}],"status":"completed","stdout":"hello world\n/tmp\n","exit_code":0}),
    )]);
    assert_eq!(
        parsed.messages[0]
            .tool_metadata
            .as_ref()
            .unwrap()
            .summary
            .as_deref(),
        Some(command)
    );
}
