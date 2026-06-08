use super::{
    Database, UsageByModelRow, UsageProjectModelDetailRow, UsageSessionModelDetailRow,
    UsageTotalsRow,
};

/// Inclusive `[start, end]` date bounds (`YYYY-MM-DD`) for usage queries.
/// `None` on either side leaves that side unbounded.
#[derive(Clone, Copy, Default)]
pub struct UsageDateBounds<'a> {
    pub start: Option<&'a str>,
    pub end: Option<&'a str>,
}

/// One row of the activity calendar: `(date, sessions, turns, tokens, cost)`.
pub type ActivityDailyRow = (String, u64, u64, u64, f64);

impl Database {
    pub fn usage_session_count(
        &self,
        providers: &[String],
        bounds: UsageDateBounds<'_>,
    ) -> Result<u64, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT COUNT(DISTINCT s.session_id) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{}",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, param_refs.as_slice(), |row| row.get(0))
    }

    pub fn usage_session_count_by_provider(
        &self,
        providers: &[String],
        bounds: UsageDateBounds<'_>,
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
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
        })?;
        rows.collect()
    }

    pub fn usage_totals(
        &self,
        providers: &[String],
        bounds: UsageDateBounds<'_>,
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
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, param_refs.as_slice(), |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
    }

    pub fn usage_daily(
        &self,
        providers: &[String],
        bounds: UsageDateBounds<'_>,
    ) -> Result<Vec<(String, String, u64, f64)>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT s.date, sess.provider, \
                    SUM(s.input_tokens + s.output_tokens + s.cache_read_tokens + s.cache_write_tokens), \
                    SUM(s.cost_usd) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY s.date, sess.provider \
             ORDER BY s.date, sess.provider",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Per-day activity over `bounds`, grouped by date only (providers merged):
    /// distinct sessions, turns, tokens, and cost. Powers the activity calendar.
    pub fn activity_daily(
        &self,
        providers: &[String],
        bounds: UsageDateBounds<'_>,
    ) -> Result<Vec<ActivityDailyRow>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT s.date, \
                    COUNT(DISTINCT s.session_id), \
                    COALESCE(SUM(s.turn_count),0), \
                    COALESCE(SUM(s.input_tokens + s.output_tokens + s.cache_read_tokens + s.cache_write_tokens),0), \
                    COALESCE(SUM(s.cost_usd),0.0) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY s.date \
             ORDER BY s.date",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Distinct calendar years (descending) that have any data for `providers`,
    /// ignoring any date window. Drives the activity-calendar year selector.
    pub fn activity_years(&self, providers: &[String]) -> Result<Vec<i32>, rusqlite::Error> {
        if providers.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, UsageDateBounds::default());
        let sql = format!(
            "SELECT DISTINCT CAST(substr(s.date, 1, 4) AS INTEGER) AS yr \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             ORDER BY yr DESC",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| row.get::<_, i32>(0))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub(crate) fn usage_by_model(
        &self,
        providers: &[String],
        bounds: UsageDateBounds<'_>,
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
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
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
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Per-project cost detail grouped by (project_path, provider, session_id, model)
    /// so callers can deduplicate sessions exactly while still pricing by model.
    pub(crate) fn usage_project_model_detail(
        &self,
        providers: &[String],
        bounds: UsageDateBounds<'_>,
    ) -> Result<Vec<UsageProjectModelDetailRow>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT sess.project_path, sess.project_name, sess.provider, s.session_id, \
                    SUM(s.turn_count), \
                    SUM(s.input_tokens), \
                    SUM(s.output_tokens), \
                    SUM(s.cache_read_tokens), \
                    SUM(s.cache_write_tokens), \
                    SUM(s.cost_usd) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY sess.project_path, sess.project_name, sess.provider, s.session_id, \
                      COALESCE(NULLIF(s.model, ''), sess.model, '') \
             ORDER BY SUM(s.input_tokens + s.output_tokens) DESC",
            where_clause
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(UsageProjectModelDetailRow {
                project_path: row.get(0)?,
                project_name: row.get(1)?,
                provider: row.get(2)?,
                session_id: row.get(3)?,
                turns: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                cache_read_tokens: row.get(7)?,
                cache_write_tokens: row.get(8)?,
                cost_usd: row.get(9)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Per-session token detail grouped by (session_id, model) for accurate cost calculation.
    pub(crate) fn usage_session_model_detail(
        &self,
        providers: &[String],
        bounds: UsageDateBounds<'_>,
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
        let session_refs: Vec<&dyn rusqlite::types::ToSql> =
            session_params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&session_sql)?;
        let rows = stmt.query_map(session_refs.as_slice(), |row| row.get::<_, String>(0))?;
        let mut session_ids = Vec::new();
        for row in rows {
            session_ids.push(row?);
        }

        if session_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Now query detail for those sessions. Re-apply the SAME date bounds
        // used to pick them: session_token_stats grain is (session_id, date,
        // model), so without them a session active both inside and outside the
        // range would sum ALL its dated rows, inflating per-session totals so
        // the Recent Sessions table no longer reconciles with the headline
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
            detail_where.push_str(&format!(" AND s.date >= ?{}", detail_params.len() + 1));
            detail_params.push(Box::new(start.to_string()));
        }
        if let Some(end) = bounds.end {
            detail_where.push_str(&format!(" AND s.date <= ?{}", detail_params.len() + 1));
            detail_params.push(Box::new(end.to_string()));
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
        let detail_refs: Vec<&dyn rusqlite::types::ToSql> =
            detail_params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&detail_sql)?;
        let rows = stmt.query_map(detail_refs.as_slice(), |row| {
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
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Totals for a specific date range [start, end).
    pub fn usage_totals_range(
        &self,
        providers: &[String],
        date_start: &str,
        date_end: &str,
    ) -> Result<UsageTotalsRow, rusqlite::Error> {
        let conn = self.lock_read()?;
        if providers.is_empty() {
            return Ok((0, 0, 0, 0, 0, 0, 0.0));
        }
        let placeholders: Vec<String> = (0..providers.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            providers.iter().map(|p| Box::new(p.clone()) as _).collect();
        params.push(Box::new(date_start.to_string()));
        params.push(Box::new(date_end.to_string()));
        let sql = format!(
            "SELECT COUNT(DISTINCT s.session_id), \
                    COALESCE(SUM(s.turn_count),0), \
                    COALESCE(SUM(s.input_tokens),0), \
                    COALESCE(SUM(s.output_tokens),0), \
                    COALESCE(SUM(s.cache_read_tokens),0), \
                    COALESCE(SUM(s.cache_write_tokens),0), \
                    COALESCE(SUM(s.cost_usd),0.0) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id \
             WHERE sess.provider IN ({}) AND s.date >= ?{} AND s.date < ?{}",
            placeholders.join(","),
            params.len() - 1,
            params.len()
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.query_row(&sql, param_refs.as_slice(), |row: &rusqlite::Row| {
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

    /// Total cost for a single date (all providers).
    pub fn cost_for_date(&self, date: &str) -> Result<f64, rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row(
            "SELECT COALESCE(SUM(cost_usd), 0.0) FROM session_token_stats WHERE date = ?1",
            [date],
            |row| row.get(0),
        )
    }

    /// Token breakdown for a single date (all providers).
    pub fn tokens_for_date(&self, date: &str) -> Result<(u64, u64, u64, u64), rusqlite::Error> {
        let conn = self.lock_read()?;
        conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), \
                    COALESCE(SUM(output_tokens), 0), \
                    COALESCE(SUM(cache_read_tokens), 0), \
                    COALESCE(SUM(cache_write_tokens), 0) \
             FROM session_token_stats WHERE date = ?1",
            [date],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
    }
}

fn build_usage_where(
    providers: &[String],
    bounds: UsageDateBounds<'_>,
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
    if let Some(date) = bounds.start {
        params.push(Box::new(date.to_string()));
        conditions.push(format!("s.date >= ?{}", params.len()));
    }
    if let Some(date) = bounds.end {
        params.push(Box::new(date.to_string()));
        conditions.push(format!("s.date <= ?{}", params.len()));
    }

    // conditions always has at least the provider IN clause (empty providers early-return above)
    let clause = format!(" WHERE {}", conditions.join(" AND "));
    (clause, params)
}
