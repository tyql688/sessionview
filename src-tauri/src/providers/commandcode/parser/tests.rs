use serde_json::{Value, json};

use super::*;

const SESSION_ID: &str = "11111111-1111-4111-a111-111111111111";

fn write_transcript(path: &Path, records: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let content = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, format!("{content}\n")).unwrap();
}

fn header(version: u32) -> Value {
    json!({
        "type": "session",
        "version": version,
        "id": SESSION_ID,
        "timestamp": "2026-09-01T10:00:00Z",
        "cwd": "/tmp/demo-project",
        "parentSession": "/tmp/source-session.jsonl"
    })
}

fn session_path(root: &Path) -> PathBuf {
    root.join(format!("projects/demo/{SESSION_ID}.jsonl"))
}

fn parsed_root(path: &Path) -> ParsedSession {
    parse_session_file(path)
        .into_iter()
        .next()
        .expect("root session")
}

fn assert_active_root(parsed: &ParsedSession) {
    assert_eq!(parsed.meta.provider, Provider::CommandCode);
    assert_eq!(parsed.meta.title, "Active title");
    assert_eq!(parsed.meta.model.as_deref(), Some("provider/model-next"));
    assert_eq!(parsed.meta.git_branch.as_deref(), Some("feature/provider"));
    assert!(!parsed.meta.is_sidechain, "fork lineage is not a subagent");
    assert_eq!(parsed.meta.parent_id, None);
    assert_eq!(
        parsed.meta.input_tokens, 17,
        "all real branches consumed tokens"
    );
    assert_eq!(parsed.meta.output_tokens, 6);
    assert_eq!(parsed.meta.cache_read_tokens, 4);
    assert_eq!(parsed.meta.cache_write_tokens, 2);
    assert_eq!(parsed.usage_events.len(), 2);
    assert_eq!(parsed.child_session_ids, [format!("{SESSION_ID}:tool-1")]);
    assert!(!parsed.content_text.contains("abandoned branch"));
    assert!(!parsed.content_text.contains("discarded answer"));
    assert!(parsed.content_text.contains("data:image/png;base64,cG5n"));
    assert!(parsed.content_text.contains("[context_compacted]"));
}

fn assert_active_root_agent_tool(parsed: &ParsedSession) {
    let tool = parsed
        .messages
        .iter()
        .find(|message| message.role == MessageRole::Tool)
        .unwrap();
    assert!(tool.content.starts_with("child done"));
    assert!(tool.content.contains("<usage>total_tokens: 42"));
    assert_eq!(tool.tool_name.as_deref(), Some("Agent"));
    assert_eq!(
        tool.tool_metadata
            .as_ref()
            .map(|metadata| metadata.raw_name.as_str()),
        Some("agent")
    );
    assert_eq!(
        tool.tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.structured.as_ref())
            .and_then(|structured| structured.get("agentId"))
            .and_then(Value::as_str),
        Some("tool-1")
    );
    let assistant = parsed
        .messages
        .iter()
        .find(|message| message.role == MessageRole::Assistant)
        .unwrap();
    assert_eq!(assistant.token_usage.as_ref().unwrap().input_tokens, 10);
}

fn assert_active_child(child: &ParsedSession) {
    assert_eq!(child.meta.id, format!("{SESSION_ID}:tool-1"));
    assert_eq!(child.meta.parent_id.as_deref(), Some(SESSION_ID));
    assert!(child.meta.is_sidechain);
    assert_eq!(child.meta.variant_name.as_deref(), Some("general"));
    assert_eq!(child.meta.title, "Inspect the transcript");
    assert_eq!(child.messages.len(), 2);
    assert_eq!(
        child.messages[0].content,
        "Read the session schema and report the result"
    );
    assert!(child.messages[1].content.starts_with("child done"));
    assert!(child.usage_events.is_empty());
}

#[test]
fn parse_session_active_branch_tools_usage_title_and_lineage() {
    let temp = tempfile::tempdir().unwrap();
    let path = session_path(temp.path());
    write_transcript(
        &path,
        &[
            header(CURRENT_SESSION_VERSION),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [
                    {"type": "text", "text": "Build the provider"},
                    {"type": "image", "source": {
                        "type": "base64", "media_type": "image/png", "data": "cG5n"
                    }}
                ], "meta": {"source": "user"}}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z", "model": "provider/model-a",
                "usage": {
                    "inputTokens": 10, "outputTokens": 5,
                    "cacheReadTokens": 3, "cacheWriteTokens": 2, "costUsd": 0.25
                },
                "message": {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "Inspect the schema", "signature": "sig"},
                    {"type": "text", "text": "I will inspect it."},
                    {"type": "tool_use", "id": "tool-1", "name": "agent", "input": {
                        "description": "Inspect the transcript",
                        "subagent_type": "general",
                        "prompt": "Read the session schema and report the result"
                    }}
                ], "meta": {"source": "assistant"}}
            }),
            json!({
                "type": "message", "id": "m3", "parentId": "m2",
                "timestamp": "2026-09-01T10:00:03Z",
                "message": {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "tool-1", "content": [{
                        "type": "text",
                        "text": "child done\n\n<usage>total_tokens: 42\ntool_uses: 1\nturns: 1</usage>"
                    }]
                }], "meta": {"source": "tool"}}
            }),
            json!({
                "type": "message", "id": "x1", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:04Z",
                "message": {"role": "user", "content": [
                    {"type": "text", "text": "abandoned branch"}
                ]}
            }),
            json!({
                "type": "message", "id": "x2", "parentId": "x1",
                "timestamp": "2026-09-01T10:00:05Z", "model": "provider/model-b",
                "usage": {
                    "inputTokens": 7, "outputTokens": 1,
                    "cacheReadTokens": 1, "cacheWriteTokens": 0, "costUsd": 0.05
                },
                "message": {"role": "assistant", "content": [
                    {"type": "text", "text": "discarded answer"},
                    {"type": "tool_use", "id": "abandoned-agent", "name": "agent", "input": {
                        "description": "Abandoned child", "prompt": "Do not index this branch"
                    }}
                ]}
            }),
            json!({
                "type": "model_change", "id": "m4", "parentId": "m3",
                "timestamp": "2026-09-01T10:00:06Z", "model": "provider/model-next"
            }),
            json!({
                "type": "compaction", "id": "m5", "parentId": "m4",
                "timestamp": "2026-09-01T10:00:07Z", "summary": "Keep the parser work",
                "firstKeptEntryId": "m1", "tokensBefore": 100
            }),
            json!({
                "type": "custom_message", "id": "m6", "parentId": "m5",
                "timestamp": "2026-09-01T10:00:08Z", "customType": "review",
                "content": [{"type": "text", "text": "Check the active branch"}], "display": true
            }),
            json!({
                "type": "session_info", "id": "m7", "parentId": "m6",
                "timestamp": "2026-09-01T10:00:09Z", "name": "Active title"
            }),
        ],
    );
    std::fs::write(
        path.with_extension("meta.json"),
        r#"{"title":"Stale title","model":"stale/model","gitBranch":"feature/provider"}"#,
    )
    .unwrap();

    let sessions = parse_session_file(&path);
    assert_eq!(sessions.len(), 2);
    let parsed = &sessions[0];
    let child = &sessions[1];

    assert_active_root(parsed);
    assert_active_root_agent_tool(parsed);
    assert_active_child(child);
}

#[test]
fn parse_session_unknown_leaf_preserves_parent_chain_and_reports_damage() {
    let temp = tempfile::tempdir().unwrap();
    let path = session_path(temp.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let records = [
        header(CURRENT_SESSION_VERSION),
        json!({
            "type": "message", "id": "m1", "parentId": null,
            "timestamp": "2026-09-01T10:00:01Z",
            "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}
        }),
        json!({
            "type": "message", "id": "m2", "parentId": "m1",
            "timestamp": "2026-09-01T10:00:02Z", "model": "model-a",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "world"}]}
        }),
        json!({
            "type": "future_entry", "id": "m3", "parentId": "m2",
            "timestamp": "2026-09-01T10:00:03Z", "payload": {"keep": true}
        }),
    ];
    let mut content = records.iter().map(Value::to_string).collect::<Vec<_>>();
    content.insert(2, "{not-json".to_string());
    std::fs::write(&path, format!("{}\n", content.join("\n"))).unwrap();

    let parsed = parsed_root(&path);

    assert_eq!(parsed.messages.len(), 2);
    assert_eq!(parsed.messages[0].content, "hello");
    assert_eq!(parsed.messages[1].content, "world");
    assert!(parsed.parse_warning_count >= 2);
}

#[test]
fn parse_session_cleared_name_ignores_stale_sidecar_title() {
    let temp = tempfile::tempdir().unwrap();
    let path = session_path(temp.path());
    write_transcript(
        &path,
        &[
            header(CURRENT_SESSION_VERSION),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "Fallback title"}]}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z", "model": "model-a",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "done"}]}
            }),
            json!({
                "type": "session_info", "id": "m3", "parentId": "m2",
                "timestamp": "2026-09-01T10:00:03Z"
            }),
        ],
    );
    std::fs::write(
        path.with_extension("meta.json"),
        r#"{"title":"Old custom title"}"#,
    )
    .unwrap();

    let parsed = parsed_root(&path);
    assert_eq!(parsed.meta.title, "Fallback title");
}

#[test]
fn parse_session_uses_filename_id_and_skips_usage_without_model() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("projects/demo/filename-session-id.jsonl");
    write_transcript(
        &path,
        &[
            header(CURRENT_SESSION_VERSION),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z",
                "usage": {
                    "inputTokens": 10, "outputTokens": 5,
                    "cacheReadTokens": 3, "cacheWriteTokens": 2, "costUsd": 0.25
                },
                "message": {"role": "assistant", "content": [{"type": "text", "text": "hi"}]}
            }),
        ],
    );

    let parsed = parsed_root(&path);

    assert_eq!(parsed.meta.id, "filename-session-id");
    assert!(parsed.usage_events.is_empty());
    assert_eq!(parsed.meta.input_tokens, 0);
    assert_eq!(
        parsed.messages[1]
            .token_usage
            .as_ref()
            .unwrap()
            .input_tokens,
        10,
        "message-local usage remains visible even when stats cannot be keyed by model"
    );
    assert!(parsed.parse_warning_count >= 2);
}

#[test]
fn parse_session_rejects_newer_wire_version() {
    let temp = tempfile::tempdir().unwrap();
    let path = session_path(temp.path());
    write_transcript(&path, &[header(CURRENT_SESSION_VERSION + 1)]);

    assert!(parse_session_file(&path).is_empty());
}

#[test]
fn parse_session_materializes_parallel_inline_subagents() {
    let temp = tempfile::tempdir().unwrap();
    let path = session_path(temp.path());
    write_transcript(
        &path,
        &[
            header(CURRENT_SESSION_VERSION),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "Delegate twice"}]}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z", "model": "model-a",
                "message": {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "agent-a", "name": "agent", "input": {
                        "description": "First check", "subagent_type": "general", "prompt": "Return first"
                    }},
                    {"type": "tool_use", "id": "agent-b", "name": "agent", "input": {
                        "description": "Second check", "prompt": "Return second", "run_in_background": false
                    }}
                ]}
            }),
            json!({
                "type": "message", "id": "m3", "parentId": "m2",
                "timestamp": "2026-09-01T10:00:03Z",
                "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "agent-a", "content": [{
                        "type": "text", "text": "first done\n\n<usage>total_tokens: 10</usage>"
                    }]},
                    {"type": "tool_result", "tool_use_id": "agent-b", "content": [{
                        "type": "text", "text": "second done\n\n<usage>total_tokens: 20</usage>"
                    }]}
                ]}
            }),
        ],
    );

    let sessions = parse_session_file(&path);

    assert_eq!(sessions.len(), 3);
    assert_eq!(
        sessions[0].child_session_ids,
        [
            format!("{SESSION_ID}:agent-a"),
            format!("{SESSION_ID}:agent-b")
        ]
    );
    assert_eq!(
        sessions[1].messages[1].content.lines().next(),
        Some("first done")
    );
    assert_eq!(
        sessions[2].messages[1].content.lines().next(),
        Some("second done")
    );
    assert_eq!(sessions[2].meta.variant_name, None);
}

#[test]
fn parse_session_routes_background_agent_output_to_child() {
    let temp = tempfile::tempdir().unwrap();
    let path = session_path(temp.path());
    write_transcript(
        &path,
        &[
            header(CURRENT_SESSION_VERSION),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "Run in background"}]}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z", "model": "model-a",
                "message": {"role": "assistant", "content": [{
                    "type": "tool_use", "id": "agent-bg", "name": "agent", "input": {
                        "description": "Background check", "prompt": "Return background",
                        "run_in_background": true
                    }
                }]}
            }),
            json!({
                "type": "message", "id": "m3", "parentId": "m2",
                "timestamp": "2026-09-01T10:00:03Z",
                "message": {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "agent-bg", "content": [{
                        "type": "text", "text": "Background agent launched.\nagent_id: bg-synthetic"
                    }]
                }]}
            }),
            json!({
                "type": "message", "id": "m4", "parentId": "m3",
                "timestamp": "2026-09-01T10:00:04Z", "model": "model-a",
                "message": {"role": "assistant", "content": [{
                    "type": "tool_use", "id": "agent-output", "name": "agent_output", "input": {
                        "agent_id": "bg-synthetic", "action": "wait"
                    }
                }]}
            }),
            json!({
                "type": "message", "id": "m5", "parentId": "m4",
                "timestamp": "2026-09-01T10:00:05Z",
                "message": {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "agent-output", "content": [{
                        "type": "text", "text": "background done\n\n<usage>total_tokens: 30</usage>"
                    }]
                }]}
            }),
        ],
    );

    let sessions = parse_session_file(&path);

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[1].messages.len(), 2);
    assert!(
        sessions[1].messages[1]
            .content
            .starts_with("background done")
    );
    assert!(
        !sessions[1]
            .content_text
            .contains("Background agent launched.")
    );
    assert_eq!(sessions[1].meta.updated_at, 1_788_256_805);
}

#[test]
fn parse_session_does_not_infer_background_from_foreground_result_text() {
    let temp = tempfile::tempdir().unwrap();
    let path = session_path(temp.path());
    write_transcript(
        &path,
        &[
            header(CURRENT_SESSION_VERSION),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "Run foreground"}]}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z", "model": "model-a",
                "message": {"role": "assistant", "content": [{
                    "type": "tool_use", "id": "agent-fg", "name": "agent", "input": {
                        "description": "Foreground check", "prompt": "Echo a launch-shaped result",
                        "run_in_background": false
                    }
                }]}
            }),
            json!({
                "type": "message", "id": "m3", "parentId": "m2",
                "timestamp": "2026-09-01T10:00:03Z",
                "message": {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "agent-fg", "content": [{
                        "type": "text", "text": "Background agent launched.\nagent_id: child-authored-text"
                    }]
                }]}
            }),
        ],
    );

    let sessions = parse_session_file(&path);

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[1].messages.len(), 2);
    assert_eq!(
        sessions[1].messages[1].content,
        "Background agent launched.\nagent_id: child-authored-text"
    );
}

#[test]
fn parse_session_keeps_incomplete_typed_agent_as_prompt_only_child() {
    let temp = tempfile::tempdir().unwrap();
    let path = session_path(temp.path());
    write_transcript(
        &path,
        &[
            header(CURRENT_SESSION_VERSION),
            json!({
                "type": "message", "id": "m1", "parentId": null,
                "timestamp": "2026-09-01T10:00:01Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "Delegate"}]}
            }),
            json!({
                "type": "message", "id": "m2", "parentId": "m1",
                "timestamp": "2026-09-01T10:00:02Z", "model": "model-a",
                "message": {"role": "assistant", "content": [{
                    "type": "tool_use", "id": "agent-live", "name": "agent", "input": {
                        "description": "Still running", "prompt": "Inspect without a result"
                    }
                }]}
            }),
        ],
    );

    let sessions = parse_session_file(&path);

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[1].messages.len(), 1);
    assert_eq!(sessions[1].messages[0].content, "Inspect without a result");
    assert_eq!(sessions[1].meta.updated_at, sessions[1].meta.created_at);
}

#[test]
fn source_state_fingerprints_meta_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let path = session_path(temp.path());
    write_transcript(&path, &[header(CURRENT_SESSION_VERSION)]);
    let before = source_state(&path).unwrap();

    std::fs::write(
        path.with_extension("meta.json"),
        r#"{"title":"A sidecar-only rename"}"#,
    )
    .unwrap();
    let after = source_state(&path).unwrap();

    assert!(after.size > before.size);
    assert!(after.mtime >= before.mtime);
}
