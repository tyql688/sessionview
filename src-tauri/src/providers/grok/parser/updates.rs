//! `updates.jsonl` scanning: timestamp/usage anchors plus the hand-off to
//! pre-compaction history reconstruction for compacted sessions.

use std::collections::HashMap;
use std::io::BufReader;
use std::ops::ControlFlow;
use std::path::Path;

use serde_json::Value;

use crate::models::Message;
use crate::provider::UsageEvent;
use crate::provider_utils::for_each_jsonl_record;

/// Per-turn usage totals summed across models, for attaching to the turn's
/// final assistant message (grok has no per-message usage — turn level is
/// the finest granularity the data offers).
pub(super) struct GrokTurnUsage {
    pub(super) timestamp: String,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_read_tokens: u64,
}

/// Timestamp/usage anchors recovered from `updates.jsonl`.
#[derive(Default)]
pub(super) struct UpdateAnchors {
    /// promptIndex → RFC3339 timestamp of the `user_message_chunk` update.
    pub(super) user_timestamps: HashMap<u64, String>,
    /// toolCallId → RFC3339 timestamp of the `tool_call` update.
    pub(super) tool_timestamps: HashMap<String, String>,
    /// promptIndex → that turn's `turn_completed` usage (last one wins —
    /// a regenerated turn re-reports its final totals).
    pub(super) turn_usages: HashMap<u64, GrokTurnUsage>,
    /// Most recent promptIndex seen while scanning; `turn_completed`
    /// carries no index of its own, so it binds to the prompt it follows.
    pub(super) last_prompt_index: Option<u64>,
    pub(super) usage_events: Vec<UsageEvent>,
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
    (
        anchors,
        builder
            .map(super::history::HistoryBuilder::into_messages)
            .unwrap_or_default(),
    )
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
            let (Some(id), Some(ts)) =
                (update.get("toolCallId").and_then(Value::as_str), timestamp)
            else {
                return;
            };
            anchors.tool_timestamps.entry(id.to_string()).or_insert(ts);
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
                        output_tokens: usage
                            .get("outputTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        cache_read_tokens: usage
                            .get("cachedReadTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    },
                );
            }
            for (model, usage) in model_usage {
                let input = usage
                    .get("inputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let cache_read = usage
                    .get("cachedReadTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(input);
                anchors.usage_events.push(UsageEvent {
                    timestamp: ts.clone(),
                    model: model.clone(),
                    input_tokens: input.saturating_sub(cache_read),
                    output_tokens: usage
                        .get("outputTokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    cache_read_input_tokens: cache_read,
                    cache_creation_input_tokens: 0,
                    usage_hash: None,
                });
            }
        }
        _ => {}
    }
}
