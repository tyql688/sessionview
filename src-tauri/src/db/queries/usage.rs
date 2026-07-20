use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::Datelike;
use chrono_tz::Tz;
use rusqlite::params_from_iter;

use super::{Database, UsageByModelRow, UsageSessionModelDetailRow, UsageTotalsRow};
use crate::services::timeday::{SUPPORTED_QUERY_YEARS, epoch_in, epoch_to_date};

/// Groups buckets into civil days, skipping and reporting any whose epoch has
/// no local time rather than filing it under a fabricated date.
pub(super) struct DayFolder {
    tz: Tz,
    query: &'static str,
    skipped: usize,
}

impl DayFolder {
    pub(super) fn new(tz: Tz, query: &'static str) -> Self {
        Self {
            tz,
            query,
            skipped: 0,
        }
    }

    pub(super) fn date(&mut self, bucket: i64) -> Option<String> {
        let date = epoch_to_date(bucket, self.tz);
        if date.is_none() {
            self.skipped += 1;
        }
        date
    }
}

impl Drop for DayFolder {
    fn drop(&mut self) {
        if self.skipped > 0 {
            log::warn!(
                "{}: skipped {} usage row(s) whose bucket has no local time in {}",
                self.query,
                self.skipped,
                self.tz
            );
        }
    }
}

/// Half-open `[start, end)` UTC epoch bounds for usage queries, matched
/// against `session_token_stats.bucket`. Computed by the command layer
/// from civil dates in the caller's timezone. `None` on either side
/// leaves that side unbounded.
#[derive(Clone, Copy, Default)]
pub(crate) struct UsageBucketBounds {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

/// One row of the activity calendar: `(date, sessions, turns, tokens, cost)`.
pub type ActivityDailyRow = (String, u64, u64, u64, f64);

impl Database {
    pub(crate) fn usage_session_count(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
    ) -> Result<u64, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT COUNT(DISTINCT s.session_id) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{}",
            where_clause
        );
        conn.query_row(&sql, params_from_iter(&params), |row| row.get(0))
    }

    pub(crate) fn usage_session_count_by_provider(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
    ) -> Result<Vec<(String, u64)>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT sess.provider, COUNT(DISTINCT s.session_id) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY sess.provider",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(&params), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?;
        rows.collect()
    }

    pub(crate) fn usage_totals(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
    ) -> Result<(u64, u64, u64, u64, u64), rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT COALESCE(SUM(s.turn_count),0), \
                    COALESCE(SUM(s.input_tokens),0), \
                    COALESCE(SUM(s.output_tokens),0), \
                    COALESCE(SUM(s.cache_read_tokens),0), \
                    COALESCE(SUM(s.cache_write_tokens),0) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{}",
            where_clause
        );
        conn.query_row(&sql, params_from_iter(&params), |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
    }

    /// Per-(date, provider) token/cost sums in `tz`, ordered by date then
    /// provider.
    pub(crate) fn usage_daily(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
        tz: Tz,
    ) -> Result<Vec<(String, String, u64, f64)>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT s.bucket, sess.provider, \
                    SUM(s.input_tokens + s.output_tokens + s.cache_read_tokens + s.cache_write_tokens), \
                    SUM(s.cost_usd) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY s.bucket, sess.provider",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(&params), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        let mut folder = DayFolder::new(tz, "usage_daily");
        let mut by_day: BTreeMap<(String, String), (u64, f64)> = BTreeMap::new();
        for row in rows {
            let (bucket, provider, tokens, cost) = row?;
            let Some(date) = folder.date(bucket) else {
                continue;
            };
            let entry = by_day.entry((date, provider)).or_default();
            entry.0 += tokens;
            entry.1 += cost;
        }
        Ok(by_day
            .into_iter()
            .map(|((date, provider), (tokens, cost))| (date, provider, tokens, cost))
            .collect())
    }

    /// Per-day activity over `bounds` in `tz` (providers merged): distinct
    /// sessions, turns, tokens, and cost. Powers the activity calendar.
    pub(crate) fn activity_daily(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
        tz: Tz,
    ) -> Result<Vec<ActivityDailyRow>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        // Distinct-session counts can't be summed across buckets, so group
        // by (bucket, session) in SQL and fold per-day session sets here.
        let sql = format!(
            "SELECT s.bucket, s.session_id, \
                    COALESCE(SUM(s.turn_count),0), \
                    COALESCE(SUM(s.input_tokens + s.output_tokens + s.cache_read_tokens + s.cache_write_tokens),0), \
                    COALESCE(SUM(s.cost_usd),0.0) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY s.bucket, s.session_id",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(&params), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, u64>(3)?,
                row.get::<_, f64>(4)?,
            ))
        })?;
        let mut folder = DayFolder::new(tz, "activity_daily");
        let mut by_day: BTreeMap<String, (HashSet<String>, u64, u64, f64)> = BTreeMap::new();
        for row in rows {
            let (bucket, session_id, turns, tokens, cost) = row?;
            let Some(date) = folder.date(bucket) else {
                continue;
            };
            let entry = by_day.entry(date).or_default();
            entry.0.insert(session_id);
            entry.1 += turns;
            entry.2 += tokens;
            entry.3 += cost;
        }
        Ok(by_day
            .into_iter()
            .map(|(date, (sessions, turns, tokens, cost))| {
                (date, sessions.len() as u64, turns, tokens, cost)
            })
            .collect())
    }

    /// Distinct calendar years (descending, in `tz`) that have any data for
    /// `providers`, ignoring any date window. Drives the year selector.
    pub(crate) fn activity_years(
        &self,
        providers: &[String],
        tz: Tz,
    ) -> Result<Vec<i32>, rusqlite::Error> {
        if providers.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, UsageBucketBounds::default());
        // A group under a day wide cannot skip a year, and both endpoints are
        // real buckets, so its extremes decide the local year exactly.
        let sql = format!(
            "SELECT MIN(s.bucket), MAX(s.bucket) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY s.bucket / 86400",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(&params), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut years = BTreeSet::new();
        for (first, last) in rows {
            for epoch in [first, last] {
                if let Some(dt) = epoch_in(epoch, tz) {
                    years.insert(dt.year());
                }
            }
        }
        // Years the command layer would refuse to query must not reach the
        // selector, or picking one errors the whole calendar.
        Ok(years
            .into_iter()
            .rev()
            .filter(|year| SUPPORTED_QUERY_YEARS.contains(year))
            .collect())
    }

    pub(crate) fn usage_by_model(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
    ) -> Result<Vec<UsageByModelRow>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT COALESCE(NULLIF(s.model, ''), sess.model, ''), \
                    SUM(s.turn_count), \
                    SUM(s.input_tokens), \
                    SUM(s.output_tokens), \
                    SUM(s.cache_read_tokens), \
                    SUM(s.cache_write_tokens), \
                    SUM(s.cost_usd) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY COALESCE(NULLIF(s.model, ''), sess.model, '') \
             ORDER BY SUM(s.input_tokens + s.output_tokens) DESC",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(&params), |row| {
            Ok(UsageByModelRow {
                model: row.get(0)?,
                turns: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cache_read_tokens: row.get(4)?,
                cache_write_tokens: row.get(5)?,
                cost_usd: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub(crate) fn usage_session_model_detail(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
        limit: u32,
    ) -> Result<Vec<UsageSessionModelDetailRow>, rusqlite::Error> {
        let conn = self.lock_read()?;

        // Two-step approach: find the top N session IDs, then fetch per-model detail.
        let (where_clause, params) = build_usage_where(providers, bounds);
        let session_sql = format!(
            "SELECT DISTINCT s.session_id, MAX(sess.updated_at) as max_updated \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
               AND sess.parent_id IS NULL \
             GROUP BY s.session_id \
             ORDER BY max_updated DESC \
             LIMIT ?{}",
            where_clause,
            params.len() + 1
        );
        let mut session_params = params;
        session_params.push(Box::new(limit));
        let mut stmt = conn.prepare(&session_sql)?;
        let rows = stmt.query_map(params_from_iter(&session_params), |row| {
            row.get::<_, String>(0)
        })?;
        let session_ids = rows.collect::<Result<Vec<String>, _>>()?;

        if session_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Now query detail for those sessions. Re-apply the SAME bounds used
        // to pick them: session_token_stats grain is (session_id, bucket,
        // model), so without them a session active both inside and outside the
        // range would sum ALL its rows, inflating per-session totals so the
        // Recent Sessions table no longer reconciles with the headline
        // total_cost / chart for any bounded range.
        let id_placeholders: Vec<String> = (0..session_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let mut detail_params: Vec<Box<dyn rusqlite::types::ToSql>> = session_ids
            .into_iter()
            .map(|id| Box::new(id) as _)
            .collect();
        let mut detail_where = format!("WHERE s.session_id IN ({})", id_placeholders.join(","));
        if let Some(start) = bounds.start {
            detail_where.push_str(&format!(" AND s.bucket >= ?{}", detail_params.len() + 1));
            detail_params.push(Box::new(start));
        }
        if let Some(end) = bounds.end {
            detail_where.push_str(&format!(" AND s.bucket < ?{}", detail_params.len() + 1));
            detail_params.push(Box::new(end));
        }
        let detail_sql = format!(
            "SELECT s.session_id, sess.project_path, sess.project_name, sess.provider, sess.updated_at, \
                    COALESCE(NULLIF(s.model, ''), sess.model, ''), \
                    SUM(s.turn_count), \
                    SUM(s.input_tokens), \
                    SUM(s.output_tokens), \
                    SUM(s.cache_read_tokens), \
                    SUM(s.cache_write_tokens), \
                    SUM(s.cost_usd) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id \
             {detail_where} \
             GROUP BY s.session_id, COALESCE(NULLIF(s.model, ''), sess.model, '') \
             ORDER BY sess.updated_at DESC, s.session_id"
        );
        let mut stmt = conn.prepare(&detail_sql)?;
        let rows = stmt.query_map(params_from_iter(&detail_params), |row| {
            Ok(UsageSessionModelDetailRow {
                session_id: row.get(0)?,
                project_path: row.get(1)?,
                project_name: row.get(2)?,
                provider: row.get(3)?,
                updated_at: row.get(4)?,
                model: row.get(5)?,
                turns: row.get(6)?,
                input_tokens: row.get(7)?,
                output_tokens: row.get(8)?,
                cache_read_tokens: row.get(9)?,
                cache_write_tokens: row.get(10)?,
                cost_usd: row.get(11)?,
            })
        })?;
        rows.collect()
    }

    /// Totals for a half-open bucket range `[start, end)`.
    pub(crate) fn usage_totals_range(
        &self,
        providers: &[String],
        start: i64,
        end: i64,
    ) -> Result<UsageTotalsRow, rusqlite::Error> {
        let conn = self.lock_read()?;
        if providers.is_empty() {
            return Ok((0, 0, 0, 0, 0, 0, 0.0));
        }
        let bounds = UsageBucketBounds {
            start: Some(start),
            end: Some(end),
        };
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT COUNT(DISTINCT s.session_id), \
                    COALESCE(SUM(s.turn_count),0), \
                    COALESCE(SUM(s.input_tokens),0), \
                    COALESCE(SUM(s.output_tokens),0), \
                    COALESCE(SUM(s.cache_read_tokens),0), \
                    COALESCE(SUM(s.cache_write_tokens),0), \
                    COALESCE(SUM(s.cost_usd),0.0) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{}",
            where_clause
        );
        conn.query_row(&sql, params_from_iter(&params), |row: &rusqlite::Row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, u64>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, u64>(5)?,
                row.get::<_, f64>(6)?,
            ))
        })
    }

    /// Total cost inside a half-open bucket range (all providers).
    pub(crate) fn cost_for_range(&self, start: i64, end: i64) -> Result<f64, rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row(
            "SELECT COALESCE(SUM(cost_usd), 0.0) FROM session_token_stats \
             WHERE bucket >= ?1 AND bucket < ?2",
            rusqlite::params![start, end],
            |row| row.get(0),
        )
    }

    /// Token breakdown inside a half-open bucket range (all providers).
    pub(crate) fn tokens_for_range(
        &self,
        start: i64,
        end: i64,
    ) -> Result<(u64, u64, u64, u64), rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), \
                    COALESCE(SUM(output_tokens), 0), \
                    COALESCE(SUM(cache_read_tokens), 0), \
                    COALESCE(SUM(cache_write_tokens), 0) \
             FROM session_token_stats WHERE bucket >= ?1 AND bucket < ?2",
            rusqlite::params![start, end],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
    }
}

pub(super) fn build_usage_where(
    providers: &[String],
    bounds: UsageBucketBounds,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    if providers.is_empty() {
        return (" WHERE 1 = 0".to_string(), Vec::new());
    }

    let mut conditions = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    let placeholders: Vec<String> = (0..providers.len())
        .map(|i| format!("?{}", i + 1))
        .collect();
    conditions.push(format!("sess.provider IN ({})", placeholders.join(",")));
    for p in providers {
        params.push(Box::new(p.clone()));
    }
    if let Some(start) = bounds.start {
        params.push(Box::new(start));
        conditions.push(format!("s.bucket >= ?{}", params.len()));
    }
    if let Some(end) = bounds.end {
        params.push(Box::new(end));
        conditions.push(format!("s.bucket < ?{}", params.len()));
    }

    // conditions always has at least the provider IN clause (empty providers early-return above)
    let clause = format!(" WHERE {}", conditions.join(" AND "));
    (clause, params)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::db::sync::TokenStatRow;
    use crate::models::{Provider, SessionMeta};
    use crate::provider::ParsedSession;
    use crate::services::timeday::day_range_epochs;
    use tempfile::TempDir;

    pub(crate) fn utc_bounds(start: Option<&str>, end: Option<&str>) -> UsageBucketBounds {
        let parse = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("valid date");
        let (start, end) =
            day_range_epochs(start.map(parse), end.map(parse), Tz::UTC).expect("bounds");
        UsageBucketBounds { start, end }
    }

    pub(crate) fn sample_meta(session_id: &str) -> SessionMeta {
        SessionMeta {
            id: session_id.to_string(),
            provider: Provider::Claude,
            title: "Test".into(),
            project_path: "/tmp/project".into(),
            project_name: "project".into(),
            created_at: 1_775_635_200,
            updated_at: 1_775_635_200,
            message_count: 1,
            file_size_bytes: 0,
            source_path: format!("/tmp/{session_id}.jsonl"),
            is_sidechain: false,
            variant_name: None,
            model: Some("claude-opus-4-6".into()),
            cc_version: None,
            git_branch: None,
            parent_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }

    pub(crate) fn parsed_session(meta: SessionMeta, content_text: String) -> ParsedSession {
        ParsedSession {
            meta,
            messages: Vec::new(),
            content_text,
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 0,
        }
    }

    pub(crate) fn stat_row(
        date: &str,
        model: &str,
        turns: u64,
        tokens: [u64; 4],
        cost: f64,
    ) -> TokenStatRow {
        TokenStatRow {
            // Noon-UTC bucket: with Tz::UTC queries these rows always land
            // on that same civil date.
            bucket: crate::provider::timestamp_to_bucket(date).expect("valid date"),
            model: model.into(),
            turn_count: turns,
            input_tokens: tokens[0],
            output_tokens: tokens[1],
            cache_read_tokens: tokens[2],
            cache_write_tokens: tokens[3],
            cost_usd: cost,
        }
    }

    #[test]
    fn activity_daily_groups_by_date_with_distinct_sessions() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let meta_a = sample_meta("session-a");
        let meta_b = sample_meta("session-b");
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[
                parsed_session(meta_a.clone(), String::new()),
                parsed_session(meta_b.clone(), String::new()),
            ],
            true,
            &[],
        )
        .unwrap();

        // Two sessions both active on 2026-04-09 (session-a across two models),
        // one trailing day on 2026-04-10, and a 2025 day to exercise the year list.
        db.replace_token_stats(
            &meta_a.id,
            &[
                stat_row("2026-04-09", "claude-opus-4-6", 3, [100, 50, 20, 10], 0.10),
                stat_row("2026-04-09", "claude-sonnet-4-6", 2, [10, 5, 0, 0], 0.02),
                stat_row("2025-12-31", "claude-opus-4-6", 1, [1, 1, 0, 0], 0.001),
            ],
        )
        .unwrap();
        db.replace_token_stats(
            &meta_b.id,
            &[stat_row(
                "2026-04-09",
                "claude-opus-4-6",
                4,
                [200, 100, 0, 0],
                0.30,
            )],
        )
        .unwrap();

        let providers = vec!["claude".to_string()];
        let bounds = utc_bounds(Some("2026-01-01"), Some("2026-12-31"));
        let days = db.activity_daily(&providers, bounds, Tz::UTC).unwrap();
        assert_eq!(days.len(), 1, "only 2026-04-09 falls inside the bounds");
        let (date, sessions, turns, tokens, cost) = &days[0];
        assert_eq!(date, "2026-04-09");
        assert_eq!(*sessions, 2, "session-a and session-b are distinct");
        assert_eq!(*turns, 3 + 2 + 4);
        assert_eq!(*tokens, 100 + 50 + 20 + 10 + 10 + 5 + 200 + 100);
        assert!((*cost - 0.42).abs() < 1e-9);

        // available_years ignores the date window and is descending.
        let years = db.activity_years(&providers, Tz::UTC).unwrap();
        assert_eq!(years, vec![2026, 2025]);

        // An unselected provider yields no data and no years.
        assert!(
            db.activity_daily(&["codex".to_string()], bounds, Tz::UTC)
                .unwrap()
                .is_empty()
        );
        assert!(
            db.activity_years(&["codex".to_string()], Tz::UTC)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn activity_years_reports_only_years_that_hold_data_in_tz() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let meta = sample_meta("session-newyear");
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[parsed_session(meta.clone(), String::new())],
            true,
            &[],
        )
        .unwrap();
        // Midnight UTC on New Year's Eve: still 2025 in UTC, already 2026
        // eight hours east. Exactly one year is correct in each timezone.
        db.replace_token_stats(
            &meta.id,
            &[TokenStatRow {
                bucket: crate::provider::timestamp_to_bucket("2025-12-31T00:00:00Z").unwrap(),
                model: "claude-opus-4-6".into(),
                turn_count: 1,
                input_tokens: 10,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.01,
            }],
        )
        .unwrap();

        let providers = vec!["claude".to_string()];
        assert_eq!(db.activity_years(&providers, Tz::UTC).unwrap(), vec![2025]);
        assert_eq!(
            db.activity_years(&providers, Tz::Asia__Shanghai).unwrap(),
            vec![2025],
            "the bucket's own local date decides the year, not the UTC day's endpoints"
        );

        // Epoch 0 reads as 1969 west of UTC. The selector must not offer a
        // year the command layer would then refuse to query.
        db.replace_token_stats(
            &meta.id,
            &[TokenStatRow {
                bucket: 0,
                model: "claude-opus-4-6".into(),
                turn_count: 1,
                input_tokens: 10,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.01,
            }],
        )
        .unwrap();
        assert_eq!(
            db.activity_years(&providers, Tz::America__Los_Angeles)
                .unwrap(),
            Vec::<i32>::new()
        );
    }

    #[test]
    fn daily_grouping_shifts_with_requested_timezone() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let meta = sample_meta("session-tz");
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[parsed_session(meta.clone(), String::new())],
            true,
            &[],
        )
        .unwrap();
        // 2026-04-09T22:00Z: still the 9th in UTC, already the 10th in UTC+8.
        let bucket = crate::provider::timestamp_to_bucket("2026-04-09T22:00:00Z").unwrap();
        db.replace_token_stats(
            &meta.id,
            &[TokenStatRow {
                bucket,
                model: "claude-opus-4-6".into(),
                turn_count: 1,
                input_tokens: 100,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.1,
            }],
        )
        .unwrap();

        let providers = vec!["claude".to_string()];
        let utc_days = db
            .usage_daily(&providers, UsageBucketBounds::default(), Tz::UTC)
            .unwrap();
        assert_eq!(utc_days[0].0, "2026-04-09");
        let shanghai_days = db
            .usage_daily(&providers, UsageBucketBounds::default(), Tz::Asia__Shanghai)
            .unwrap();
        assert_eq!(shanghai_days[0].0, "2026-04-10");

        // Bounds computed for the same civil day differ per timezone and
        // select the row only in the timezone where it belongs to that day.
        let day = chrono::NaiveDate::parse_from_str("2026-04-10", "%Y-%m-%d").unwrap();
        let shanghai_day_bounds = {
            let (start, end) = day_range_epochs(Some(day), Some(day), Tz::Asia__Shanghai).unwrap();
            UsageBucketBounds { start, end }
        };
        assert_eq!(
            db.usage_daily(&providers, shanghai_day_bounds, Tz::Asia__Shanghai)
                .unwrap()
                .len(),
            1
        );
        let utc_day_bounds = {
            let (start, end) = day_range_epochs(Some(day), Some(day), Tz::UTC).unwrap();
            UsageBucketBounds { start, end }
        };
        assert!(
            db.usage_daily(&providers, utc_day_bounds, Tz::UTC)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn usage_session_detail_excludes_out_of_range_dates() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let meta = sample_meta("session-range");
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[parsed_session(meta.clone(), String::new())],
            true,
            &[],
        )
        .unwrap();
        db.replace_token_stats(
            &meta.id,
            &[
                // Out of range (before the 2026-05-10 cutoff): must NOT be summed.
                stat_row("2026-05-01", "m", 1, [1000, 0, 0, 0], 1.0),
                // In range: the only row that should count.
                stat_row("2026-05-20", "m", 1, [10, 0, 0, 0], 0.01),
            ],
        )
        .unwrap();

        let rows = db
            .usage_session_model_detail(
                &[Provider::Claude.key().to_string()],
                utc_bounds(Some("2026-05-10"), None),
                50,
            )
            .unwrap();

        let total_input: u64 = rows
            .iter()
            .filter(|r| r.session_id == "session-range")
            .map(|r| r.input_tokens)
            .sum();
        assert_eq!(
            total_input, 10,
            "per-session detail must exclude rows dated before the cutoff"
        );

        // Custom range [2026-04-25, 2026-05-10]: the 05-20 row falls after the
        // inclusive end bound and must be excluded everywhere the bounds apply.
        let rows = db
            .usage_session_model_detail(
                &[Provider::Claude.key().to_string()],
                utc_bounds(Some("2026-04-25"), Some("2026-05-10")),
                50,
            )
            .unwrap();
        let total_input: u64 = rows
            .iter()
            .filter(|r| r.session_id == "session-range")
            .map(|r| r.input_tokens)
            .sum();
        assert_eq!(
            total_input, 1000,
            "per-session detail must exclude rows dated after the end bound"
        );

        let (_, total_in, _, _, _) = db
            .usage_totals(
                &[Provider::Claude.key().to_string()],
                utc_bounds(Some("2026-05-01"), Some("2026-05-01")),
            )
            .unwrap();
        assert_eq!(
            total_in, 1000,
            "single-day bounds must include only that day's rows (inclusive end)"
        );
    }
}
