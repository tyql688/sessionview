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

/// Generate a `#[tauri::command]` wrapper per core function whose last
/// parameter is the owned `AppState`. Commands without state (or with
/// GUI-only dependencies) are written out below the macro invocation.
///
/// Argument identifiers pass through the macro verbatim, so Tauri's
/// camelCase invoke-key derivation sees exactly what a handwritten wrapper
/// would declare; `generate_handler!` in lib.rs compile-checks every entry.
macro_rules! gui_commands {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {$(
        #[tauri::command]
        pub async fn $name($($arg: $ty,)* state: State<'_, AppState>) -> CommandResult<$ret> {
            super::$name($($arg,)* state.inner().clone()).await
        }
    )*};
}

gui_commands! {
    reindex() -> usize;
    reindex_providers(providers: Vec<String>, aggressive: Option<bool>) -> usize;
    get_tree() -> Vec<TreeNode>;
    get_session_detail(session_id: String, request_seq: Option<u64>) -> SessionDetail;
    get_session_meta(session_id: String) -> SessionMeta;
    get_session_open_window(session_id: String, offset: i64, limit: usize, request_id: Option<String>, request_seq: Option<u64>) -> SessionOpenWindow;
    get_session_messages_window(session_id: String, offset: i64, limit: usize, request_id: Option<String>, request_seq: Option<u64>) -> SessionMessagesWindow;
    get_session_turn_outline(session_id: String, request_seq: Option<u64>) -> SessionTurnOutline;
    cancel_session_load(session_id: String, request_id: Option<String>) -> ();
    get_child_sessions(parent_id: String) -> Vec<SessionMeta>;
    get_child_session_counts(parent_ids: Vec<String>) -> HashMap<String, u64>;
    search_sessions(filters: SearchFilters) -> Vec<SearchResult>;
    rename_session(session_id: String, new_title: String) -> ();
    get_session_count() -> u64;
    export_session(session_id: String, format: String, output_path: String) -> ();
    export_sessions_batch(items: Vec<String>, format: String, output_path: String) -> ();
    get_index_stats() -> IndexStats;
    get_pricing_catalog_status() -> PricingCatalogStatus;
    refresh_pricing_catalog() -> PricingCatalogStatus;
    start_rebuild_index() -> bool;
    clear_index() -> ();
    clear_usage_stats() -> ();
    start_refresh_usage() -> bool;
    get_provider_snapshots() -> Vec<ProviderSnapshot>;
    get_resume_command(session_id: String) -> String;
    resume_session(session_id: String, terminal_app: String) -> ();
    resolve_persisted_output(path: String) -> String;
    toggle_favorite(session_id: String) -> bool;
    list_recent_sessions(limit: usize) -> Vec<SessionMeta>;
    list_favorites() -> Vec<SessionMeta>;
    is_favorite(session_id: String) -> bool;
    get_usage_stats(providers: Vec<String>, range_days: Option<u32>, date_start: Option<String>, date_end: Option<String>, timezone: Option<String>) -> UsageStats;
    get_activity_calendar(providers: Vec<String>, date_start: String, date_end: String, timezone: Option<String>) -> ActivityCalendar;
    get_project_tool_usage(project_path: String, providers: Vec<String>, range_days: Option<u32>, date_start: Option<String>, date_end: Option<String>, timezone: Option<String>) -> ProjectToolUsageStats;
    get_project_daily_usage(project_path: String, providers: Vec<String>, range_days: Option<u32>, date_start: Option<String>, date_end: Option<String>, timezone: Option<String>) -> Vec<ProjectDailyUsage>;
    get_today_cost(timezone: Option<String>) -> f64;
    get_today_tokens(timezone: Option<String>) -> TodayTokens;
}

#[tauri::command]
pub async fn detect_terminal() -> String {
    super::detect_terminal().await
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
