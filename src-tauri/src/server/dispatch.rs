//! Generic invoke dispatcher: maps `POST /api/invoke/{command}` bodies onto
//! the same command cores the Tauri shell wraps. Argument shapes mirror the
//! frontend's `BackendCommandMap` (camelCase), so the browser transport can
//! send exactly what it would pass to `invoke()`.
//!
//! `open_external` is intentionally absent: in the headless shell the
//! frontend runs in a real browser and opens links itself.

use serde::Deserialize;
use serde_json::Value;

use crate::commands::{self, AppState};
use crate::error::CommandError;
use crate::models::SearchFilters;

pub enum DispatchError {
    UnknownCommand,
    BadArgs(String),
    Command(CommandError),
}

impl From<CommandError> for DispatchError {
    fn from(e: CommandError) -> Self {
        Self::Command(e)
    }
}

fn args<T: serde::de::DeserializeOwned>(raw: Value) -> Result<T, DispatchError> {
    serde_json::from_value(raw)
        .map_err(|e| DispatchError::BadArgs(format!("invalid arguments: {e}")))
}

fn ok<T: serde::Serialize>(value: T) -> Result<Value, DispatchError> {
    serde_json::to_value(value).map_err(|e| {
        DispatchError::Command(CommandError(anyhow::anyhow!(
            "failed to serialize command result: {e}"
        )))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionIdArgs {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReindexProvidersArgs {
    providers: Vec<String>,
    aggressive: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionLoadArgs {
    session_id: String,
    request_seq: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionWindowArgs {
    session_id: String,
    offset: i64,
    limit: usize,
    request_id: Option<String>,
    request_seq: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelLoadArgs {
    session_id: String,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchArgs {
    filters: SearchFilters,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameArgs {
    session_id: String,
    new_title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportSessionArgs {
    session_id: String,
    format: String,
    output_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportBatchArgs {
    items: Vec<String>,
    format: String,
    output_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParentIdArgs {
    parent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParentIdsArgs {
    parent_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeArgs {
    session_id: String,
    terminal_app: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitArgs {
    limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageStatsArgs {
    providers: Vec<String>,
    range_days: Option<u32>,
    date_start: Option<String>,
    date_end: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityCalendarArgs {
    providers: Vec<String>,
    date_start: String,
    date_end: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectUsageArgs {
    project_path: String,
    providers: Vec<String>,
    range_days: Option<u32>,
    date_start: Option<String>,
    date_end: Option<String>,
}

/// Dispatch one invoke request. `raw_args` is the JSON body (`{}` when the
/// command takes no arguments).
pub async fn dispatch(
    state: AppState,
    command: &str,
    raw_args: Value,
) -> Result<Value, DispatchError> {
    match command {
        "reindex" => ok(commands::reindex(state).await?),
        "reindex_providers" => {
            let a: ReindexProvidersArgs = args(raw_args)?;
            ok(commands::reindex_providers(a.providers, a.aggressive, state).await?)
        }
        "get_tree" => ok(commands::get_tree(state).await?),
        "get_session_detail" => {
            let a: SessionLoadArgs = args(raw_args)?;
            ok(commands::get_session_detail(a.session_id, a.request_seq, state).await?)
        }
        "get_session_meta" => {
            let a: SessionIdArgs = args(raw_args)?;
            ok(commands::get_session_meta(a.session_id, state).await?)
        }
        "get_session_open_window" => {
            let a: SessionWindowArgs = args(raw_args)?;
            ok(commands::get_session_open_window(
                a.session_id,
                a.offset,
                a.limit,
                a.request_id,
                a.request_seq,
                state,
            )
            .await?)
        }
        "get_session_messages_window" => {
            let a: SessionWindowArgs = args(raw_args)?;
            ok(commands::get_session_messages_window(
                a.session_id,
                a.offset,
                a.limit,
                a.request_id,
                a.request_seq,
                state,
            )
            .await?)
        }
        "get_session_turn_outline" => {
            let a: SessionLoadArgs = args(raw_args)?;
            ok(commands::get_session_turn_outline(a.session_id, a.request_seq, state).await?)
        }
        "cancel_session_load" => {
            let a: CancelLoadArgs = args(raw_args)?;
            ok(commands::cancel_session_load(a.session_id, a.request_id, state).await?)
        }
        "resolve_persisted_output" => {
            let a: PathArgs = args(raw_args)?;
            ok(commands::resolve_persisted_output(a.path, state).await?)
        }
        "search_sessions" => {
            let a: SearchArgs = args(raw_args)?;
            ok(commands::search_sessions(a.filters, state).await?)
        }
        "rename_session" => {
            let a: RenameArgs = args(raw_args)?;
            ok(commands::rename_session(a.session_id, a.new_title, state).await?)
        }
        "get_session_count" => ok(commands::get_session_count(state).await?),
        "export_session" => {
            let a: ExportSessionArgs = args(raw_args)?;
            ok(commands::export_session(a.session_id, a.format, a.output_path, state).await?)
        }
        "export_sessions_batch" => {
            let a: ExportBatchArgs = args(raw_args)?;
            ok(commands::export_sessions_batch(a.items, a.format, a.output_path, state).await?)
        }
        "get_child_sessions" => {
            let a: ParentIdArgs = args(raw_args)?;
            ok(commands::get_child_sessions(a.parent_id, state).await?)
        }
        "get_child_session_counts" => {
            let a: ParentIdsArgs = args(raw_args)?;
            ok(commands::get_child_session_counts(a.parent_ids, state).await?)
        }
        "get_index_stats" => ok(commands::get_index_stats(state).await?),
        "get_pricing_catalog_status" => ok(commands::get_pricing_catalog_status(state).await?),
        "refresh_pricing_catalog" => ok(commands::refresh_pricing_catalog(state).await?),
        "start_rebuild_index" => ok(commands::start_rebuild_index(state).await?),
        "clear_index" => ok(commands::clear_index(state).await?),
        "start_refresh_usage" => ok(commands::start_refresh_usage(state).await?),
        "clear_usage_stats" => ok(commands::clear_usage_stats(state).await?),
        "detect_terminal" => ok(commands::detect_terminal().await),
        "get_provider_snapshots" => ok(commands::get_provider_snapshots(state).await?),
        "resume_session" => {
            let a: ResumeArgs = args(raw_args)?;
            ok(commands::resume_session(a.session_id, a.terminal_app, state).await?)
        }
        "get_resume_command" => {
            let a: SessionIdArgs = args(raw_args)?;
            ok(commands::get_resume_command(a.session_id, state).await?)
        }
        "list_recent_sessions" => {
            let a: LimitArgs = args(raw_args)?;
            ok(commands::list_recent_sessions(a.limit, state).await?)
        }
        "toggle_favorite" => {
            let a: SessionIdArgs = args(raw_args)?;
            ok(commands::toggle_favorite(a.session_id, state).await?)
        }
        "list_favorites" => ok(commands::list_favorites(state).await?),
        "is_favorite" => {
            let a: SessionIdArgs = args(raw_args)?;
            ok(commands::is_favorite(a.session_id, state).await?)
        }
        "read_image_base64" => {
            let a: PathArgs = args(raw_args)?;
            ok(commands::read_image_base64(a.path).await?)
        }
        "read_tool_result_text" => {
            let a: PathArgs = args(raw_args)?;
            ok(commands::read_tool_result_text(a.path).await?)
        }
        "open_in_folder" => {
            let a: PathArgs = args(raw_args)?;
            ok(commands::open_in_folder(a.path).await?)
        }
        "get_usage_stats" => {
            let a: UsageStatsArgs = args(raw_args)?;
            ok(commands::get_usage_stats(
                a.providers,
                a.range_days,
                a.date_start,
                a.date_end,
                state,
            )
            .await?)
        }
        "get_activity_calendar" => {
            let a: ActivityCalendarArgs = args(raw_args)?;
            ok(
                commands::get_activity_calendar(a.providers, a.date_start, a.date_end, state)
                    .await?,
            )
        }
        "get_project_tool_usage" => {
            let a: ProjectUsageArgs = args(raw_args)?;
            ok(commands::get_project_tool_usage(
                a.project_path,
                a.providers,
                a.range_days,
                a.date_start,
                a.date_end,
                state,
            )
            .await?)
        }
        "get_project_daily_usage" => {
            let a: ProjectUsageArgs = args(raw_args)?;
            ok(commands::get_project_daily_usage(
                a.project_path,
                a.providers,
                a.range_days,
                a.date_start,
                a.date_end,
                state,
            )
            .await?)
        }
        "get_today_cost" => ok(commands::get_today_cost(state).await?),
        "get_today_tokens" => ok(commands::get_today_tokens(state).await?),
        _ => Err(DispatchError::UnknownCommand),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::services::NullEventBus;

    fn test_state(dir: &std::path::Path) -> AppState {
        crate::build_app_state(dir, Arc::new(NullEventBus)).unwrap()
    }

    #[tokio::test]
    async fn dispatch_rejects_unknown_command() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = dispatch(test_state(dir.path()), "open_external", Value::Null).await;
        assert!(matches!(result, Err(DispatchError::UnknownCommand)));
    }

    #[tokio::test]
    async fn dispatch_rejects_malformed_args() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = dispatch(
            test_state(dir.path()),
            "get_session_meta",
            serde_json::json!({ "sessionId": 42 }),
        )
        .await;
        match result {
            Err(DispatchError::BadArgs(message)) => {
                assert!(message.contains("invalid arguments"), "got: {message}");
            }
            _ => panic!("expected BadArgs"),
        }
    }

    #[tokio::test]
    async fn dispatch_runs_no_arg_commands_against_empty_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = dispatch(
            test_state(dir.path()),
            "get_session_count",
            Value::Object(serde_json::Map::new()),
        )
        .await;
        match result {
            Ok(value) => assert_eq!(value, serde_json::json!(0)),
            Err(_) => panic!("expected Ok(0)"),
        }
    }

    #[tokio::test]
    async fn dispatch_surfaces_command_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = dispatch(
            test_state(dir.path()),
            "get_session_meta",
            serde_json::json!({ "sessionId": "missing-session" }),
        )
        .await;
        match result {
            Err(DispatchError::Command(e)) => {
                assert!(format!("{:#}", e.0).contains("session not found"));
            }
            _ => panic!("expected Command error"),
        }
    }
}
