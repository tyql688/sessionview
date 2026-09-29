//! Inline Command Code subagent extraction.
//!
//! The CLI runs a real child agent for each typed `agent` tool call, but v3
//! persists only the parent's call input and the child's final text result.
//! It does not persist the child's internal messages, tools, model, or
//! disjoint usage. SessionView therefore materializes a deliberately limited
//! child session from those two authoritative blocks and never invents the
//! missing execution trace.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::models::{Message, Provider, SessionMeta};
use crate::provider::ParsedSession;
use crate::provider::util::session_title;

use super::super::types::Entry;
use super::ActiveEntry;
use super::messages::render_tool_result_content;

pub(super) struct InlineSubagents {
    pub link_by_call_id: HashMap<String, String>,
    children: Vec<InlineSubagent>,
}

struct InlineSubagent {
    call_id: String,
    title: String,
    variant_name: Option<String>,
    model: Option<String>,
    is_background: bool,
    prompt: String,
    started_at: Option<String>,
    created_at: i64,
    result: Option<InlineResult>,
}

#[derive(Clone)]
struct InlineResult {
    content: String,
    timestamp: Option<String>,
    epoch_seconds: Option<i64>,
}

struct AgentOutputCall {
    call_id: String,
    runtime_id: String,
    waits_for_completion: bool,
}

pub(super) fn collect_inline_subagents(
    active: &[ActiveEntry<'_>],
    path: &Path,
    parse_warning_count: &mut u32,
) -> InlineSubagents {
    let mut children = Vec::new();
    let mut child_by_call_id = HashMap::new();
    let mut results = HashMap::new();
    let mut output_calls = Vec::new();

    for active_entry in active {
        let Entry::Message(entry) = &active_entry.stored.entry else {
            continue;
        };
        for block in &entry.message.content {
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => collect_tool_call(
                    block,
                    active_entry,
                    path,
                    parse_warning_count,
                    &mut children,
                    &mut child_by_call_id,
                    &mut output_calls,
                ),
                Some("tool_result") => collect_tool_result(block, active_entry, &mut results),
                _ => {}
            }
        }
    }

    attach_results(&mut children, &child_by_call_id, &results, &output_calls);
    let link_by_call_id = children
        .iter()
        .map(|child| (child.call_id.clone(), child.call_id.clone()))
        .collect();
    InlineSubagents {
        link_by_call_id,
        children,
    }
}

#[allow(clippy::too_many_arguments)]
// The arguments are the typed collections populated during one ordered pass.
fn collect_tool_call(
    block: &Value,
    active: &ActiveEntry<'_>,
    path: &Path,
    parse_warning_count: &mut u32,
    children: &mut Vec<InlineSubagent>,
    child_by_call_id: &mut HashMap<String, usize>,
    output_calls: &mut Vec<AgentOutputCall>,
) {
    let Some(name) = block.get("name").and_then(Value::as_str) else {
        return;
    };
    if name.eq_ignore_ascii_case("agent") {
        collect_agent_call(
            block,
            active,
            path,
            parse_warning_count,
            children,
            child_by_call_id,
        );
    } else if name.eq_ignore_ascii_case("agent_output") {
        collect_agent_output_call(block, output_calls);
    }
}

fn collect_agent_call(
    block: &Value,
    active: &ActiveEntry<'_>,
    path: &Path,
    parse_warning_count: &mut u32,
    children: &mut Vec<InlineSubagent>,
    child_by_call_id: &mut HashMap<String, usize>,
) {
    let (Some(call_id), Some(input), Some(created_at)) = (
        block.get("id").and_then(Value::as_str),
        block.get("input").and_then(Value::as_object),
        active.epoch_seconds,
    ) else {
        return;
    };
    let Some(prompt) = input
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    else {
        log::warn!(
            "Command Code agent call '{call_id}' has no typed prompt at line {} in '{}'",
            active.stored.line_no,
            path.display()
        );
        *parse_warning_count = parse_warning_count.saturating_add(1);
        return;
    };
    if child_by_call_id.contains_key(call_id) {
        log::warn!(
            "duplicate Command Code agent call id '{call_id}' at line {} in '{}'",
            active.stored.line_no,
            path.display()
        );
        *parse_warning_count = parse_warning_count.saturating_add(1);
        return;
    }
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|description| !description.is_empty());
    let child = InlineSubagent {
        call_id: call_id.to_string(),
        title: description
            .map(str::to_string)
            .unwrap_or_else(|| session_title(Some(prompt))),
        variant_name: nonempty_input(input.get("subagent_type")),
        model: nonempty_input(input.get("model")),
        is_background: input
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        prompt: prompt.to_string(),
        started_at: active.timestamp.clone(),
        created_at,
        result: None,
    };
    child_by_call_id.insert(call_id.to_string(), children.len());
    children.push(child);
}

fn collect_agent_output_call(block: &Value, output_calls: &mut Vec<AgentOutputCall>) {
    let (Some(call_id), Some(input), Some(runtime_id)) = (
        block.get("id").and_then(Value::as_str),
        block.get("input").and_then(Value::as_object),
        block
            .get("input")
            .and_then(|input| input.get("agent_id"))
            .and_then(Value::as_str),
    ) else {
        return;
    };
    let waits_for_completion = input
        .get("action")
        .and_then(Value::as_str)
        .is_none_or(|action| action == "wait");
    output_calls.push(AgentOutputCall {
        call_id: call_id.to_string(),
        runtime_id: runtime_id.to_string(),
        waits_for_completion,
    });
}

fn collect_tool_result(
    block: &Value,
    active: &ActiveEntry<'_>,
    results: &mut HashMap<String, InlineResult>,
) {
    let (Some(call_id), Some(content)) = (
        block.get("tool_use_id").and_then(Value::as_str),
        block.get("content"),
    ) else {
        return;
    };
    results.insert(
        call_id.to_string(),
        InlineResult {
            content: render_tool_result_content(content).text,
            timestamp: active.timestamp.clone(),
            epoch_seconds: active.epoch_seconds,
        },
    );
}

fn attach_results(
    children: &mut [InlineSubagent],
    child_by_call_id: &HashMap<String, usize>,
    results: &HashMap<String, InlineResult>,
    output_calls: &[AgentOutputCall],
) {
    let mut runtime_to_child = HashMap::new();
    for (call_id, &index) in child_by_call_id {
        let Some(result) = results.get(call_id) else {
            continue;
        };
        match children[index]
            .is_background
            .then(|| background_runtime_id(&result.content))
            .flatten()
        {
            Some(runtime_id) => {
                runtime_to_child.insert(runtime_id.to_string(), index);
            }
            None => children[index].result = Some(result.clone()),
        }
    }
    for output in output_calls
        .iter()
        .filter(|output| output.waits_for_completion)
    {
        let (Some(&index), Some(result)) = (
            runtime_to_child.get(&output.runtime_id),
            results.get(&output.call_id),
        ) else {
            continue;
        };
        children[index].result = Some(result.clone());
    }
}

fn background_runtime_id(content: &str) -> Option<&str> {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("Background agent launched.") {
        return None;
    }
    lines.find_map(|line| {
        line.trim()
            .strip_prefix("agent_id:")
            .map(str::trim)
            .filter(|id| !id.is_empty())
    })
}

fn nonempty_input(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

impl InlineSubagents {
    pub(super) fn into_sessions(self, root: &SessionMeta, source_mtime: i64) -> Vec<ParsedSession> {
        self.children
            .into_iter()
            .map(|child| child.into_session(root, source_mtime))
            .collect()
    }
}

impl InlineSubagent {
    fn into_session(self, root: &SessionMeta, source_mtime: i64) -> ParsedSession {
        let mut messages = vec![Message {
            timestamp: self.started_at,
            ..Message::user(self.prompt.clone())
        }];
        let updated_at = self
            .result
            .as_ref()
            .and_then(|result| result.epoch_seconds)
            .unwrap_or(self.created_at);
        if let Some(result) = self.result
            && !result.content.trim().is_empty()
        {
            messages.push(Message {
                timestamp: result.timestamp,
                model: self.model.clone(),
                ..Message::assistant(result.content)
            });
        }
        let content_text = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        ParsedSession {
            meta: SessionMeta {
                id: format!("{}:{}", root.id, self.call_id),
                provider: Provider::CommandCode,
                title: self.title,
                project_name: root.project_name.clone(),
                project_path: root.project_path.clone(),
                created_at: self.created_at,
                updated_at,
                message_count: messages.len() as u32,
                file_size_bytes: root.file_size_bytes,
                source_path: root.source_path.clone(),
                is_sidechain: true,
                variant_name: self.variant_name,
                model: self.model,
                cc_version: root.cc_version.clone(),
                git_branch: root.git_branch.clone(),
                parent_id: Some(root.id.clone()),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            messages,
            content_text,
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime,
        }
    }
}
