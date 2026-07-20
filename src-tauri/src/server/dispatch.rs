//! Generic invoke dispatcher: maps `POST /api/invoke/{command}` bodies onto
//! the same command cores the Tauri shell wraps. Argument keys mirror the
//! frontend's `BackendCommandMap` (camelCase), so the browser transport can
//! send exactly what it would pass to `invoke()`.
//!
//! `open_external` is intentionally absent: in the headless shell the
//! frontend runs in a real browser and opens links itself.

use serde_json::Value;

use crate::commands::{self, AppState};
use crate::error::CommandError;

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

/// Pull one named argument out of the request body. Absent keys deserialize
/// from JSON null, so `Option<T>` parameters accept omission for free.
fn arg<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T, DispatchError> {
    let value = args.get(key).cloned().unwrap_or(Value::Null);
    serde_json::from_value(value)
        .map_err(|e| DispatchError::BadArgs(format!("invalid argument '{key}': {e}")))
}

fn ok<T: serde::Serialize>(value: T) -> Result<Value, DispatchError> {
    serde_json::to_value(value).map_err(|e| {
        DispatchError::Command(CommandError(anyhow::anyhow!(
            "failed to serialize command result: {e}"
        )))
    })
}

/// Argument keys each command accepts. Unknown commands return `None`.
///
/// This is the strict half of the dispatch contract: request bodies may only
/// carry keys listed here, so a typo (`range_days` for `rangeDays`) fails
/// loudly instead of silently running the unfiltered query. Keep in sync with
/// the `dispatch` match below and the frontend's `BackendCommandMap`.
fn allowed_keys(command: &str) -> Option<&'static [&'static str]> {
    Some(match command {
        "reindex"
        | "get_tree"
        | "get_session_count"
        | "get_index_stats"
        | "get_pricing_catalog_status"
        | "refresh_pricing_catalog"
        | "start_rebuild_index"
        | "clear_index"
        | "start_refresh_usage"
        | "clear_usage_stats"
        | "detect_terminal"
        | "get_provider_snapshots"
        | "list_favorites" => &[],
        "reindex_providers" => &["providers", "aggressive"],
        "get_session_detail" | "get_session_turn_outline" => &["sessionId", "requestSeq"],
        "get_session_meta" | "toggle_favorite" | "is_favorite" | "get_resume_command" => {
            &["sessionId"]
        }
        "get_session_open_window" | "get_session_messages_window" => {
            &["sessionId", "offset", "limit", "requestId", "requestSeq"]
        }
        "cancel_session_load" => &["sessionId", "requestId"],
        "resolve_persisted_output"
        | "read_image_base64"
        | "read_tool_result_text"
        | "open_in_folder" => &["path"],
        "search_sessions" => &["filters"],
        "rename_session" => &["sessionId", "newTitle"],
        "export_session" => &["sessionId", "format", "outputPath"],
        "export_sessions_batch" => &["items", "format", "outputPath"],
        "get_child_sessions" => &["parentId"],
        "get_child_session_counts" => &["parentIds"],
        "resume_session" => &["sessionId", "terminalApp"],
        "list_recent_sessions" => &["limit"],
        "get_usage_stats" => &["providers", "rangeDays", "dateStart", "dateEnd", "timezone"],
        "get_activity_calendar" => &["providers", "dateStart", "dateEnd", "timezone"],
        "get_project_tool_usage" | "get_project_daily_usage" => &[
            "projectPath",
            "providers",
            "rangeDays",
            "dateStart",
            "dateEnd",
            "timezone",
        ],
        "get_today_cost" | "get_today_tokens" => &["timezone"],
        _ => return None,
    })
}

/// Dispatch one invoke request. `raw` is the JSON body (`{}` when the
/// command takes no arguments).
// A flat name → command table mirroring `generate_handler!`. The score is the
// arm count; sub-dispatchers would add indirection, not remove branches.
#[allow(clippy::cognitive_complexity)]
pub async fn dispatch(state: AppState, command: &str, raw: Value) -> Result<Value, DispatchError> {
    let allowed = allowed_keys(command).ok_or(DispatchError::UnknownCommand)?;
    if let Value::Object(map) = &raw
        && let Some(unknown) = map.keys().find(|k| !allowed.contains(&k.as_str()))
    {
        return Err(DispatchError::BadArgs(format!(
            "unknown argument '{unknown}' for command '{command}'"
        )));
    }
    let a = &raw;
    match command {
        "reindex" => ok(commands::reindex(state).await?),
        "reindex_providers" => {
            ok(
                commands::reindex_providers(arg(a, "providers")?, arg(a, "aggressive")?, state)
                    .await?,
            )
        }
        "get_tree" => ok(commands::get_tree(state).await?),
        "get_session_detail" => {
            ok(
                commands::get_session_detail(arg(a, "sessionId")?, arg(a, "requestSeq")?, state)
                    .await?,
            )
        }
        "get_session_meta" => ok(commands::get_session_meta(arg(a, "sessionId")?, state).await?),
        "get_session_open_window" => ok(commands::get_session_open_window(
            arg(a, "sessionId")?,
            arg(a, "offset")?,
            arg(a, "limit")?,
            arg(a, "requestId")?,
            arg(a, "requestSeq")?,
            state,
        )
        .await?),
        "get_session_messages_window" => ok(commands::get_session_messages_window(
            arg(a, "sessionId")?,
            arg(a, "offset")?,
            arg(a, "limit")?,
            arg(a, "requestId")?,
            arg(a, "requestSeq")?,
            state,
        )
        .await?),
        "get_session_turn_outline" => ok(commands::get_session_turn_outline(
            arg(a, "sessionId")?,
            arg(a, "requestSeq")?,
            state,
        )
        .await?),
        "cancel_session_load" => {
            ok(
                commands::cancel_session_load(arg(a, "sessionId")?, arg(a, "requestId")?, state)
                    .await?,
            )
        }
        "resolve_persisted_output" => {
            ok(commands::resolve_persisted_output(arg(a, "path")?, state).await?)
        }
        "search_sessions" => ok(commands::search_sessions(arg(a, "filters")?, state).await?),
        "rename_session" => {
            ok(commands::rename_session(arg(a, "sessionId")?, arg(a, "newTitle")?, state).await?)
        }
        "get_session_count" => ok(commands::get_session_count(state).await?),
        "export_session" => ok(commands::export_session(
            arg(a, "sessionId")?,
            arg(a, "format")?,
            arg(a, "outputPath")?,
            state,
        )
        .await?),
        "export_sessions_batch" => ok(commands::export_sessions_batch(
            arg(a, "items")?,
            arg(a, "format")?,
            arg(a, "outputPath")?,
            state,
        )
        .await?),
        "get_child_sessions" => ok(commands::get_child_sessions(arg(a, "parentId")?, state).await?),
        "get_child_session_counts" => {
            ok(commands::get_child_session_counts(arg(a, "parentIds")?, state).await?)
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
            ok(
                commands::resume_session(arg(a, "sessionId")?, arg(a, "terminalApp")?, state)
                    .await?,
            )
        }
        "get_resume_command" => {
            ok(commands::get_resume_command(arg(a, "sessionId")?, state).await?)
        }
        "list_recent_sessions" => {
            ok(commands::list_recent_sessions(arg(a, "limit")?, state).await?)
        }
        "toggle_favorite" => ok(commands::toggle_favorite(arg(a, "sessionId")?, state).await?),
        "list_favorites" => ok(commands::list_favorites(state).await?),
        "is_favorite" => ok(commands::is_favorite(arg(a, "sessionId")?, state).await?),
        "read_image_base64" => ok(commands::read_image_base64(arg(a, "path")?).await?),
        "read_tool_result_text" => ok(commands::read_tool_result_text(arg(a, "path")?).await?),
        "open_in_folder" => ok(commands::open_in_folder(arg(a, "path")?).await?),
        "get_usage_stats" => ok(commands::get_usage_stats(
            arg(a, "providers")?,
            arg(a, "rangeDays")?,
            arg(a, "dateStart")?,
            arg(a, "dateEnd")?,
            arg(a, "timezone")?,
            state,
        )
        .await?),
        "get_activity_calendar" => ok(commands::get_activity_calendar(
            arg(a, "providers")?,
            arg(a, "dateStart")?,
            arg(a, "dateEnd")?,
            arg(a, "timezone")?,
            state,
        )
        .await?),
        "get_project_tool_usage" => ok(commands::get_project_tool_usage(
            arg(a, "projectPath")?,
            arg(a, "providers")?,
            arg(a, "rangeDays")?,
            arg(a, "dateStart")?,
            arg(a, "dateEnd")?,
            arg(a, "timezone")?,
            state,
        )
        .await?),
        "get_project_daily_usage" => ok(commands::get_project_daily_usage(
            arg(a, "projectPath")?,
            arg(a, "providers")?,
            arg(a, "rangeDays")?,
            arg(a, "dateStart")?,
            arg(a, "dateEnd")?,
            arg(a, "timezone")?,
            state,
        )
        .await?),
        "get_today_cost" => ok(commands::get_today_cost(arg(a, "timezone")?, state).await?),
        "get_today_tokens" => ok(commands::get_today_tokens(arg(a, "timezone")?, state).await?),
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
    async fn dispatch_rejects_unknown_argument_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        // snake_case for a camelCase key must fail loudly, not silently run
        // the unfiltered query.
        let result = dispatch(
            test_state(dir.path()),
            "get_usage_stats",
            serde_json::json!({ "providers": [], "range_days": 7 }),
        )
        .await;
        match result {
            Err(DispatchError::BadArgs(message)) => {
                assert!(message.contains("range_days"), "got: {message}");
            }
            _ => panic!("expected BadArgs"),
        }
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
                assert!(message.contains("sessionId"), "got: {message}");
            }
            _ => panic!("expected BadArgs"),
        }
    }

    #[tokio::test]
    async fn dispatch_treats_missing_optional_args_as_none() {
        let dir = tempfile::TempDir::new().unwrap();
        // requestSeq omitted entirely — must not be a BadArgs error.
        let result = dispatch(
            test_state(dir.path()),
            "get_session_detail",
            serde_json::json!({ "sessionId": "missing-session" }),
        )
        .await;
        assert!(matches!(result, Err(DispatchError::Command(_))));
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
