//! `updates.jsonl` scanning: timestamp/usage anchors, tool result enrichment,
//! session-level notes (plan/goal/recap), plus the hand-off to pre-compaction
//! history reconstruction for compacted sessions.

use std::collections::HashMap;
use std::io::BufReader;
use std::ops::ControlFlow;
use std::path::Path;

use serde_json::Value;

use crate::models::Message;
use crate::provider::UsageEvent;
use crate::provider::util::for_each_jsonl_record;

/// Grok reports cost as integer ticks; 1e10 ticks = $1 USD.
const USD_TICKS_PER_USD: f64 = 1e10;

/// Per-turn usage totals summed across models, for attaching to the turn's
/// final assistant message (grok has no per-message usage — turn level is
/// the finest granularity the data offers).
pub(super) struct GrokTurnUsage {
    pub(super) timestamp: String,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_read_tokens: u64,
}

/// Status / body recovered from `tool_call` / `tool_call_update` lines so the
/// main chat_history path can enrich tools that only store a bare success
/// string (or no result at all, for backend_tool_call).
#[derive(Debug, Clone, Default)]
pub(super) struct ToolCallState {
    pub(super) timestamp: Option<String>,
    pub(super) status: Option<String>,
    pub(super) title: Option<String>,
    pub(super) raw_name: Option<String>,
    pub(super) raw_input: Option<Value>,
    pub(super) display_text: Option<String>,
    pub(super) raw_output: Option<Value>,
}

/// Timestamp/usage anchors recovered from `updates.jsonl`.
#[derive(Default)]
pub(super) struct UpdateAnchors {
    /// promptIndex → RFC3339 timestamp of the `user_message_chunk` update.
    pub(super) user_timestamps: HashMap<u64, String>,
    /// toolCallId → RFC3339 timestamp of the `tool_call` update.
    pub(super) tool_timestamps: HashMap<String, String>,
    /// toolCallId → latest status/body from tool_call / tool_call_update.
    pub(super) tool_states: HashMap<String, ToolCallState>,
    /// promptIndex → that turn's `turn_completed` usage (last one wins —
    /// a regenerated turn re-reports its final totals).
    pub(super) turn_usages: HashMap<u64, GrokTurnUsage>,
    /// Most recent promptIndex seen while scanning; `turn_completed`
    /// carries no index of its own, so it binds to the prompt it follows.
    pub(super) last_prompt_index: Option<u64>,
    pub(super) usage_events: Vec<UsageEvent>,
    /// Session-level notes (plan snapshots, goals, recaps) to surface as
    /// system messages. Ordered as they appeared in the stream.
    pub(super) session_notes: Vec<Message>,
    pub(super) parse_warning_count: u32,
}

/// Scan `updates.jsonl` for anchors and, when `history_cutoff` is set,
/// reconstruct pre-compaction messages. A missing file degrades gracefully.
pub(super) fn scan_updates(
    updates_path: &Path,
    history_cutoff: Option<u64>,
) -> (UpdateAnchors, Vec<Message>) {
    let mut anchors = UpdateAnchors::default();
    let mut builder = history_cutoff.map(super::history::HistoryBuilder::new);
    let file = match std::fs::File::open(updates_path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "failed to open Grok updates '{}': {error}; messages will lack timestamps, usage, and pre-compaction history",
                updates_path.display()
            );
            return (anchors, Vec::new());
        }
    };

    let stats = for_each_jsonl_record(BufReader::new(file), updates_path, |_, line: Value| {
        collect_anchor(&mut anchors, &line, updates_path);
        if let Some(builder) = builder.as_mut()
            && let (Some(update), Some(kind)) = (
                line.pointer("/params/update"),
                line.pointer("/params/update/sessionUpdate")
                    .and_then(Value::as_str),
            )
        {
            let timestamp = line
                .get("timestamp")
                .and_then(Value::as_i64)
                .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
                .map(|dt| dt.to_rfc3339());
            builder.push_update(kind, update, timestamp.as_deref());
        }
        ControlFlow::Continue(())
    });
    anchors.parse_warning_count = anchors
        .parse_warning_count
        .saturating_add(stats.read_error_count)
        .saturating_add(stats.parse_error_count);
    let mut history_messages = Vec::new();
    if let Some(builder) = builder {
        anchors.parse_warning_count = anchors.parse_warning_count.saturating_add(builder.warnings);
        history_messages = builder.into_messages();
    }
    (anchors, history_messages)
}

pub(super) fn collect_anchor(anchors: &mut UpdateAnchors, line: &Value, updates_path: &Path) {
    let Some(update) = line.pointer("/params/update") else {
        return;
    };
    let Some(kind) = update.get("sessionUpdate").and_then(Value::as_str) else {
        return;
    };
    let timestamp = line
        .get("timestamp")
        .and_then(Value::as_i64)
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|dt| dt.to_rfc3339());

    match kind {
        "user_message_chunk" => {
            let (Some(index), Some(ts)) = (
                update.pointer("/_meta/promptIndex").and_then(Value::as_u64),
                timestamp,
            ) else {
                return;
            };
            // First chunk wins (later chunks would carry later timestamps).
            anchors.user_timestamps.entry(index).or_insert(ts);
            anchors.last_prompt_index = Some(index);
        }
        "tool_call" => {
            let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            if let Some(ts) = timestamp.clone() {
                anchors.tool_timestamps.entry(id.to_string()).or_insert(ts);
            }
            let state = anchors.tool_states.entry(id.to_string()).or_default();
            if state.timestamp.is_none() {
                state.timestamp = timestamp;
            }
            if let Some(status) = update.get("status").and_then(Value::as_str) {
                state.status = Some(status.to_string());
            }
            if let Some(title) = update.get("title").and_then(Value::as_str) {
                state.title = Some(title.to_string());
            }
            if let Some(name) = update
                .pointer("/_meta/x.ai~1tool/name")
                .and_then(Value::as_str)
            {
                state.raw_name = Some(name.to_string());
            }
            if let Some(input) = update.get("rawInput") {
                state.raw_input = Some(input.clone());
            }
        }
        "tool_call_update" => {
            let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            let state = anchors.tool_states.entry(id.to_string()).or_default();
            if let Some(status) = update.get("status").and_then(Value::as_str) {
                state.status = Some(status.to_string());
            }
            if let Some(title) = update.get("title").and_then(Value::as_str) {
                state.title = Some(title.to_string());
            }
            let display = content_blocks_text(update);
            if !display.is_empty() {
                state.display_text = Some(display);
            }
            if let Some(raw) = update.get("rawOutput") {
                state.raw_output = Some(raw.clone());
            }
        }
        "turn_completed" => {
            // Rate-limited/canceled turns legitimately carry no usage.
            let Some(usage) = update.get("usage") else {
                return;
            };
            let Some(ts) = timestamp else {
                log::warn!(
                    "Grok turn_completed without timestamp in '{}'; skipping usage",
                    updates_path.display()
                );
                anchors.parse_warning_count = anchors.parse_warning_count.saturating_add(1);
                return;
            };
            let Some(model_usage) = usage.get("modelUsage").and_then(Value::as_object) else {
                log::warn!(
                    "Grok turn_completed without usage.modelUsage in '{}'; skipping usage",
                    updates_path.display()
                );
                anchors.parse_warning_count = anchors.parse_warning_count.saturating_add(1);
                return;
            };
            let reasoning = usage
                .get("reasoningTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let output = usage
                .get("outputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            // Grok reports reasoning separately; SessionView has no dedicated
            // field, so fold it into output so totals are not undercounted.
            let output_with_reasoning = output.saturating_add(reasoning);
            let cost_usd = usage
                .get("costUsdTicks")
                .and_then(Value::as_u64)
                .map(|ticks| ticks as f64 / USD_TICKS_PER_USD);
            // Bind the turn's totals to the prompt it completes; last
            // write wins (regenerated turns re-report).
            if let Some(prompt_index) = anchors.last_prompt_index {
                anchors.turn_usages.insert(
                    prompt_index,
                    GrokTurnUsage {
                        timestamp: ts.clone(),
                        input_tokens: usage
                            .get("inputTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        output_tokens: output_with_reasoning,
                        cache_read_tokens: usage
                            .get("cachedReadTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    },
                );
            }
            for (model, model_usage_value) in model_usage {
                let input = model_usage_value
                    .get("inputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let cache_read = model_usage_value
                    .get("cachedReadTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(input);
                let model_reasoning = model_usage_value
                    .get("reasoningTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let model_output = model_usage_value
                    .get("outputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .saturating_add(model_reasoning);
                // Turn-level cost only stands in for a single-model turn:
                // copying it onto every model of a multi-model turn would
                // double-count, and the data offers no split to apportion it.
                let model_cost = model_usage_value
                    .get("costUsdTicks")
                    .and_then(Value::as_u64)
                    .map(|ticks| ticks as f64 / USD_TICKS_PER_USD)
                    .or(if model_usage.len() == 1 {
                        cost_usd
                    } else {
                        None
                    });
                anchors.usage_events.push(UsageEvent {
                    timestamp: ts.clone(),
                    model: model.clone(),
                    input_tokens: input.saturating_sub(cache_read),
                    output_tokens: model_output,
                    cache_read_input_tokens: cache_read,
                    cache_creation_input_tokens: 0,
                    usage_hash: None,
                    cost_usd: model_cost,
                });
            }
        }
        "plan" => {
            if let Some(note) = format_plan_note(update) {
                anchors.session_notes.push(Message {
                    timestamp: timestamp.clone(),
                    ..Message::system(note)
                });
            }
        }
        "goal_updated" => {
            if let Some(note) = format_goal_note(update) {
                anchors.session_notes.push(Message {
                    timestamp: timestamp.clone(),
                    ..Message::system(note)
                });
            }
        }
        "session_recap" => {
            if let Some(summary) = update.get("summary").and_then(Value::as_str) {
                let trimmed = summary.trim();
                if !trimmed.is_empty() {
                    anchors.session_notes.push(Message {
                        timestamp: timestamp.clone(),
                        ..Message::system(format!("[Recap] {trimmed}"))
                    });
                }
            }
        }
        "image_compressed" => {
            if let Some(message) = update.get("message").and_then(Value::as_str) {
                let trimmed = message.trim();
                if !trimmed.is_empty() {
                    anchors.session_notes.push(Message {
                        timestamp: timestamp.clone(),
                        ..Message::system(format!("[Image] {trimmed}"))
                    });
                }
            }
        }
        _ => {}
    }
}

fn content_blocks_text(update: &Value) -> String {
    let Some(blocks) = update.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    blocks
        .iter()
        .filter_map(|block| block.pointer("/content/text").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_plan_note(update: &Value) -> Option<String> {
    let entries = update.get("entries").and_then(Value::as_array)?;
    if entries.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for entry in entries {
        let content = entry
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            continue;
        }
        let status = entry
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        lines.push(format!("- [{status}] {content}"));
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!("[Plan]\n{}", lines.join("\n")))
}

fn format_goal_note(update: &Value) -> Option<String> {
    let objective = update
        .get("objective")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let status = update
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("active");
    let phase = update.get("phase").and_then(Value::as_str).unwrap_or("");
    let mut header = format!("[Goal:{status}]");
    if !phase.is_empty() {
        header.push(' ');
        header.push_str(phase);
    }
    Some(format!("{header} {objective}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn turn_completed_line(usage: Value) -> Value {
        json!({
            "timestamp": 1_782_892_920,
            "params": { "update": {
                "sessionUpdate": "turn_completed",
                "usage": usage,
            }},
        })
    }

    fn scan_line(line: &Value) -> UpdateAnchors {
        let mut anchors = UpdateAnchors::default();
        collect_anchor(&mut anchors, line, Path::new("test-updates.jsonl"));
        anchors
    }

    #[test]
    fn cost_ticks_convert_to_usd_per_model() {
        let line = turn_completed_line(json!({
            "inputTokens": 100, "outputTokens": 10,
            "modelUsage": { "grok-4.5": {
                "inputTokens": 100, "outputTokens": 10,
                "costUsdTicks": 25_000_000_000_u64,
            }},
        }));
        let anchors = scan_line(&line);
        assert_eq!(anchors.usage_events.len(), 1);
        assert_eq!(anchors.usage_events[0].cost_usd, Some(2.5));
    }

    #[test]
    fn turn_level_cost_falls_back_only_for_single_model_turns() {
        let single = turn_completed_line(json!({
            "inputTokens": 100, "outputTokens": 10,
            "costUsdTicks": 10_000_000_000_u64,
            "modelUsage": { "grok-4.5": { "inputTokens": 100, "outputTokens": 10 }},
        }));
        let anchors = scan_line(&single);
        assert_eq!(anchors.usage_events[0].cost_usd, Some(1.0));

        // Two models, only a turn-level cost: copying it onto both events
        // would double-count, so both must stay None (catalog estimate).
        let multi = turn_completed_line(json!({
            "inputTokens": 200, "outputTokens": 20,
            "costUsdTicks": 10_000_000_000_u64,
            "modelUsage": {
                "grok-4.5": { "inputTokens": 100, "outputTokens": 10 },
                "grok-4.5-mini": { "inputTokens": 100, "outputTokens": 10 },
            },
        }));
        let anchors = scan_line(&multi);
        assert_eq!(anchors.usage_events.len(), 2);
        assert!(anchors.usage_events.iter().all(|e| e.cost_usd.is_none()));
    }

    #[test]
    fn reasoning_tokens_fold_into_output() {
        let line = turn_completed_line(json!({
            "inputTokens": 100, "outputTokens": 10, "reasoningTokens": 7,
            "modelUsage": { "grok-4.5": {
                "inputTokens": 100, "outputTokens": 10, "reasoningTokens": 7,
            }},
        }));
        let anchors = scan_line(&line);
        assert_eq!(anchors.usage_events[0].output_tokens, 17);
        assert_eq!(anchors.usage_events[0].cost_usd, None);
    }
}
