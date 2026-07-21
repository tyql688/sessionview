//! Per-step accumulation for Antigravity transcripts: the scan state
//! shared by the full-file and tail parsers, plus the content helpers
//! (user-input cleanup, model extraction, subagent block parsing) that
//! feed it.

use serde::Deserialize;
use serde_json::Value;
use std::collections::VecDeque;

use crate::models::{Message, MessageRole, Provider, TokenUsage};
use crate::tool_metadata::{
    ToolCallFacts, ToolResultFacts, build_tool_metadata, enrich_tool_metadata,
};

use super::lenient_json::{
    decode_antigravity_value, extract_top_level_json_objects, invoke_subagent_prompts,
};
use super::workspace::extract_absolute_paths_from_value;
use super::{Step, ToolCall};

pub(super) fn clean_user_content(content: &str) -> String {
    if let Some(start_idx) = content.find("<USER_REQUEST>")
        && let Some(end_idx) = content.find("</USER_REQUEST>")
    {
        let start = start_idx + "<USER_REQUEST>".len();
        if end_idx > start {
            return content[start..end_idx].trim().to_string();
        }
    }
    content.trim().to_string()
}

/// Extract uploaded image absolute paths from a USER_INPUT step's
/// `<ADDITIONAL_METADATA>` block. Antigravity records images as plain
/// text bullets:
///
/// ```text
/// <ADDITIONAL_METADATA>
/// ...
/// The user has uploaded 1 image(s):
/// - /Users/.../brain/{conv_id}/uploaded_media_{ts}.png
/// You can embed this image in an artifact ...
/// </ADDITIONAL_METADATA>
/// ```
///
/// Returns paths in document order; empty when no uploads are listed
/// or the metadata block is absent.
pub(super) fn extract_uploaded_image_paths(content: &str) -> Vec<String> {
    let Some(start_idx) = content.find("<ADDITIONAL_METADATA>") else {
        return Vec::new();
    };
    let after_open = start_idx + "<ADDITIONAL_METADATA>".len();
    let body_end = content[after_open..]
        .find("</ADDITIONAL_METADATA>")
        .map(|off| after_open + off)
        .unwrap_or(content.len());
    let body = &content[after_open..body_end];

    let Some(header_idx) = body.find("The user has uploaded ") else {
        return Vec::new();
    };
    // The uploads list lives on the lines after the "uploaded N image(s):"
    // header until either an empty line or the next prose line that does
    // not start with "- ". Stop the scan at the first such break so we
    // don't accidentally absorb later metadata bullets.
    let after_header = match body[header_idx..].find('\n') {
        Some(off) => &body[header_idx + off + 1..],
        None => return Vec::new(),
    };
    let mut paths = Vec::new();
    for line in after_header.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        let Some(rest) = trimmed.strip_prefix("- ") else {
            break;
        };
        let path = rest.trim();
        if !path.is_empty() {
            paths.push(path.to_string());
        }
    }
    paths
}

/// Output of scanning an `INVOKE_SUBAGENT` step: the conversationId of each
/// spawned subagent plus the first workspace URI declared in that block.
///
/// The step content is *not* a single JSON document — antigravity glues
/// one or more pretty-printed JSON objects together with prose ("Created
/// the following subagents:\n{...}\n{...}"). We split it into candidate
/// objects with a brace-counting scanner that respects string literals
/// and escapes, then deserialise each block with serde. Malformed blocks
/// are skipped with a warning so we never extract garbage UUIDs from
/// surrounding prose.
#[derive(Debug, Default, Clone)]
pub(super) struct InvokeSubagentInfo {
    pub(super) conversation_ids: Vec<String>,
    pub(super) workspace: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InvokeSubagentBlock {
    #[serde(rename = "conversationId")]
    conversation_id: Option<String>,
    #[serde(rename = "workspaceUris", default)]
    workspace_uris: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub(super) struct ManageSubagentsInfo {
    pub(super) conversation_ids: Vec<String>,
    pub(super) prompts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ManageSubagentBlock {
    spec: Option<ManageSubagentSpec>,
    result: Option<ManageSubagentResult>,
}

#[derive(Debug, Deserialize)]
struct ManageSubagentSpec {
    #[serde(rename = "initialPrompt")]
    initial_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManageSubagentResult {
    #[serde(rename = "conversationId")]
    conversation_id: Option<String>,
}

pub(super) fn parse_invoke_subagent_content(content: &str) -> InvokeSubagentInfo {
    let mut info = InvokeSubagentInfo::default();
    for block in extract_top_level_json_objects(content) {
        let parsed: InvokeSubagentBlock = match serde_json::from_str(&block) {
            Ok(b) => b,
            Err(error) => {
                log::warn!("skipping INVOKE_SUBAGENT block (parse error: {error})");
                continue;
            }
        };
        if let Some(id) = parsed
            .conversation_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && !info.conversation_ids.iter().any(|existing| existing == id)
        {
            info.conversation_ids.push(id.to_string());
        }
        if info.workspace.is_none() {
            for uri in &parsed.workspace_uris {
                if let Some(path) = uri.strip_prefix("file://") {
                    let path = path.trim();
                    if !path.is_empty() {
                        info.workspace = Some(path.to_string());
                        break;
                    }
                }
            }
        }
    }
    info
}

pub(super) fn parse_manage_subagents_content(content: &str) -> ManageSubagentsInfo {
    let mut info = ManageSubagentsInfo::default();
    for block in extract_top_level_json_objects(content) {
        let parsed: ManageSubagentBlock = match serde_json::from_str(&block) {
            Ok(b) => b,
            Err(error) => {
                log::warn!("skipping manage_subagents block (parse error: {error})");
                continue;
            }
        };
        let Some(id) = parsed
            .result
            .as_ref()
            .and_then(|result| result.conversation_id.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if info.conversation_ids.iter().any(|existing| existing == id) {
            continue;
        }
        let prompt = parsed
            .spec
            .as_ref()
            .and_then(|spec| spec.initial_prompt.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();
        info.conversation_ids.push(id.to_string());
        info.prompts.push(prompt);
    }
    info
}

/// Pull the `Recipient` UUID out of a `send_message` tool call. The child
/// transcript uses this to tell us who its parent is — independent of
/// whether we have already seen the parent's `INVOKE_SUBAGENT` record.
///
/// Routes through [`decode_antigravity_value`] so the JSON-encoded string
/// (`"\"<uuid>\""`) is unwrapped by the same code path that decodes every
/// other antigravity arg — keeps both consumers in sync on edge cases like
/// escaped inner quotes.
pub(super) fn recipient_from_send_message(tool_call: &ToolCall) -> Option<String> {
    if tool_call.name != "send_message" {
        return None;
    }
    let raw = tool_call.args.as_ref()?.get("Recipient")?;
    match decode_antigravity_value(raw) {
        Value::String(decoded) => {
            let trimmed = decoded.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

fn normalize_antigravity_model(model: &str) -> String {
    let lower = model.to_lowercase();
    lower
        .replace(" (high)", "")
        .replace(" (low)", "")
        .replace(" (medium)", "")
        .replace(" (balanced)", "")
        .replace(" flash", "-flash")
        .replace(" pro", "-pro")
        .replace(' ', "-")
}

pub(super) fn extract_model_from_content(content: &str) -> Option<String> {
    let start_tag = "<USER_SETTINGS_CHANGE>";
    let end_tag = "</USER_SETTINGS_CHANGE>";
    let start_idx = content.find(start_tag)?;
    let end_idx = content.find(end_tag)?;
    if end_idx <= start_idx {
        return None;
    }
    let block = &content[start_idx + start_tag.len()..end_idx];

    let model_sel = "Model Selection";
    let pos = block.find(model_sel)?;
    let from_pos = block[pos..].find(" from ")?;
    let to_pos = block[pos + from_pos..].find(" to ")?;

    let model_start = pos + from_pos + to_pos + " to ".len();
    let rest = &block[model_start..];

    let mut chars = rest.chars().peekable();
    let mut model_len = 0;
    while let Some(c) = chars.next() {
        if c == '\n' || c == '`' {
            break;
        }
        if c == '.' {
            if let Some(&next_c) = chars.peek() {
                if next_c == ' ' || next_c == '\n' || next_c == '`' {
                    break;
                }
            } else {
                break;
            }
        }
        model_len += c.len_utf8();
    }
    let model_name = rest[..model_len].trim().to_string();
    if !model_name.is_empty() {
        Some(normalize_antigravity_model(&model_name))
    } else {
        None
    }
}

/// Mutable scan state shared by the full-file parser and the tail-only
/// parser. All per-step accumulation lives here so both entry points
/// dispatch through the same `process_step` body.
pub(super) struct AntigravityScanAccum {
    pub(super) messages: Vec<Message>,
    pub(super) pending_tool_indices: VecDeque<usize>,
    /// Absolute paths observed inside tool_call args — used by the full
    /// parser to recover `project_path` when the history file doesn't
    /// have an entry. The tail parser collects them too but discards them.
    pub(super) candidate_paths: Vec<String>,
    pub(super) first_user_msg: Option<String>,
    pub(super) first_timestamp: Option<String>,
    pub(super) last_timestamp: Option<String>,
    pub(super) current_model: Option<String>,
    context_chars: usize,
    /// Structured subagent links extracted from the transcript itself —
    /// only populated by the full parser since the tail rarely sees the
    /// INVOKE_SUBAGENT step (it lives near the start of the parent file).
    pub(super) child_session_ids: Vec<String>,
    pub(super) invoke_workspace: Option<String>,
    pub(super) parent_from_send: Option<String>,
    pub(super) parse_warning_count: u32,
}

impl AntigravityScanAccum {
    pub(super) fn new() -> Self {
        Self {
            messages: Vec::new(),
            pending_tool_indices: VecDeque::new(),
            candidate_paths: Vec::new(),
            first_user_msg: None,
            first_timestamp: None,
            last_timestamp: None,
            current_model: None,
            context_chars: 0,
            child_session_ids: Vec::new(),
            invoke_workspace: None,
            parent_from_send: None,
            parse_warning_count: 0,
        }
    }

    /// Dispatch a single step into the running message stream. Called by
    /// both `parse_session_file` (every step) and `parse_session_tail`
    /// (only the tail window's steps) — they share this body to keep
    /// rendering identical inside the overlap.
    ///
    /// This is a thin dispatcher: it runs the per-step preamble (timestamp
    /// bookkeeping, tool-arg path/recipient scan, INVOKE_SUBAGENT
    /// pre-parse) that applies to every step type, then routes to the
    /// per-step-type handler. Each handler runs to completion and returns;
    /// there is no early-exit / loop control flow at the dispatcher level,
    /// so the handlers are plain method calls.
    pub(super) fn process_step(&mut self, step: &Step, conversation_id: &str) {
        let timestamp_str = Some(step.created_at.clone());
        if self.first_timestamp.is_none() {
            self.first_timestamp = Some(step.created_at.clone());
        }
        self.last_timestamp = Some(step.created_at.clone());

        if let Some(ref tool_calls) = step.tool_calls {
            for tc in tool_calls {
                if let Some(ref args) = tc.args {
                    extract_absolute_paths_from_value(args, &mut self.candidate_paths);
                }
                if self.parent_from_send.is_none()
                    && let Some(recipient) = recipient_from_send_message(tc)
                    && recipient != conversation_id
                {
                    self.parent_from_send = Some(recipient);
                }
            }
        }

        // Parse INVOKE_SUBAGENT step content once per step and stash the result
        // so the inner `_` arm (which enriches the pending tool message) doesn't
        // have to re-scan and risk diverging from the session-level child list.
        let invoke_info: Option<InvokeSubagentInfo> = if step.step_type == "INVOKE_SUBAGENT" {
            let content = step.content.as_deref().unwrap_or("");
            let info = parse_invoke_subagent_content(content);
            for id in &info.conversation_ids {
                if id != conversation_id && !self.child_session_ids.contains(id) {
                    self.child_session_ids.push(id.clone());
                }
            }
            if self.invoke_workspace.is_none() {
                self.invoke_workspace = info.workspace.clone();
            }
            Some(info)
        } else {
            None
        };

        match step.step_type.as_str() {
            "USER_INPUT" => self.handle_user_input(step, timestamp_str),
            "PLANNER_RESPONSE" => self.handle_planner_response(step, timestamp_str),
            "CONVERSATION_HISTORY" => {}
            _ => self.enrich_pending_tool(step, conversation_id, invoke_info.as_ref()),
        }
    }

    /// Handle a `USER_INPUT` step: record the (cleaned) user message,
    /// append any uploaded-image markers, track the current model, and
    /// seed the first-user-message / title source.
    fn handle_user_input(&mut self, step: &Step, timestamp_str: Option<String>) {
        let content = step.content.clone().unwrap_or_default();
        if let Some(m) = extract_model_from_content(&content) {
            self.current_model = Some(m);
        }
        let mut clean = clean_user_content(&content);
        // Append uploaded-image markers so the UI can render
        // them (and the shared image cache picks them up).
        // First-user-msg / title still uses the bare text — see
        // `provider_utils::session_title` which strips markers.
        let image_paths = extract_uploaded_image_paths(&content);
        if !image_paths.is_empty() {
            if !clean.is_empty() {
                clean.push('\n');
            }
            for (i, path) in image_paths.iter().enumerate() {
                if i > 0 {
                    clean.push('\n');
                }
                clean.push_str(&format!("[Image: source: {path}]"));
            }
        }
        self.context_chars += clean.len();
        if self.first_user_msg.is_none() {
            self.first_user_msg = Some(clean.clone());
        }
        self.messages.push(Message {
            timestamp: timestamp_str,
            model: self.current_model.clone(),
            ..Message::user(clean)
        });
    }

    /// Handle a `PLANNER_RESPONSE` step: emit the optional thinking
    /// message, the assistant content message, and one tool message per
    /// `tool_call`, attaching the estimated token usage to whichever
    /// message anchors the turn.
    fn handle_planner_response(&mut self, step: &Step, timestamp_str: Option<String>) {
        let mut thinking_len = 0;
        if let Some(thinking) = &step.thinking {
            thinking_len = thinking.len();
            if !thinking.trim().is_empty() {
                self.messages.push(Message {
                    timestamp: timestamp_str.clone(),
                    ..Message::system(format!("[thinking]\n{}", thinking.trim()))
                });
            }
        }

        let mut assistant_content_len = 0;
        let mut has_assistant_msg = false;
        if let Some(content) = &step.content {
            assistant_content_len = content.len();
            if !content.trim().is_empty() {
                let input_tokens = (self.context_chars / 4).max(1) as u32;
                let output_tokens = ((thinking_len + assistant_content_len) / 4).max(1) as u32;

                self.messages.push(Message {
                    timestamp: timestamp_str.clone(),
                    token_usage: Some(TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                    }),
                    model: self.current_model.clone(),
                    ..Message::assistant(content.clone())
                });
                has_assistant_msg = true;
            }
        }

        if let Some(tool_calls) = &step.tool_calls {
            for (tc_idx, tc) in tool_calls.iter().enumerate() {
                let decoded_args = tc.args.as_ref().map(decode_antigravity_value);
                let subagent_prompts: Vec<String> = if tc.name == "invoke_subagent" {
                    invoke_subagent_prompts(tc.args.as_ref().and_then(|args| args.get("Subagents")))
                } else {
                    Vec::new()
                };
                let mut metadata = build_tool_metadata(ToolCallFacts {
                    provider: Provider::Antigravity,
                    raw_name: &tc.name,
                    input: decoded_args.as_ref(),
                    call_id: None,
                    assistant_id: None,
                });
                if !subagent_prompts.is_empty() {
                    metadata.structured = Some(serde_json::json!({
                        "childPrompts": subagent_prompts.clone(),
                    }));
                }
                let canonical = metadata.canonical_name.clone();
                let idx = self.messages.len();
                let tool_input_str = decoded_args
                    .as_ref()
                    .map(|args| serde_json::to_string(args).unwrap_or_default());
                if let Some(ref args_str) = tool_input_str {
                    self.context_chars += args_str.len();
                }

                let token_usage = if !has_assistant_msg && tc_idx == 0 {
                    let input_tokens = (self.context_chars / 4).max(1) as u32;
                    let output_tokens = (thinking_len / 4).max(1) as u32;
                    Some(TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                    })
                } else {
                    None
                };

                let model = if !has_assistant_msg && tc_idx == 0 {
                    self.current_model.clone()
                } else {
                    None
                };

                self.messages.push(Message {
                    timestamp: timestamp_str.clone(),
                    tool_name: Some(canonical),
                    tool_input: tool_input_str,
                    tool_metadata: Some(metadata),
                    token_usage,
                    model,
                    ..Message::new(MessageRole::Tool, String::new())
                });
                self.pending_tool_indices.push_back(idx);
            }
        }

        self.context_chars += thinking_len;
        self.context_chars += assistant_content_len;
    }

    /// Handle a tool-result step (the catch-all `_` arm): when the step
    /// comes from the MODEL/SYSTEM source, pop the oldest pending tool
    /// message and enrich it with this step's content, error status, and —
    /// for INVOKE_SUBAGENT results — the spawned child conversationIds.
    pub(super) fn enrich_pending_tool(
        &mut self,
        step: &Step,
        conversation_id: &str,
        invoke_info: Option<&InvokeSubagentInfo>,
    ) {
        if (step.source == "MODEL" || step.source == "SYSTEM")
            && let Some(idx) = self.pending_tool_indices.pop_front()
        {
            let content = step.content.clone().unwrap_or_default();
            let invoke_children: Vec<String> = invoke_info
                .as_ref()
                .map(|info| {
                    info.conversation_ids
                        .iter()
                        .filter(|id| id.as_str() != conversation_id)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let mut manage_info = self
                .messages
                .get(idx)
                .and_then(|message| message.tool_metadata.as_ref())
                .filter(|metadata| metadata.raw_name == "manage_subagents")
                .map(|_| parse_manage_subagents_content(&content))
                .unwrap_or_default();

            if !manage_info.conversation_ids.is_empty() {
                let mut new_ids = Vec::new();
                let mut new_prompts = Vec::new();
                for (id, prompt) in manage_info
                    .conversation_ids
                    .iter()
                    .zip(manage_info.prompts.iter())
                {
                    if id == conversation_id || self.child_session_ids.contains(id) {
                        continue;
                    }
                    self.child_session_ids.push(id.clone());
                    new_ids.push(id.clone());
                    new_prompts.push(prompt.clone());
                }
                manage_info.conversation_ids = new_ids;
                manage_info.prompts = new_prompts;
            }

            if let Some(msg) = self.messages.get_mut(idx) {
                self.context_chars += content.len();
                msg.content = content;

                if let Some(metadata) = msg.tool_metadata.as_mut() {
                    enrich_tool_metadata(
                        metadata,
                        ToolResultFacts {
                            raw_result: None,
                            is_error: Some(step.status == "ERROR"),
                            status: Some(&step.status),
                            artifact_path: None,
                            raw_output: None,
                        },
                    );

                    if !invoke_children.is_empty() || !manage_info.conversation_ids.is_empty() {
                        let (child_ids, prompts) = if !invoke_children.is_empty() {
                            let prompts = metadata
                                .structured
                                .as_ref()
                                .and_then(|v| v.get("childPrompts"))
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!([]));
                            (invoke_children, prompts)
                        } else {
                            (
                                manage_info.conversation_ids.clone(),
                                serde_json::json!(manage_info.prompts),
                            )
                        };
                        metadata.structured = Some(serde_json::json!({
                            "childConversationIds": child_ids,
                            "childPrompts": prompts,
                        }));
                        metadata.result_kind = Some("agent_summary".to_string());
                    }
                }
            }
        }
    }
}
