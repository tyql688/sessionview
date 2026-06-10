use std::collections::{HashMap, HashSet};

use crate::pricing::{self, PricingCatalog};

use super::ParsedSession;

/// A single per-(date, model) token-usage row, written to
/// `session_token_stats` by the indexer. Defined here so the provider
/// trait can produce them without depending on `db::sync`.
#[derive(Clone, Debug)]
pub struct TokenStatRow {
    pub date: String,
    pub model: String,
    pub turn_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

/// A single out-of-band token-usage event captured during parse.
///
/// Codex emits per-turn token counts as `event_msg.token_count` lines
/// that aren't attached to any single message — the indexer's per-date
/// aggregation reads from this Vec instead of re-opening the file.
/// Populated only by the Codex parser today; the shape is generic
/// enough that future providers with similar out-of-band usage streams
/// can reuse the slot.
#[derive(Clone, Debug)]
pub struct UsageEvent {
    pub timestamp: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
}

/// Default token-stats aggregation: walk per-message `token_usage`,
/// dedup by `Message.usage_hash`, apply pricing. Used by all providers
/// except Codex (which overrides with usage-event aggregation).
pub fn default_compute_token_stats_from_messages(
    parsed: &ParsedSession,
    pricing_catalog: Option<&PricingCatalog>,
    mut seen_hashes: Option<&mut HashSet<String>>,
) -> Vec<TokenStatRow> {
    let mut stats_map: HashMap<(String, String), TokenStatRow> = HashMap::with_capacity(32);
    for msg in &parsed.messages {
        let Some(usage) = &msg.token_usage else {
            continue;
        };
        // Dedup: skip if this usage entry was already counted (cross-file).
        if let Some(ref mut seen) = seen_hashes {
            if let Some(ref hash) = msg.usage_hash {
                if !seen.insert(hash.clone()) {
                    continue;
                }
            }
        }

        let Some(timestamp) = msg.timestamp.as_deref() else {
            log::warn!(
                "skipping token usage without message timestamp in session {}",
                parsed.meta.id
            );
            continue;
        };
        let Some(date) = timestamp_to_local_date(timestamp) else {
            log::warn!(
                "skipping token usage with invalid timestamp '{timestamp}' in session {}",
                parsed.meta.id
            );
            continue;
        };
        let Some(model) = msg.model.as_deref().filter(|model| !model.is_empty()) else {
            log::warn!(
                "skipping token usage without message model in session {}",
                parsed.meta.id
            );
            continue;
        };
        // Claude emits `<synthetic>` as the model name for internal
        // placeholder entries (continuation stubs, retry shells, etc.)
        // that don't represent a real API call. Exclude them from token
        // aggregates so daily totals reflect actual usage.
        if model == "<synthetic>" {
            continue;
        }
        let model = model.to_string();
        let entry = stats_map
            .entry((date.clone(), model.clone()))
            .or_insert_with(|| TokenStatRow {
                date,
                model,
                turn_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.0,
            });
        entry.turn_count += 1;
        entry.input_tokens += usage.input_tokens as u64;
        entry.output_tokens += usage.output_tokens as u64;
        entry.cache_read_tokens += usage.cache_read_input_tokens as u64;
        entry.cache_write_tokens += usage.cache_creation_input_tokens as u64;
        entry.cost_usd += pricing::estimate_cost_with_catalog(
            pricing_catalog,
            &entry.model,
            usage.input_tokens as u64,
            usage.output_tokens as u64,
            usage.cache_read_input_tokens as u64,
            usage.cache_creation_input_tokens as u64,
        );
    }

    stats_map.into_values().collect()
}

/// Convert an RFC 3339 timestamp string to a `YYYY-MM-DD` date in the
/// user's local timezone. Falls back to the first 10 chars when parsing
/// fails, which covers the legacy date-only timestamps some providers
/// still produce.
pub fn timestamp_to_local_date(timestamp: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .or_else(|| timestamp.get(..10).map(ToString::to_string))
}
