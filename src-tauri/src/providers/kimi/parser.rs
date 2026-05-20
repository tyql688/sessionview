use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use serde_json::Value;

use crate::models::{Message, MessageRole, Provider, SessionMeta, TokenUsage};
use crate::provider::ParsedSession;
use crate::provider_utils::{
    project_name_from_path, session_title, truncate_to_bytes, FTS_CONTENT_LIMIT, NO_PROJECT,
};
use crate::tool_metadata::{
    build_tool_metadata, enrich_tool_metadata, ToolCallFacts, ToolResultFacts,
};

use super::tools::*;
use super::KimiProvider;

fn parse_json_value(text: Option<&str>) -> Option<Value> {
    serde_json::from_str(text?).ok()
}

fn kimi_result_status(payload: &Value) -> Option<&str> {
    payload.get("status").and_then(|v| v.as_str()).or_else(|| {
        payload
            .get("return_value")
            .and_then(|rv| rv.get("status"))
            .and_then(|v| v.as_str())
    })
}

fn kimi_result_is_error(payload: &Value) -> Option<bool> {
    payload
        .get("is_error")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            payload
                .get("return_value")
                .and_then(|rv| rv.get("is_error"))
                .and_then(|v| v.as_bool())
        })
        .or_else(|| payload.get("error").map(|v| !v.is_null()))
        .or_else(|| {
            payload
                .get("return_value")
                .and_then(|rv| rv.get("success"))
                .and_then(|v| v.as_bool())
                .map(|success| !success)
        })
        .or_else(|| {
            kimi_result_status(payload)
                .map(|status| matches!(status, "error" | "failed" | "failure"))
        })
}

fn enrich_kimi_tool_metadata(message: &mut Message, payload: &Value) {
    let raw_result = payload.get("return_value").or(Some(payload));
    if let Some(metadata) = message.tool_metadata.as_mut() {
        enrich_tool_metadata(
            metadata,
            ToolResultFacts {
                raw_result,
                is_error: kimi_result_is_error(payload),
                status: kimi_result_status(payload),
                artifact_path: None,
            },
        );

        // Promote Kimi display diff into structured fields so the frontend
        // can render it with LineDiff (buildToolLineDiff) like Claude Edit.
        let (old_text, new_text) = {
            let display_diff = metadata.structured.as_ref().and_then(|s| {
                s.get("display").and_then(|d| d.as_array()).and_then(|arr| {
                    arr.iter()
                        .find(|item| item.get("type").and_then(|v| v.as_str()) == Some("diff"))
                })
            });
            let old_text = display_diff
                .and_then(|d| d.get("old_text"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let new_text = display_diff
                .and_then(|d| d.get("new_text"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            (old_text, new_text)
        };
        if let Some(Value::Object(obj)) = metadata.structured.as_mut() {
            // Bash tool: Kimi uses "output" for stdout. Promote it so the
            // frontend formatToolResultMetadata can display it.
            if !obj.contains_key("stdout") {
                if let Some(output) = obj.get("output").and_then(|v| v.as_str()) {
                    if !output.is_empty() {
                        obj.insert("stdout".to_string(), Value::String(output.to_string()));
                    }
                }
            }
            if let Some(text) = old_text {
                obj.insert("old_string".to_string(), Value::String(text));
            }
            if let Some(text) = new_text {
                obj.insert("new_string".to_string(), Value::String(text));
            }
        }
    }
}

/// Read subagent description from meta.json in the subagents directory.
fn subagent_title_from_meta(session_dir: &std::path::Path, agent_id: &str) -> Option<String> {
    let meta_path = session_dir
        .join("subagents")
        .join(agent_id)
        .join("meta.json");
    if !meta_path.exists() {
        return None;
    }
    let content = match fs::read_to_string(&meta_path) {
        Ok(content) => content,
        Err(error) => {
            log::warn!(
                "failed to read Kimi subagent meta '{}': {}",
                meta_path.display(),
                error
            );
            return None;
        }
    };
    let json: Value = match serde_json::from_str(&content) {
        Ok(json) => json,
        Err(error) => {
            log::warn!(
                "failed to parse Kimi subagent meta '{}': {}",
                meta_path.display(),
                error
            );
            return None;
        }
    };
    json.get("description")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

impl KimiProvider {
    /// Parse a wire.jsonl file and return the main session plus any embedded subagent sessions.
    pub fn parse_session_with_subagents(
        &self,
        path: &PathBuf,
        project_map: &HashMap<String, String>,
    ) -> Vec<ParsedSession> {
        let mut results = Vec::new();
        if let Some(main_session) = self.parse_session_file(path, project_map) {
            let session_id = main_session.meta.id.clone();
            let project_path = main_session.meta.project_path.clone();
            let project_name = main_session.meta.project_name.clone();
            let source_path = main_session.meta.source_path.clone();

            // Extract subagent sessions from SubagentEvent entries
            let session_dir = path.parent();
            let subagent_sessions = self.extract_subagents(
                path,
                &session_id,
                &project_path,
                &project_name,
                &source_path,
                session_dir,
            );

            results.push(main_session);
            results.extend(subagent_sessions);
        }
        results
    }

    pub fn parse_session_file(
        &self,
        path: &PathBuf,
        project_map: &HashMap<String, String>,
    ) -> Option<ParsedSession> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) => {
                log::warn!(
                    "failed to open Kimi session '{}': {}",
                    path.display(),
                    error
                );
                return None;
            }
        };
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                log::warn!(
                    "failed to read Kimi session metadata '{}': {}",
                    path.display(),
                    error
                );
                return None;
            }
        };
        let file_size = metadata.len();

        let reader = BufReader::new(file);
        let mut messages = Vec::new();
        let mut first_user_message: Option<String> = None;
        let mut first_timestamp: Option<i64> = None;
        let mut last_timestamp: Option<i64> = None;
        let mut content_parts: Vec<String> = Vec::new();
        // Map call_id -> message index for merging ToolResult into ToolCall
        let mut call_id_map: HashMap<String, usize> = HashMap::new();
        // Track the most recent ToolCall for ToolCallPart append
        let mut last_tool_call_idx: Option<usize> = None;
        let mut parse_warning_count: u32 = 0;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(error) => {
                    log::warn!(
                        "failed to read Kimi session line from '{}': {}",
                        path.display(),
                        error
                    );
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }

            let entry: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(error) => {
                    log::warn!(
                        "skipping malformed Kimi JSONL in '{}': {}",
                        path.display(),
                        error
                    );
                    parse_warning_count = parse_warning_count.saturating_add(1);
                    continue;
                }
            };

            // Extract timestamp (float seconds)
            let ts_secs = entry.get("timestamp").and_then(|v| v.as_f64());
            let ts_epoch = ts_secs.map(|t| t as i64);

            if let Some(ts) = ts_epoch {
                if first_timestamp.is_none() {
                    first_timestamp = Some(ts);
                }
                last_timestamp = Some(ts);
            }

            // Get message object
            let message = match entry.get("message") {
                Some(m) => m,
                None => continue,
            };

            let msg_type = match message.get("type").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => continue,
            };

            let payload = match message.get("payload") {
                Some(p) => p,
                None => continue,
            };

            let ts_str = ts_secs.map(|t| {
                chrono::DateTime::from_timestamp(t as i64, ((t.fract()) * 1_000_000_000.0) as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            });

            match msg_type {
                "TurnBegin" => {
                    // Extract user input text + images
                    if let Some(Value::String(text)) = payload.get("user_input") {
                        if text.is_empty() {
                            continue;
                        }
                        if first_user_message.is_none() {
                            first_user_message = Some(text.to_string());
                        }
                        content_parts.push(text.to_string());
                        messages.push(Message {
                            role: MessageRole::User,
                            content: text.to_string(),
                            timestamp: ts_str.clone(),
                            tool_name: None,
                            tool_input: None,
                            token_usage: None,
                            model: None,
                            usage_hash: None,
                            tool_metadata: None,
                        });
                    } else if let Some(Value::Array(parts)) = payload.get("user_input") {
                        let has_image = parts
                            .iter()
                            .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("image_url"));
                        let mut text_parts = Vec::new();
                        for part in parts {
                            let part_type =
                                part.get("type").and_then(|t| t.as_str()).unwrap_or("text");
                            match part_type {
                                "text" => {
                                    let text =
                                        part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                    // Skip <image path="..."> and </image> markers when inline image data exists
                                    if has_image
                                        && (text.contains("<image path=")
                                            || text.trim() == "</image>")
                                    {
                                        continue;
                                    }
                                    if !text.is_empty() {
                                        text_parts.push(text.to_string());
                                    }
                                }
                                "image_url" => {
                                    // Extract image: prefer local path from prompt-cache, fall back to data URI
                                    if let Some(url) = part
                                        .get("image_url")
                                        .and_then(|iu| iu.get("url"))
                                        .and_then(|v| v.as_str())
                                    {
                                        text_parts.push(format!("[Image: source: {url}]"));
                                    }
                                }
                                _ => {}
                            }
                        }
                        let text = text_parts.join("\n");
                        if text.is_empty() {
                            continue;
                        }
                        if first_user_message.is_none() {
                            // Strip image markers from title
                            let title_text = text
                                .lines()
                                .find(|l| !l.starts_with("[Image:"))
                                .unwrap_or(&text)
                                .to_string();
                            first_user_message = Some(title_text);
                        }
                        content_parts.push(text.clone());
                        messages.push(Message {
                            role: MessageRole::User,
                            content: text,
                            timestamp: ts_str.clone(),
                            tool_name: None,
                            tool_input: None,
                            token_usage: None,
                            model: None,
                            usage_hash: None,
                            tool_metadata: None,
                        });
                    }
                }
                "ContentPart" => {
                    let part_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match part_type {
                        "think" => {
                            let think_text =
                                payload.get("think").and_then(|v| v.as_str()).unwrap_or("");
                            if !think_text.is_empty() {
                                messages.push(Message {
                                    role: MessageRole::System,
                                    content: format!("[thinking]\n{think_text}"),
                                    timestamp: ts_str.clone(),
                                    tool_name: None,
                                    tool_input: None,
                                    token_usage: None,
                                    model: None,
                                    usage_hash: None,
                                    tool_metadata: None,
                                });
                            }
                        }
                        "text" => {
                            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if !text.is_empty() {
                                content_parts.push(text.to_string());
                                messages.push(Message {
                                    role: MessageRole::Assistant,
                                    content: text.to_string(),
                                    timestamp: ts_str.clone(),
                                    tool_name: None,
                                    tool_input: None,
                                    token_usage: None,
                                    model: None,
                                    usage_hash: None,
                                    tool_metadata: None,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                "ToolCall" => {
                    let call_id = payload.get("id").and_then(|v| v.as_str());
                    let func = payload.get("function");
                    let raw_name = func
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let arguments_str = func
                        .and_then(|f| f.get("arguments"))
                        .and_then(|v| v.as_str());

                    let input_value = parse_json_value(arguments_str);
                    let metadata = build_tool_metadata(ToolCallFacts {
                        provider: Provider::Kimi,
                        raw_name,
                        input: input_value.as_ref(),
                        call_id,
                        assistant_id: None,
                    });
                    let display_name = metadata.canonical_name.clone();
                    let tool_input = arguments_str.map(|s| s.to_string());

                    let idx = messages.len();
                    if let Some(cid) = call_id {
                        call_id_map.insert(cid.to_string(), idx);
                    }
                    last_tool_call_idx = Some(idx);
                    messages.push(Message {
                        role: MessageRole::Tool,
                        content: String::new(),
                        timestamp: ts_str.clone(),
                        tool_name: Some(display_name.to_string()),
                        tool_input,
                        token_usage: None,
                        model: None,
                        usage_hash: None,
                        tool_metadata: Some(metadata),
                    });
                }
                "ToolCallPart" => {
                    if let Some(part) = payload.get("arguments_part").and_then(|v| v.as_str()) {
                        if let Some(idx) = last_tool_call_idx {
                            if idx < messages.len() && messages[idx].role == MessageRole::Tool {
                                let current = messages[idx].tool_input.clone().unwrap_or_default();
                                let merged = if current.is_empty() {
                                    part.to_string()
                                } else {
                                    format!("{}{}", current, part)
                                };
                                messages[idx].tool_input = Some(merged.clone());
                                // Re-parse and update metadata if JSON is now valid
                                if let Ok(value) = serde_json::from_str::<Value>(&merged) {
                                    if let Some(meta) = messages[idx].tool_metadata.as_mut() {
                                        let old_ids = meta.ids.clone();
                                        let old_mcp = meta.mcp.clone();
                                        let new_meta = build_tool_metadata(ToolCallFacts {
                                            provider: Provider::Kimi,
                                            raw_name: &meta.raw_name,
                                            input: Some(&value),
                                            call_id: None,
                                            assistant_id: None,
                                        });
                                        *meta = new_meta;
                                        meta.ids = old_ids;
                                        meta.mcp = old_mcp;
                                    }
                                }
                            }
                        }
                    }
                }
                "ToolResult" => {
                    // Merge output into the matching ToolCall message
                    let call_id = payload.get("tool_call_id").and_then(|v| v.as_str());
                    let tool_name = call_id
                        .and_then(|cid| call_id_map.get(cid))
                        .copied()
                        .and_then(|idx| messages.get(idx))
                        .and_then(|msg| msg.tool_name.as_deref());
                    let output = extract_tool_output(payload, tool_name);

                    if !output.is_empty() {
                        content_parts.push(output.clone());
                    }
                    if let Some(idx) = call_id.and_then(|cid| call_id_map.get(cid)).copied() {
                        if idx < messages.len() {
                            messages[idx].content = output;
                            enrich_kimi_tool_metadata(&mut messages[idx], payload);
                            continue;
                        }
                    }
                    // Fallback: standalone output message
                    messages.push(Message {
                        role: MessageRole::Tool,
                        content: output,
                        timestamp: ts_str.clone(),
                        tool_name: None,
                        tool_input: None,
                        token_usage: None,
                        model: None,
                        usage_hash: None,
                        tool_metadata: None,
                    });
                }
                "StatusUpdate" => {
                    if let Some(tu) = payload.get("token_usage") {
                        let input_other =
                            tu.get("input_other").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let output = tu.get("output").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let cache_read = tu
                            .get("input_cache_read")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        let cache_creation = tu
                            .get("input_cache_creation")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;

                        let usage = TokenUsage {
                            input_tokens: input_other + cache_read + cache_creation,
                            output_tokens: output,
                            cache_read_input_tokens: cache_read,
                            cache_creation_input_tokens: cache_creation,
                        };

                        // Attach to last assistant or tool message
                        if let Some(last_msg) = messages.iter_mut().rev().find(|m| {
                            m.role == MessageRole::Assistant || m.role == MessageRole::Tool
                        }) {
                            last_msg.token_usage = Some(usage);
                        }
                    }
                }
                // Skip: metadata, TurnEnd, StepBegin, etc.
                _ => continue,
            }
        }

        if messages.is_empty() {
            return None;
        }

        // Derive session ID from directory name (session UUID)
        let session_id = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let title = session_title(first_user_message.as_deref());

        // Resolve project path from the MD5 directory name
        let project_path = path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|d| d.file_name())
            .and_then(|name| project_map.get(name.to_string_lossy().as_ref()))
            .cloned()
            .unwrap_or_else(|| NO_PROJECT.to_string());
        let project_name = project_name_from_path(&project_path);

        let Some(created_at) = first_timestamp else {
            log::warn!(
                "skipping Kimi session without first timestamp '{}': no usable timestamp found",
                path.display()
            );
            return None;
        };
        let Some(updated_at) = last_timestamp else {
            log::warn!(
                "skipping Kimi session without last timestamp '{}': no usable timestamp found",
                path.display()
            );
            return None;
        };

        let full_content = content_parts.join("\n");
        let content_text = truncate_to_bytes(&full_content, FTS_CONTENT_LIMIT);

        let meta = SessionMeta {
            id: session_id,
            provider: Provider::Kimi,
            title,
            project_path,
            project_name,
            created_at,
            updated_at,
            message_count: messages.len() as u32,
            file_size_bytes: file_size,
            source_path: path.to_string_lossy().to_string(),
            is_sidechain: false,
            variant_name: None,
            model: None,
            cc_version: None,
            git_branch: None,
            parent_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };

        Some(ParsedSession {
            meta,
            messages,
            content_text,
            parse_warning_count,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
        })
    }

    /// Extract subagent sessions from SubagentEvent entries in a parent wire.jsonl.
    #[allow(clippy::too_many_arguments)]
    fn extract_subagents(
        &self,
        path: &PathBuf,
        parent_session_id: &str,
        project_path: &str,
        project_name: &str,
        source_path: &str,
        session_dir: Option<&std::path::Path>,
    ) -> Vec<ParsedSession> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(error) => {
                log::warn!(
                    "failed to open Kimi session for subagent extraction '{}': {}",
                    path.display(),
                    error
                );
                return Vec::new();
            }
        };

        // Collect SubagentEvent entries grouped by agent_id
        let mut agent_events: HashMap<String, Vec<(f64, Value)>> = HashMap::new();
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(error) => {
                    log::warn!(
                        "failed to read Kimi subagent extraction line from '{}': {}",
                        path.display(),
                        error
                    );
                    continue;
                }
            };
            if !line.contains("SubagentEvent") {
                continue;
            }
            let entry: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(error) => {
                    log::warn!(
                        "skipping malformed Kimi subagent JSONL in '{}': {}",
                        path.display(),
                        error
                    );
                    continue;
                }
            };
            let ts = entry
                .get("timestamp")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let message = match entry.get("message") {
                Some(m) => m,
                None => continue,
            };
            if message.get("type").and_then(|v| v.as_str()) != Some("SubagentEvent") {
                continue;
            }
            let payload = match message.get("payload") {
                Some(p) => p,
                None => continue,
            };
            let agent_id = match payload.get("agent_id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            let inner_event = match payload.get("event") {
                Some(e) => e.clone(),
                None => continue,
            };
            agent_events
                .entry(agent_id)
                .or_default()
                .push((ts, inner_event));
        }

        // Sort agent_ids for deterministic iteration order
        let mut sorted_ids: Vec<String> = agent_events.keys().cloned().collect();
        sorted_ids.sort();

        let mut results = Vec::new();
        for agent_id in sorted_ids {
            if let Some(events) = agent_events.get(&agent_id) {
                if let Some(session) = self.parse_subagent_events(
                    &agent_id,
                    events,
                    parent_session_id,
                    project_path,
                    project_name,
                    source_path,
                    session_dir,
                ) {
                    results.push(session);
                }
            }
        }
        results
    }

    /// Parse a sequence of unwrapped SubagentEvent inner events into a ParsedSession.
    #[allow(clippy::too_many_arguments)]
    fn parse_subagent_events(
        &self,
        agent_id: &str,
        events: &[(f64, Value)],
        parent_session_id: &str,
        project_path: &str,
        project_name: &str,
        source_path: &str,
        session_dir: Option<&std::path::Path>,
    ) -> Option<ParsedSession> {
        let mut messages = Vec::new();
        let mut first_user_message: Option<String> = None;
        let mut first_timestamp: Option<i64> = None;
        let mut last_timestamp: Option<i64> = None;
        let mut content_parts: Vec<String> = Vec::new();
        let mut call_id_map: HashMap<String, usize> = HashMap::new();
        let mut last_tool_call_idx: Option<usize> = None;

        for (ts, event) in events {
            let ts_epoch = *ts as i64;
            if first_timestamp.is_none() {
                first_timestamp = Some(ts_epoch);
            }
            last_timestamp = Some(ts_epoch);

            let msg_type = match event.get("type").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => continue,
            };
            let payload = match event.get("payload") {
                Some(p) => p,
                None => continue,
            };

            let ts_str = Some(
                chrono::DateTime::from_timestamp(ts_epoch, ((*ts % 1.0) * 1_000_000_000.0) as u32)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            );

            match msg_type {
                "TurnBegin" => {
                    if let Some(Value::String(text)) = payload.get("user_input") {
                        if text.is_empty() {
                            continue;
                        }
                        if first_user_message.is_none() {
                            first_user_message = Some(text.to_string());
                        }
                        content_parts.push(text.to_string());
                        messages.push(Message {
                            role: MessageRole::User,
                            content: text.to_string(),
                            timestamp: ts_str.clone(),
                            tool_name: None,
                            tool_input: None,
                            token_usage: None,
                            model: None,
                            usage_hash: None,
                            tool_metadata: None,
                        });
                    } else if let Some(Value::Array(parts)) = payload.get("user_input") {
                        let has_image = parts
                            .iter()
                            .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("image_url"));
                        let mut text_parts = Vec::new();
                        for part in parts {
                            let part_type =
                                part.get("type").and_then(|t| t.as_str()).unwrap_or("text");
                            match part_type {
                                "text" => {
                                    let text =
                                        part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                    if has_image
                                        && (text.contains("<image path=")
                                            || text.trim() == "</image>")
                                    {
                                        continue;
                                    }
                                    if !text.is_empty() {
                                        text_parts.push(text.to_string());
                                    }
                                }
                                "image_url" => {
                                    if let Some(url) = part
                                        .get("image_url")
                                        .and_then(|iu| iu.get("url"))
                                        .and_then(|v| v.as_str())
                                    {
                                        text_parts.push(format!("[Image: source: {url}]"));
                                    }
                                }
                                _ => {}
                            }
                        }
                        let text = text_parts.join("\n");
                        if text.is_empty() {
                            continue;
                        }
                        if first_user_message.is_none() {
                            let title_text = text
                                .lines()
                                .find(|l| !l.starts_with("[Image:"))
                                .unwrap_or(&text)
                                .to_string();
                            first_user_message = Some(title_text);
                        }
                        content_parts.push(text.clone());
                        messages.push(Message {
                            role: MessageRole::User,
                            content: text,
                            timestamp: ts_str.clone(),
                            tool_name: None,
                            tool_input: None,
                            token_usage: None,
                            model: None,
                            usage_hash: None,
                            tool_metadata: None,
                        });
                    }
                }
                "ContentPart" => {
                    let part_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match part_type {
                        "think" => {
                            let think_text =
                                payload.get("think").and_then(|v| v.as_str()).unwrap_or("");
                            if !think_text.is_empty() {
                                messages.push(Message {
                                    role: MessageRole::System,
                                    content: format!("[thinking]\n{think_text}"),
                                    timestamp: ts_str.clone(),
                                    tool_name: None,
                                    tool_input: None,
                                    token_usage: None,
                                    model: None,
                                    usage_hash: None,
                                    tool_metadata: None,
                                });
                            }
                        }
                        "text" => {
                            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if !text.is_empty() {
                                content_parts.push(text.to_string());
                                messages.push(Message {
                                    role: MessageRole::Assistant,
                                    content: text.to_string(),
                                    timestamp: ts_str.clone(),
                                    tool_name: None,
                                    tool_input: None,
                                    token_usage: None,
                                    model: None,
                                    usage_hash: None,
                                    tool_metadata: None,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                "ToolCall" => {
                    let call_id = payload.get("id").and_then(|v| v.as_str());
                    let func = payload.get("function");
                    let raw_name = func
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let arguments_str = func
                        .and_then(|f| f.get("arguments"))
                        .and_then(|v| v.as_str());
                    let input_value = parse_json_value(arguments_str);
                    let metadata = build_tool_metadata(ToolCallFacts {
                        provider: Provider::Kimi,
                        raw_name,
                        input: input_value.as_ref(),
                        call_id,
                        assistant_id: None,
                    });
                    let display_name = metadata.canonical_name.clone();
                    let tool_input = arguments_str.map(|s| s.to_string());
                    let idx = messages.len();
                    if let Some(cid) = call_id {
                        call_id_map.insert(cid.to_string(), idx);
                    }
                    last_tool_call_idx = Some(idx);
                    messages.push(Message {
                        role: MessageRole::Tool,
                        content: String::new(),
                        timestamp: ts_str.clone(),
                        tool_name: Some(display_name.to_string()),
                        tool_input,
                        token_usage: None,
                        model: None,
                        usage_hash: None,
                        tool_metadata: Some(metadata),
                    });
                }
                "ToolCallPart" => {
                    if let Some(part) = payload.get("arguments_part").and_then(|v| v.as_str()) {
                        if let Some(idx) = last_tool_call_idx {
                            if idx < messages.len() && messages[idx].role == MessageRole::Tool {
                                let current = messages[idx].tool_input.clone().unwrap_or_default();
                                let merged = if current.is_empty() {
                                    part.to_string()
                                } else {
                                    format!("{}{}", current, part)
                                };
                                messages[idx].tool_input = Some(merged.clone());
                                if let Ok(value) = serde_json::from_str::<Value>(&merged) {
                                    if let Some(meta) = messages[idx].tool_metadata.as_mut() {
                                        let old_ids = meta.ids.clone();
                                        let old_mcp = meta.mcp.clone();
                                        let new_meta = build_tool_metadata(ToolCallFacts {
                                            provider: Provider::Kimi,
                                            raw_name: &meta.raw_name,
                                            input: Some(&value),
                                            call_id: None,
                                            assistant_id: None,
                                        });
                                        *meta = new_meta;
                                        meta.ids = old_ids;
                                        meta.mcp = old_mcp;
                                    }
                                }
                            }
                        }
                    }
                }
                "ToolResult" => {
                    let call_id = payload.get("tool_call_id").and_then(|v| v.as_str());
                    let tool_name = call_id
                        .and_then(|cid| call_id_map.get(cid))
                        .copied()
                        .and_then(|idx| messages.get(idx))
                        .and_then(|msg| msg.tool_name.as_deref());
                    let output = extract_tool_output(payload, tool_name);
                    if !output.is_empty() {
                        content_parts.push(output.clone());
                    }
                    if let Some(idx) = call_id.and_then(|cid| call_id_map.get(cid)).copied() {
                        if idx < messages.len() {
                            messages[idx].content = output;
                            enrich_kimi_tool_metadata(&mut messages[idx], payload);
                            continue;
                        }
                    }
                    messages.push(Message {
                        role: MessageRole::Tool,
                        content: output,
                        timestamp: ts_str.clone(),
                        tool_name: None,
                        tool_input: None,
                        token_usage: None,
                        model: None,
                        usage_hash: None,
                        tool_metadata: None,
                    });
                }
                "StatusUpdate" => {
                    if let Some(tu) = payload.get("token_usage") {
                        let input_other =
                            tu.get("input_other").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let output = tu.get("output").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let cache_read = tu
                            .get("input_cache_read")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        let cache_creation = tu
                            .get("input_cache_creation")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        let usage = TokenUsage {
                            input_tokens: input_other + cache_read + cache_creation,
                            output_tokens: output,
                            cache_read_input_tokens: cache_read,
                            cache_creation_input_tokens: cache_creation,
                        };
                        if let Some(last_msg) = messages.iter_mut().rev().find(|m| {
                            m.role == MessageRole::Assistant || m.role == MessageRole::Tool
                        }) {
                            last_msg.token_usage = Some(usage);
                        }
                    }
                }
                _ => continue,
            }
        }

        if messages.is_empty() {
            return None;
        }

        // Title: prefer meta.json description, fall back to first user message
        let title = session_dir
            .and_then(|dir| subagent_title_from_meta(dir, agent_id))
            .unwrap_or_else(|| session_title(first_user_message.as_deref()));

        let full_content = content_parts.join("\n");
        let content_text = truncate_to_bytes(&full_content, FTS_CONTENT_LIMIT);

        let Some(created_at) = first_timestamp else {
            log::warn!(
                "skipping Kimi subagent '{}' without first timestamp from '{}'",
                agent_id,
                source_path
            );
            return None;
        };
        let Some(updated_at) = last_timestamp else {
            log::warn!(
                "skipping Kimi subagent '{}' without last timestamp from '{}'",
                agent_id,
                source_path
            );
            return None;
        };

        let meta = SessionMeta {
            id: agent_id.to_string(),
            provider: Provider::Kimi,
            title,
            project_path: project_path.to_string(),
            project_name: project_name.to_string(),
            created_at,
            updated_at,
            message_count: messages.len() as u32,
            file_size_bytes: 0,
            source_path: source_path.to_string(),
            is_sidechain: true,
            variant_name: None,
            model: None,
            cc_version: None,
            git_branch: None,
            parent_id: Some(parent_session_id.to_string()),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };

        Some(ParsedSession {
            meta,
            messages,
            content_text,
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
        })
    }
}
