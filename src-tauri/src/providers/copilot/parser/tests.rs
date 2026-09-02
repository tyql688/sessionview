use super::*;
use crate::models::MessageRole;
use tempfile::TempDir;

/// Real-shape CLI event log: context on `session.start`, object-shaped
/// tool arguments, a completion carrying a result, and shutdown metrics
/// whose `inputTokens` are cache-inclusive.
const CLI_LOG: &str = r#"{"type":"session.start","data":{"sessionId":"11111111-1111-4111-a111-111111111111","version":1,"producer":"copilot-agent","copilotVersion":"0.0.420","startTime":"2026-03-02T15:10:04.678Z","context":{"cwd":"/home/dev/my-project","gitRoot":"/home/dev/my-project","branch":"master"}},"id":"e0","timestamp":"2026-03-02T15:10:04.817Z","parentId":null}
{"type":"user.message","data":{"content":"review my staged changes","transformedContent":"<current_datetime>noise</current_datetime>\n\nreview my staged changes","attachments":[],"interactionId":"i1"},"id":"e1","timestamp":"2026-03-02T15:10:45.058Z","parentId":"e0"}
{"type":"assistant.message","data":{"messageId":"m1","content":"I'll review the staged diff.","toolRequests":[{"toolCallId":"tooluse_1","name":"powershell","arguments":{"command":"git --no-pager diff --cached"},"type":"function"}],"reasoningText":"internal reasoning"},"id":"e2","timestamp":"2026-03-02T15:10:50.235Z","parentId":"e1"}
{"type":"tool.execution_start","data":{"toolCallId":"tooluse_1","toolName":"bash","arguments":{"command":"git --no-pager diff --cached"}},"id":"e3","timestamp":"2026-03-02T15:10:50.500Z","parentId":"e2"}
{"type":"tool.execution_complete","data":{"toolCallId":"tooluse_1","success":true,"result":"diff --git a/src/main.rs"},"id":"e4","timestamp":"2026-03-02T15:10:51.000Z","parentId":"e3"}
{"type":"assistant.message","data":{"messageId":"m2","content":"The staged diff looks good.","toolRequests":[],"reasoningText":""},"id":"e5","timestamp":"2026-03-02T15:10:55.000Z","parentId":"e4"}
{"type":"session.shutdown","data":{"shutdownType":"routine","totalPremiumRequests":2,"modelMetrics":{"claude-sonnet-4.5":{"requests":{"count":10,"cost":2},"usage":{"inputTokens":71282,"outputTokens":900,"cacheReadTokens":35495,"cacheWriteTokens":35783}}},"currentModel":"claude-sonnet-4.5"},"id":"e6","timestamp":"2026-03-06T17:08:10.988Z","parentId":"e5"}
"#;

/// Real-shape sync `task` run (Copilot CLI 1.0.82): the subagent's own
/// events carry `parentToolCallId`; its opening prompt does not; the
/// parent's `task` completion lands *before* `subagent.completed`.
const SUBAGENT_LOG: &str = r#"{"type":"session.start","data":{"sessionId":"22222222-2222-4222-a222-222222222222","copilotVersion":"1.0.82","context":{"cwd":"/home/dev/my-project","branch":"main"}},"id":"s0","timestamp":"2026-09-02T04:53:36.000Z"}
{"type":"session.model_change","data":{"newModel":"auto"},"id":"s1","timestamp":"2026-09-02T04:53:36.100Z"}
{"type":"user.message","data":{"content":"delegate the git log to a subagent"},"id":"s2","timestamp":"2026-09-02T04:53:37.000Z"}
{"type":"assistant.message","data":{"messageId":"m0","model":"mai-code-1.1-flash","content":"","toolRequests":[]},"id":"s3","timestamp":"2026-09-02T04:53:40.000Z"}
{"type":"tool.execution_start","data":{"toolCallId":"call_task1","toolName":"task","arguments":{"description":"Get recent commit subjects","prompt":"Run `git log --oneline -3` and return the subjects.","agent_type":"task","name":"recent-commits","mode":"sync"},"model":"mai-code-1.1-flash"},"id":"s4","timestamp":"2026-09-02T04:53:41.000Z"}
{"type":"subagent.started","data":{"toolCallId":"call_task1","agentName":"task","agentDisplayName":"recent-commits","agentDescription":"Get recent commit subjects","model":"claude-haiku-4.5","agentType":"task","executionMode":"sync"},"id":"s5","timestamp":"2026-09-02T04:53:41.500Z"}
{"type":"user.message","data":{"content":"Run `git log --oneline -3` and return the subjects."},"id":"s6","timestamp":"2026-09-02T04:53:42.000Z"}
{"type":"assistant.message","data":{"messageId":"m1","model":"mai-code-1.1-flash","content":"","toolRequests":[],"parentToolCallId":"call_task1"},"id":"s7","timestamp":"2026-09-02T04:53:43.000Z"}
{"type":"tool.execution_start","data":{"toolCallId":"call_bash1","toolName":"bash","arguments":{"command":"git --no-pager log --oneline -3"},"parentToolCallId":"call_task1"},"id":"s8","timestamp":"2026-09-02T04:53:44.000Z"}
{"type":"tool.execution_complete","data":{"toolCallId":"call_bash1","success":true,"result":{"content":"abc first\ndef second\n123 third"},"parentToolCallId":"call_task1"},"id":"s9","timestamp":"2026-09-02T04:53:45.000Z"}
{"type":"tool.execution_complete","data":{"toolCallId":"call_task1","success":true,"result":{"content":"abc first\ndef second\n123 third"}},"id":"s10","timestamp":"2026-09-02T04:53:46.000Z"}
{"type":"assistant.message","data":{"messageId":"m2","model":"mai-code-1.1-flash","content":"abc first\ndef second\n123 third","toolRequests":[],"parentToolCallId":"call_task1"},"id":"s11","timestamp":"2026-09-02T04:53:47.000Z"}
{"type":"subagent.completed","data":{"toolCallId":"call_task1","agentName":"task","agentDisplayName":"recent-commits","model":"claude-haiku-4.5","totalToolCalls":1,"totalTokens":36285},"id":"s12","timestamp":"2026-09-02T04:53:48.000Z"}
{"type":"assistant.message","data":{"messageId":"m3","model":"mai-code-1.1-flash","content":"The subagent reports: abc first, def second, 123 third.","toolRequests":[]},"id":"s13","timestamp":"2026-09-02T04:53:49.000Z"}
"#;

fn write_session(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let session_dir = dir.path().join("session-state").join(name);
    std::fs::create_dir_all(&session_dir).unwrap();
    let path = session_dir.join("events.jsonl");
    std::fs::write(&path, body).unwrap();
    path
}

fn parse_all(body: &str, rows: &[UsageRow]) -> Vec<ParsedSession> {
    let dir = TempDir::new().unwrap();
    let path = write_session(&dir, "sid", body);
    parse_path(&path, rows)
}

fn parse_path(path: &Path, rows: &[UsageRow]) -> Vec<ParsedSession> {
    let copilot_home = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("synthetic Copilot home");
    let state =
        source_state(path, &copilot_home.join("session-store.db")).expect("synthetic source state");
    parse_session_file(path, rows, &state)
}

fn parse_str(body: &str) -> ParsedSession {
    let mut sessions = parse_all(body, &[]);
    assert!(!sessions.is_empty(), "fixture must parse");
    sessions.remove(0)
}

fn row(
    row_id: i64,
    parent: Option<&str>,
    created_at: &str,
    input: u32,
    output: u32,
    read: u32,
    write: u32,
) -> UsageRow {
    UsageRow {
        row_id,
        parent_tool_call_id: parent.map(str::to_string),
        model: "mai-code-1.1-flash".to_string(),
        created_at: created_at.to_string(),
        usage: TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: read,
            cache_creation_input_tokens: write,
        },
    }
}

#[test]
fn cli_session_parses_surface_and_context() {
    let parsed = parse_str(CLI_LOG);
    assert_eq!(parsed.meta.id, "11111111-1111-4111-a111-111111111111");
    assert_eq!(parsed.meta.provider, Provider::Copilot);
    assert_eq!(parsed.meta.project_path, "/home/dev/my-project");
    assert_eq!(parsed.meta.project_name, "my-project");
    assert_eq!(parsed.meta.git_branch.as_deref(), Some("master"));
    assert_eq!(parsed.meta.cc_version.as_deref(), Some("0.0.420"));
    assert_eq!(parsed.meta.created_at, 1_772_464_204); // 2026-03-02T15:10:04Z
    assert!(!parsed.meta.is_sidechain);
    assert!(parsed.meta.parent_id.is_none());
    // transformedContent must never leak into the transcript.
    assert_eq!(parsed.messages[0].role, MessageRole::User);
    assert_eq!(parsed.messages[0].content, "review my staged changes");
    // Assistant reasoningText stays out; visible text stays in.
    assert!(
        parsed
            .messages
            .iter()
            .all(|m| !m.content.contains("internal reasoning"))
    );
    assert!(parsed.content_text.contains("I'll review the staged diff."));
}

#[test]
fn tool_call_pairs_with_result() {
    let parsed = parse_str(CLI_LOG);
    let tools: Vec<&Message> = parsed
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .collect();
    assert_eq!(tools.len(), 1, "announced requests don't duplicate");
    let tool = tools[0];
    assert_eq!(tool.content, "diff --git a/src/main.rs");
    let metadata = tool.tool_metadata.as_ref().unwrap();
    assert_eq!(
        metadata.ids.get("tool_use_id").map(String::as_str),
        Some("tooluse_1")
    );
    assert!(metadata.status.as_deref() != Some("error"));
}

#[test]
fn shutdown_metrics_normalize_cache_inclusive_input() {
    let parsed = parse_str(CLI_LOG);
    // 71282 input - 35495 read - 35783 write = 4 pure input.
    assert_eq!(parsed.usage_events.len(), 1);
    let event = &parsed.usage_events[0];
    assert_eq!(event.model, "claude-sonnet-4.5");
    assert_eq!(event.input_tokens, 4);
    assert_eq!(event.output_tokens, 900);
    assert_eq!(event.cache_read_input_tokens, 35_495);
    assert_eq!(event.cache_creation_input_tokens, 35_783);
    assert_eq!(event.turn_count, 10);
    assert_eq!(parsed.meta.input_tokens, 4);
    assert_eq!(parsed.meta.cache_read_tokens, 35_495);
    assert_eq!(parsed.meta.cache_write_tokens, 35_783);
}

/// Store rows replace the shutdown aggregate: per-call timestamps, the
/// same cache-inclusive normalisation, and a stable dedup hash.
#[test]
fn store_rows_replace_shutdown_usage() {
    let rows = [
        row(7, None, "2026-03-02T15:10:50.000Z", 18074, 48, 5888, 0),
        row(8, None, "2026-03-02 15:10:55", 18159, 26, 18048, 0),
    ];
    let parsed = parse_all(CLI_LOG, &rows).remove(0);
    assert_eq!(parsed.usage_events.len(), 2, "shutdown aggregate dropped");
    assert_eq!(parsed.usage_events[0].input_tokens, 12_186);
    assert_eq!(parsed.usage_events[0].cache_read_input_tokens, 5_888);
    assert_eq!(parsed.usage_events[0].turn_count, 1);
    assert_eq!(
        parsed.usage_events[0].usage_hash.as_deref(),
        Some("copilot-store:7")
    );
    // SQLite's default `datetime('now')` shape is accepted too.
    assert_eq!(parsed.usage_events[1].timestamp, "2026-03-02T15:10:55Z");
    assert_eq!(parsed.meta.input_tokens, 12_186 + 111);
    assert_eq!(parsed.meta.output_tokens, 74);
}

/// Auto mode: `session.model_change.newModel` is the literal `auto`
/// (the selection); `assistant.message.data.model` then names the model
/// the router actually used and refines the session model.
#[test]
fn auto_mode_refines_model_from_assistant_message() {
    let change = r#"{"type":"session.model_change","data":{"newModel":"auto"},"id":"a0","timestamp":"2026-03-02T15:10:44.000Z"}"#;
    let user = r#"{"type":"user.message","data":{"content":"hi"},"id":"a1","timestamp":"2026-03-02T15:10:45.058Z"}"#;
    let reply = r#"{"type":"assistant.message","data":{"messageId":"m1","model":"claude-haiku-4.5","content":"Hey!","toolRequests":[]},"id":"a2","timestamp":"2026-03-02T15:10:46.000Z"}"#;

    let no_reply = parse_str(&format!("{change}\n{user}\n"));
    assert_eq!(no_reply.meta.model.as_deref(), Some("auto"));

    let parsed = parse_str(&format!("{change}\n{user}\n{reply}\n"));
    assert_eq!(parsed.meta.model.as_deref(), Some("claude-haiku-4.5"));
    assert_eq!(
        parsed.messages[1].model.as_deref(),
        Some("claude-haiku-4.5")
    );
}

#[test]
fn subagent_events_split_into_child_session() {
    let sessions = parse_all(SUBAGENT_LOG, &[]);
    assert_eq!(sessions.len(), 2, "root + one subagent");
    let (root, child) = (&sessions[0], &sessions[1]);
    let root_id = "22222222-2222-4222-a222-222222222222";

    assert_eq!(root.meta.id, root_id);
    assert_eq!(
        root.child_session_ids,
        vec![format!("{root_id}:call_task1")]
    );
    // Parent keeps: user prompt, the Agent tool call (with its own
    // completion, which landed inside the bracket), final reply.
    let root_roles: Vec<MessageRole> = root.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        root_roles,
        vec![MessageRole::User, MessageRole::Tool, MessageRole::Assistant]
    );
    assert_eq!(
        root.messages[0].content,
        "delegate the git log to a subagent"
    );
    let agent_tool = &root.messages[1];
    assert_eq!(agent_tool.tool_name.as_deref(), Some("Agent"));
    assert_eq!(agent_tool.content, "abc first\ndef second\n123 third");
    let structured = agent_tool
        .tool_metadata
        .as_ref()
        .unwrap()
        .structured
        .as_ref()
        .unwrap();
    assert_eq!(
        structured.get("agentId").and_then(Value::as_str),
        Some("call_task1"),
        "Agent tool links to the child by task call id"
    );
    assert_eq!(root.meta.model.as_deref(), Some("mai-code-1.1-flash"));

    assert_eq!(child.meta.id, format!("{root_id}:call_task1"));
    assert_eq!(child.meta.parent_id.as_deref(), Some(root_id));
    assert!(child.meta.is_sidechain);
    assert_eq!(child.meta.title, "recent-commits");
    assert_eq!(child.meta.variant_name.as_deref(), Some("task"));
    assert_eq!(child.meta.project_path, "/home/dev/my-project");
    assert_eq!(child.meta.created_at, 1_788_324_821); // subagent.started
    let child_roles: Vec<MessageRole> = child.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(
        child_roles,
        vec![MessageRole::User, MessageRole::Tool, MessageRole::Assistant]
    );
    assert_eq!(
        child.messages[0].content,
        "Run `git log --oneline -3` and return the subjects."
    );
    assert_eq!(child.messages[1].tool_name.as_deref(), Some("Bash"));
    assert_eq!(
        child.messages[1].content,
        "abc first\ndef second\n123 third"
    );
    assert!(child.child_session_ids.is_empty());
}

/// Background mode: the user keeps talking to the parent while the
/// subagent is open. A user message that is not the task prompt stays
/// with the parent even though the bracket is still open.
#[test]
fn background_subagent_does_not_capture_parent_user_messages() {
    let log = concat!(
        r#"{"type":"session.start","data":{"sessionId":"bg"},"id":"b0","timestamp":"2026-09-02T04:00:00.000Z"}"#,
        "\n",
        r#"{"type":"user.message","data":{"content":"start a background research agent"},"id":"b1","timestamp":"2026-09-02T04:00:01.000Z"}"#,
        "\n",
        r#"{"type":"tool.execution_start","data":{"toolCallId":"toolu_bg","toolName":"task","arguments":{"prompt":"research the codebase","mode":"background","agent_type":"explore"}},"id":"b2","timestamp":"2026-09-02T04:00:02.000Z"}"#,
        "\n",
        r#"{"type":"tool.execution_complete","data":{"toolCallId":"toolu_bg","success":true,"result":"Agent started in background with agent_id: agent_00000000. You'll be notified."},"id":"b3","timestamp":"2026-09-02T04:00:03.000Z"}"#,
        "\n",
        r#"{"type":"subagent.started","data":{"toolCallId":"toolu_bg","agentName":"explore","agentDisplayName":"研究","model":"claude-haiku-4.5","executionMode":"background"},"id":"b4","timestamp":"2026-09-02T04:00:04.000Z"}"#,
        "\n",
        r#"{"type":"user.message","data":{"content":"research the codebase"},"id":"b5","timestamp":"2026-09-02T04:00:05.000Z"}"#,
        "\n",
        r#"{"type":"user.message","data":{"content":"meanwhile, what time is it?"},"id":"b6","timestamp":"2026-09-02T04:00:06.000Z"}"#,
        "\n",
        r#"{"type":"assistant.message","data":{"model":"claude-haiku-4.5","content":"About four."},"id":"b7","timestamp":"2026-09-02T04:00:07.000Z"}"#,
        "\n",
        r#"{"type":"assistant.message","data":{"model":"claude-haiku-4.5","content":"Findings: …","parentToolCallId":"toolu_bg"},"id":"b8","timestamp":"2026-09-02T04:00:08.000Z"}"#,
        "\n",
        r#"{"type":"subagent.completed","data":{"toolCallId":"toolu_bg"},"id":"b9","timestamp":"2026-09-02T04:00:09.000Z"}"#,
        "\n",
        r#"{"type":"tool.execution_start","data":{"toolCallId":"toolu_read","toolName":"read_agent","arguments":"{\"agent_id\":\"agent_00000000\",\"wait\":true}"},"id":"b10","timestamp":"2026-09-02T04:00:10.000Z"}"#,
        "\n",
        r#"{"type":"tool.execution_complete","data":{"toolCallId":"toolu_read","success":true,"result":"Agent is idle. agent_id: agent_00000000"},"id":"b11","timestamp":"2026-09-02T04:00:11.000Z"}"#,
        "\n",
    );
    let sessions = parse_all(log, &[]);
    assert_eq!(sessions.len(), 2);
    // `read_agent` (string-shaped arguments) links to the same child as
    // the spawning task, via the runtime hash the task reported.
    let read_agent = sessions[0].messages.last().unwrap();
    assert_eq!(read_agent.tool_name.as_deref(), Some("Agent"));
    let structured = read_agent
        .tool_metadata
        .as_ref()
        .unwrap()
        .structured
        .as_ref()
        .unwrap();
    assert_eq!(
        structured.get("agentId").and_then(Value::as_str),
        Some("toolu_bg")
    );
    let root_text: Vec<&str> = sessions[0]
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        root_text,
        vec![
            "start a background research agent",
            "Agent started in background with agent_id: agent_00000000. You'll be notified.",
            "meanwhile, what time is it?",
            "About four.",
            "Agent is idle. agent_id: agent_00000000",
        ]
    );
    let child_text: Vec<&str> = sessions[1]
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(child_text, vec!["research the codebase", "Findings: …"]);
    assert_eq!(sessions[1].meta.title, "研究");
    assert_eq!(sessions[1].meta.variant_name.as_deref(), Some("explore"));
    assert_eq!(sessions[1].meta.model.as_deref(), Some("claude-haiku-4.5"));
}

/// A subagent spawning its own subagent chains ids through its parent,
/// which is the shape the frontend matcher (`<parent>:<agentId>`) expects.
#[test]
fn nested_subagent_chains_ids_through_parent() {
    let log = concat!(
        r#"{"type":"session.start","data":{"sessionId":"root"},"id":"n0","timestamp":"2026-09-02T05:00:00.000Z"}"#,
        "\n",
        r#"{"type":"user.message","data":{"content":"go"},"id":"n1","timestamp":"2026-09-02T05:00:01.000Z"}"#,
        "\n",
        r#"{"type":"tool.execution_start","data":{"toolCallId":"call_outer","toolName":"task","arguments":"{\"prompt\":\"outer job\"}"},"id":"n2","timestamp":"2026-09-02T05:00:02.000Z"}"#,
        "\n",
        r#"{"type":"subagent.started","data":{"toolCallId":"call_outer","agentName":"task","agentDisplayName":"outer"},"id":"n3","timestamp":"2026-09-02T05:00:03.000Z"}"#,
        "\n",
        r#"{"type":"user.message","data":{"content":"outer job"},"id":"n4","timestamp":"2026-09-02T05:00:04.000Z"}"#,
        "\n",
        r#"{"type":"tool.execution_start","data":{"toolCallId":"call_inner","toolName":"task","arguments":{"prompt":"inner job"},"parentToolCallId":"call_outer"},"id":"n5","timestamp":"2026-09-02T05:00:05.000Z"}"#,
        "\n",
        r#"{"type":"subagent.started","data":{"toolCallId":"call_inner","agentName":"task","agentDisplayName":"inner"},"id":"n6","timestamp":"2026-09-02T05:00:06.000Z"}"#,
        "\n",
        r#"{"type":"user.message","data":{"content":"inner job"},"id":"n7","timestamp":"2026-09-02T05:00:07.000Z"}"#,
        "\n",
        r#"{"type":"assistant.message","data":{"content":"inner done","parentToolCallId":"call_inner"},"id":"n8","timestamp":"2026-09-02T05:00:08.000Z"}"#,
        "\n",
        r#"{"type":"assistant.message","data":{"content":"outer done","parentToolCallId":"call_outer"},"id":"n9","timestamp":"2026-09-02T05:00:09.000Z"}"#,
        "\n",
    );
    let sessions = parse_all(log, &[]);
    let ids: Vec<&str> = sessions.iter().map(|s| s.meta.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["root", "root:call_outer", "root:call_outer:call_inner"]
    );
    assert_eq!(
        sessions[2].meta.parent_id.as_deref(),
        Some("root:call_outer")
    );
    assert_eq!(
        sessions[1].child_session_ids,
        vec!["root:call_outer:call_inner"]
    );
    // String-shaped `arguments` still yield the prompt, so the opening
    // user message lands on the child, not the root.
    assert_eq!(sessions[0].messages.len(), 2, "root: user + Agent tool");
    assert_eq!(sessions[1].messages[0].content, "outer job");
    assert_eq!(sessions[2].messages[0].content, "inner job");
}

/// An event naming a subagent nobody announced stays on the root and
/// flags the session instead of vanishing silently.
#[test]
fn unknown_parent_tool_call_id_warns() {
    let parsed = parse_str(concat!(
        r#"{"type":"user.message","data":{"content":"hi"},"id":"k0","timestamp":"2026-03-02T15:10:45.058Z"}"#,
        "\n",
        r#"{"type":"assistant.message","data":{"content":"stray","parentToolCallId":"call_nobody"},"id":"k1","timestamp":"2026-03-02T15:10:46.000Z"}"#,
        "\n",
    ));
    assert_eq!(parsed.parse_warning_count, 1);
    assert_eq!(parsed.messages.len(), 2);
}

#[test]
fn ambiguous_opening_prompt_is_not_guessed_as_a_child() {
    let sessions = parse_all(
        concat!(
            r#"{"type":"session.start","data":{"sessionId":"root"},"id":"a0","timestamp":"2026-09-02T05:00:00.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"go"},"id":"a1","timestamp":"2026-09-02T05:00:01.000Z"}"#,
            "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"call_one","toolName":"task","arguments":{"prompt":"same prompt"}},"id":"a2","timestamp":"2026-09-02T05:00:02.000Z"}"#,
            "\n",
            r#"{"type":"subagent.started","data":{"toolCallId":"call_one","agentName":"task"},"id":"a3","timestamp":"2026-09-02T05:00:03.000Z"}"#,
            "\n",
            r#"{"type":"tool.execution_start","data":{"toolCallId":"call_two","toolName":"task","arguments":{"prompt":"same prompt"}},"id":"a4","timestamp":"2026-09-02T05:00:04.000Z"}"#,
            "\n",
            r#"{"type":"subagent.started","data":{"toolCallId":"call_two","agentName":"task"},"id":"a5","timestamp":"2026-09-02T05:00:05.000Z"}"#,
            "\n",
            r#"{"type":"user.message","data":{"content":"same prompt"},"id":"a6","timestamp":"2026-09-02T05:00:06.000Z"}"#,
            "\n",
        ),
        &[],
    );
    assert_eq!(
        sessions.len(),
        1,
        "ambiguous empty children are not materialized"
    );
    assert_eq!(sessions[0].messages.last().unwrap().content, "same prompt");
    assert_eq!(sessions[0].parse_warning_count, 1);
}

/// Store rows carrying `parent_tool_call_id` land on the child session.
#[test]
fn store_rows_route_subagent_usage_to_child() {
    let rows = [
        row(1, None, "2026-09-02T04:53:40.000Z", 18121, 68, 17408, 0),
        row(
            2,
            Some("call_task1"),
            "2026-09-02T04:53:43.000Z",
            18019,
            50,
            0,
            0,
        ),
        row(
            3,
            Some("call_task1"),
            "2026-09-02T04:53:47.000Z",
            18151,
            65,
            17920,
            0,
        ),
        row(4, None, "2026-09-02T04:53:49.000Z", 18257, 65, 18048, 0),
    ];
    let sessions = parse_all(SUBAGENT_LOG, &rows);
    assert_eq!(sessions[0].usage_events.len(), 2);
    assert_eq!(sessions[1].usage_events.len(), 2);
    assert_eq!(sessions[1].meta.input_tokens, 18_019 + 231);
    assert_eq!(sessions[1].meta.output_tokens, 115);
    assert_eq!(sessions[0].meta.input_tokens, 713 + 209);
    // Per-message: the k-th row in a scope is the k-th assistant event.
    // Root: m0 (tool-only, no message) ← row 1; m3 ← row 4.
    let root_final = &sessions[0].messages[2];
    assert_eq!(root_final.role, MessageRole::Assistant);
    let usage = root_final.token_usage.as_ref().unwrap();
    assert_eq!(
        (
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_input_tokens
        ),
        (209, 65, 18_048)
    );
    // Child: m1 (tool-only) ← row 2; m2 ← row 3.
    let child_final = &sessions[1].messages[2];
    let usage = child_final.token_usage.as_ref().unwrap();
    assert_eq!(
        (
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_input_tokens
        ),
        (231, 65, 17_920)
    );
}

#[test]
fn store_row_for_unknown_child_is_not_charged_to_root() {
    let rows = [row(
        99,
        Some("call_missing"),
        "2026-09-02T04:53:40.000Z",
        100,
        10,
        0,
        0,
    )];
    let sessions = parse_all(CLI_LOG, &rows);
    assert!(sessions[0].usage_events.is_empty());
    assert_eq!(sessions[0].meta.input_tokens, 0);
    assert_eq!(sessions[0].meta.output_tokens, 0);
    assert_eq!(sessions[0].parse_warning_count, 1);
}

/// A model mismatch means the row/message order assumption broke;
/// session totals stay, per-message usage is left unset.
#[test]
fn store_rows_with_mismatched_model_do_not_label_messages() {
    let mut rows = [
        row(1, None, "2026-09-02T04:53:40.000Z", 100, 1, 0, 0),
        row(2, None, "2026-09-02T04:53:49.000Z", 200, 2, 0, 0),
    ];
    rows[1].model = "some-other-model".to_string();
    let sessions = parse_all(SUBAGENT_LOG, &rows);
    assert_eq!(sessions[0].usage_events.len(), 2);
    assert!(sessions[0].messages[2].token_usage.is_none());
}

#[test]
fn image_attachment_resolves_binary_asset() {
    let log = concat!(
        r#"{"type":"session.start","data":{"sessionId":"img"},"id":"i0","timestamp":"2026-09-02T04:19:00.000Z"}"#,
        "\n",
        r#"{"type":"session.binary_asset","data":{"assetId":"sha256:abc","type":"image","mimeType":"image/png","byteLength":4,"data":"AAAA"},"id":"i1","timestamp":"2026-09-02T04:19:53.000Z"}"#,
        "\n",
        r#"{"type":"user.message","data":{"content":"[image: shot.png] what is this","attachments":[{"type":"file","path":"/tmp/shot.png","displayName":"shot.png","assetId":"sha256:abc","mimeType":"image/png"}]},"id":"i2","timestamp":"2026-09-02T04:19:54.000Z"}"#,
        "\n",
        r#"{"type":"user.message","data":{"content":"","attachments":[{"type":"file","displayName":"other.png","assetId":"sha256:abc"}]},"id":"i3","timestamp":"2026-09-02T04:19:55.000Z"}"#,
        "\n",
    );
    let parsed = parse_str(log);
    assert_eq!(
        parsed.messages[0].content,
        "[Image: source: data:image/png;base64,AAAA] what is this"
    );
    // Placeholder absent from the text: the marker is appended instead.
    assert_eq!(
        parsed.messages[1].content,
        "[Image: source: data:image/png;base64,AAAA]"
    );
    // Search text carries the user's words, never the payload.
    assert!(parsed.content_text.contains("what is this"));
    assert!(!parsed.content_text.contains("[image:"));
    assert!(!parsed.content_text.contains("base64,AAAA"));
    assert_eq!(parsed.meta.title, "what is this");
}

#[test]
fn attachment_only_user_message_renders_markers() {
    let parsed = parse_str(
        r#"{"type":"user.message","data":{"content":"","attachments":[{"name":"a.png"}]},"id":"u1","timestamp":"2026-03-02T15:10:45.058Z"}"#,
    );
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].content, "[Attachment]");
    assert_eq!(parsed.parse_warning_count, 1);
}

#[test]
fn malformed_asset_keeps_text_and_surfaces_attachment_marker() {
    let parsed = parse_str(concat!(
        r#"{"type":"session.binary_asset","data":{"assetId":"asset-synthetic","type":"image","data":"AAAA"},"id":"i0","timestamp":"2026-03-02T15:10:44.000Z"}"#,
        "\n",
        r#"{"type":"user.message","data":{"content":"look [image: shot.png]","attachments":[{"displayName":"shot.png","assetId":"asset-synthetic"}]},"id":"i1","timestamp":"2026-03-02T15:10:45.058Z"}"#,
        "\n",
    ));
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].content, "look [Attachment]");
    assert_eq!(parsed.meta.title, "look");
    assert_eq!(
        parsed.parse_warning_count, 2,
        "bad asset plus unresolved reference"
    );
}

#[test]
fn orphan_completion_becomes_standalone_tool_message() {
    let parsed = parse_str(
        r#"{"type":"user.message","data":{"content":"go"},"id":"o0","timestamp":"2026-03-02T15:10:45.058Z"}
{"type":"tool.execution_complete","data":{"toolCallId":"call_y","success":false,"result":"boom"},"id":"o1","timestamp":"2026-03-02T15:11:00.000Z"}"#,
    );
    let tools: Vec<&Message> = parsed
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .collect();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].content, "boom");
    assert_eq!(
        tools[0].tool_metadata.as_ref().unwrap().status.as_deref(),
        Some("error")
    );
}

#[test]
fn malformed_line_warns_but_unknown_events_do_not() {
    let parsed = parse_str(concat!(
        r#"{"type":"user.message","data":{"content":"hi"},"id":"w0","timestamp":"2026-03-02T15:10:45.058Z"}"#,
        "\nnot json\n",
        r#"{"type":"hook.start","data":{}}"#,
        "\n",
        r#"{"no-type-here":true}"#,
        "\n",
    ));
    assert_eq!(parsed.parse_warning_count, 2, "bad json + missing type");
    assert_eq!(parsed.messages.len(), 1, "unknown/lifecycle rows stay out");
}

#[test]
fn torn_final_line_is_not_a_warning() {
    let body = concat!(
        r#"{"type":"user.message","data":{"content":"hi"},"id":"z0","timestamp":"2026-03-02T15:10:45.058Z"}"#,
        "\n{\"type\":\"assistant.mess",
    );
    let parsed = parse_str(body);
    assert_eq!(parsed.parse_warning_count, 0);
    assert_eq!(parsed.messages.len(), 1);
}

#[test]
fn sidecar_title_and_cwd_fill_gaps() {
    let dir = TempDir::new().unwrap();
    let session_dir = dir.path().join("session-state").join("sid");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
            session_dir.join("workspace.yaml"),
            "id: sid\ncwd: c:\\code\\tmp\\proj\nname: 'Improve ''case'' resolution'\nsummary_count: 0\n",
        )
        .unwrap();
    let log = r#"{"type":"user.message","data":{"content":"first prompt"},"id":"s0","timestamp":"2026-03-02T15:10:45.058Z"}"#;
    std::fs::write(session_dir.join("events.jsonl"), log).unwrap();

    let parsed = parse_path(&session_dir.join("events.jsonl"), &[]).remove(0);
    assert_eq!(parsed.meta.title, "Improve 'case' resolution");
    assert_eq!(parsed.meta.project_path, "c:\\code\\tmp\\proj");
    assert_eq!(parsed.meta.project_name, "proj");
}

#[test]
fn empty_log_yields_no_session() {
    let dir = TempDir::new().unwrap();
    let path = write_session(&dir, "empty", "");
    assert!(parse_path(&path, &[]).is_empty());
}
