use anyhow::Context;

use crate::error::CommandResult;
use crate::models::{SearchFilters, SearchResult};

use super::AppState;

pub async fn search_sessions(
    filters: SearchFilters,
    state: AppState,
) -> CommandResult<Vec<SearchResult>> {
    tokio::task::spawn_blocking(move || {
        state
            .db
            .search_filtered(&filters)
            .context("failed to search")
    })
    .await
    .context("task join error")?
    .map_err(crate::error::CommandError::from)
}
