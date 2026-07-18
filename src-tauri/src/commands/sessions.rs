use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use serde::Serialize;

use crate::db::Database;
use crate::error::{CommandError, CommandResult};
use crate::models::{Message, Provider, SessionDetail, SessionMeta, TokenTotals};
use crate::services::load_cancel;
use crate::services::load_session_meta;
use crate::services::session_view::{
    build_session_turn_outline, session_window_bounds, subagent_meta_title, with_load_guard,
    LoadRequest, SessionTurnOutline,
};

use super::session_tail::try_tail_fast_path;
use super::AppState;

/// Sentinel error returned when a load was cancelled mid-flight. Mapped
/// to a typed string the frontend can ignore (rather than show as an
/// error toast).
const CANCEL_ERROR: &str = "__sessionview_load_canceled__";

pub(super) fn canceled_error() -> anyhow::Error {
    anyhow!(CANCEL_ERROR)
}

/// Window of messages from a cached parsed session. `total` reflects the
/// full message count so the frontend can compute scroll metrics without
/// loading every message.
#[derive(Serialize, Clone)]
pub struct SessionMessagesWindow {
    pub total: usize,
    pub start: usize,
    pub messages: Vec<Message>,
    pub parse_warning_count: u32,
    pub token_totals: TokenTotals,
}

/// Initial session-open payload: metadata plus the newest message window.
/// This lets the frontend open a session with one IPC / one meta lookup while
/// keeping the paged window endpoint available for older-message loads.
#[derive(Serialize, Clone)]
pub struct SessionOpenWindow {
    pub meta: SessionMeta,
    pub window: SessionMessagesWindow,
}

pub async fn reindex(state: AppState) -> CommandResult<usize> {
    use std::sync::atomic::Ordering;

    if state.maintenance_running.swap(true, Ordering::SeqCst) {
        return Err(CommandError::from(anyhow!(
            "maintenance task already running"
        )));
    }

    let worker_state = state.clone();
    let result = match tokio::task::spawn_blocking(move || worker_state.indexer.reindex()).await {
        Ok(result) => result.map_err(CommandError::from),
        Err(error) => Err(CommandError::from(anyhow!("task join error: {error:#}"))),
    };
    state.maintenance_running.store(false, Ordering::SeqCst);
    result
}

pub async fn reindex_providers(
    providers: Vec<String>,
    aggressive: Option<bool>,
    state: AppState,
) -> CommandResult<usize> {
    use std::sync::atomic::Ordering;

    if state.maintenance_running.swap(true, Ordering::SeqCst) {
        return Err(CommandError::from(anyhow!(
            "maintenance task already running"
        )));
    }

    let worker_state = state.clone();
    let result = match tokio::task::spawn_blocking(move || {
        let filter: Vec<crate::models::Provider> = providers
            .iter()
            .filter_map(|s| crate::models::Provider::parse(s))
            .collect();
        if filter.is_empty() {
            return Ok(0);
        }
        worker_state
            .indexer
            .reindex_providers(Some(&filter), aggressive.unwrap_or(false))
    })
    .await
    {
        Ok(result) => result.map_err(CommandError::from),
        Err(error) => Err(CommandError::from(anyhow!("task join error: {error:#}"))),
    };
    state.maintenance_running.store(false, Ordering::SeqCst);
    result
}

pub async fn get_tree(state: AppState) -> CommandResult<Vec<crate::models::TreeNode>> {
    tokio::task::spawn_blocking(move || state.indexer.build_tree())
        .await
        .context("task join error")?
        .map_err(CommandError::from)
}

pub async fn get_session_detail(
    session_id: String,
    request_seq: Option<u64>,
    state: AppState,
) -> CommandResult<SessionDetail> {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<SessionDetail> {
        let meta = load_session_meta(&state.db, &session_id).map_err(anyhow::Error::msg)?;
        let source_path = meta.source_path.clone();
        let request = LoadRequest {
            id: None,
            seq: request_seq,
        };
        with_load_guard(&state, &session_id, &source_path, request, |_flag| {
            let (messages, parse_warning_count, token_totals) =
                load_messages_cached(&state, &meta)?;
            if load_cancel::is_canceled() {
                return Err(canceled_error());
            }
            let mut meta = meta;
            let token_totals =
                indexed_or_loaded_token_totals(&state.db, &session_id, token_totals)?;
            apply_token_totals(&mut meta, token_totals);
            Ok(SessionDetail {
                meta,
                messages: (*messages).clone(),
                parse_warning_count,
            })
        })
    })
    .await;
    result
        .context("task join error")?
        .map_err(CommandError::from)
}

pub async fn get_session_meta(session_id: String, state: AppState) -> CommandResult<SessionMeta> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<SessionMeta> {
        load_session_meta(&state.db, &session_id).map_err(anyhow::Error::msg)
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn get_session_open_window(
    session_id: String,
    offset: i64,
    limit: usize,
    request_id: Option<String>,
    request_seq: Option<u64>,
    state: AppState,
) -> CommandResult<SessionOpenWindow> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<SessionOpenWindow> {
        let mut meta = load_session_meta(&state.db, &session_id).map_err(anyhow::Error::msg)?;
        let source_path = meta.source_path.clone();
        with_load_guard(
            &state,
            &session_id,
            &source_path,
            LoadRequest {
                id: request_id.as_deref(),
                seq: request_seq,
            },
            |_flag| {
                // Same tail fast path as `get_session_messages_window`, but the
                // token totals are also reflected into the returned metadata so
                // the frontend doesn't need a separate meta request.
                if let Some(window) = try_tail_fast_path(&state, &meta, offset, limit, &session_id)?
                {
                    apply_token_totals(&mut meta, window.token_totals);
                    return Ok(SessionOpenWindow { meta, window });
                }

                let (messages, parse_warning_count, token_totals) =
                    load_messages_cached(&state, &meta)?;
                if load_cancel::is_canceled() {
                    return Err(canceled_error());
                }
                let token_totals =
                    indexed_or_loaded_token_totals(&state.db, &session_id, token_totals)?;
                apply_token_totals(&mut meta, token_totals);
                let window = build_session_messages_window(
                    messages.as_ref(),
                    parse_warning_count,
                    token_totals,
                    offset,
                    limit,
                );
                Ok(SessionOpenWindow { meta, window })
            },
        )
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn get_session_messages_window(
    session_id: String,
    offset: i64,
    limit: usize,
    request_id: Option<String>,
    request_seq: Option<u64>,
    state: AppState,
) -> CommandResult<SessionMessagesWindow> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<SessionMessagesWindow> {
        let meta = load_session_meta(&state.db, &session_id).map_err(anyhow::Error::msg)?;
        let source_path = meta.source_path.clone();
        with_load_guard(
            &state,
            &session_id,
            &source_path,
            LoadRequest {
                id: request_id.as_deref(),
                seq: request_seq,
            },
            |_flag| {
                // Fast path: when the frontend asks for a tail-of-file window
                // (negative offset) and the cache hasn't seen this session yet,
                // skip the full-file parse by reading only the trailing bytes.
                // See `session_tail::try_tail_fast_path` for provider eligibility
                // and background-promote setup.
                if let Some(window) = try_tail_fast_path(&state, &meta, offset, limit, &session_id)?
                {
                    return Ok(window);
                }

                let (messages, parse_warning_count, token_totals) =
                    load_messages_cached(&state, &meta)?;
                if load_cancel::is_canceled() {
                    return Err(canceled_error());
                }
                let token_totals =
                    indexed_or_loaded_token_totals(&state.db, &session_id, token_totals)?;
                Ok(build_session_messages_window(
                    messages.as_ref(),
                    parse_warning_count,
                    token_totals,
                    offset,
                    limit,
                ))
            },
        )
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn get_session_turn_outline(
    session_id: String,
    request_seq: Option<u64>,
    state: AppState,
) -> CommandResult<SessionTurnOutline> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<SessionTurnOutline> {
        let meta = load_session_meta(&state.db, &session_id).map_err(anyhow::Error::msg)?;
        let source_path = meta.source_path.clone();
        // Guard under a dedicated key: window fetches (scroll paging, search
        // jumps, minimap reveals) cancel by plain session id, and on huge
        // sessions the multi-second outline parse would lose that race every
        // time — the minimap simply never appeared. Opening a different
        // session doesn't need to cancel this either; a stale result is
        // discarded by the frontend version check.
        let outline_guard_key = format!("{session_id}#outline");
        let request = LoadRequest {
            id: None,
            seq: request_seq,
        };
        with_load_guard(&state, &outline_guard_key, &source_path, request, |_flag| {
            let (messages, _, _) = load_messages_cached(&state, &meta)?;
            if load_cancel::is_canceled() {
                return Err(canceled_error());
            }

            Ok(build_session_turn_outline(messages.as_ref()))
        })
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn cancel_session_load(
    session_id: String,
    request_id: Option<String>,
    state: AppState,
) -> CommandResult<()> {
    let map = match state.load_tokens.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some(token) = map.get(&session_id) {
        let matches_request = match request_id.as_deref() {
            Some(id) => token.request_id.as_deref() == Some(id),
            None => token.request_id.is_none(),
        };
        if matches_request {
            load_cancel::cancel(&token.flag);
        }
    }
    Ok(())
}

pub async fn get_child_sessions(
    parent_id: String,
    state: AppState,
) -> CommandResult<Vec<SessionMeta>> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SessionMeta>> {
        let mut sessions = state
            .db
            .get_child_sessions(&parent_id)
            .context("failed to load child sessions")?;
        hydrate_variant_names(&mut sessions);
        for session in &mut sessions {
            if session.provider != Provider::Claude
                || !session.is_sidechain
                || session.title != "Untitled"
            {
                continue;
            }
            if let Some(title) = subagent_meta_title(&session.source_path) {
                session.title = title;
            }
        }
        Ok(sessions)
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn get_child_session_counts(
    parent_ids: Vec<String>,
    state: AppState,
) -> CommandResult<HashMap<String, u64>> {
    tokio::task::spawn_blocking(move || {
        state
            .db
            .child_session_counts(&parent_ids)
            .context("failed to load child session counts")
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn rename_session(
    session_id: String,
    new_title: String,
    state: AppState,
) -> CommandResult<()> {
    tokio::task::spawn_blocking(move || {
        state
            .db
            .rename_session(&session_id, &new_title)
            .context("failed to rename session")
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn get_session_count(state: AppState) -> CommandResult<u64> {
    let count = tokio::task::spawn_blocking(move || {
        state
            .db
            .session_count()
            .context("failed to get session count")
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)?;
    Ok(count)
}

pub async fn toggle_favorite(session_id: String, state: AppState) -> CommandResult<bool> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
        let is_fav = state
            .db
            .is_favorite(&session_id)
            .context("failed to check favorite")?;

        if is_fav {
            state
                .db
                .remove_favorite(&session_id)
                .context("failed to remove favorite")?;
            Ok(false)
        } else {
            state
                .db
                .add_favorite(&session_id)
                .context("failed to add favorite")?;
            Ok(true)
        }
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn list_recent_sessions(
    limit: usize,
    state: AppState,
) -> CommandResult<Vec<SessionMeta>> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SessionMeta>> {
        let mut sessions = state
            .db
            .list_recent_sessions(limit)
            .context("failed to list recent sessions")?;
        hydrate_variant_names(&mut sessions);
        Ok(sessions)
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn list_favorites(state: AppState) -> CommandResult<Vec<SessionMeta>> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SessionMeta>> {
        let mut sessions = state
            .db
            .list_favorites()
            .context("failed to list favorites")?;
        hydrate_variant_names(&mut sessions);
        Ok(sessions)
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn is_favorite(session_id: String, state: AppState) -> CommandResult<bool> {
    tokio::task::spawn_blocking(move || {
        state
            .db
            .is_favorite(&session_id)
            .context("failed to check favorite")
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub(crate) fn load_detail(session_id: &str, db: &Database) -> anyhow::Result<SessionDetail> {
    let meta = load_session_meta(db, session_id).map_err(anyhow::Error::msg)?;
    let loaded = load_messages_from_provider(&meta.provider, session_id, &meta.source_path)?;
    Ok(SessionDetail {
        meta,
        messages: loaded.messages,
        parse_warning_count: loaded.parse_warning_count,
    })
}

/// Load messages either from the in-memory cache or by re-parsing the
/// source file. Returns an `Arc` so cache hits and full-detail clones
/// share the parsed data without an extra copy.
///
/// Honors the thread-local cancel flag installed by `with_load_guard`:
/// the parser may bail out mid-line-loop and return an empty/partial
/// result, which we surface here as `canceled_error()` so callers can
/// distinguish "user navigated away" from a real parse failure.
pub(crate) fn load_messages_cached(
    state: &AppState,
    meta: &SessionMeta,
) -> anyhow::Result<(Arc<Vec<Message>>, u32, TokenTotals)> {
    if load_cancel::is_canceled() {
        return Err(canceled_error());
    }

    if meta.source_path.is_empty() {
        let loaded =
            load_messages_from_provider_or_canceled(&meta.provider, &meta.id, &meta.source_path)?;
        return Ok((
            Arc::new(loaded.messages),
            loaded.parse_warning_count,
            loaded.token_totals,
        ));
    }

    let mtime = std::fs::metadata(&meta.source_path)
        .ok()
        .and_then(|m| m.modified().ok());

    let cache_key = session_cache_key(meta);
    if let Some(hit) = state.session_cache.get(&cache_key, mtime) {
        return Ok((hit.messages, hit.parse_warning_count, hit.token_totals));
    }

    let loaded =
        load_messages_from_provider_or_canceled(&meta.provider, &meta.id, &meta.source_path)?;
    let total_messages = loaded.messages.len();
    let cached = state.session_cache.insert(
        cache_key,
        meta.source_path.clone(),
        loaded.messages,
        loaded.parse_warning_count,
        loaded.token_totals,
        mtime,
        false,
        Some(total_messages),
    );
    Ok((
        cached.messages,
        cached.parse_warning_count,
        cached.token_totals,
    ))
}

pub(super) fn session_cache_key(meta: &SessionMeta) -> String {
    format!("{}:{}:{}", meta.provider.key(), meta.id, meta.source_path)
}

fn apply_token_totals(meta: &mut SessionMeta, totals: TokenTotals) {
    meta.input_tokens = totals.input_tokens;
    meta.output_tokens = totals.output_tokens;
    meta.cache_read_tokens = totals.cache_read_tokens;
    meta.cache_write_tokens = totals.cache_write_tokens;
}

fn build_session_messages_window(
    messages: &[Message],
    parse_warning_count: u32,
    token_totals: TokenTotals,
    offset: i64,
    limit: usize,
) -> SessionMessagesWindow {
    let total = messages.len();
    let (start, end) = session_window_bounds(total, offset, limit);
    SessionMessagesWindow {
        total,
        start,
        messages: messages[start..end].to_vec(),
        parse_warning_count,
        token_totals,
    }
}

pub(super) fn indexed_or_loaded_token_totals(
    db: &Database,
    session_id: &str,
    loaded_totals: TokenTotals,
) -> anyhow::Result<TokenTotals> {
    db.get_session_token_totals(session_id)
        .with_context(|| format!("failed to load token totals for session {session_id}"))
        .map(|totals| totals.unwrap_or(loaded_totals))
}

fn hydrate_variant_names(sessions: &mut [SessionMeta]) {
    crate::providers::cc_mirror::hydrate_variant_names(sessions);
}

pub(super) fn load_messages_from_provider(
    provider: &Provider,
    session_id: &str,
    source_path: &str,
) -> anyhow::Result<crate::provider::LoadedSession> {
    provider
        .require_runtime()?
        .load_messages(session_id, source_path)
        .context("failed to load messages")
}

/// Like `load_messages_from_provider`, but if the parser bailed out due
/// to the thread-local cancel flag (returning `None`/parse error), we
/// surface `canceled_error()` instead of the parse error so the
/// frontend sentinel check (`isLoadCanceledError`) suppresses the
/// "failed to parse" toast on tab-switch races.
fn load_messages_from_provider_or_canceled(
    provider: &Provider,
    session_id: &str,
    source_path: &str,
) -> anyhow::Result<crate::provider::LoadedSession> {
    if load_cancel::is_canceled() {
        return Err(canceled_error());
    }
    match load_messages_from_provider(provider, session_id, source_path) {
        Ok(loaded) => {
            if load_cancel::is_canceled() {
                Err(canceled_error())
            } else {
                Ok(loaded)
            }
        }
        Err(e) => {
            if load_cancel::is_canceled() {
                Err(canceled_error())
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests;
