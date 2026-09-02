use super::*;

fn touch_timestamps(accum: &mut SessionAccum, secs: Option<i64>) {
    let Some(secs) = secs else {
        return;
    };
    if accum.first_timestamp_secs.is_none() {
        accum.first_timestamp_secs = Some(secs);
    }
    if accum.last_timestamp_secs.is_none_or(|seen| secs >= seen) {
        accum.last_timestamp_secs = Some(secs);
    }
}

pub(super) fn handle_record(record: &Value, path: &Path, line_no: usize, state: &mut ParseState) {
    let timestamp = record
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string);
    let timestamp_secs = timestamp.as_deref().and_then(parse_rfc3339_epoch_seconds);
    // The root's span covers everything in the file, subagents included.
    touch_timestamps(&mut state.sessions[0], timestamp_secs);
    let Some(event_type) = record.get("type").and_then(Value::as_str) else {
        log::warn!("skipping Copilot record without a type tag");
        state.parse_warning_count = state.parse_warning_count.saturating_add(1);
        return;
    };
    let data = record.get("data").unwrap_or(&Value::Null);
    match event_type {
        "session.start" => handle_session_start(data, state),
        "session.binary_asset" => handle_binary_asset(data, path, line_no, state),
        "user.message" => {
            handle_user_message(data, timestamp, timestamp_secs, path, line_no, state)
        }
        "assistant.message" => handle_assistant_message(data, timestamp, timestamp_secs, state),
        "tool.execution_start" => handle_tool_start(data, timestamp, timestamp_secs, state),
        "tool.execution_complete" => handle_tool_complete(data, timestamp, timestamp_secs, state),
        "session.model_change" => {
            if let Some(model) = data
                .get("model")
                .or_else(|| data.get("newModel"))
                .and_then(Value::as_str)
                .filter(|model| !model.is_empty())
            {
                state.sessions[0].model = Some(model.to_string());
            }
        }
        "subagent.started" => handle_subagent_started(data, timestamp_secs, state),
        "subagent.completed" => {
            if let Some(index) = data
                .get("toolCallId")
                .and_then(Value::as_str)
                .and_then(|id| state.by_call_id.get(id).copied())
            {
                state.sessions[index].open = false;
                touch_timestamps(&mut state.sessions[index], timestamp_secs);
            }
        }
        "session.shutdown" => handle_shutdown(data, timestamp, state),
        _ => {
            // The event stream intentionally carries internal/ephemeral
            // types (hooks, plan/mode changes, compaction brackets, usage
            // checkpoints, model call telemetry, …) that no external
            // consumer can rely on; none alter the surfaced transcript.
            log::debug!("skipping unknown Copilot event '{event_type}'");
        }
    }
}

fn handle_session_start(data: &Value, state: &mut ParseState) {
    if state.session_id.is_none() {
        state.session_id = data
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if state.copilot_version.is_none() {
        state.copilot_version = data
            .get("copilotVersion")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    let context = data.get("context");
    if state.cwd.is_none() {
        state.cwd = context
            .and_then(|c| c.get("cwd"))
            .and_then(Value::as_str)
            .filter(|cwd| !cwd.is_empty())
            .map(str::to_string);
    }
    if state.git_branch.is_none() {
        state.git_branch = context
            .and_then(|c| c.get("branch"))
            .and_then(Value::as_str)
            .filter(|branch| !branch.is_empty())
            .map(str::to_string);
    }
}

fn handle_binary_asset(data: &Value, path: &Path, line_no: usize, state: &mut ParseState) {
    if data.get("type").and_then(Value::as_str) != Some("image") {
        return;
    }
    let Some(asset_id) = data.get("assetId").and_then(Value::as_str) else {
        warn_record(state, path, line_no, "image asset has no assetId");
        return;
    };
    let Some(payload) = data
        .get("data")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|payload| !payload.is_empty())
    else {
        warn_record(state, path, line_no, "image asset has no base64 payload");
        return;
    };
    let Some(mime) = data
        .get("mimeType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|mime| !mime.is_empty())
    else {
        warn_record(state, path, line_no, "image asset has no MIME type");
        return;
    };
    state.assets.insert(
        asset_id.to_string(),
        format!("[Image: source: data:{mime};base64,{payload}]"),
    );
}

fn handle_user_message(
    data: &Value,
    timestamp: Option<String>,
    timestamp_secs: Option<i64>,
    path: &Path,
    line_no: usize,
    state: &mut ParseState,
) {
    let text = data
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let attachments = data
        .get("attachments")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    // A subagent's opening prompt has no `parentToolCallId`; it is the
    // `task` prompt verbatim, delivered while that subagent is open.
    let trimmed = text.trim();
    let mut matching = state
        .sessions
        .iter()
        .enumerate()
        .filter_map(|(index, accum)| {
            (accum.open
                && accum.first_user_text.is_none()
                && accum.prompt.as_deref().map(str::trim) == Some(trimmed))
            .then_some(index)
        })
        .take(2);
    let first_match = matching.next();
    let index = match (first_match, matching.next()) {
        (Some(index), None) => index,
        (Some(_), Some(_)) => {
            warn_record(
                state,
                path,
                line_no,
                "subagent opening prompt matches multiple open task calls",
            );
            0
        }
        _ => 0,
    };

    let mut body = text.clone();
    let mut plain = text.clone();
    let mut markers: Vec<String> = Vec::new();
    for attachment in attachments {
        let marker = attachment
            .get("assetId")
            .and_then(Value::as_str)
            .and_then(|id| state.assets.get(id));
        match marker {
            Some(marker) => {
                let placeholder = attachment
                    .get("displayName")
                    .and_then(Value::as_str)
                    .map(|name| format!("[image: {name}]"));
                match placeholder.filter(|placeholder| body.contains(placeholder.as_str())) {
                    Some(placeholder) => {
                        body = body.replace(&placeholder, marker);
                        plain = plain.replace(&placeholder, "").trim().to_string();
                    }
                    None => markers.push(marker.clone()),
                }
            }
            None => {
                warn_record(
                    state,
                    path,
                    line_no,
                    "image attachment does not resolve to a persisted asset",
                );
                let placeholder = attachment
                    .get("displayName")
                    .and_then(Value::as_str)
                    .map(|name| format!("[image: {name}]"));
                match placeholder.filter(|placeholder| body.contains(placeholder.as_str())) {
                    Some(placeholder) => {
                        body = body.replace(&placeholder, "[Attachment]");
                        plain = plain.replace(&placeholder, "").trim().to_string();
                    }
                    None => markers.push("[Attachment]".to_string()),
                }
            }
        }
    }
    if trimmed.is_empty() && markers.is_empty() {
        return;
    }
    if !markers.is_empty() {
        if !body.trim().is_empty() {
            body.push('\n');
        }
        body.push_str(&markers.join("\n"));
    }

    let accum = &mut state.sessions[index];
    touch_timestamps(accum, timestamp_secs);
    // Search text and the title keep the user's words, never the base64
    // payload; an image-only message contributes nothing to either.
    if !plain.trim().is_empty() {
        if accum.first_user_text.is_none() {
            accum.first_user_text = Some(plain.clone());
        }
        accum.content_parts.push(plain);
    }
    accum.messages.push(Message {
        timestamp,
        ..Message::user(body)
    });
}

fn warn_record(state: &mut ParseState, path: &Path, line_no: usize, reason: &str) {
    log::warn!(
        "skipping malformed Copilot content at line {line_no} in '{}': {reason}",
        path.display()
    );
    state.parse_warning_count = state.parse_warning_count.saturating_add(1);
}

fn handle_assistant_message(
    data: &Value,
    timestamp: Option<String>,
    timestamp_secs: Option<i64>,
    state: &mut ParseState,
) {
    let index = state.route(data);
    let model = data
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .map(str::to_string);
    let accum = &mut state.sessions[index];
    if model.is_some() {
        accum.model = model.clone();
    }
    let content = data
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.trim().is_empty());
    let Some(content) = content else {
        accum.assistant_calls.push(None);
        return;
    };
    touch_timestamps(accum, timestamp_secs);
    accum.content_parts.push(content.to_string());
    accum.assistant_calls.push(Some(accum.messages.len()));
    accum.messages.push(Message {
        timestamp,
        model,
        ..Message::assistant(content.to_string())
    });
}

fn handle_tool_start(
    data: &Value,
    timestamp: Option<String>,
    timestamp_secs: Option<i64>,
    state: &mut ParseState,
) {
    let Some(call_id) = data.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    let index = state.route(data);
    if state.sessions[index].tool_by_call_id.contains_key(call_id) {
        // Out-of-order duplicate start for a call we already surfaced.
        return;
    }
    let raw_name = data
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let arguments_value = data.get("arguments").unwrap_or(&Value::Null);
    // Older builds serialise `arguments` as a JSON string; decode it so
    // the prompt lookup and the tool summary see the same object.
    let decoded_arguments = match arguments_value {
        Value::String(raw) => serde_json::from_str::<Value>(raw).unwrap_or(Value::Null),
        other => other.clone(),
    };
    if raw_name == "task"
        && let Some(prompt) = decoded_arguments.get("prompt").and_then(Value::as_str)
    {
        state
            .task_prompts
            .insert(call_id.to_string(), prompt.to_string());
    }
    state.call_owner.insert(call_id.to_string(), index);
    let arguments_raw = normalize_arguments(arguments_value);
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Copilot,
        raw_name,
        input: decoded_arguments.is_object().then_some(&decoded_arguments),
        call_id: Some(call_id),
        assistant_id: None,
    });
    // `read_agent` & co. address a background agent by its runtime hash;
    // point them at the child the spawning `task` call created.
    if let Some(task_call) = decoded_arguments
        .get("agent_id")
        .and_then(Value::as_str)
        .and_then(|hash| state.agent_hash_to_call.get(hash))
    {
        let link = serde_json::json!({ "agent_id": task_call });
        enrich_tool_metadata(
            &mut metadata,
            ToolResultFacts {
                raw_result: Some(&link),
                ..ToolResultFacts::default()
            },
        );
    }
    let canonical_name = metadata.canonical_name.clone();
    let accum = &mut state.sessions[index];
    touch_timestamps(accum, timestamp_secs);
    let message_index = accum.messages.len();
    accum.messages.push(Message {
        timestamp,
        tool_name: Some(canonical_name),
        tool_input: Some(arguments_raw),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, String::new())
    });
    accum
        .tool_by_call_id
        .insert(call_id.to_string(), message_index);
}

fn handle_tool_complete(
    data: &Value,
    timestamp: Option<String>,
    timestamp_secs: Option<i64>,
    state: &mut ParseState,
) {
    let Some(call_id) = data.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    let is_error = data.get("success").and_then(Value::as_bool) == Some(false);
    let result_text = extract_result_text(data.get("result").unwrap_or(&Value::Null));
    if (state.task_prompts.contains_key(call_id) || state.by_call_id.contains_key(call_id))
        && let Some(hash) = background_agent_hash(&result_text)
    {
        state
            .agent_hash_to_call
            .insert(hash.to_string(), call_id.to_string());
    }
    let result_facts = ToolResultFacts {
        is_error: if data.get("success").is_some() {
            Some(is_error)
        } else {
            None
        },
        ..ToolResultFacts::default()
    };
    // The completion carries the same `parentToolCallId` as its start; the
    // issuing session is the authoritative owner either way.
    let index = state
        .call_owner
        .get(call_id)
        .copied()
        .unwrap_or_else(|| state.route(data));
    let accum = &mut state.sessions[index];
    touch_timestamps(accum, timestamp_secs);
    if let Some(&message_index) = accum.tool_by_call_id.get(call_id) {
        if !result_text.is_empty() {
            accum.messages[message_index].content = result_text;
        }
        if let Some(metadata) = accum.messages[message_index].tool_metadata.as_mut() {
            metadata
                .ids
                .insert("tool_use_id".to_string(), call_id.to_string());
            enrich_tool_metadata(metadata, result_facts);
        }
        return;
    }
    // Orphan completion: the paired start was never logged (interrupted
    // turn). Standalone Tool message so the result is not lost.
    let mut metadata = build_tool_metadata(ToolCallFacts {
        provider: Provider::Copilot,
        raw_name: "tool",
        input: None,
        call_id: Some(call_id),
        assistant_id: None,
    });
    enrich_tool_metadata(&mut metadata, result_facts);
    let canonical_name = metadata.canonical_name.clone();
    accum.messages.push(Message {
        timestamp,
        tool_name: Some(canonical_name),
        tool_metadata: Some(metadata),
        ..Message::new(MessageRole::Tool, result_text)
    });
}

fn handle_subagent_started(data: &Value, timestamp_secs: Option<i64>, state: &mut ParseState) {
    let Some(call_id) = data.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    if state.by_call_id.contains_key(call_id) {
        return;
    }
    let parent = state.call_owner.get(call_id).copied().unwrap_or(0);
    let string = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let mut child = SessionAccum {
        call_id: Some(call_id.to_string()),
        parent: Some(parent),
        title: string("agentDisplayName").or_else(|| string("agentDescription")),
        agent_name: string("agentName").or_else(|| string("agentType")),
        model: string("model"),
        prompt: state.task_prompts.remove(call_id),
        open: true,
        ..SessionAccum::default()
    };
    touch_timestamps(&mut child, timestamp_secs);
    let index = state.sessions.len();
    state.sessions.push(child);
    state.by_call_id.insert(call_id.to_string(), index);

    // Link the parent's Agent tool message to the child so the UI can open it.
    let owner = &mut state.sessions[parent];
    if let Some(&message_index) = owner.tool_by_call_id.get(call_id)
        && let Some(metadata) = owner.messages[message_index].tool_metadata.as_mut()
    {
        let link = serde_json::json!({ "agent_id": call_id });
        enrich_tool_metadata(
            metadata,
            ToolResultFacts {
                raw_result: Some(&link),
                ..ToolResultFacts::default()
            },
        );
    }
}
