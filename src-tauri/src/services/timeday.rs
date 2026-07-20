//! Timezone-aware day bucketing for usage stats.
//!
//! `session_token_stats` stores UTC 15-minute buckets
//! ([`crate::provider::STATS_BUCKET_SECONDS`]); every read path converts
//! them into civil dates for a caller-chosen IANA timezone here, so the
//! same index serves clients in any timezone (headless remote included).

use anyhow::{Context, anyhow};
use chrono::{DateTime, Duration, NaiveDate, TimeZone};
use chrono_tz::Tz;

use crate::provider::STATS_BUCKET_SECONDS;

/// Calendar years a usage query may address. `%Y-%m-%d` also parses chrono's
/// expanded years (`+262142-12-31` is `NaiveDate::MAX`), where the day
/// arithmetic these ranges feed overflows. The command layer rejects dates
/// outside this range, so the year selector must not offer them either.
pub const SUPPORTED_QUERY_YEARS: std::ops::RangeInclusive<i32> = 1970..=9999;

/// Resolve a requested IANA timezone name, falling back to the machine's
/// timezone (then UTC) when none is given. Invalid names are rejected —
/// this runs at the command trust boundary.
pub fn resolve_timezone(requested: Option<&str>) -> anyhow::Result<Tz> {
    match requested {
        Some(name) => name
            .parse::<Tz>()
            .map_err(|_| anyhow!("invalid timezone '{name}', expected an IANA name")),
        None => Ok(iana_time_zone::get_timezone()
            .ok()
            .and_then(|name| name.parse::<Tz>().ok())
            .unwrap_or(Tz::UTC)),
    }
}

/// Local instant of a UTC epoch second in `tz`. `None` for epochs outside
/// the representable range — callers skip such rows rather than bucket them
/// under a fabricated date.
pub fn epoch_in(epoch: i64, tz: Tz) -> Option<DateTime<Tz>> {
    tz.timestamp_opt(epoch, 0).single()
}

/// Civil date (`YYYY-MM-DD`) of a UTC epoch second in `tz`.
pub fn epoch_to_date(epoch: i64, tz: Tz) -> Option<String> {
    epoch_in(epoch, tz).map(|dt| dt.format("%Y-%m-%d").to_string())
}

/// Today's civil date in `tz`.
pub fn today_in(tz: Tz) -> NaiveDate {
    chrono::Utc::now().with_timezone(&tz).date_naive()
}

/// UTC epoch second at which the local day `date` starts in `tz`.
///
/// Local midnight does not always exist: DST gaps skip an hour, and a few
/// zones have skipped whole days (Pacific/Apia dropped 2011-12-30 crossing
/// the date line). Both are handled by walking forward to the first instant
/// that does exist — for a skipped day that is the next day's start, so a
/// range over it collapses to empty rather than erroring.
fn day_start_epoch(date: NaiveDate, tz: Tz) -> anyhow::Result<i64> {
    let midnight = date
        .and_hms_opt(0, 0, 0)
        .context("invalid midnight for date")?;
    // Probe a whole day plus the largest offset jump, on the bucket grid.
    let probe_seconds = 26 * 3600 / STATS_BUCKET_SECONDS;
    for step in 0..=probe_seconds {
        let candidate = midnight + Duration::seconds(step * STATS_BUCKET_SECONDS);
        if let Some(dt) = tz.from_local_datetime(&candidate).earliest() {
            return Ok(dt.timestamp());
        }
    }
    Err(anyhow!("no valid local time on {date} in {tz}"))
}

/// Half-open UTC epoch range `[start, end)` covering the inclusive local
/// date range `[start_date, end_date]` in `tz`. Either side may be open.
pub fn day_range_epochs(
    start_date: Option<NaiveDate>,
    end_date_inclusive: Option<NaiveDate>,
    tz: Tz,
) -> anyhow::Result<(Option<i64>, Option<i64>)> {
    let start = start_date
        .map(|date| day_start_epoch(date, tz))
        .transpose()?;
    let end = end_date_inclusive
        .map(|date| {
            let next = date
                .succ_opt()
                .ok_or_else(|| anyhow!("date {date} has no following day"))?;
            day_start_epoch(next, tz)
        })
        .transpose()?;
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn resolves_iana_names_and_rejects_garbage() {
        assert_eq!(
            resolve_timezone(Some("Asia/Shanghai")).unwrap(),
            Tz::Asia__Shanghai
        );
        assert!(resolve_timezone(Some("Not/AZone")).is_err());
        resolve_timezone(None).expect("machine fallback must resolve");
    }

    #[test]
    fn day_range_respects_offset() {
        // Asia/Shanghai is UTC+8: local 2026-07-19 spans
        // [2026-07-18T16:00Z, 2026-07-19T16:00Z).
        let (start, end) = day_range_epochs(
            Some(date("2026-07-19")),
            Some(date("2026-07-19")),
            Tz::Asia__Shanghai,
        )
        .unwrap();
        assert_eq!(start, Some(1_784_419_200 - 8 * 3600));
        assert_eq!(end, Some(1_784_505_600 - 8 * 3600));
    }

    #[test]
    fn epoch_maps_to_correct_civil_date_across_offsets() {
        // 2026-07-18T20:00Z is already the 19th in Shanghai, still the 18th in UTC.
        let epoch = 1_784_419_200 - 4 * 3600;
        assert_eq!(
            epoch_to_date(epoch, Tz::Asia__Shanghai).as_deref(),
            Some("2026-07-19")
        );
        assert_eq!(epoch_to_date(epoch, Tz::UTC).as_deref(), Some("2026-07-18"));
        // Out-of-range epochs yield None instead of an empty date key.
        assert_eq!(epoch_to_date(i64::MAX, Tz::UTC), None);
    }

    #[test]
    fn dst_gap_midnight_falls_forward() {
        // America/Santiago 2026-09-06: clocks jump 00:00 → 01:00, so local
        // midnight does not exist; the day must start at the earliest valid
        // instant instead of erroring.
        let start = day_start_epoch(date("2026-09-06"), Tz::America__Santiago).unwrap();
        let rendered = Tz::America__Santiago
            .timestamp_opt(start, 0)
            .single()
            .unwrap()
            .format("%Y-%m-%d %H:%M")
            .to_string();
        assert_eq!(rendered, "2026-09-06 01:00");
    }

    #[test]
    fn skipped_whole_day_collapses_to_an_empty_range() {
        // Pacific/Apia jumped the date line: 2011-12-30 never happened
        // locally. Querying it must yield an empty range, not an error.
        let (start, end) = day_range_epochs(
            Some(date("2011-12-30")),
            Some(date("2011-12-30")),
            Tz::Pacific__Apia,
        )
        .unwrap();
        assert_eq!(start, end);
        // The surrounding days still resolve normally.
        assert!(day_start_epoch(date("2011-12-29"), Tz::Pacific__Apia).unwrap() < start.unwrap());
    }

    #[test]
    fn day_range_rejects_the_last_representable_date_without_panicking() {
        assert!(day_range_epochs(None, Some(NaiveDate::MAX), Tz::UTC).is_err());
        // The last date the command layer accepts must still resolve: its
        // end bound rolls into year 10000 and probes forward from there.
        assert!(day_range_epochs(None, Some(date("9999-12-31")), Tz::UTC).is_ok());
    }
}
