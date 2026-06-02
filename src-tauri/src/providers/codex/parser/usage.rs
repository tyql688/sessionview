//! Codex token-usage parsing and aggregation. Codex emits usage as
//! `event_msg.token_count` payloads carrying either a per-turn
//! `last_token_usage` or a running `total_token_usage`; this module
//! normalizes those raw counts, derives per-event deltas when only the
//! cumulative total is present, and folds usage onto the most recent
//! assistant message. The cross-line running total lives on the
//! accumulator (`previous_token_totals`) so consecutive `token_count`
//! events resolve deltas correctly.

use serde_json::Value;

use crate::models::{Message, MessageRole, TokenUsage};

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

pub(super) type CodexRawUsageCounts = (u64, u64, u64, u64, u64);
type RawCodexUsage = (Option<String>, CodexRawUsageCounts);

fn normalize_codex_raw_usage(value: &Value) -> Option<RawCodexUsage> {
    let input = value
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cached = value
        .get("cached_input_tokens")
        .or_else(|| value.get("cache_read_input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = value
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reasoning = value
        .get("reasoning_output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total = value
        .get("total_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| input + output);
    let model = value
        .get("model")
        .or_else(|| value.get("model_name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some((model, (input, cached, output, reasoning, total)))
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
        (Some(last), total) => {
            if let Some((_, total_counts)) = total {
                *previous_totals = Some(total_counts);
            }
            Some(last)
        }
        (None, Some((model, total_counts))) => {
            let delta = subtract_codex_usage(total_counts, *previous_totals);
            *previous_totals = Some(total_counts);
            Some((model, delta))
        }
        (None, None) => None,
    }
}

pub(super) fn codex_token_usage_from_counts(
    (input, cached, output, reasoning, total): CodexRawUsageCounts,
) -> Option<TokenUsage> {
    if input == 0 && cached == 0 && output == 0 && reasoning == 0 && total == 0 {
        return None;
    }

    Some(TokenUsage {
        input_tokens: token_count_to_u32("input_tokens", input)?,
        output_tokens: token_count_to_u32("output_tokens", output)?,
        cache_read_input_tokens: token_count_to_u32("cache_read_input_tokens", cached.min(input))?,
        cache_creation_input_tokens: 0,
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
    let prev = previous.unwrap_or((0, 0, 0, 0, 0));
    (
        current.0.saturating_sub(prev.0),
        current.1.saturating_sub(prev.1),
        current.2.saturating_sub(prev.2),
        current.3.saturating_sub(prev.3),
        current.4.saturating_sub(prev.4),
    )
}
