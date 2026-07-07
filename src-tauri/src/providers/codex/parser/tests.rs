use super::{parse_session_tail, CodexProvider};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn parse_session_surfaces_top_level_compacted_handoff_summary() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-1\",\"cwd\":\"/tmp\",\"cli_version\":\"0.123.0\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"compacted\",\"payload\":{\"message\":\"Recap so far: did X and Y.\",\"replacement_history\":[]}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"after compaction\"}]}}\n"
        ),
    )
    .unwrap();
    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    let compacted = parsed
        .messages
        .iter()
        .find(|m| m.content.contains("[context_compacted]"))
        .expect("compacted system event");
    assert!(
        compacted.content.contains("Recap so far: did X and Y."),
        "compacted handoff summary missing from {:?}",
        compacted.content
    );
}

#[test]
fn parse_session_skips_usage_event_with_no_resolvable_model() {
    // No turn_context, no info.model — resolved_model is None. We
    // must NOT fabricate "gpt-5"; we drop the usage event entirely.
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"sess-2\",\"cwd\":\"/tmp\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":0,\"output_tokens\":10,\"reasoning_output_tokens\":0,\"total_tokens\":110}}}}\n"
        ),
    )
    .unwrap();
    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    assert!(
        parsed.usage_events.is_empty(),
        "must NOT fabricate a model name when none resolves; got {:?}",
        parsed.usage_events
    );
    // Both paths must skip together: assistant message also gets no
    // phantom usage stamp when the model is unresolvable.
    let assistant = parsed
        .messages
        .iter()
        .find(|m| m.role == crate::models::MessageRole::Assistant)
        .expect("assistant message");
    assert!(
        assistant.token_usage.is_none(),
        "assistant must not carry usage with no resolved model; got {:?}",
        assistant.token_usage
    );
}

#[test]
fn parse_session_collects_usage_events_keeping_total_input_and_cached_input() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    let events = &parsed.usage_events;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].model, "gpt-5.4");
    assert_eq!(events[0].input_tokens, 1000);
    assert_eq!(events[0].cache_read_input_tokens, 600);
    assert_eq!(events[0].output_tokens, 50);
}

#[test]
fn parse_session_prefers_last_token_usage_when_both_last_and_total_are_present() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1700,\"cached_input_tokens\":1000,\"output_tokens\":70,\"reasoning_output_tokens\":25,\"total_tokens\":1770},\"last_token_usage\":{\"input_tokens\":700,\"cached_input_tokens\":400,\"output_tokens\":20,\"reasoning_output_tokens\":0,\"total_tokens\":720}}}}\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    let events = &parsed.usage_events;
    // E1, E2, E3 have distinct timestamps, so none are exact duplicates;
    // each contributes its last_token_usage.
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].input_tokens, 1000);
    assert_eq!(events[0].cache_read_input_tokens, 600);
    assert_eq!(events[0].output_tokens, 50);
    assert_eq!(events[1].input_tokens, 1000);
    assert_eq!(events[1].cache_read_input_tokens, 600);
    assert_eq!(events[1].output_tokens, 50);
    assert_eq!(events[2].input_tokens, 700);
    assert_eq!(events[2].cache_read_input_tokens, 400);
    assert_eq!(events[2].output_tokens, 20);
}

#[test]
fn parse_session_file_accumulates_repeated_last_token_usage() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:03Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    let assistant = parsed
        .messages
        .iter()
        .find(|message| message.role == crate::models::MessageRole::Assistant)
        .expect("assistant message");
    let usage = assistant.token_usage.as_ref().expect("token usage");

    assert_eq!(assistant.model.as_deref(), Some("gpt-5.4"));
    // E1 and E2 have distinct timestamps (not exact duplicates), so both
    // fold onto the assistant message.
    assert_eq!(usage.input_tokens, 2000);
    assert_eq!(usage.cache_read_input_tokens, 1200);
    assert_eq!(usage.output_tokens, 100);
}

#[test]
fn parse_session_dedups_token_count_events_with_identical_timestamp_and_usage() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    // A and B are verbatim re-emits (same timestamp AND same usage), so B is
    // dropped. C is a distinct turn at a later timestamp.
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050},\"last_token_usage\":{\"input_tokens\":1000,\"cached_input_tokens\":600,\"output_tokens\":50,\"reasoning_output_tokens\":25,\"total_tokens\":1050}}}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1700,\"cached_input_tokens\":1000,\"output_tokens\":70,\"reasoning_output_tokens\":25,\"total_tokens\":1770},\"last_token_usage\":{\"input_tokens\":700,\"cached_input_tokens\":400,\"output_tokens\":20,\"reasoning_output_tokens\":0,\"total_tokens\":720}}}}\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    let events = &parsed.usage_events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].input_tokens, 1000);
    assert_eq!(events[0].cache_read_input_tokens, 600);
    assert_eq!(events[1].input_tokens, 700);
    assert_eq!(events[1].cache_read_input_tokens, 400);
}

#[test]
fn parse_session_file_counts_malformed_lines_without_aborting() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"first\"}}\n",
            "{ this is not valid json\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"second\"}}\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    assert_eq!(
        parsed.parse_warning_count, 1,
        "the single malformed line must be counted"
    );
    // The two well-formed user events should still produce messages.
    assert!(
        parsed.messages.len() >= 2,
        "well-formed lines must still produce messages; got {}",
        parsed.messages.len()
    );
}

#[test]
fn parse_session_file_emits_tool_metadata_for_web_search_end_event() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"search docs\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"web_search_end\",\"call_id\":\"ws_123\",\"query\":\"notify kqueue\",\"action\":{\"type\":\"search\",\"query\":\"notify kqueue\"}}}\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    let tool = parsed
        .messages
        .iter()
        .find(|message| message.tool_metadata.is_some())
        .expect("web search tool message");
    let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

    assert_eq!(tool.tool_name.as_deref(), Some("WebSearch"));
    assert_eq!(tool.content, "notify kqueue");
    assert_eq!(metadata.raw_name, "web_search_call");
    assert_eq!(metadata.canonical_name, "WebSearch");
    assert_eq!(metadata.status.as_deref(), Some("success"));
    assert_eq!(metadata.summary.as_deref(), Some("notify kqueue"));
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("action"))
            .and_then(|value| value.get("query"))
            .and_then(|value| value.as_str()),
        Some("notify kqueue")
    );
}

#[test]
fn parse_session_file_merges_exec_command_end_into_existing_tool_message() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"pwd\\\"}\",\"call_id\":\"exec_123\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"exec_123\",\"output\":\"{\\\"output\\\":\\\"/tmp/project\\n\\\",\\\"metadata\\\":{\\\"exit_code\\\":0,\\\"duration_seconds\\\":0.2}}\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"exec_command_end\",\"call_id\":\"exec_123\",\"process_id\":\"42\",\"turn_id\":\"turn_1\",\"command\":[\"pwd\"],\"cwd\":\"/tmp/project\",\"parsed_cmd\":[],\"source\":\"agent\",\"stdout\":\"/tmp/project\\n\",\"stderr\":\"\",\"aggregated_output\":\"/tmp/project\\n\",\"exit_code\":0,\"duration\":{\"secs\":1,\"nanos\":500000000},\"formatted_output\":\"/tmp/project\\n\",\"status\":\"completed\"}}\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    let tool = parsed
        .messages
        .iter()
        .find(|message| message.tool_name.as_deref() == Some("Bash"))
        .expect("bash tool message");
    let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

    assert_eq!(tool.content, "/tmp/project\n");
    assert_eq!(metadata.status.as_deref(), Some("completed"));
    assert_eq!(metadata.result_kind.as_deref(), Some("terminal_output"));
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("cwd"))
            .and_then(|value| value.as_str()),
        Some("/tmp/project")
    );
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("source"))
            .and_then(|value| value.as_str()),
        Some("agent")
    );
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("exitCode"))
            .and_then(|value| value.as_i64()),
        Some(0)
    );
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("durationSeconds"))
            .and_then(|value| value.as_f64()),
        Some(1.5)
    );
}

#[test]
fn parse_session_file_merges_patch_apply_end_into_existing_tool_message() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call\",\"status\":\"completed\",\"call_id\":\"patch_123\",\"name\":\"apply_patch\",\"input\":\"*** Begin Patch\\n*** Update File: src/file.rs\\n@@\\n-old\\n+new\\n*** End Patch\\n\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"custom_tool_call_output\",\"call_id\":\"patch_123\",\"output\":\"{\\\"output\\\":\\\"Success. Updated the following files:\\\\nM src/file.rs\\\\n\\\",\\\"metadata\\\":{\\\"exit_code\\\":0,\\\"duration_seconds\\\":0.0}}\"}}\n",
            "{\"timestamp\":\"2026-04-10T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"patch_apply_end\",\"call_id\":\"patch_123\",\"turn_id\":\"turn_1\",\"stdout\":\"Success. Updated the following files:\\nM src/file.rs\\n\",\"stderr\":\"\",\"success\":true,\"changes\":{\"src/file.rs\":{\"type\":\"update\",\"unified_diff\":\"@@ -1 +1 @@\\n-old\\n+new\\n\",\"move_path\":null}},\"status\":\"completed\"}}\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    let tool = parsed
        .messages
        .iter()
        .find(|message| message.tool_name.as_deref() == Some("Edit"))
        .expect("apply patch tool message");
    let metadata = tool.tool_metadata.as_ref().expect("tool metadata");

    assert_eq!(metadata.status.as_deref(), Some("completed"));
    assert_eq!(metadata.result_kind.as_deref(), Some("file_patch"));
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("diff"))
            .and_then(|value| value.as_str())
            .map(|value| value.contains("*** Update File: src/file.rs")),
        Some(true)
    );
    assert_eq!(
        metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("patches"))
            .and_then(|value| value.as_array())
            .and_then(|patches| patches.first())
            .and_then(|patch| patch.get("files"))
            .and_then(|value| value.as_array())
            .and_then(|files| files.first())
            .and_then(|value| value.as_str()),
        Some("src/file.rs")
    );
}

#[test]
fn parse_session_file_handles_recent_codex_events_without_base64_output() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    fs::write(
        &file,
        concat!(
            "{\"timestamp\":\"2026-04-28T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-1\",\"cwd\":\"/tmp/project\"}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:01Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"thread_name_updated\",\"thread_id\":\"thread-1\",\"thread_name\":\"Generated image task\"}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"see image\",\"local_images\":[\"data:image/png;base64,USER_IMAGE\"]}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"missing_image\",\"output\":\"[{\\\"detail\\\":\\\"original\\\",\\\"image_url\\\":\\\"data:image/png;base64,TOOL_IMAGE\\\"}]\"}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"image_generation_call\",\"id\":\"ig_1\",\"status\":\"generating\",\"revised_prompt\":\"make an icon\"}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"image_generation_end\",\"call_id\":\"ig_1\",\"status\":\"completed\",\"revised_prompt\":\"make an icon\",\"saved_path\":\"/Users/alice/.codex/generated_images/ig_1.png\",\"base64\":\"SHOULD_NOT_APPEAR\"}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:05Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"dynamic_tool_call_request\",\"callId\":\"dyn_1\",\"tool\":\"load_workspace_dependencies\",\"arguments\":{}}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:06Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"dynamic_tool_call_response\",\"call_id\":\"dyn_1\",\"tool\":\"load_workspace_dependencies\",\"arguments\":{},\"content_items\":[{\"type\":\"inputText\",\"text\":\"Workspace dependencies are available\"}],\"success\":true,\"error\":null,\"duration\":{\"secs\":0,\"nanos\":1000000}}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:07Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"item_completed\",\"item\":{\"type\":\"Plan\",\"id\":\"plan_1\",\"text\":\"# Plan\\n- Do the work\"}}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:08Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"error\",\"message\":\"unexpected status 502 Bad Gateway\",\"codex_error_info\":\"other\"}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:09Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_aborted\",\"turn_id\":\"turn_1\",\"reason\":\"interrupted\",\"duration_ms\":1500}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:10Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"context_compacted\"}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:11Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"send_input\",\"arguments\":\"{\\\"target\\\":\\\"agent-1\\\",\\\"message\\\":\\\"continue\\\"}\",\"call_id\":\"send_1\"}}\n",
            "{\"timestamp\":\"2026-04-28T10:00:12Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"collab_agent_interaction_end\",\"call_id\":\"send_1\",\"status\":\"completed\",\"receiver_thread_id\":\"agent-1\"}}\n"
        ),
    )
    .unwrap();

    let provider = CodexProvider {
        home_dir: PathBuf::from("/tmp"),
    };
    let parsed = provider.parse_session_file(&file).expect("parsed session");
    assert_eq!(parsed.meta.title, "Generated image task");
    assert!(
        !parsed
            .messages
            .iter()
            .any(|message| message.content.contains(";base64,")),
        "base64 image payloads should not be stored in message content"
    );

    let image = parsed
        .messages
        .iter()
        .find(|message| message.tool_name.as_deref() == Some("ImageGeneration"))
        .expect("image generation tool");
    assert_eq!(
        image.content,
        "[Image: source: /Users/alice/.codex/generated_images/ig_1.png]"
    );
    assert!(!image.content.contains("SHOULD_NOT_APPEAR"));
    let image_metadata = image.tool_metadata.as_ref().expect("image metadata");
    assert_eq!(image_metadata.category, "media");
    assert_eq!(image_metadata.result_kind.as_deref(), Some("image"));
    assert_eq!(
        image_metadata
            .structured
            .as_ref()
            .and_then(|value| value.get("savedPath"))
            .and_then(|value| value.as_str()),
        Some("/Users/alice/.codex/generated_images/ig_1.png")
    );

    let dynamic = parsed
        .messages
        .iter()
        .find(|message| {
            message
                .tool_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.raw_name == "load_workspace_dependencies")
        })
        .expect("dynamic tool");
    assert_eq!(dynamic.tool_name.as_deref(), Some("DynamicTool"));
    assert_eq!(dynamic.content, "Workspace dependencies are available");
    assert_eq!(
        dynamic
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.status.as_deref()),
        Some("success")
    );

    assert!(
        parsed.messages.iter().any(
            |message| message.role == crate::models::MessageRole::Assistant
                && message.content.starts_with("# Plan")
        ),
        "Plan item should be emitted as a visible assistant message"
    );
    for marker in ["[error]", "[turn_aborted]", "[context_compacted]"] {
        assert!(
            parsed
                .messages
                .iter()
                .any(|message| message.content.contains(marker)),
            "{marker} should be visible as a system event"
        );
    }

    let send_input = parsed
        .messages
        .iter()
        .find(|message| {
            message
                .tool_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.raw_name == "send_input")
        })
        .expect("send_input tool");
    assert_eq!(
        send_input
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.status.as_deref()),
        Some("completed")
    );
    assert_eq!(
        send_input
            .tool_metadata
            .as_ref()
            .and_then(|metadata| metadata.structured.as_ref())
            .and_then(|value| value.get("receiver_thread_id"))
            .and_then(|value| value.as_str()),
        Some("agent-1")
    );
}

#[test]
fn parse_session_tail_returns_only_the_last_n_messages() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    let mut content = String::new();
    // Leading turn_context so the model is non-None for the bulk
    // of the file (matches real-world Codex JSONL layout).
    content.push_str(
        "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
    );
    for i in 0..200 {
        let ts = format!("2026-04-10T10:00:{:02}Z", i % 60);
        content.push_str(&format!(
            "{{\"timestamp\":\"{ts}\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"msg-{i}\"}}]}}}}\n"
        ));
    }
    fs::write(&file, content).unwrap();

    let tail = parse_session_tail(&file, 20).expect("tail parse");
    assert_eq!(tail.messages.len(), 20);
    let first = tail.messages.first().expect("first").content.clone();
    let last = tail.messages.last().expect("last").content.clone();
    assert!(
        first.contains("msg-180"),
        "first tail message should be msg-180, got {first:?}"
    );
    assert!(
        last.contains("msg-199"),
        "last tail message should be msg-199, got {last:?}"
    );
}

#[test]
fn parse_session_tail_returns_full_file_when_smaller_than_window() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("codex.jsonl");
    let mut content = String::new();
    content.push_str(
        "{\"timestamp\":\"2026-04-10T10:00:00Z\",\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5.4\"}}\n",
    );
    for i in 0..5 {
        content.push_str(&format!(
            "{{\"timestamp\":\"2026-04-10T10:00:0{i}Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"only-{i}\"}}]}}}}\n"
        ));
    }
    fs::write(&file, content).unwrap();

    let tail = parse_session_tail(&file, 100).expect("tail parse");
    assert_eq!(
        tail.messages.len(),
        5,
        "tail must return all messages when the file is smaller than the requested window"
    );
}
