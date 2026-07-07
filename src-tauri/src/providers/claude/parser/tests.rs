use super::{parse_session_file, parse_session_tail};
use std::fs;
use tempfile::TempDir;

#[test]
fn parse_session_file_counts_malformed_lines_without_aborting() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let good =
        r#"{"type":"user","timestamp":"2026-04-10T10:00:00Z","message":{"content":"hello"}}"#;
    let broken = r#"{ this is not valid json "#;
    let good2 =
        r#"{"type":"user","timestamp":"2026-04-10T10:00:05Z","message":{"content":"world"}}"#;
    fs::write(&file, format!("{good}\n{broken}\n{good2}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("file-level parse must succeed");
    assert_eq!(
        parsed.messages.len(),
        2,
        "both well-formed lines should produce messages"
    );
    assert_eq!(
        parsed.parse_warning_count, 1,
        "the single broken line should be counted as one parse warning"
    );
}

#[test]
fn parse_session_file_deduplicates_same_message_request_pair() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let line = r#"{"type":"assistant","requestId":"req-1","timestamp":"2026-04-10T10:00:00Z","message":{"id":"msg-1","model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20},"content":[{"type":"text","text":"hello"}]}}"#;
    fs::write(&file, format!("{line}\n{line}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    assert_eq!(parsed.messages.len(), 1);
    let usage = parsed.messages[0].token_usage.as_ref().expect("usage");
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.cache_creation_input_tokens, 10);
    assert_eq!(usage.cache_read_input_tokens, 20);
}

#[test]
fn parse_session_file_keeps_distinct_chunks_with_same_message_request_pair() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let thinking = r#"{"type":"assistant","requestId":"req-1","uuid":"assistant-thinking","timestamp":"2026-04-10T10:00:00Z","message":{"id":"msg-1","model":"claude-opus-4-6","role":"assistant","content":[{"type":"thinking","thinking":"I should inspect a file."}]}}"#;
    let tool_use = r#"{"type":"assistant","requestId":"req-1","uuid":"assistant-tool","timestamp":"2026-04-10T10:00:01Z","message":{"id":"msg-1","model":"claude-opus-4-6","role":"assistant","content":[{"type":"tool_use","id":"toolu_same_request","name":"Read","input":{"file_path":"/Users/alice/project/src/App.tsx"}}]}}"#;
    let result = r#"{"type":"user","timestamp":"2026-04-10T10:00:02Z","sourceToolAssistantUUID":"assistant-tool","toolUseResult":{"type":"text","file":{"filePath":"/Users/alice/project/src/App.tsx","content":"export default App;","numLines":1,"startLine":1,"totalLines":1}},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_same_request","content":"1\texport default App;"}]}}"#;
    fs::write(&file, format!("{thinking}\n{tool_use}\n{result}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    assert!(
        parsed
            .messages
            .iter()
            .any(|message| message.content.starts_with("[thinking]")),
        "thinking chunk should be preserved"
    );
    let tool = parsed
        .messages
        .iter()
        .find(|message| message.role == crate::models::MessageRole::Tool)
        .expect("tool message");

    assert_eq!(tool.tool_name.as_deref(), Some("Read"));
    assert_eq!(tool.content, "1\texport default App;");
    assert_ne!(tool.tool_name.as_deref(), Some("toolu_same_request"));
}

#[test]
fn parse_session_file_matches_tool_result_that_arrives_before_tool_use() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let result = r#"{"type":"user","timestamp":"2026-04-10T10:00:00Z","sourceToolAssistantUUID":"assistant-late","toolUseResult":"Error: File has not been read yet. Read it first before writing to it.","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_late","content":"<tool_use_error>File has not been read yet. Read it first before writing to it.</tool_use_error>"}]}}"#;
    let tool_use = r#"{"type":"assistant","requestId":"req-1","uuid":"assistant-late","timestamp":"2026-04-10T10:00:01Z","message":{"id":"msg-1","model":"claude-opus-4-6","role":"assistant","content":[{"type":"tool_use","id":"toolu_late","name":"Edit","input":{"file_path":"/Users/alice/project/src/App.tsx","old_string":"old","new_string":"new"}}]}}"#;
    fs::write(&file, format!("{result}\n{tool_use}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    let tool_messages = parsed
        .messages
        .iter()
        .filter(|message| message.role == crate::models::MessageRole::Tool)
        .collect::<Vec<_>>();

    assert_eq!(tool_messages.len(), 1);
    assert_eq!(tool_messages[0].tool_name.as_deref(), Some("Edit"));
    assert!(tool_messages[0]
        .content
        .contains("File has not been read yet"));
}

#[test]
fn parse_session_file_adds_claude_tool_metadata() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let assistant = r#"{"type":"assistant","uuid":"assistant-1","timestamp":"2026-04-10T10:00:00Z","message":{"id":"msg-1","model":"claude-opus-4-6","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"ToolSearch","input":{"query":"select:TaskCreate","max_results":2}}]}}"#;
    let result = r#"{"type":"user","timestamp":"2026-04-10T10:00:01Z","sourceToolAssistantUUID":"assistant-1","toolUseResult":{"matches":[{"tool_name":"TaskCreate"}],"query":"select:TaskCreate","total_deferred_tools":1},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"TaskCreate found"}]}]}}"#;
    fs::write(&file, format!("{assistant}\n{result}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    let tool = parsed
        .messages
        .iter()
        .find(|message| message.tool_name.as_deref() == Some("ToolSearch"))
        .expect("tool message");
    let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

    assert_eq!(metadata.raw_name, "ToolSearch");
    assert_eq!(metadata.category, "search");
    assert_eq!(metadata.summary.as_deref(), Some("select:TaskCreate"));
    assert_eq!(metadata.status.as_deref(), Some("success"));
    assert_eq!(metadata.result_kind.as_deref(), Some("search_result"));
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("matches"))
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(tool.content, "TaskCreate found");
}

#[test]
fn parse_session_file_keeps_model_and_timestamp_on_usage_attached_to_tool_message() {
    // A tool_use-only assistant turn has its usage attached to the Tool
    // message. Tool messages are emitted with model=None by design, so the
    // parser must backfill the entry's model/timestamp on the usage-bearing
    // message — otherwise `compute_token_stats_dedup` silently drops the
    // usage via its "missing model" filter.
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let line = r#"{"type":"assistant","requestId":"req-1","uuid":"assistant-1","timestamp":"2026-04-21T10:00:00Z","message":{"id":"msg-1","model":"claude-opus-4-7","role":"assistant","usage":{"input_tokens":12,"output_tokens":34,"cache_creation_input_tokens":5,"cache_read_input_tokens":7},"content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/tmp/x.txt"}}]}}"#;
    fs::write(&file, format!("{line}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    let usage_msg = parsed
        .messages
        .iter()
        .find(|m| m.token_usage.is_some())
        .expect("usage-bearing message");
    assert_eq!(usage_msg.model.as_deref(), Some("claude-opus-4-7"));
    assert_eq!(usage_msg.timestamp.as_deref(), Some("2026-04-21T10:00:00Z"));
    assert_eq!(
        usage_msg.usage_hash.as_deref(),
        Some("msg-1:req-1"),
        "usage_hash must be msg:req for cross-file dedup"
    );
}

#[test]
fn parse_session_file_keeps_model_and_timestamp_on_thinking_only_turn() {
    // A turn whose only content is `thinking` produces no Assistant/Tool
    // message (thinking is emitted as System). The fallback placeholder
    // for the usage must carry the entry's model/timestamp, not a guess
    // read from adjacent messages.
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let line = r#"{"type":"assistant","requestId":"req-2","uuid":"assistant-2","timestamp":"2026-04-21T10:05:00Z","message":{"id":"msg-2","model":"claude-opus-4-7","role":"assistant","usage":{"input_tokens":3,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"content":[{"type":"thinking","thinking":"reasoning only"}]}}"#;
    fs::write(&file, format!("{line}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    let usage_msg = parsed
        .messages
        .iter()
        .find(|m| m.token_usage.is_some())
        .expect("usage-bearing message");
    assert_eq!(usage_msg.model.as_deref(), Some("claude-opus-4-7"));
    assert_eq!(usage_msg.timestamp.as_deref(), Some("2026-04-21T10:05:00Z"));
}

#[test]
fn parse_session_file_recovers_unmatched_edit_tool_result() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let result = r#"{"type":"user","timestamp":"2026-04-10T10:00:01Z","toolUseResult":{"filePath":"/project/src/App.tsx","oldString":"old","newString":"new","originalFile":"very large","structuredPatch":[],"userModified":false,"replaceAll":false},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_missing","content":"The file /project/src/App.tsx has been updated successfully."}]}}"#;
    fs::write(&file, format!("{result}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    let tool = parsed
        .messages
        .iter()
        .find(|message| message.role == crate::models::MessageRole::Tool)
        .expect("tool result");
    let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

    assert_eq!(tool.tool_name.as_deref(), Some("Edit"));
    assert_eq!(metadata.raw_name, "Edit");
    assert_eq!(metadata.result_kind.as_deref(), Some("file_patch"));
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("originalFile"))
            .and_then(|value| value.as_str()),
        Some("very large")
    );
}

#[test]
fn parse_session_file_handles_new_claude_events_and_tool_aliases() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let agent_name = r#"{"type":"agent-name","agentName":"blush-task-polling-refactor"}"#;
    let attachment = r#"{"type":"attachment","timestamp":"2026-04-25T02:03:02Z","attachment":{"type":"skill_listing","content":"skill listing noise that should not render"}}"#;
    let assistant = r##"{"type":"assistant","uuid":"assistant-1","timestamp":"2026-04-25T02:03:03Z","message":{"id":"msg-1","model":"claude-opus-4-7","role":"assistant","content":[{"type":"tool_use","id":"toolu_wakeup","name":"ScheduleWakeup","input":{"delaySeconds":60,"reason":"wait for startup"}},{"type":"tool_use","id":"toolu_monitor","name":"Monitor","input":{"command":"tail -f app.log","description":"Watch startup logs"}},{"type":"tool_use","id":"toolu_plan","name":"ExitPlanMode","input":{"plan":"# Plan\nDo it"}}]}}"##;
    let away = r#"{"type":"system","subtype":"away_summary","timestamp":"2026-04-25T02:03:04Z","content":"Work is paused."}"#;
    let scheduled = r#"{"type":"system","subtype":"scheduled_task_fire","timestamp":"2026-04-25T02:03:05Z","content":"Claude resuming /loop wakeup"}"#;
    let pr = r#"{"type":"pr-link","timestamp":"2026-04-25T02:03:06Z","prUrl":"https://github.com/example/repo/pull/7","prNumber":7}"#;
    fs::write(
        &file,
        format!("{agent_name}\n{attachment}\n{assistant}\n{away}\n{scheduled}\n{pr}\n"),
    )
    .unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    assert_eq!(parsed.meta.title, "blush-task-polling-refactor");
    assert!(
        !parsed
            .messages
            .iter()
            .any(|message| message.content.contains("skill listing noise")),
        "attachment skill listings must stay hidden"
    );

    let wakeup = parsed
        .messages
        .iter()
        .find(|message| message.tool_name.as_deref() == Some("ScheduleWakeup"))
        .expect("ScheduleWakeup tool");
    let wakeup_metadata = wakeup.tool_metadata.as_ref().expect("metadata");
    assert_eq!(wakeup_metadata.category, "cron");
    assert_eq!(
        wakeup_metadata.summary.as_deref(),
        Some("60s · wait for startup")
    );

    let monitor = parsed
        .messages
        .iter()
        .find(|message| {
            message
                .tool_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.raw_name == "Monitor")
        })
        .expect("Monitor tool");
    assert_eq!(monitor.tool_name.as_deref(), Some("Bash"));
    assert_eq!(
        monitor
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.summary.as_deref()),
        Some("Watch startup logs")
    );

    let exit_plan = parsed
        .messages
        .iter()
        .find(|message| {
            message
                .tool_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.raw_name == "ExitPlanMode")
        })
        .expect("ExitPlanMode tool");
    assert_eq!(exit_plan.tool_name.as_deref(), Some("Plan"));

    for marker in ["[away_summary]", "[scheduled_task_fire]", "[pr_link]"] {
        assert!(
            parsed
                .messages
                .iter()
                .any(|message| message.content.contains(marker)),
            "{marker} should be visible as a system event"
        );
    }
}

#[test]
fn parse_session_file_surfaces_mode_transitions_and_dedupes_them() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    // Five mode lines: normal → plan → plan (dup) → normal → accept_edits.
    // Expected emissions: [mode] plan, [mode] normal, [mode] accept_edits.
    //   - Leading `normal` is suppressed (it matches the default).
    //   - Duplicate `plan` is deduped.
    //   - Transition back to `normal` is still emitted.
    let lines = [
        r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
        r#"{"type":"user","timestamp":"2026-04-25T02:03:00Z","message":{"content":"hi"}}"#,
        r#"{"type":"mode","mode":"plan","sessionId":"s"}"#,
        r#"{"type":"mode","mode":"plan","sessionId":"s"}"#,
        r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
        r#"{"type":"mode","mode":"accept_edits","sessionId":"s"}"#,
    ];
    fs::write(&file, lines.join("\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    let mode_msgs: Vec<&str> = parsed
        .messages
        .iter()
        .filter_map(|m| {
            if m.content.starts_with("[mode]") {
                Some(m.content.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        mode_msgs,
        vec!["[mode] plan", "[mode] normal", "[mode] accept_edits"]
    );
}

#[test]
fn parse_session_file_does_not_emit_leading_mode_normal_for_default_state() {
    // The common case: session opens with `mode: normal` (the default).
    // We must NOT inject a [mode] normal System message at the top —
    // that would clutter every Claude session's timeline.
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let lines = [
        r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
        r#"{"type":"user","timestamp":"2026-04-25T02:03:00Z","message":{"content":"hi"}}"#,
        r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
    ];
    fs::write(&file, lines.join("\n")).unwrap();
    let parsed = parse_session_file(&file).expect("parsed");
    assert!(
        !parsed
            .messages
            .iter()
            .any(|m| m.content.starts_with("[mode]")),
        "no [mode] messages should appear when only `normal` was seen; got {:?}",
        parsed
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn parse_session_file_splits_local_command_input_and_output_roles() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let caveat = r#"{"type":"user","isMeta":true,"timestamp":"2026-04-25T02:02:59Z","message":{"content":"<local-command-caveat>Do not answer local command records.</local-command-caveat>"}}"#;
    let tagged_user = r#"{"type":"user","timestamp":"2026-04-25T02:03:00Z","message":{"content":"<command-name>/compact</command-name><command-args>now</command-args>"}}"#;
    let system_output = r#"{"type":"system","subtype":"local_command","timestamp":"2026-04-25T02:03:01Z","content":"<local-command-stdout>queued compaction</local-command-stdout>"}"#;
    let system_input = r#"{"type":"system","subtype":"local_command","timestamp":"2026-04-25T02:03:02Z","content":"<command-name>/reload-skills</command-name><command-message>reload-skills</command-message><command-args></command-args>"}"#;
    let user_output = r#"{"type":"user","timestamp":"2026-04-25T02:03:03Z","message":{"content":"<local-command-stderr>reload failed</local-command-stderr>"}}"#;
    let real_user = r#"{"type":"user","timestamp":"2026-04-25T02:03:04Z","message":{"content":"Actual user question"}}"#;
    fs::write(
        &file,
        format!("{caveat}\n{tagged_user}\n{system_output}\n{system_input}\n{user_output}\n{real_user}\n"),
    )
    .unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    assert_eq!(parsed.messages.len(), 5);
    assert_eq!(parsed.messages[0].role, crate::models::MessageRole::User);
    assert_eq!(
        parsed.messages[0].message_kind,
        Some(crate::models::MessageKind::CommandInput)
    );
    assert_eq!(parsed.messages[0].content, "/compact now");
    assert_eq!(
        parsed.messages[1].role,
        crate::models::MessageRole::Assistant
    );
    assert_eq!(
        parsed.messages[1].message_kind,
        Some(crate::models::MessageKind::CommandOutput)
    );
    assert_eq!(parsed.messages[1].content, "queued compaction");
    assert_eq!(parsed.messages[2].role, crate::models::MessageRole::User);
    assert_eq!(
        parsed.messages[2].message_kind,
        Some(crate::models::MessageKind::CommandInput)
    );
    assert_eq!(parsed.messages[2].content, "/reload-skills");
    assert_eq!(
        parsed.messages[3].role,
        crate::models::MessageRole::Assistant
    );
    assert_eq!(
        parsed.messages[3].message_kind,
        Some(crate::models::MessageKind::CommandOutput)
    );
    assert_eq!(parsed.messages[3].content, "reload failed");
    assert_eq!(parsed.meta.title, "Actual user question");
}

#[test]
fn parse_session_file_uses_claude_agent_type_meta_as_subagent_title() {
    let dir = TempDir::new().unwrap();
    let project_dir = dir.path().join("project");
    let parent_dir = project_dir.join("parent-session");
    let subagents_dir = parent_dir.join("subagents");
    fs::create_dir_all(&subagents_dir).unwrap();

    fs::write(
        project_dir.join("parent-session.jsonl"),
        r#"{"type":"user","timestamp":"2026-04-25T02:03:00Z","cwd":"/workspace/project","message":{"content":"Parent prompt"}}"#,
    )
    .unwrap();

    let child = subagents_dir.join("agent-a1111111111111111.jsonl");
    fs::write(
        child.with_extension("meta.json"),
        r#"{"agentType":"ws_nte2_v2"}"#,
    )
    .unwrap();
    fs::write(
        &child,
        r#"{"type":"user","timestamp":"2026-04-25T02:03:01Z","isSidechain":true,"agentId":"a1111111111111111","message":{"content":"<teammate-message teammate_id=\"team-lead\">Do the work</teammate-message>"}}"#,
    )
    .unwrap();

    let parsed = parse_session_file(&child).expect("parsed");
    assert_eq!(parsed.meta.title, "ws_nte2_v2");
    assert_eq!(parsed.meta.parent_id.as_deref(), Some("parent-session"));
}

#[test]
fn parse_session_file_handles_tool_reference_inside_tool_result_content() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let assistant = r##"{"type":"assistant","uuid":"a1","timestamp":"2026-04-25T02:03:00Z","message":{"id":"msg-1","model":"claude-opus-4-7","role":"assistant","content":[{"type":"tool_use","id":"toolu_ref","name":"ToolSearch","input":{"query":"task"}}]}}"##;
    let tool_result = r##"{"type":"user","timestamp":"2026-04-25T02:03:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_ref","content":[{"type":"tool_reference","tool_name":"TaskCreate"},{"type":"tool_reference","tool_name":"TaskUpdate"}]}]},"toolUseResult":{"matches":[],"total_deferred_tools":2}}"##;
    fs::write(&file, format!("{assistant}\n{tool_result}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    let tool_msg = parsed
        .messages
        .iter()
        .find(|m| m.tool_name.as_deref() == Some("ToolSearch"))
        .expect("tool message");
    assert!(
        tool_msg.content.contains("[Tool: TaskCreate]"),
        "tool_reference parts must render as [Tool: <name>], got {:?}",
        tool_msg.content
    );
    assert!(tool_msg.content.contains("[Tool: TaskUpdate]"));
}

#[test]
fn parse_session_file_surfaces_async_agent_status() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    let assistant = r##"{"type":"assistant","uuid":"a1","timestamp":"2026-04-25T02:03:00Z","message":{"id":"msg-1","model":"claude-opus-4-7","role":"assistant","content":[{"type":"tool_use","id":"toolu_a","name":"Task","input":{"description":"audit","prompt":"go","subagent_type":"general-purpose"}}]}}"##;
    let tool_result = r##"{"type":"user","timestamp":"2026-04-25T02:03:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_a","content":[{"type":"text","text":"launched"}]}]},"toolUseResult":{"agentId":"abc","isAsync":true,"status":"async_launched"}}"##;
    fs::write(&file, format!("{assistant}\n{tool_result}\n")).unwrap();

    let parsed = parse_session_file(&file).expect("parsed");
    let tool_msg = parsed
        .messages
        .iter()
        .find(|m| {
            m.tool_metadata
                .as_ref()
                .is_some_and(|md| md.raw_name == "Task")
        })
        .expect("Task tool message");
    let status = tool_msg
        .tool_metadata
        .as_ref()
        .and_then(|md| md.status.as_deref());
    assert_eq!(status, Some("async_launched"));
}

#[test]
fn parse_session_tail_returns_only_the_last_n_messages() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");

    let mut content = String::new();
    for i in 0..200 {
        let ts = format!("2026-04-10T10:00:{:02}Z", i % 60);
        content.push_str(&format!(
            r#"{{"type":"user","timestamp":"{ts}","message":{{"content":"msg-{i}"}}}}"#
        ));
        content.push('\n');
    }
    fs::write(&file, content).unwrap();

    let tail = parse_session_tail(&file, 20).expect("tail parse");
    assert_eq!(tail.messages.len(), 20);
    // Tail must be the LAST 20 messages — msg-180 through msg-199.
    let first = tail.messages.first().expect("first").content.clone();
    let last = tail.messages.last().expect("last").content.clone();
    assert!(
        first.ends_with("msg-180"),
        "first tail message expected to contain msg-180, got {first:?}"
    );
    assert!(
        last.ends_with("msg-199"),
        "last tail message expected to contain msg-199, got {last:?}"
    );
}

#[test]
fn parse_session_tail_falls_back_to_full_file_when_smaller_than_window() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");

    let mut content = String::new();
    for i in 0..5 {
        content.push_str(&format!(
            r#"{{"type":"user","timestamp":"2026-04-10T10:00:0{i}Z","message":{{"content":"only-{i}"}}}}"#
        ));
        content.push('\n');
    }
    fs::write(&file, content).unwrap();

    let tail = parse_session_tail(&file, 100).expect("tail parse");
    assert_eq!(
        tail.messages.len(),
        5,
        "tail must return all messages when the file is smaller than the requested window"
    );
}
