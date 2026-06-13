use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use serde::Serialize;
use tauri::State;

use crate::db::Database;
use crate::error::{CommandError, CommandResult};
use crate::models::{Message, Provider, SessionDetail, SessionMeta, TokenTotals};
use crate::services::load_cancel::{self, CancelFlag};
use crate::services::{load_session_meta, SessionLifecycleService, SourceSyncService};

use super::session_tail::try_tail_fast_path;
use super::AppState;

/// Sentinel error returned when a load was cancelled mid-flight. Mapped
/// to a typed string the frontend can ignore (rather than show as an
/// error toast).
const CANCEL_ERROR: &str = "__cc_session_load_canceled__";

/// RAII guard that registers a cancel flag for `session_id` on
/// construction and removes it on drop. If a previous flag is still
/// in flight for the same session, it is tripped so the prior parser
/// bails out at its next checkpoint. The drop pass only removes the
/// entry if it is still ours (a newer load may have replaced it).
struct CancelFlagGuard<'a> {
    state: &'a AppState,
    session_id: String,
    flag: CancelFlag,
}

impl<'a> CancelFlagGuard<'a> {
    fn new(state: &'a AppState, session_id: &str) -> Self {
        let flag = load_cancel::fresh();
        {
            let mut map = match state.load_tokens.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if let Some(prev) = map.insert(session_id.to_string(), Arc::clone(&flag)) {
                load_cancel::cancel(&prev);
            }
        }
        Self {
            state,
            session_id: session_id.to_string(),
            flag,
        }
    }

    fn flag(&self) -> &CancelFlag {
        &self.flag
    }
}

impl Drop for CancelFlagGuard<'_> {
    fn drop(&mut self) {
        let mut map = match self.state.load_tokens.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(existing) = map.get(&self.session_id) {
            if Arc::ptr_eq(existing, &self.flag) {
                map.remove(&self.session_id);
            }
        }
    }
}

/// RAII guard that marks `path` as currently loading on construction
/// and clears it on drop. `sync_sources` consults this set to skip
/// reparses while a viewer load is in flight (watcher-feedback-loop
/// suppression). No-op when `path` is empty.
struct LoadingPathGuard<'a> {
    state: &'a AppState,
    path: Option<PathBuf>,
}

impl<'a> LoadingPathGuard<'a> {
    fn new(state: &'a AppState, source_path: &str) -> Self {
        let path: Option<PathBuf> = (!source_path.is_empty()).then(|| PathBuf::from(source_path));
        if let Some(p) = path.as_ref() {
            let mut set = match state.loading_paths.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            set.insert(p.clone());
        }
        Self { state, path }
    }
}

impl Drop for LoadingPathGuard<'_> {
    fn drop(&mut self) {
        if let Some(p) = self.path.as_ref() {
            let mut set = match self.state.loading_paths.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            set.remove(p);
        }
    }
}

/// Run `work` with both guards installed. Panics in `work` correctly
/// drop the guards via stack unwinding so the AppState maps don't
/// leak entries on a failed parse.
fn with_load_guard<F, R>(state: &AppState, session_id: &str, source_path: &str, work: F) -> R
where
    F: FnOnce(CancelFlag) -> R,
{
    let cancel_guard = CancelFlagGuard::new(state, session_id);
    let _path_guard = LoadingPathGuard::new(state, source_path);
    let flag = cancel_guard.flag().clone();
    load_cancel::run_with(flag.clone(), move || work(flag))
}

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

#[tauri::command]
pub async fn reindex(state: State<'_, AppState>) -> CommandResult<usize> {
    let state = state.inner().clone();
    let count = tokio::task::spawn_blocking(move || state.indexer.reindex())
        .await
        .context("task join error")?
        .map_err(CommandError::from)?;
    Ok(count)
}

#[tauri::command]
pub async fn reindex_providers(
    providers: Vec<String>,
    aggressive: Option<bool>,
    state: State<'_, AppState>,
) -> CommandResult<usize> {
    let state = state.inner().clone();
    let count = tokio::task::spawn_blocking(move || {
        let filter: Vec<crate::models::Provider> = providers
            .iter()
            .filter_map(|s| crate::models::Provider::parse(s))
            .collect();
        if filter.is_empty() {
            return Ok(0);
        }
        state
            .indexer
            .reindex_providers(Some(&filter), aggressive.unwrap_or(false))
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)?;
    Ok(count)
}

#[tauri::command]
pub async fn sync_sources(paths: Vec<String>, state: State<'_, AppState>) -> CommandResult<usize> {
    let state = state.inner().clone();
    let count = tokio::task::spawn_blocking(move || {
        let source_sync = SourceSyncService::new(&state.db);
        let mut unique_paths = std::collections::HashSet::new();
        let mut synced = 0;

        // Snapshot the in-flight set so we don't trample a session being
        // viewed: re-parsing the same JSONL while the user is reading it
        // is the watcher feedback loop we're suppressing.
        let loading: std::collections::HashSet<PathBuf> = match state.loading_paths.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        };

        for path in paths {
            if path.is_empty() || !unique_paths.insert(path.clone()) {
                continue;
            }
            if loading.contains(Path::new(&path)) {
                // Skip — an active viewer load owns this file. Don't
                // invalidate its just-populated cache entry; the mtime
                // check inside SessionCache::get is sufficient to surface
                // any later changes once the viewer load completes.
                log::debug!("sync_sources: skipping loading path '{path}'");
                continue;
            }
            if source_sync.sync_source_path(&path)? {
                synced += 1;
            }
            // Drop the parsed-message cache so the next viewer load
            // re-parses against the (possibly mutated) source. Belt-and-
            // suspenders with the mtime check; explicit eviction frees
            // memory sooner for sessions the user is no longer viewing.
            state.session_cache.invalidate_source(&path);
        }

        Ok::<usize, crate::services::ServiceError>(synced)
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)?;
    Ok(count)
}

#[tauri::command]
pub async fn get_tree(state: State<'_, AppState>) -> CommandResult<Vec<crate::models::TreeNode>> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || state.indexer.build_tree())
        .await
        .context("task join error")?
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_session_detail(
    session_id: String,
    state: State<'_, AppState>,
) -> CommandResult<SessionDetail> {
    let state = state.inner().clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<SessionDetail> {
        let meta = load_session_meta(&state.db, &session_id).map_err(anyhow::Error::msg)?;
        let source_path = meta.source_path.clone();
        with_load_guard(&state, &session_id, &source_path, |_flag| {
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

#[tauri::command]
pub async fn get_session_meta(
    session_id: String,
    state: State<'_, AppState>,
) -> CommandResult<SessionMeta> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<SessionMeta> {
        load_session_meta(&state.db, &session_id).map_err(anyhow::Error::msg)
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_session_open_window(
    session_id: String,
    offset: i64,
    limit: usize,
    state: State<'_, AppState>,
) -> CommandResult<SessionOpenWindow> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<SessionOpenWindow> {
        let mut meta = load_session_meta(&state.db, &session_id).map_err(anyhow::Error::msg)?;
        let source_path = meta.source_path.clone();
        with_load_guard(&state, &session_id, &source_path, |_flag| {
            // Same tail fast path as `get_session_messages_window`, but the
            // token totals are also reflected into the returned metadata so
            // the frontend doesn't need a separate meta request.
            if let Some(window) = try_tail_fast_path(&state, &meta, offset, limit, &session_id)? {
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
        })
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_session_messages_window(
    session_id: String,
    offset: i64,
    limit: usize,
    state: State<'_, AppState>,
) -> CommandResult<SessionMessagesWindow> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<SessionMessagesWindow> {
        let meta = load_session_meta(&state.db, &session_id).map_err(anyhow::Error::msg)?;
        let source_path = meta.source_path.clone();
        with_load_guard(&state, &session_id, &source_path, |_flag| {
            // Fast path: when the frontend asks for a tail-of-file window
            // (negative offset) and the cache hasn't seen this session yet,
            // skip the full-file parse by reading only the trailing bytes.
            // See `session_tail::try_tail_fast_path` for provider eligibility
            // and background-promote setup.
            if let Some(window) = try_tail_fast_path(&state, &meta, offset, limit, &session_id)? {
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
        })
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn cancel_session_load(
    session_id: String,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    let map = match state.load_tokens.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some(flag) = map.get(&session_id) {
        load_cancel::cancel(flag);
    }
    Ok(())
}

#[tauri::command]
pub async fn get_child_sessions(
    parent_id: String,
    state: State<'_, AppState>,
) -> CommandResult<Vec<SessionMeta>> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SessionMeta>> {
        let mut sessions = state
            .db
            .get_child_sessions(&parent_id)
            .context("failed to load child sessions")?;
        hydrate_variant_names(&mut sessions);
        Ok(sessions)
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_child_session_counts(
    parent_ids: Vec<String>,
    state: State<'_, AppState>,
) -> CommandResult<HashMap<String, u64>> {
    let state = state.inner().clone();
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

#[tauri::command]
pub async fn delete_session(session_id: String, state: State<'_, AppState>) -> CommandResult<()> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        SessionLifecycleService::new(&state.db).purge_session(&session_id)
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

#[tauri::command]
pub async fn rename_session(
    session_id: String,
    new_title: String,
    state: State<'_, AppState>,
) -> CommandResult<()> {
    let state = state.inner().clone();
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

#[tauri::command]
pub async fn get_session_count(state: State<'_, AppState>) -> CommandResult<u64> {
    let state = state.inner().clone();
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

#[tauri::command]
pub async fn toggle_favorite(
    session_id: String,
    state: State<'_, AppState>,
) -> CommandResult<bool> {
    let state = state.inner().clone();
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

#[tauri::command]
pub async fn list_recent_sessions(
    limit: usize,
    state: State<'_, AppState>,
) -> CommandResult<Vec<SessionMeta>> {
    let state = state.inner().clone();
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

#[tauri::command]
pub async fn list_favorites(state: State<'_, AppState>) -> CommandResult<Vec<SessionMeta>> {
    let state = state.inner().clone();
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

#[tauri::command]
pub async fn is_favorite(session_id: String, state: State<'_, AppState>) -> CommandResult<bool> {
    let state = state.inner().clone();
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

fn session_window_bounds(total: usize, offset: i64, limit: usize) -> (usize, usize) {
    // Negative offset = window from the end. -N selects the newest N
    // messages; -1 + limit=200 means "last 200". Positive offset is a
    // direct index from the start. Both forms clamp to [0, total].
    let start = if offset < 0 {
        let from_end = offset.unsigned_abs() as usize;
        total.saturating_sub(from_end.max(limit))
    } else {
        (offset as usize).min(total)
    };
    let end = start.saturating_add(limit).min(total);
    (start, end)
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
        .require_runtime()
        .map_err(anyhow::Error::msg)?
        .load_messages(session_id, source_path)
        .map_err(anyhow::Error::msg)
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
mod tests {
    use super::{
        build_session_messages_window, canceled_error, session_window_bounds, CANCEL_ERROR,
    };
    use crate::error::CommandError;
    use crate::models::{Message, TokenTotals};

    /// The cancel sentinel must reach the command boundary unchanged so
    /// the frontend's `isLoadCanceledError` (`msg.includes(...)`) keeps
    /// suppressing the toast on tab-switch races. This locks the exact
    /// serialized text the frontend matches against.
    #[test]
    fn canceled_error_serializes_with_cancel_sentinel() {
        let command: CommandError = canceled_error().into();
        let serialized = format!("{:#}", command.0);
        assert_eq!(serialized, CANCEL_ERROR);
        assert!(serialized.contains("__cc_session_load_canceled__"));
    }

    #[test]
    fn session_window_bounds_negative_offset_uses_tail_window() {
        assert_eq!(session_window_bounds(1_000, -300, 300), (700, 1_000));
        assert_eq!(session_window_bounds(1_000, -1, 200), (800, 1_000));
    }

    #[test]
    fn session_window_bounds_clamps_to_total() {
        assert_eq!(session_window_bounds(20, 10, 100), (10, 20));
        assert_eq!(session_window_bounds(20, 30, 100), (20, 20));
        assert_eq!(session_window_bounds(0, -300, 300), (0, 0));
    }

    #[test]
    fn build_session_messages_window_preserves_full_total() {
        let messages: Vec<Message> = (0..5)
            .map(|idx| Message::assistant(format!("message {idx}")))
            .collect();

        let window = build_session_messages_window(&messages, 2, TokenTotals::default(), -2, 2);

        assert_eq!(window.total, 5);
        assert_eq!(window.start, 3);
        assert_eq!(window.messages.len(), 2);
        assert_eq!(window.messages[0].content, "message 3");
        assert_eq!(window.parse_warning_count, 2);
    }
}
