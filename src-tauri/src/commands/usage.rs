mod aggregate;

use anyhow::Context;
use serde::Serialize;

use super::sessions::load_messages_cached;
use super::AppState;
use crate::db::queries::UsageDateBounds;
use crate::db::sync::build_tool_stats;
use crate::error::CommandResult;
use crate::models::*;
use crate::services::load_session_meta;
use aggregate::{build_project_costs, build_recent_sessions};

pub async fn get_usage_stats(
    providers: Vec<String>,
    range_days: Option<u32>,
    date_start: Option<String>,
    date_end: Option<String>,
    state: AppState,
) -> CommandResult<UsageStats> {
    // Tauri commands are a trust boundary: reject malformed dates instead of
    // silently passing them into SQL string comparisons.
    let custom_range = parse_custom_range(date_start.as_deref(), date_end.as_deref())?;
    let stats = tokio::task::spawn_blocking(move || {
        build_usage_stats(&state, &providers, range_days, custom_range)
    })
    .await
    .context("task join error")??;
    Ok(stats)
}

/// Validate and order an optional custom `[start, end]` date range (inclusive).
fn parse_custom_range(
    date_start: Option<&str>,
    date_end: Option<&str>,
) -> anyhow::Result<Option<(chrono::NaiveDate, chrono::NaiveDate)>> {
    let (Some(start), Some(end)) = (date_start, date_end) else {
        if date_start.is_some() || date_end.is_some() {
            anyhow::bail!("custom date range requires both date_start and date_end");
        }
        return Ok(None);
    };
    let start = chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d")
        .with_context(|| format!("invalid date_start '{start}', expected YYYY-MM-DD"))?;
    let end = chrono::NaiveDate::parse_from_str(end, "%Y-%m-%d")
        .with_context(|| format!("invalid date_end '{end}', expected YYYY-MM-DD"))?;
    if start > end {
        anyhow::bail!("date_start must not be after date_end");
    }
    Ok(Some((start, end)))
}

/// GitHub-style activity calendar: per-day aggregates over `[date_start,
/// date_end]` (inclusive) plus the years that have data. The window is computed
/// on the frontend from the selected year, so the calendar is independent of
/// the usage panel's range filter.
pub async fn get_activity_calendar(
    providers: Vec<String>,
    date_start: String,
    date_end: String,
    state: AppState,
) -> CommandResult<ActivityCalendar> {
    // Trust boundary: reject malformed dates instead of passing them into SQL.
    parse_custom_range(Some(&date_start), Some(&date_end))?;
    let calendar = tokio::task::spawn_blocking(move || -> anyhow::Result<ActivityCalendar> {
        let bounds = UsageDateBounds {
            start: Some(&date_start),
            end: Some(&date_end),
        };
        let days = state
            .db
            .activity_daily(&providers, bounds)
            .context("failed to query activity calendar")?
            .into_iter()
            .map(|(date, sessions, turns, tokens, cost)| ActivityDay {
                date,
                sessions,
                turns,
                tokens,
                cost,
            })
            .collect();
        let available_years = state
            .db
            .activity_years(&providers)
            .context("failed to query activity years")?;
        Ok(ActivityCalendar {
            days,
            available_years,
        })
    })
    .await
    .context("task join error")??;
    Ok(calendar)
}

pub async fn get_project_tool_usage(
    project_path: String,
    providers: Vec<String>,
    range_days: Option<u32>,
    date_start: Option<String>,
    date_end: Option<String>,
    state: AppState,
) -> CommandResult<ProjectToolUsageStats> {
    let custom_range = parse_custom_range(date_start.as_deref(), date_end.as_deref())?;
    let stats = tokio::task::spawn_blocking(move || {
        build_project_tool_usage(&state, &project_path, &providers, range_days, custom_range)
    })
    .await
    .context("task join error")??;
    Ok(stats)
}

pub async fn get_project_daily_usage(
    project_path: String,
    providers: Vec<String>,
    range_days: Option<u32>,
    date_start: Option<String>,
    date_end: Option<String>,
    state: AppState,
) -> CommandResult<Vec<ProjectDailyUsage>> {
    let custom_range = parse_custom_range(date_start.as_deref(), date_end.as_deref())?;
    let days = tokio::task::spawn_blocking(move || {
        build_project_daily_usage(&state, &project_path, &providers, range_days, custom_range)
    })
    .await
    .context("task join error")??;
    Ok(days)
}

pub async fn get_today_cost(state: AppState) -> CommandResult<f64> {
    let cost = tokio::task::spawn_blocking(move || -> anyhow::Result<f64> {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let cost = state
            .db
            .cost_for_date(&today)
            .context("failed to query today cost")?;
        Ok(cost)
    })
    .await
    .context("task join error")??;
    Ok(cost)
}

#[derive(Serialize)]
pub struct TodayTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

pub async fn get_today_tokens(state: AppState) -> CommandResult<TodayTokens> {
    let tokens = tokio::task::spawn_blocking(move || -> anyhow::Result<TodayTokens> {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let (input, output, cache_read, cache_write) = state
            .db
            .tokens_for_date(&today)
            .context("failed to query today tokens")?;
        Ok(TodayTokens {
            input,
            output,
            cache_read,
            cache_write,
        })
    })
    .await
    .context("task join error")??;
    Ok(tokens)
}

fn build_usage_stats(
    state: &AppState,
    providers: &[String],
    range_days: Option<u32>,
    custom_range: Option<(chrono::NaiveDate, chrono::NaiveDate)>,
) -> anyhow::Result<UsageStats> {
    // A validated custom range takes precedence over the preset day count.
    let (start_date, end_date) = match custom_range {
        Some((start, end)) => (
            Some(start.format("%Y-%m-%d").to_string()),
            Some(end.format("%Y-%m-%d").to_string()),
        ),
        None => (range_days.and_then(cutoff_date_for_range_days), None),
    };
    let bounds = UsageDateBounds {
        start: start_date.as_deref(),
        end: end_date.as_deref(),
    };

    let total_sessions = state
        .db
        .usage_session_count(providers, bounds)
        .context("failed to count usage sessions")?;

    let (total_turns, total_in, total_out, total_cr, total_cw) = state
        .db
        .usage_totals(providers, bounds)
        .context("failed to query usage totals")?;

    let daily_rows = state
        .db
        .usage_daily(providers, bounds)
        .context("failed to query daily usage")?;
    let daily_usage: Vec<DailyUsage> = daily_rows
        .into_iter()
        .map(|(date, provider, tokens, cost)| DailyUsage {
            date,
            provider,
            tokens,
            cost,
        })
        .collect();

    let model_rows = state
        .db
        .usage_by_model(providers, bounds)
        .context("failed to query usage by model")?;
    let model_costs: Vec<ModelCost> = model_rows
        .into_iter()
        .map(|row| ModelCost {
            model: row.model,
            turns: row.turns,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_tokens: row.cache_read_tokens + row.cache_write_tokens,
            cost: row.cost_usd,
        })
        .collect();

    let total_cost: f64 = model_costs.iter().map(|m| m.cost).sum();

    // Project costs: query per (project, provider, session, model) for accurate
    // per-model pricing while deduplicating session counts exactly.
    let project_model_rows = state
        .db
        .usage_project_model_detail(providers, bounds)
        .context("failed to query project model detail")?;

    let project_costs = build_project_costs(project_model_rows);

    // Recent sessions: query per (session, model) for accurate per-model pricing,
    // then aggregate by session with the dominant model label.
    let session_model_rows = state
        .db
        .usage_session_model_detail(providers, bounds, 100)
        .context("failed to query session model detail")?;

    let recent_sessions = build_recent_sessions(session_model_rows);

    let cache_input_total = total_cr + total_in;
    let cache_hit_rate = if cache_input_total > 0 {
        total_cr as f64 / cache_input_total as f64
    } else {
        0.0
    };

    // Previous period for trend comparison: the same number of days
    // immediately before the current window. Only computed when a concrete
    // range is given (preset days or custom dates).
    let prev_window = match custom_range {
        Some((start, end)) => {
            let days = (end - start).num_days() + 1;
            Some((start - chrono::Duration::days(days), start))
        }
        None => range_days.filter(|days| *days > 0).map(|days| {
            let today = chrono::Local::now().date_naive();
            let cur_start = today - chrono::Duration::days(i64::from(days.saturating_sub(1)));
            (
                cur_start - chrono::Duration::days(i64::from(days)),
                cur_start,
            )
        }),
    };
    let prev_period = if let Some((prev_start, prev_end)) = prev_window {
        let prev_start_str = prev_start.format("%Y-%m-%d").to_string();
        let prev_end_str = prev_end.format("%Y-%m-%d").to_string();

        let (sessions, turns, inp, out, cr, cw, cost) = state
            .db
            .usage_totals_range(providers, &prev_start_str, &prev_end_str)
            .context("failed to query previous-period usage totals")?;

        // Only return if prev period has data
        let total_tokens = inp + out + cr + cw;
        if sessions == 0 && turns == 0 {
            None
        } else {
            Some(PrevPeriodTotals {
                total_sessions: sessions,
                total_turns: turns,
                total_tokens,
                total_cost: cost,
            })
        }
    } else {
        None
    };

    let provider_session_counts = state
        .db
        .usage_session_count_by_provider(providers, bounds)
        .context("failed to count sessions by provider")?
        .into_iter()
        .map(|(provider, count)| ProviderSessionCount { provider, count })
        .collect();

    Ok(UsageStats {
        total_sessions,
        total_turns,
        total_input_tokens: total_in,
        total_output_tokens: total_out,
        total_cache_read_tokens: total_cr,
        total_cache_write_tokens: total_cw,
        total_cost,
        cache_hit_rate,
        daily_usage,
        model_costs,
        project_costs,
        recent_sessions,
        provider_session_counts,
        prev_period,
    })
}

fn build_project_tool_usage(
    state: &AppState,
    project_path: &str,
    providers: &[String],
    range_days: Option<u32>,
    custom_range: Option<(chrono::NaiveDate, chrono::NaiveDate)>,
) -> anyhow::Result<ProjectToolUsageStats> {
    let (start_date, end_date) = match custom_range {
        Some((start, end)) => (
            Some(start.format("%Y-%m-%d").to_string()),
            Some(end.format("%Y-%m-%d").to_string()),
        ),
        None => (range_days.and_then(cutoff_date_for_range_days), None),
    };
    let bounds = UsageDateBounds {
        start: start_date.as_deref(),
        end: end_date.as_deref(),
    };
    let session_ids = state
        .db
        .usage_project_session_ids(providers, bounds, project_path)
        .with_context(|| format!("failed to query sessions for project_path={project_path}"))?;
    let missing_tool_stats = state
        .db
        .usage_project_sessions_missing_tool_stats(providers, bounds, project_path)
        .with_context(|| {
            format!("failed to query tool-stat cache gaps for project_path={project_path}")
        })?;
    for session_id in &missing_tool_stats {
        let meta = load_session_meta(&state.db, session_id).map_err(anyhow::Error::msg)?;
        let (messages, _, _) = load_messages_cached(state, &meta)
            .with_context(|| format!("failed to load messages for session {session_id}"))?;
        let tool_stats = build_tool_stats(&messages);
        state
            .db
            .replace_tool_stats(session_id, &tool_stats)
            .with_context(|| format!("failed to cache tool stats for session {session_id}"))?;
    }

    let tools = state
        .db
        .usage_project_tool_usage(providers, bounds, project_path)
        .with_context(|| format!("failed to query tool usage for project_path={project_path}"))?
        .into_iter()
        .map(|row| ProjectToolUsage {
            key: row.key,
            label: row.label,
            category: row.category,
            count: row.count,
            sessions: row.sessions,
        })
        .collect::<Vec<_>>();
    let tool_calls = tools.iter().map(|tool| tool.count).sum();

    Ok(ProjectToolUsageStats {
        project_path: project_path.to_string(),
        sessions_scanned: session_ids.len() as u64,
        tool_calls,
        tools,
    })
}

fn build_project_daily_usage(
    state: &AppState,
    project_path: &str,
    providers: &[String],
    range_days: Option<u32>,
    custom_range: Option<(chrono::NaiveDate, chrono::NaiveDate)>,
) -> anyhow::Result<Vec<ProjectDailyUsage>> {
    let (start_date, end_date) = match custom_range {
        Some((start, end)) => (
            Some(start.format("%Y-%m-%d").to_string()),
            Some(end.format("%Y-%m-%d").to_string()),
        ),
        None => (range_days.and_then(cutoff_date_for_range_days), None),
    };
    let bounds = UsageDateBounds {
        start: start_date.as_deref(),
        end: end_date.as_deref(),
    };
    let rows = state
        .db
        .usage_project_daily(providers, bounds, project_path)
        .with_context(|| format!("failed to query daily usage for project_path={project_path}"))?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let tokens = row.input_tokens
                + row.output_tokens
                + row.cache_read_tokens
                + row.cache_write_tokens;
            ProjectDailyUsage {
                date: row.date,
                provider: row.provider,
                model: row.model,
                sessions: row.sessions,
                turns: row.turns,
                input_tokens: row.input_tokens,
                output_tokens: row.output_tokens,
                cache_read_tokens: row.cache_read_tokens,
                cache_write_tokens: row.cache_write_tokens,
                tokens,
                cost: row.cost_usd,
            }
        })
        .collect())
}

fn cutoff_date_for_range_days(days: u32) -> Option<String> {
    if days == 0 {
        return None;
    }

    let today = chrono::Local::now().date_naive();
    let cutoff = today - chrono::Duration::days(i64::from(days.saturating_sub(1)));
    Some(cutoff.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        build_project_costs, build_recent_sessions, cutoff_date_for_range_days, parse_custom_range,
    };
    use crate::db::queries::{UsageProjectModelDetailRow, UsageSessionModelDetailRow};

    fn project_usage_row(
        project_path: &str,
        provider: &str,
        model: &str,
        session_id: &str,
        cost_usd: f64,
    ) -> UsageProjectModelDetailRow {
        UsageProjectModelDetailRow {
            project_path: project_path.to_string(),
            project_name: "project".to_string(),
            provider: provider.to_string(),
            session_id: session_id.to_string(),
            model: model.to_string(),
            turns: 1,
            input_tokens: 10,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd,
        }
    }

    fn session_usage_row(
        session_id: &str,
        model: &str,
        input_tokens: u64,
        cost_usd: f64,
    ) -> UsageSessionModelDetailRow {
        UsageSessionModelDetailRow {
            session_id: session_id.to_string(),
            project_path: "/tmp/project".to_string(),
            project_name: "project".to_string(),
            provider: "codex".to_string(),
            updated_at: 1_700_000_000,
            model: model.to_string(),
            turns: 1,
            input_tokens,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd,
        }
    }

    #[test]
    fn custom_range_parses_valid_inclusive_dates() {
        let range = parse_custom_range(Some("2026-05-01"), Some("2026-05-20"))
            .unwrap()
            .unwrap();
        assert_eq!(range.0.to_string(), "2026-05-01");
        assert_eq!(range.1.to_string(), "2026-05-20");
    }

    #[test]
    fn custom_range_absent_when_no_dates_given() {
        assert!(parse_custom_range(None, None).unwrap().is_none());
    }

    #[test]
    fn custom_range_rejects_malformed_or_partial_input() {
        assert!(parse_custom_range(Some("05/01/2026"), Some("2026-05-20")).is_err());
        assert!(parse_custom_range(Some("2026-05-01"), Some("not-a-date")).is_err());
        assert!(parse_custom_range(Some("2026-05-01"), None).is_err());
        assert!(parse_custom_range(None, Some("2026-05-20")).is_err());
        assert!(
            parse_custom_range(Some("2026-05-21"), Some("2026-05-20")).is_err(),
            "start after end must be rejected"
        );
    }

    #[test]
    fn project_costs_count_distinct_sessions_exactly() {
        let rows = vec![
            UsageProjectModelDetailRow {
                project_path: "/tmp/drama/sessionview".to_string(),
                project_name: "drama/sessionview".to_string(),
                provider: "claude".to_string(),
                session_id: "session-a".to_string(),
                model: "sonnet-4-6".to_string(),
                turns: 12,
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 20,
                cache_write_tokens: 10,
                cost_usd: 1.0,
            },
            UsageProjectModelDetailRow {
                project_path: "/tmp/drama/sessionview".to_string(),
                project_name: "drama/sessionview".to_string(),
                provider: "claude".to_string(),
                session_id: "session-a".to_string(),
                model: "opus-4-6".to_string(),
                turns: 8,
                input_tokens: 40,
                output_tokens: 10,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.5,
            },
            UsageProjectModelDetailRow {
                project_path: "/tmp/drama/sessionview".to_string(),
                project_name: "drama/sessionview".to_string(),
                provider: "claude".to_string(),
                session_id: "session-b".to_string(),
                model: "opus-4-6".to_string(),
                turns: 4,
                input_tokens: 20,
                output_tokens: 10,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.25,
            },
        ];

        let project_costs = build_project_costs(rows);
        assert_eq!(project_costs.len(), 1);
        assert_eq!(project_costs[0].sessions, 2);
        assert_eq!(project_costs[0].project_path, "/tmp/drama/sessionview");
        assert_eq!(project_costs[0].turns, 24);
        assert_eq!(project_costs[0].input_tokens, 160);
        assert_eq!(project_costs[0].output_tokens, 70);
        assert_eq!(project_costs[0].cache_read_tokens, 20);
        assert_eq!(project_costs[0].cache_write_tokens, 10);
        assert_eq!(project_costs[0].tokens, 260);
        let by_model = &project_costs[0].by_model;
        assert_eq!(by_model.len(), 2);
        assert_eq!(by_model[0].model, "sonnet-4-6");
        assert_eq!(by_model[0].sessions, 1);
        assert_eq!(by_model[0].turns, 12);
        assert_eq!(by_model[0].tokens, 180);
        assert_eq!(by_model[1].model, "opus-4-6");
        assert_eq!(by_model[1].sessions, 2);
        assert_eq!(by_model[1].tokens, 80);
    }

    #[test]
    fn project_costs_merge_providers_for_same_project() {
        let rows = vec![
            UsageProjectModelDetailRow {
                project_path: "/tmp/myproj".to_string(),
                project_name: "myproj".to_string(),
                provider: "claude".to_string(),
                session_id: "session-a".to_string(),
                model: "claude-opus".to_string(),
                turns: 10,
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 2.0,
            },
            UsageProjectModelDetailRow {
                project_path: "/tmp/myproj".to_string(),
                project_name: "myproj".to_string(),
                provider: "codex".to_string(),
                session_id: "session-b".to_string(),
                model: "gpt-5".to_string(),
                turns: 5,
                input_tokens: 60,
                output_tokens: 40,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 1.0,
            },
        ];

        // Same project used under two tools merges into ONE row that combines
        // their usage and lists both providers.
        let project_costs = build_project_costs(rows);
        assert_eq!(project_costs.len(), 1);
        assert_eq!(project_costs[0].providers, vec!["claude", "codex"]);
        assert_eq!(project_costs[0].sessions, 2);
        assert_eq!(project_costs[0].turns, 15);
        assert_eq!(project_costs[0].input_tokens, 160);
        assert_eq!(project_costs[0].output_tokens, 90);
        assert_eq!(project_costs[0].tokens, 250);
        assert_eq!(project_costs[0].cost, 3.0);
        // Per-provider breakdown, sorted by cost desc (claude $2 before codex $1).
        let bp = &project_costs[0].by_provider;
        assert_eq!(bp.len(), 2);
        assert_eq!(bp[0].provider, "claude");
        assert_eq!(bp[0].sessions, 1);
        assert_eq!(bp[0].turns, 10);
        assert_eq!(bp[0].tokens, 150);
        assert_eq!(bp[0].cost, 2.0);
        assert_eq!(bp[1].provider, "codex");
        assert_eq!(bp[1].cost, 1.0);
    }

    #[test]
    fn recent_sessions_keep_dominant_model_label() {
        let rows = vec![
            UsageSessionModelDetailRow {
                session_id: "session-a".to_string(),
                project_path: "/tmp/drama/sessionview".to_string(),
                project_name: "drama/sessionview".to_string(),
                provider: "claude".to_string(),
                updated_at: 1_700_000_000,
                model: "sonnet-4-6".to_string(),
                turns: 6,
                input_tokens: 200,
                output_tokens: 40,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.5,
            },
            UsageSessionModelDetailRow {
                session_id: "session-a".to_string(),
                project_path: "/tmp/drama/sessionview".to_string(),
                project_name: "drama/sessionview".to_string(),
                provider: "claude".to_string(),
                updated_at: 1_700_000_000,
                model: "opus-4-6".to_string(),
                turns: 2,
                input_tokens: 1_200,
                output_tokens: 300,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 1.0,
            },
        ];

        let recent_sessions = build_recent_sessions(rows);
        assert_eq!(recent_sessions.len(), 1);
        assert_eq!(recent_sessions[0].model, "opus-4-6");
        assert_eq!(recent_sessions[0].project_path, "/tmp/drama/sessionview");
        assert_eq!(recent_sessions[0].turns, 8);
        assert_eq!(recent_sessions[0].tokens, 1_740);
    }

    #[test]
    fn recent_sessions_with_interleaved_rows_keep_first_seen_order() {
        let recent_sessions = build_recent_sessions(vec![
            session_usage_row("session-b", "model-b1", 10, 0.1),
            session_usage_row("session-a", "model-a1", 20, 0.2),
            session_usage_row("session-b", "model-b2", 30, 0.3),
            session_usage_row("session-c", "model-c1", 5, 0.05),
            session_usage_row("session-a", "model-a2", 1, 0.01),
        ]);

        assert_eq!(
            recent_sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-b", "session-a", "session-c"]
        );
        assert_eq!(recent_sessions[0].tokens, 40);
        assert_eq!(recent_sessions[0].model, "model-b2");
        assert_eq!(recent_sessions[1].tokens, 21);
    }

    #[test]
    fn project_costs_with_equal_costs_use_stable_name_order() {
        let project_costs = build_project_costs(vec![
            project_usage_row("/tmp/project-z", "zeta", "model-z", "session-z", 1.0),
            project_usage_row("/tmp/project-z", "alpha", "model-a", "session-a", 1.0),
            project_usage_row("/tmp/project-a", "codex", "model-c", "session-c", 2.0),
        ]);

        assert_eq!(
            project_costs
                .iter()
                .map(|project| project.project_path.as_str())
                .collect::<Vec<_>>(),
            vec!["/tmp/project-a", "/tmp/project-z"]
        );
        assert_eq!(project_costs[1].providers, vec!["alpha", "zeta"]);
        assert_eq!(project_costs[1].by_provider[0].provider, "alpha");
        assert_eq!(project_costs[1].by_provider[1].provider, "zeta");
        assert_eq!(project_costs[1].by_model[0].model, "model-a");
        assert_eq!(project_costs[1].by_model[1].model, "model-z");
    }

    #[test]
    fn project_costs_keep_same_name_different_paths_separate() {
        let rows = vec![
            UsageProjectModelDetailRow {
                project_path: "/tmp/api-server".to_string(),
                project_name: "api-server".to_string(),
                provider: "codex".to_string(),
                session_id: "session-a".to_string(),
                model: "gpt-5".to_string(),
                turns: 2,
                input_tokens: 100,
                output_tokens: 40,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.1,
            },
            UsageProjectModelDetailRow {
                project_path: "/work/api-server".to_string(),
                project_name: "api-server".to_string(),
                provider: "codex".to_string(),
                session_id: "session-b".to_string(),
                model: "gpt-5".to_string(),
                turns: 3,
                input_tokens: 120,
                output_tokens: 60,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.2,
            },
        ];

        let project_costs = build_project_costs(rows);
        assert_eq!(project_costs.len(), 2);
    }

    #[test]
    fn cutoff_range_is_inclusive_of_today() {
        let cutoff = cutoff_date_for_range_days(7).expect("cutoff");
        let expected = (chrono::Local::now().date_naive() - chrono::Duration::days(6))
            .format("%Y-%m-%d")
            .to_string();
        assert_eq!(cutoff, expected);
    }
}
