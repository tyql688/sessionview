use std::collections::{HashMap, HashSet};

use crate::models::TokenTotals;
use crate::pricing::{self, PricingCatalog};

use super::ParsedSession;

/// A single per-(bucket, model) token-usage row, written to
/// `session_token_stats` by the indexer. Defined here so the provider
/// trait can produce them without depending on `db::sync`.
///
/// `bucket` is the UTC epoch second of a 15-minute-aligned window start, so
/// queries can group into days for any requested timezone.
#[derive(Clone, Debug)]
pub struct TokenStatRow {
    pub bucket: i64,
    pub model: String,
    pub turn_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

/// A normalized out-of-band usage event. Input and cache components are disjoint.
#[derive(Clone, Debug)]
pub struct UsageEvent {
    pub timestamp: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub usage_hash: Option<String>,
}

pub fn token_totals_from_usage_events(events: &[UsageEvent]) -> TokenTotals {
    events
        .iter()
        .fold(TokenTotals::default(), |mut totals, event| {
            totals.input_tokens += event.input_tokens;
            totals.output_tokens += event.output_tokens;
            totals.cache_read_tokens += event.cache_read_input_tokens;
            totals.cache_write_tokens += event.cache_creation_input_tokens;
            totals
        })
}

pub fn compute_token_stats_from_usage_events(
    parsed: &ParsedSession,
    pricing_catalog: Option<&PricingCatalog>,
    mut seen_hashes: Option<&mut HashSet<String>>,
) -> Vec<TokenStatRow> {
    let mut stats_map: HashMap<(i64, String), TokenStatRow> = HashMap::with_capacity(16);
    let mut invalid_timestamps = 0usize;
    let mut first_invalid_timestamp = None;
    let mut missing_models = 0usize;
    for event in &parsed.usage_events {
        if let (Some(seen), Some(hash)) = (&mut seen_hashes, &event.usage_hash)
            && !seen.insert(hash.clone())
        {
            continue;
        }
        let Some(bucket) = timestamp_to_bucket(&event.timestamp) else {
            invalid_timestamps += 1;
            first_invalid_timestamp.get_or_insert(event.timestamp.as_str());
            continue;
        };
        if event.model.is_empty() {
            missing_models += 1;
            continue;
        }
        let entry = stats_map
            .entry((bucket, event.model.clone()))
            .or_insert_with(|| TokenStatRow {
                bucket,
                model: event.model.clone(),
                turn_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.0,
            });
        entry.turn_count += 1;
        entry.input_tokens += event.input_tokens;
        entry.output_tokens += event.output_tokens;
        entry.cache_read_tokens += event.cache_read_input_tokens;
        entry.cache_write_tokens += event.cache_creation_input_tokens;
        entry.cost_usd += pricing::estimate_cost_with_catalog(
            pricing_catalog,
            &entry.model,
            event.input_tokens,
            event.output_tokens,
            event.cache_read_input_tokens,
            event.cache_creation_input_tokens,
        );
    }
    if invalid_timestamps > 0 {
        log::warn!(
            "skipped {invalid_timestamps} {} usage event(s) with invalid timestamps in session {} (first: '{}')",
            parsed.meta.provider.key(),
            parsed.meta.id,
            first_invalid_timestamp.unwrap_or_default()
        );
    }
    if missing_models > 0 {
        log::warn!(
            "skipped {missing_models} {} usage event(s) without a model in session {}",
            parsed.meta.provider.key(),
            parsed.meta.id
        );
    }
    stats_map.into_values().collect()
}

/// Default token-stats aggregation: walk per-message `token_usage`,
/// dedup by `Message.usage_hash` (keeping the largest entry per hash —
/// Claude streams cumulative usage across the lines of one API call),
/// apply pricing. Used by all providers except Codex (which overrides
/// with usage-event aggregation).
pub fn default_compute_token_stats_from_messages(
    parsed: &ParsedSession,
    pricing_catalog: Option<&PricingCatalog>,
    mut seen_hashes: Option<&mut HashSet<String>>,
) -> Vec<TokenStatRow> {
    let mut stats_map: HashMap<(i64, String), TokenStatRow> = HashMap::with_capacity(32);
    let mut missing_timestamps = 0usize;
    let mut invalid_timestamps = 0usize;
    let mut first_invalid_timestamp = None;
    let mut missing_models = 0usize;
    for msg in crate::models::dedup_usage_messages(&parsed.messages) {
        let Some(usage) = &msg.token_usage else {
            continue;
        };
        // Dedup: skip if this usage entry was already counted (cross-file).
        if let Some(ref mut seen) = seen_hashes
            && let Some(ref hash) = msg.usage_hash
            && !seen.insert(hash.clone())
        {
            continue;
        }

        let Some(timestamp) = msg.timestamp.as_deref() else {
            missing_timestamps += 1;
            continue;
        };
        let Some(bucket) = timestamp_to_bucket(timestamp) else {
            invalid_timestamps += 1;
            first_invalid_timestamp.get_or_insert(timestamp);
            continue;
        };
        let Some(model) = msg.model.as_deref().filter(|model| !model.is_empty()) else {
            missing_models += 1;
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
            .entry((bucket, model.clone()))
            .or_insert_with(|| TokenStatRow {
                bucket,
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

    if missing_timestamps > 0 {
        log::warn!(
            "skipped {missing_timestamps} token usage message(s) without timestamps in session {}",
            parsed.meta.id
        );
    }
    if invalid_timestamps > 0 {
        log::warn!(
            "skipped {invalid_timestamps} token usage message(s) with invalid timestamps in session {} (first: '{}')",
            parsed.meta.id,
            first_invalid_timestamp.unwrap_or_default()
        );
    }
    if missing_models > 0 {
        log::warn!(
            "skipped {missing_models} token usage message(s) without models in session {}",
            parsed.meta.id
        );
    }
    stats_map.into_values().collect()
}

/// Seconds per stats bucket. 15 minutes divides every IANA UTC offset
/// (including the :30 and :45 ones), so buckets always map wholly into
/// one local day for any timezone chosen at query time.
pub const STATS_BUCKET_SECONDS: i64 = 900;

/// Convert an RFC 3339 timestamp or legacy `YYYY-MM-DD` value to a UTC
/// 15-minute bucket start (epoch seconds). Date-only values have no clock
/// time, so they pin to noon UTC: the civil date holds for every offset in
/// `-12:00 ..= +11:59`, and zones at or past +12 read them a day later.
pub fn timestamp_to_bucket(timestamp: &str) -> Option<i64> {
    let epoch = chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|dt| dt.timestamp())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(timestamp, "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(12, 0, 0))
                .map(|dt| dt.and_utc().timestamp())
        })?;
    Some(epoch.div_euclid(STATS_BUCKET_SECONDS) * STATS_BUCKET_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::{STATS_BUCKET_SECONDS, timestamp_to_bucket};

    #[test]
    fn timestamp_to_bucket_floors_to_utc_quarter_hour() {
        let bucket = timestamp_to_bucket("2026-07-19T10:14:59+02:00").expect("bucket");
        assert_eq!(bucket % STATS_BUCKET_SECONDS, 0);
        // 08:14:59 UTC floors to 08:00 UTC.
        assert_eq!(bucket, 1_784_448_000);
    }

    #[test]
    fn timestamp_to_bucket_pins_date_only_values_to_noon_utc() {
        assert_eq!(
            timestamp_to_bucket("2026-07-19"),
            Some(1_784_462_400) // 2026-07-19T12:00:00Z
        );
    }

    #[test]
    fn timestamp_to_bucket_rejects_plausible_invalid_prefixes() {
        assert_eq!(timestamp_to_bucket("not-a-date-anything"), None);
        assert_eq!(timestamp_to_bucket("2026-02-30"), None);
        assert_eq!(timestamp_to_bucket("2026-07-19 garbage"), None);
    }
}
