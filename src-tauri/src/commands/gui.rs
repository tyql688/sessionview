//! Tauri shell: thin `#[tauri::command]` wrappers over the transport-agnostic
//! command cores in the sibling modules. The headless HTTP shell dispatches to
//! the same cores, so business logic lives in exactly one place.
//!
//! Wrapper argument names must match the core signatures verbatim — Tauri maps
//! the frontend's camelCase invoke args onto these snake_case names.

use std::collections::HashMap;

use anyhow::Context;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::error::CommandResult;
use crate::models::{
    ActivityCalendar, IndexStats, PricingCatalogStatus, ProjectDailyUsage, ProjectToolUsageStats,
    ProviderSnapshot, SearchFilters, SearchResult, SessionDetail, SessionMeta, TreeNode,
    UsageStats,
};
use crate::services::session_view::SessionTurnOutline;

use super::{AppState, SessionMessagesWindow, SessionOpenWindow, TodayTokens};

#[tauri::command]
pub async fn reindex(state: State<'_, AppState>) -> CommandResult<usize> {
    super::reindex(state.inner().clone()).await
}

#[tauri::command]
pub async fn reindex_providers(
    providers: Vec<String>,
    aggressive: Option<bool>,
    state: State<'_, AppState>,
) -> CommandResult<usize> {
    super::reindex_providers(providers, aggressive, state.inner().clone()).await
}

#[tauri::command]
pub async fn get_tree(state: State<'_, AppState>) -> CommandResult<Vec<TreeNode>> {
    super::get_tree(state.inner().clone()).await
}

#[tauri::command]
pub async fn get_session_detail(
    session_id: String,
    request_seq: Option<u64>,
    state: State<'_, AppState>,
) -> CommandResult<SessionDetail> {
    super::get_session_detail(session_id, request_seq, state.inner().clone()).await
}

#[tauri::command]
pub async fn get_session_meta(
    session_id: String,
    state: State<'_, AppState>,
) -> CommandResult<SessionMeta> {
    super::get_session_meta(session_id, state.inner().clone()).await
}

#[tauri::command]
pub async fn get_session_open_window(
    session_id: String,
    offset: i64,
    limit: usize,
    request_id: Option<String>,
    request_seq: Option<u64>,
    state: State<'_, AppState>,
) -> CommandResult<SessionOpenWindow> {
    super::get_session_open_window(
        session_id,
        offset,
        limit,
        request_id,
        request_seq,
        state.inner().clone(),
    )
    .await
}

#[tauri::command]
pub async fn get_session_messages_window(
    session_id: String,
    offset: i64,
    limit: usize,
    request_id: Option<String>,
    request_seq: Option<u64>,
    state: State<'_, AppState>,
) -> CommandResult<SessionMessagesWindow> {
    super::get_session_messages_window(
        session_id,
        offset,
        limit,
        request_id,
        request_seq,
        state.inner().clone(),
    )
    .await
}

#[tauri::command]
pub async fn get_session_turn_outline(
    session_id: String,
    request_seq: Option<u64>,
    state: State<'_, AppState>,
) -> CommandResult<SessionTurnOutline> {
    super::get_session_turn_outline(session_id, request_seq, state.inner().clone()).await
}

#[tauri::command]
pub async fn cancel_session_load(
    session_id: String,
    request_id: Option<String>,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    super::cancel_session_load(session_id, request_id, state.inner().clone()).await
}

#[tauri::command]
pub async fn get_child_sessions(
    parent_id: String,
    state: State<'_, AppState>,
) -> CommandResult<Vec<SessionMeta>> {
    super::get_child_sessions(parent_id, state.inner().clone()).await
}

#[tauri::command]
pub async fn get_child_session_counts(
    parent_ids: Vec<String>,
    state: State<'_, AppState>,
) -> CommandResult<HashMap<String, u64>> {
    super::get_child_session_counts(parent_ids, state.inner().clone()).await
}

#[tauri::command]
pub async fn search_sessions(
    filters: SearchFilters,
    state: State<'_, AppState>,
) -> CommandResult<Vec<SearchResult>> {
    super::search_sessions(filters, state.inner().clone()).await
}

#[tauri::command]
pub async fn rename_session(
    session_id: String,
    new_title: String,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    super::rename_session(session_id, new_title, state.inner().clone()).await
}

#[tauri::command]
pub async fn get_session_count(state: State<'_, AppState>) -> CommandResult<u64> {
    super::get_session_count(state.inner().clone()).await
}

#[tauri::command]
pub async fn export_session(
    session_id: String,
    format: String,
    output_path: String,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    super::export_session(session_id, format, output_path, state.inner().clone()).await
}

#[tauri::command]
pub async fn get_index_stats(state: State<'_, AppState>) -> CommandResult<IndexStats> {
    super::get_index_stats(state.inner().clone()).await
}

#[tauri::command]
pub async fn get_pricing_catalog_status(
    state: State<'_, AppState>,
) -> CommandResult<PricingCatalogStatus> {
    super::get_pricing_catalog_status(state.inner().clone()).await
}

#[tauri::command]
pub async fn refresh_pricing_catalog(
    state: State<'_, AppState>,
) -> CommandResult<PricingCatalogStatus> {
    super::refresh_pricing_catalog(state.inner().clone()).await
}

#[tauri::command]
pub async fn start_rebuild_index(state: State<'_, AppState>) -> CommandResult<bool> {
    super::start_rebuild_index(state.inner().clone()).await
}

#[tauri::command]
pub async fn clear_index(state: State<'_, AppState>) -> CommandResult<()> {
    super::clear_index(state.inner().clone()).await
}

#[tauri::command]
pub async fn clear_usage_stats(state: State<'_, AppState>) -> CommandResult<()> {
    super::clear_usage_stats(state.inner().clone()).await
}

#[tauri::command]
pub async fn start_refresh_usage(state: State<'_, AppState>) -> CommandResult<bool> {
    super::start_refresh_usage(state.inner().clone()).await
}

#[tauri::command]
pub async fn get_provider_snapshots(
    state: State<'_, AppState>,
) -> CommandResult<Vec<ProviderSnapshot>> {
    super::get_provider_snapshots(state.inner().clone()).await
}

#[tauri::command]
pub async fn get_resume_command(
    session_id: String,
    state: State<'_, AppState>,
) -> CommandResult<String> {
    super::get_resume_command(session_id, state.inner().clone()).await
}

#[tauri::command]
pub async fn detect_terminal() -> String {
    super::detect_terminal().await
}

#[tauri::command]
pub async fn resume_session(
    session_id: String,
    terminal_app: String,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    super::resume_session(session_id, terminal_app, state.inner().clone()).await
}

#[tauri::command]
pub async fn export_sessions_batch(
    items: Vec<String>,
    format: String,
    output_path: String,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    super::export_sessions_batch(items, format, output_path, state.inner().clone()).await
}

#[tauri::command]
pub async fn toggle_favorite(
    session_id: String,
    state: State<'_, AppState>,
) -> CommandResult<bool> {
    super::toggle_favorite(session_id, state.inner().clone()).await
}

#[tauri::command]
pub async fn list_recent_sessions(
    limit: usize,
    state: State<'_, AppState>,
) -> CommandResult<Vec<SessionMeta>> {
    super::list_recent_sessions(limit, state.inner().clone()).await
}

#[tauri::command]
pub async fn list_favorites(state: State<'_, AppState>) -> CommandResult<Vec<SessionMeta>> {
    super::list_favorites(state.inner().clone()).await
}

#[tauri::command]
pub async fn is_favorite(session_id: String, state: State<'_, AppState>) -> CommandResult<bool> {
    super::is_favorite(session_id, state.inner().clone()).await
}

#[tauri::command]
pub async fn read_image_base64(path: String) -> CommandResult<String> {
    super::read_image_base64(path).await
}

#[tauri::command]
pub async fn read_tool_result_text(path: String) -> CommandResult<String> {
    super::read_tool_result_text(path).await
}

#[tauri::command]
pub async fn resolve_persisted_output(
    path: String,
    state: State<'_, AppState>,
) -> CommandResult<String> {
    super::resolve_persisted_output(path, state.inner().clone()).await
}

#[tauri::command]
pub async fn open_in_folder(path: String) -> CommandResult<()> {
    super::open_in_folder(path).await
}

/// Open external URL in the system browser. GUI-only: in the headless shell
/// the frontend runs in a real browser and opens links itself.
#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> CommandResult<()> {
    app.opener()
        .open_url(&url, None::<String>)
        .context("failed to open URL")?;
    Ok(())
}

#[tauri::command]
pub async fn get_usage_stats(
    providers: Vec<String>,
    range_days: Option<u32>,
    date_start: Option<String>,
    date_end: Option<String>,
    state: State<'_, AppState>,
) -> CommandResult<UsageStats> {
    super::get_usage_stats(
        providers,
        range_days,
        date_start,
        date_end,
        state.inner().clone(),
    )
    .await
}

#[tauri::command]
pub async fn get_activity_calendar(
    providers: Vec<String>,
    date_start: String,
    date_end: String,
    state: State<'_, AppState>,
) -> CommandResult<ActivityCalendar> {
    super::get_activity_calendar(providers, date_start, date_end, state.inner().clone()).await
}

#[tauri::command]
pub async fn get_project_tool_usage(
    project_path: String,
    providers: Vec<String>,
    range_days: Option<u32>,
    date_start: Option<String>,
    date_end: Option<String>,
    state: State<'_, AppState>,
) -> CommandResult<ProjectToolUsageStats> {
    super::get_project_tool_usage(
        project_path,
        providers,
        range_days,
        date_start,
        date_end,
        state.inner().clone(),
    )
    .await
}

#[tauri::command]
pub async fn get_project_daily_usage(
    project_path: String,
    providers: Vec<String>,
    range_days: Option<u32>,
    date_start: Option<String>,
    date_end: Option<String>,
    state: State<'_, AppState>,
) -> CommandResult<Vec<ProjectDailyUsage>> {
    super::get_project_daily_usage(
        project_path,
        providers,
        range_days,
        date_start,
        date_end,
        state.inner().clone(),
    )
    .await
}

#[tauri::command]
pub async fn get_today_cost(state: State<'_, AppState>) -> CommandResult<f64> {
    super::get_today_cost(state.inner().clone()).await
}

#[tauri::command]
pub async fn get_today_tokens(state: State<'_, AppState>) -> CommandResult<TodayTokens> {
    super::get_today_tokens(state.inner().clone()).await
}
