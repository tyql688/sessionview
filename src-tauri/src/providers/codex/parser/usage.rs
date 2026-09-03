//! Codex token-usage parsing and aggregation. Current rollouts emit one
//! top-level `token_usage_record` per model response, with a stable
//! `response_id`; older `event_msg.token_count` records remain in the same
//! stream and usually repeat that response usage a few lines later. Both
//! channels are parsed, then paired within the active turn by model and all
//! token components so the response is represented once. Records present in
//! only one channel still contribute usage. The older channel's cumulative
//! `total_token_usage` remains a delta fallback for legacy logs.

use std::path::Path;

use serde_json::Value;

use crate::models::{Message, MessageRole, TokenUsage};
use crate::provider::UsageEvent;

use super::{CodexLine, CodexScanAccum};

pub(super) fn extract_codex_model(value: &Value) -> Option<String> {
    value
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            value
                .get("info")
                .and_then(|info| info.get("model"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("info")
                .and_then(|info| info.get("model_name"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("metadata")
                .and_then(|meta| meta.get("model"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::providers::codex) struct CodexRawUsageCounts {
    pub(super) input: u64,
    pub(super) cache_read: u64,
    pub(super) cache_write: u64,
    pub(super) output: u64,
    pub(super) reasoning: u64,
    pub(super) total: u64,
}

impl CodexRawUsageCounts {
    pub(super) fn any_nonzero(self) -> bool {
        self.input != 0
            || self.cache_read != 0
            || self.cache_write != 0
            || self.output != 0
            || self.reasoning != 0
            || self.total != 0
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(in crate::providers::codex) struct CodexUsageFingerprint {
    turn_id: Option<String>,
    model: String,
    counts: CodexRawUsageCounts,
}

type RawCodexUsage = (Option<String>, CodexRawUsageCounts);

fn normalize_codex_raw_usage(value: &Value) -> Option<RawCodexUsage> {
    let input = value.get("input_tokens").and_then(Value::as_u64);
    let cache_read = value
        .get("cached_input_tokens")
        .or_else(|| value.get("cache_read_input_tokens"))
        .and_then(Value::as_u64);
    let cache_write = value
        .get("cache_write_input_tokens")
        .or_else(|| value.get("cache_creation_input_tokens"))
        .or_else(|| value.get("cache_write_tokens"))
        .and_then(Value::as_u64);
    let output = value.get("output_tokens").and_then(Value::as_u64);
    let reasoning = value.get("reasoning_output_tokens").and_then(Value::as_u64);
    let total = value.get("total_tokens").and_then(Value::as_u64);
    if input.is_none()
        && cache_read.is_none()
        && cache_write.is_none()
        && output.is_none()
        && reasoning.is_none()
        && total.is_none()
    {
        return None;
    }
    let input = input.unwrap_or(0);
    let output = output.unwrap_or(0);
    let model = value
        .get("model")
        .or_else(|| value.get("model_name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some((
        model,
        CodexRawUsageCounts {
            input,
            cache_read: cache_read.unwrap_or(0),
            cache_write: cache_write.unwrap_or(0),
            output,
            reasoning: reasoning.unwrap_or(0),
            total: total.unwrap_or_else(|| input.saturating_add(output)),
        },
    ))
}

pub(super) fn codex_usage_from_info(
    info: &Value,
    previous_totals: &mut Option<CodexRawUsageCounts>,
) -> Option<RawCodexUsage> {
    let last_usage = info
        .get("last_token_usage")
        .and_then(normalize_codex_raw_usage);
    let total_usage = info
        .get("total_token_usage")
        .and_then(normalize_codex_raw_usage);

    match (last_usage, total_usage) {
        // Per-turn `last_token_usage` is authoritative; keep the running total in
        // sync for the delta fallback below.
        (Some(last), total) => {
            if let Some((_, total_counts)) = total {
                *previous_totals = Some(total_counts);
            }
            Some(last)
        }
        // Older logs carry only the cumulative total — recover the per-turn
        // amount as the delta from the previous event.
        (None, Some((model, total_counts))) => {
            let delta = subtract_codex_usage(total_counts, *previous_totals);
            *previous_totals = Some(total_counts);
            Some((model, delta))
        }
        (None, None) => None,
    }
}

pub(super) fn codex_token_usage_from_counts(counts: CodexRawUsageCounts) -> Option<TokenUsage> {
    if !counts.any_nonzero() {
        return None;
    }
    let Some(cache_total) = counts.cache_read.checked_add(counts.cache_write) else {
        log::warn!("skipping Codex token usage event: cache input count overflowed u64");
        return None;
    };
    if cache_total > counts.input {
        log::warn!(
            "skipping Codex token usage event: cache input {cache_total} exceeds total input {}",
            counts.input
        );
        return None;
    }

    Some(TokenUsage {
        input_tokens: token_count_to_u32("input_tokens", counts.input - cache_total)?,
        output_tokens: token_count_to_u32("output_tokens", counts.output)?,
        cache_read_input_tokens: token_count_to_u32("cache_read_input_tokens", counts.cache_read)?,
        cache_creation_input_tokens: token_count_to_u32(
            "cache_creation_input_tokens",
            counts.cache_write,
        )?,
    })
}

fn token_count_to_u32(field: &str, value: u64) -> Option<u32> {
    match u32::try_from(value) {
        Ok(value) => Some(value),
        Err(_) => {
            log::warn!("skipping Codex token usage event: {field}={value} exceeds u32");
            None
        }
    }
}

pub(super) fn add_usage_to_last_assistant(
    messages: &mut [Message],
    usage: TokenUsage,
    model: Option<String>,
) {
    let Some(last_msg) = messages
        .iter_mut()
        .rev()
        .find(|m| m.role == MessageRole::Assistant)
    else {
        return;
    };

    if last_msg.model.is_none() {
        last_msg.model = model;
    }

    if let Some(existing) = last_msg.token_usage.as_mut() {
        existing.input_tokens = existing.input_tokens.saturating_add(usage.input_tokens);
        existing.output_tokens = existing.output_tokens.saturating_add(usage.output_tokens);
        existing.cache_read_input_tokens = existing
            .cache_read_input_tokens
            .saturating_add(usage.cache_read_input_tokens);
        existing.cache_creation_input_tokens = existing
            .cache_creation_input_tokens
            .saturating_add(usage.cache_creation_input_tokens);
    } else {
        last_msg.token_usage = Some(usage);
    }
}

fn subtract_codex_usage(
    current: CodexRawUsageCounts,
    previous: Option<CodexRawUsageCounts>,
) -> CodexRawUsageCounts {
    let prev = previous.unwrap_or(CodexRawUsageCounts {
        input: 0,
        cache_read: 0,
        cache_write: 0,
        output: 0,
        reasoning: 0,
        total: 0,
    });
    CodexRawUsageCounts {
        input: current.input.saturating_sub(prev.input),
        cache_read: current.cache_read.saturating_sub(prev.cache_read),
        cache_write: current.cache_write.saturating_sub(prev.cache_write),
        output: current.output.saturating_sub(prev.output),
        reasoning: current.reasoning.saturating_sub(prev.reasoning),
        total: current.total.saturating_sub(prev.total),
    }
}

impl CodexScanAccum {
    pub(super) fn begin_usage_turn(&mut self, turn_id: Option<&str>) {
        let next_turn_id = turn_id.filter(|id| !id.is_empty()).map(str::to_string);
        if self.current_turn_id != next_turn_id {
            self.unmatched_token_count_usage.clear();
            self.unmatched_token_usage_records.clear();
        }
        self.current_turn_id = next_turn_id;
    }

    pub(super) fn handle_token_usage_record(
        &mut self,
        entry: &CodexLine,
        payload: &Value,
        path: &Path,
    ) {
        let Some(response_id) = payload
            .get("response_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            self.warn_bad_token_usage_record(path, "missing response_id");
            return;
        };
        let Some(turn_id) = payload
            .get("turn_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            self.warn_bad_token_usage_record(path, "missing turn_id");
            return;
        };
        if let Some(current_turn_id) = self.current_turn_id.as_deref() {
            if current_turn_id != turn_id {
                self.warn_bad_token_usage_record(path, "turn_id disagrees with turn_context");
                return;
            }
        } else {
            self.current_turn_id = Some(turn_id.to_string());
        }
        let Some(usage_value) = payload.get("usage") else {
            self.warn_bad_token_usage_record(path, "missing usage");
            return;
        };
        let required_counts_are_valid = ["input_tokens", "output_tokens"]
            .into_iter()
            .all(|field| usage_value.get(field).and_then(Value::as_u64).is_some());
        let optional_counts_are_valid = [
            "cached_input_tokens",
            "cache_read_input_tokens",
            "cache_write_input_tokens",
            "cache_creation_input_tokens",
            "cache_write_tokens",
            "reasoning_output_tokens",
            "total_tokens",
        ]
        .into_iter()
        .all(|field| usage_value.get(field).is_none_or(Value::is_u64));
        if !required_counts_are_valid || !optional_counts_are_valid {
            self.warn_bad_token_usage_record(path, "usage token fields are not unsigned integers");
            return;
        }
        let Some((usage_model, counts)) = normalize_codex_raw_usage(usage_value) else {
            self.warn_bad_token_usage_record(path, "usage has no token fields");
            return;
        };
        let Some(timestamp) = entry.timestamp.as_ref() else {
            self.warn_bad_token_usage_record(path, "missing timestamp");
            return;
        };
        let resolved_model = extract_codex_model(usage_value)
            .or_else(|| extract_codex_model(payload))
            .or_else(|| self.current_model.clone())
            .or(usage_model);
        let Some(resolved_model) = resolved_model else {
            self.warn_bad_token_usage_record(path, "model cannot be resolved");
            return;
        };
        self.current_model = Some(resolved_model.clone());
        self.models_seen.insert(resolved_model.clone());
        if self.model.is_none() {
            self.model = Some(resolved_model.clone());
        }
        // `turn_token_usage` and `thread_token_usage` are cumulative snapshots.
        // Emitting either beside per-response `usage` would double-count; the
        // stable response id plus the per-response counts are the normalized
        // event SessionView needs for display and aggregation.
        self.ingest_token_usage_record(
            timestamp,
            resolved_model,
            counts,
            Some(turn_id.to_string()),
            response_id,
            path,
        );
    }

    pub(super) fn ingest_token_count_usage(
        &mut self,
        timestamp: &str,
        model: String,
        counts: CodexRawUsageCounts,
        turn_id: Option<String>,
    ) {
        if !counts.any_nonzero() {
            return;
        }
        let fingerprint = CodexUsageFingerprint {
            turn_id,
            model: model.clone(),
            counts,
        };
        if self
            .unmatched_token_usage_records
            .get_mut(&fingerprint)
            .and_then(Vec::pop)
            .is_some()
        {
            return;
        }
        if let Some(event_index) = self.push_usage_event(timestamp, model, counts, None) {
            self.unmatched_token_count_usage
                .entry(fingerprint)
                .or_default()
                .push(event_index);
        }
    }

    fn ingest_token_usage_record(
        &mut self,
        timestamp: &str,
        model: String,
        counts: CodexRawUsageCounts,
        turn_id: Option<String>,
        response_id: &str,
        path: &Path,
    ) {
        if !counts.any_nonzero() {
            return;
        }
        let fingerprint = CodexUsageFingerprint {
            turn_id,
            model: model.clone(),
            counts,
        };
        if let Some(previous) = self.seen_token_usage_record_ids.get(response_id) {
            if previous != &fingerprint {
                self.warn_bad_token_usage_record(path, "response_id repeats with different usage");
            }
            return;
        }
        self.seen_token_usage_record_ids
            .insert(response_id.to_string(), fingerprint.clone());
        let usage_hash = format!("codex-response:{response_id}");
        if let Some(event_index) = self
            .unmatched_token_count_usage
            .get_mut(&fingerprint)
            .and_then(Vec::pop)
        {
            self.usage_events[event_index].usage_hash = Some(usage_hash);
            return;
        }
        if let Some(event_index) = self.push_usage_event(timestamp, model, counts, Some(usage_hash))
        {
            self.unmatched_token_usage_records
                .entry(fingerprint)
                .or_default()
                .push(event_index);
        }
    }

    fn push_usage_event(
        &mut self,
        timestamp: &str,
        model: String,
        counts: CodexRawUsageCounts,
        usage_hash: Option<String>,
    ) -> Option<usize> {
        let Some(usage) = codex_token_usage_from_counts(counts) else {
            self.parse_warning_count = self.parse_warning_count.saturating_add(1);
            return None;
        };
        let event_index = self.usage_events.len();
        self.usage_events.push(UsageEvent {
            timestamp: timestamp.to_string(),
            model: model.clone(),
            turn_count: 1,
            input_tokens: u64::from(usage.input_tokens),
            output_tokens: u64::from(usage.output_tokens),
            cache_read_input_tokens: u64::from(usage.cache_read_input_tokens),
            cache_creation_input_tokens: u64::from(usage.cache_creation_input_tokens),
            usage_hash,
            cost_usd: None,
        });
        add_usage_to_last_assistant(&mut self.messages, usage, Some(model));
        Some(event_index)
    }

    fn warn_bad_token_usage_record(&mut self, path: &Path, reason: &str) {
        log::warn!(
            "skipping malformed Codex token_usage_record in '{}': {reason}",
            path.display()
        );
        self.parse_warning_count = self.parse_warning_count.saturating_add(1);
    }
}
