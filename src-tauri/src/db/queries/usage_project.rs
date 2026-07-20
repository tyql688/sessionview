//! Project-scoped usage queries: per-project cost/model detail, session
//! membership, cached tool stats, and per-day breakdowns.

use std::collections::{BTreeMap, HashSet};

use chrono_tz::Tz;
use rusqlite::params_from_iter;

use super::usage::{DayFolder, build_usage_where};
use super::{Database, UsageProjectDailyRow, UsageProjectModelDetailRow, UsageProjectToolRow};
use crate::db::queries::UsageBucketBounds;

impl Database {
    /// Per-project cost detail grouped by (project_path, provider, session_id, model)
    /// so callers can deduplicate sessions exactly while still pricing by model.
    pub(crate) fn usage_project_model_detail(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
    ) -> Result<Vec<UsageProjectModelDetailRow>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (where_clause, params) = build_usage_where(providers, bounds);
        let sql = format!(
            "SELECT sess.project_path, sess.project_name, sess.provider, s.session_id, \
                    COALESCE(NULLIF(s.model, ''), sess.model, ''), \
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
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(&params), |row| {
            Ok(UsageProjectModelDetailRow {
                project_path: row.get(0)?,
                project_name: row.get(1)?,
                provider: row.get(2)?,
                session_id: row.get(3)?,
                model: row.get(4)?,
                turns: row.get(5)?,
                input_tokens: row.get(6)?,
                output_tokens: row.get(7)?,
                cache_read_tokens: row.get(8)?,
                cache_write_tokens: row.get(9)?,
                cost_usd: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    pub(crate) fn usage_project_session_ids(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
        project_path: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (mut where_clause, mut params) = build_usage_where(providers, bounds);
        where_clause.push_str(&format!(" AND sess.project_path = ?{}", params.len() + 1));
        params.push(Box::new(project_path.to_string()));
        let sql = format!(
            "SELECT DISTINCT s.session_id \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             ORDER BY sess.updated_at DESC",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(&params), |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    pub(crate) fn usage_project_sessions_missing_tool_stats(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
        project_path: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (mut where_clause, mut params) = build_usage_where(providers, bounds);
        where_clause.push_str(&format!(" AND sess.project_path = ?{}", params.len() + 1));
        params.push(Box::new(project_path.to_string()));
        let sql = format!(
            "SELECT active.session_id \
             FROM (
                SELECT DISTINCT s.session_id, MAX(sess.updated_at) AS updated_at \
                FROM session_token_stats s \
                JOIN sessions sess ON s.session_id = sess.id{} \
                GROUP BY s.session_id
             ) active \
             LEFT JOIN session_tool_index idx ON active.session_id = idx.session_id \
             WHERE idx.session_id IS NULL \
             ORDER BY active.updated_at DESC",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(&params), |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    pub(crate) fn usage_project_tool_usage(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
        project_path: &str,
    ) -> Result<Vec<UsageProjectToolRow>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (mut where_clause, mut params) = build_usage_where(providers, bounds);
        where_clause.push_str(&format!(" AND sess.project_path = ?{}", params.len() + 1));
        params.push(Box::new(project_path.to_string()));
        let sql = format!(
            "WITH project_sessions AS (
                SELECT DISTINCT s.session_id
                FROM session_token_stats s
                JOIN sessions sess ON s.session_id = sess.id{}
             )
             SELECT tools.tool_key,
                    COALESCE(NULLIF(MAX(tools.label), ''), tools.tool_key),
                    COALESCE(NULLIF(MAX(tools.category), ''), 'tool'),
                    COALESCE(SUM(tools.count), 0),
                    COUNT(DISTINCT tools.session_id)
             FROM session_tool_stats tools
             JOIN project_sessions active ON active.session_id = tools.session_id
             GROUP BY tools.tool_key
             ORDER BY COALESCE(SUM(tools.count), 0) DESC, tools.tool_key",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(&params), |row| {
            Ok(UsageProjectToolRow {
                key: row.get(0)?,
                label: row.get(1)?,
                category: row.get(2)?,
                count: row.get(3)?,
                sessions: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub(crate) fn usage_project_daily(
        &self,
        providers: &[String],
        bounds: UsageBucketBounds,
        project_path: &str,
        tz: Tz,
    ) -> Result<Vec<UsageProjectDailyRow>, rusqlite::Error> {
        let conn = self.lock_read()?;
        let (mut where_clause, mut params) = build_usage_where(providers, bounds);
        where_clause.push_str(&format!(" AND sess.project_path = ?{}", params.len() + 1));
        params.push(Box::new(project_path.to_string()));
        // Distinct-session counts fold per day in Rust (see activity_daily).
        let sql = format!(
            "SELECT s.bucket, sess.provider, COALESCE(NULLIF(s.model, ''), sess.model, ''), \
                    s.session_id, \
                    COALESCE(SUM(s.turn_count),0), \
                    COALESCE(SUM(s.input_tokens),0), \
                    COALESCE(SUM(s.output_tokens),0), \
                    COALESCE(SUM(s.cache_read_tokens),0), \
                    COALESCE(SUM(s.cache_write_tokens),0), \
                    COALESCE(SUM(s.cost_usd),0.0) \
             FROM session_token_stats s \
             JOIN sessions sess ON s.session_id = sess.id{} \
             GROUP BY s.bucket, sess.provider, COALESCE(NULLIF(s.model, ''), sess.model, ''), \
                      s.session_id",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        struct Acc {
            sessions: HashSet<String>,
            turns: u64,
            input_tokens: u64,
            output_tokens: u64,
            cache_read_tokens: u64,
            cache_write_tokens: u64,
            cost_usd: f64,
        }
        let rows = stmt.query_map(params_from_iter(&params), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, u64>(5)?,
                row.get::<_, u64>(6)?,
                row.get::<_, u64>(7)?,
                row.get::<_, u64>(8)?,
                row.get::<_, f64>(9)?,
            ))
        })?;
        let mut folder = DayFolder::new(tz, "usage_project_daily");
        let mut by_day: BTreeMap<(String, String, String), Acc> = BTreeMap::new();
        for row in rows {
            let (bucket, provider, model, session_id, turns, input, output, cr, cw, cost) = row?;
            let Some(date) = folder.date(bucket) else {
                continue;
            };
            let entry = by_day
                .entry((date, provider, model))
                .or_insert_with(|| Acc {
                    sessions: HashSet::new(),
                    turns: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    cost_usd: 0.0,
                });
            entry.sessions.insert(session_id);
            entry.turns += turns;
            entry.input_tokens += input;
            entry.output_tokens += output;
            entry.cache_read_tokens += cr;
            entry.cache_write_tokens += cw;
            entry.cost_usd += cost;
        }
        Ok(by_day
            .into_iter()
            .map(|((date, provider, model), acc)| UsageProjectDailyRow {
                date,
                provider,
                model,
                sessions: acc.sessions.len() as u64,
                turns: acc.turns,
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                cache_read_tokens: acc.cache_read_tokens,
                cache_write_tokens: acc.cache_write_tokens,
                cost_usd: acc.cost_usd,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::super::usage::tests::{parsed_session, sample_meta, stat_row, utc_bounds};
    use super::*;
    use crate::db::sync::ToolStatRow;
    use crate::models::Provider;
    use chrono_tz::Tz;
    use tempfile::TempDir;

    #[test]
    fn usage_project_daily_groups_by_provider_model_with_token_breakdown() {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path()).unwrap();
        let meta_a = sample_meta("session-a");
        let meta_b = sample_meta("session-b");
        let mut other_project = sample_meta("session-other-project");
        other_project.project_path = "/tmp/other".into();
        other_project.project_name = "other".into();
        db.sync_provider_snapshot(
            &Provider::Claude,
            &[
                parsed_session(meta_a.clone(), String::new()),
                parsed_session(meta_b.clone(), String::new()),
                parsed_session(other_project.clone(), String::new()),
            ],
            true,
            &[],
        )
        .unwrap();

        db.replace_token_stats(
            &meta_a.id,
            &[
                stat_row("2026-04-09", "claude-opus-4-6", 3, [100, 50, 20, 10], 0.10),
                stat_row("2026-04-09", "claude-sonnet-4-6", 2, [10, 5, 0, 0], 0.02),
            ],
        )
        .unwrap();
        db.replace_token_stats(
            &meta_b.id,
            &[stat_row(
                "2026-04-09",
                "claude-opus-4-6",
                4,
                [200, 100, 7, 3],
                0.30,
            )],
        )
        .unwrap();
        db.replace_token_stats(
            &other_project.id,
            &[stat_row(
                "2026-04-09",
                "claude-opus-4-6",
                9,
                [900, 900, 900, 900],
                9.00,
            )],
        )
        .unwrap();

        let rows = db
            .usage_project_daily(
                &[Provider::Claude.key().to_string()],
                utc_bounds(Some("2026-04-09"), Some("2026-04-09")),
                "/tmp/project",
                Tz::UTC,
            )
            .unwrap();

        assert_eq!(rows.len(), 2);
        let opus = rows
            .iter()
            .find(|row| row.model == "claude-opus-4-6")
            .unwrap();
        assert_eq!(opus.date, "2026-04-09");
        assert_eq!(opus.provider, Provider::Claude.key());
        assert_eq!(opus.sessions, 2);
        assert_eq!(opus.turns, 7);
        assert_eq!(opus.input_tokens, 300);
        assert_eq!(opus.output_tokens, 150);
        assert_eq!(opus.cache_read_tokens, 27);
        assert_eq!(opus.cache_write_tokens, 13);
        assert!((opus.cost_usd - 0.40).abs() < 1e-9);

        let sonnet = rows
            .iter()
            .find(|row| row.model == "claude-sonnet-4-6")
            .unwrap();
        assert_eq!(sonnet.sessions, 1);
        assert_eq!(sonnet.turns, 2);
        assert_eq!(sonnet.input_tokens, 10);
        assert_eq!(sonnet.output_tokens, 5);
        assert_eq!(sonnet.cache_read_tokens, 0);
        assert_eq!(sonnet.cache_write_tokens, 0);
        assert!((sonnet.cost_usd - 0.02).abs() < 1e-9);
    }

    #[test]
    fn usage_project_tool_usage_uses_cached_stats_without_token_join_duplication() {
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

        db.replace_token_stats(
            &meta_a.id,
            &[
                stat_row("2026-04-09", "claude-opus-4-6", 3, [100, 50, 20, 10], 0.10),
                stat_row("2026-04-09", "claude-sonnet-4-6", 2, [10, 5, 0, 0], 0.02),
            ],
        )
        .unwrap();
        db.replace_token_stats(
            &meta_b.id,
            &[stat_row(
                "2026-04-09",
                "claude-opus-4-6",
                4,
                [200, 100, 7, 3],
                0.30,
            )],
        )
        .unwrap();

        let providers = vec![Provider::Claude.key().to_string()];
        let bounds = utc_bounds(Some("2026-04-09"), Some("2026-04-09"));
        db.replace_tool_stats(
            &meta_a.id,
            &[
                ToolStatRow {
                    key: "Bash".into(),
                    label: "Bash".into(),
                    category: "terminal".into(),
                    count: 2,
                },
                ToolStatRow {
                    key: "Read".into(),
                    label: "Read".into(),
                    category: "file".into(),
                    count: 1,
                },
            ],
        )
        .unwrap();
        db.replace_tool_stats(
            &meta_b.id,
            &[ToolStatRow {
                key: "Bash".into(),
                label: "Bash".into(),
                category: "terminal".into(),
                count: 3,
            }],
        )
        .unwrap();

        let tools = db
            .usage_project_tool_usage(&providers, bounds, "/tmp/project")
            .unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].key, "Bash");
        assert_eq!(
            tools[0].count, 5,
            "session-a has two token rows but must count once"
        );
        assert_eq!(tools[0].sessions, 2);
        assert_eq!(tools[1].key, "Read");
        assert_eq!(tools[1].count, 1);
        assert_eq!(tools[1].sessions, 1);
    }
}
