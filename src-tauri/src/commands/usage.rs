use std::collections::{HashMap, HashSet};

use anyhow::Context;
use tauri::State;

use super::AppState;
use crate::db::queries::{UsageProjectModelDetailRow, UsageSessionModelDetailRow};
use crate::error::CommandResult;
use crate::models::*;

#[tauri::command]
pub async fn get_usage_stats(
    providers: Vec<String>,
    range_days: Option<u32>,
    state: State<'_, AppState>,
) -> CommandResult<UsageStats> {
    let state = state.inner().clone();
    let stats =
        tokio::task::spawn_blocking(move || build_usage_stats(&state, &providers, range_days))
            .await
            .context("task join error")??;
    Ok(stats)
}

#[tauri::command]
pub async fn get_today_cost(state: State<'_, AppState>) -> CommandResult<f64> {
    let state = state.inner().clone();
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

#[derive(serde::Serialize)]
pub struct TodayTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[tauri::command]
pub async fn get_today_tokens(state: State<'_, AppState>) -> CommandResult<TodayTokens> {
    let state = state.inner().clone();
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
) -> anyhow::Result<UsageStats> {
    let cutoff_date = range_days.and_then(cutoff_date_for_range_days);
    let cutoff_ref = cutoff_date.as_deref();

    let total_sessions = state
        .db
        .usage_session_count(providers, cutoff_ref)
        .context("failed to count usage sessions")?;

    let (total_turns, total_in, total_out, total_cr, total_cw) = state
        .db
        .usage_totals(providers, cutoff_ref)
        .context("failed to query usage totals")?;

    let daily_rows = state
        .db
        .usage_daily(providers, cutoff_ref)
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
        .usage_by_model(providers, cutoff_ref)
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
        .usage_project_model_detail(providers, cutoff_ref)
        .context("failed to query project model detail")?;

    let project_costs = build_project_costs(project_model_rows);

    // Recent sessions: query per (session, model) for accurate per-model pricing,
    // then aggregate by session with the dominant model label.
    let session_model_rows = state
        .db
        .usage_session_model_detail(providers, cutoff_ref, 100)
        .context("failed to query session model detail")?;

    let recent_sessions = build_recent_sessions(session_model_rows);

    let cache_input_total = total_cr + total_in;
    let cache_hit_rate = if cache_input_total > 0 {
        total_cr as f64 / cache_input_total as f64
    } else {
        0.0
    };

    // Previous period for trend comparison.
    // Only computed when a concrete range is given and enough historical data exists.
    let prev_period = if let Some(days) = range_days {
        if days == 0 {
            None
        } else {
            let today = chrono::Local::now().date_naive();
            let cur_start = today - chrono::Duration::days(i64::from(days.saturating_sub(1)));
            let prev_start = cur_start - chrono::Duration::days(i64::from(days));
            let prev_start_str = prev_start.format("%Y-%m-%d").to_string();
            let prev_end_str = cur_start.format("%Y-%m-%d").to_string();

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
        }
    } else {
        None
    };

    let provider_session_counts = state
        .db
        .usage_session_count_by_provider(providers, cutoff_ref)
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

fn cutoff_date_for_range_days(days: u32) -> Option<String> {
    if days == 0 {
        return None;
    }

    let today = chrono::Local::now().date_naive();
    let cutoff = today - chrono::Duration::days(i64::from(days.saturating_sub(1)));
    Some(cutoff.format("%Y-%m-%d").to_string())
}

fn build_project_costs(project_model_rows: Vec<UsageProjectModelDetailRow>) -> Vec<ProjectCost> {
    // Merge across providers: one row per project_path, summing usage from every
    // tool used in that project, and collecting the set of providers that
    // contributed. Distinct project paths stay separate even if they share a name.
    let mut project_map: HashMap<String, ProjectCost> = HashMap::new();
    let mut project_sessions: HashMap<String, HashSet<String>> = HashMap::new();
    // Per-(project_path, provider) breakdown so each merged row can expand to
    // show how much each tool contributed.
    let mut pp_map: HashMap<(String, String), ProjectProviderUsage> = HashMap::new();
    let mut pp_sessions: HashMap<(String, String), HashSet<String>> = HashMap::new();

    for row in project_model_rows {
        let path = row.project_path.clone();
        let provider = row.provider.clone();
        let tokens =
            row.input_tokens + row.output_tokens + row.cache_read_tokens + row.cache_write_tokens;

        project_sessions
            .entry(path.clone())
            .or_default()
            .insert(row.session_id.clone());
        pp_sessions
            .entry((path.clone(), provider.clone()))
            .or_default()
            .insert(row.session_id);

        let entry = project_map
            .entry(path.clone())
            .or_insert_with(|| ProjectCost {
                project: row.project_name,
                project_path: path.clone(),
                providers: Vec::new(),
                by_provider: Vec::new(),
                sessions: 0,
                turns: 0,
                tokens: 0,
                cost: 0.0,
            });
        entry.turns += row.turns;
        entry.tokens += tokens;
        entry.cost += row.cost_usd;

        let pp = pp_map
            .entry((path, provider.clone()))
            .or_insert_with(|| ProjectProviderUsage {
                provider,
                sessions: 0,
                turns: 0,
                tokens: 0,
                cost: 0.0,
            });
        pp.turns += row.turns;
        pp.tokens += tokens;
        pp.cost += row.cost_usd;
    }

    // Group the per-provider rows under their project, with distinct session
    // counts, each list sorted by cost desc.
    let mut by_project: HashMap<String, Vec<ProjectProviderUsage>> = HashMap::new();
    for ((path, provider), mut pp) in pp_map {
        pp.sessions = pp_sessions
            .get(&(path.clone(), provider))
            .map(|sessions| sessions.len() as u64)
            .unwrap_or(0);
        by_project.entry(path).or_default().push(pp);
    }
    for list in by_project.values_mut() {
        list.sort_by(|a, b| {
            b.cost
                .partial_cmp(&a.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    let mut project_costs: Vec<ProjectCost> = project_map
        .into_iter()
        .map(|(key, mut cost_row)| {
            cost_row.sessions = project_sessions
                .remove(&key)
                .map(|sessions| sessions.len() as u64)
                .unwrap_or(0);
            let breakdown = by_project.remove(&key).unwrap_or_default();
            let mut providers: Vec<String> = breakdown.iter().map(|p| p.provider.clone()).collect();
            providers.sort();
            cost_row.providers = providers;
            cost_row.by_provider = breakdown;
            cost_row
        })
        .collect();
    project_costs.sort_by(|a, b| {
        b.cost
            .partial_cmp(&a.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    project_costs
}

fn build_recent_sessions(
    session_model_rows: Vec<UsageSessionModelDetailRow>,
) -> Vec<SessionCostRow> {
    let mut session_map: HashMap<String, SessionCostRow> = HashMap::new();
    let mut session_order: Vec<String> = Vec::new();
    let mut dominant_model: HashMap<String, (String, u64, f64)> = HashMap::new();

    for row in session_model_rows {
        let tokens =
            row.input_tokens + row.output_tokens + row.cache_read_tokens + row.cache_write_tokens;
        let entry = session_map
            .entry(row.session_id.clone())
            .or_insert_with(|| {
                session_order.push(row.session_id.clone());
                SessionCostRow {
                    id: row.session_id.clone(),
                    project: row.project_name.clone(),
                    project_path: row.project_path.clone(),
                    provider: row.provider.clone(),
                    model: String::new(),
                    updated_at: row.updated_at,
                    turns: 0,
                    tokens: 0,
                    cost: 0.0,
                }
            });
        entry.turns += row.turns;
        entry.tokens += tokens;
        entry.cost += row.cost_usd;

        let best = dominant_model
            .entry(row.session_id)
            .or_insert_with(|| (row.model.clone(), tokens, row.cost_usd));
        if tokens > best.1 || (tokens == best.1 && row.cost_usd > best.2 && !row.model.is_empty()) {
            *best = (row.model, tokens, row.cost_usd);
        }
    }

    for (id, (model, _, _)) in dominant_model {
        if let Some(entry) = session_map.get_mut(&id) {
            entry.model = model;
        }
    }

    session_order
        .into_iter()
        .filter_map(|id| session_map.remove(&id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{build_project_costs, build_recent_sessions, cutoff_date_for_range_days};
    use crate::db::queries::{UsageProjectModelDetailRow, UsageSessionModelDetailRow};

    #[test]
    fn project_costs_count_distinct_sessions_exactly() {
        let rows = vec![
            UsageProjectModelDetailRow {
                project_path: "/tmp/drama/ccsession".to_string(),
                project_name: "drama/ccsession".to_string(),
                provider: "claude".to_string(),
                session_id: "session-a".to_string(),
                turns: 12,
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 20,
                cache_write_tokens: 10,
                cost_usd: 1.0,
            },
            UsageProjectModelDetailRow {
                project_path: "/tmp/drama/ccsession".to_string(),
                project_name: "drama/ccsession".to_string(),
                provider: "claude".to_string(),
                session_id: "session-a".to_string(),
                turns: 8,
                input_tokens: 40,
                output_tokens: 10,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.5,
            },
            UsageProjectModelDetailRow {
                project_path: "/tmp/drama/ccsession".to_string(),
                project_name: "drama/ccsession".to_string(),
                provider: "claude".to_string(),
                session_id: "session-b".to_string(),
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
        assert_eq!(project_costs[0].project_path, "/tmp/drama/ccsession");
        assert_eq!(project_costs[0].turns, 24);
        assert_eq!(project_costs[0].tokens, 260);
    }

    #[test]
    fn project_costs_merge_providers_for_same_project() {
        let rows = vec![
            UsageProjectModelDetailRow {
                project_path: "/tmp/myproj".to_string(),
                project_name: "myproj".to_string(),
                provider: "claude".to_string(),
                session_id: "session-a".to_string(),
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
                project_path: "/tmp/drama/ccsession".to_string(),
                project_name: "drama/ccsession".to_string(),
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
                project_path: "/tmp/drama/ccsession".to_string(),
                project_name: "drama/ccsession".to_string(),
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
        assert_eq!(recent_sessions[0].project_path, "/tmp/drama/ccsession");
        assert_eq!(recent_sessions[0].turns, 8);
        assert_eq!(recent_sessions[0].tokens, 1_740);
    }

    #[test]
    fn project_costs_keep_same_name_different_paths_separate() {
        let rows = vec![
            UsageProjectModelDetailRow {
                project_path: "/tmp/api-server".to_string(),
                project_name: "api-server".to_string(),
                provider: "codex".to_string(),
                session_id: "session-a".to_string(),
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
