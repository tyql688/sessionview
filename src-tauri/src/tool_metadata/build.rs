use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::names::{canonical_tool_name, display_tool_name, parse_mcp_tool_name, tool_category};
use super::presentation::refresh_tool_presentation;
use super::result::{normalize_json_value, normalized_status, result_kind_for_tool};
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

    let mut metadata = ToolMetadata {
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
        presentation: None,
    };
    refresh_tool_presentation(&mut metadata, call.input);
    metadata
}

pub fn attach_call_metadata<I>(
    metadata: &mut ToolMetadata,
    description: Option<&str>,
    display: Option<&Value>,
    ids: I,
) where
    I: IntoIterator<Item = (&'static str, String)>,
{
    for (key, value) in ids {
        if !value.is_empty() {
            metadata.ids.entry(key.to_string()).or_insert(value);
        }
    }

    let description = description.filter(|value| !value.is_empty());
    if description.is_none() && display.is_none() {
        return;
    }

    let mut structured = metadata
        .structured
        .take()
        .unwrap_or_else(|| Value::Object(Map::new()));
    if !structured.is_object() {
        structured = Value::Object(Map::new());
    }
    if let Value::Object(obj) = &mut structured {
        if let Some(description) = description {
            obj.entry("callDescription".to_string())
                .or_insert_with(|| Value::String(description.to_string()));
        }
        if let Some(display) = display {
            obj.entry("callDisplay".to_string())
                .or_insert_with(|| normalize_json_value(display));
        }
    }
    metadata.structured = Some(structured);
    refresh_tool_presentation(metadata, None);
}

pub fn enrich_tool_metadata(metadata: &mut ToolMetadata, result: ToolResultFacts<'_>) {
    metadata.status = normalized_status(ToolResultFacts { ..result });
    metadata.result_kind = result_kind_for_tool(
        &metadata.canonical_name,
        &metadata.category,
        &metadata.raw_name,
        result.raw_result,
    )
    .or_else(|| metadata.result_kind.clone());
    let existing_structured = metadata.structured.take();
    let result_structured = result.raw_result.map(|value| {
        let mut normalized = normalize_json_value(value);
        if !normalized.is_object() {
            normalized = json!({ "output": normalized });
        }
        normalize_structured_result(&mut normalized);
        normalized
    });
    metadata.structured = merge_structured(existing_structured, result_structured);
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
    refresh_tool_presentation(metadata, None);
}

fn merge_structured(existing: Option<Value>, result: Option<Value>) -> Option<Value> {
    match (existing, result) {
        (Some(Value::Object(existing_obj)), Some(Value::Object(mut result_obj))) => {
            for (key, value) in existing_obj {
                result_obj.entry(key).or_insert(value);
            }
            Some(Value::Object(result_obj))
        }
        (_, Some(result)) => Some(result),
        (existing, None) => existing,
    }
}

fn normalize_structured_result(value: &mut Value) {
    let Value::Object(obj) = value else {
        return;
    };

    promote_string_alias(obj, "agent_id", "agentId");
    promote_string_alias(obj, "teammate_id", "agentId");
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
mod tests;
