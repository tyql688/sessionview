use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::models::{Message, Provider, SessionMeta, TokenTotals};

use super::sessions::{
    canceled_error, indexed_or_loaded_token_totals, load_messages_from_provider, session_cache_key,
    SessionMessagesWindow,
};
use super::AppState;

/// Try to satisfy a window request by mmap-reading just the trailing
/// portion of the source file instead of parsing it whole.
///
/// Returns `Ok(Some(_))` when the fast path applied; the caller can
/// return the window directly. Returns `Ok(None)` when the caller
/// should fall through to the normal cached parse — either the request
/// doesn't fit the fast-path criteria (positive offset, unsupported
/// provider, in-memory cache already has the full session) or the tail
/// parse came up empty.
///
/// When the fast path applies, this also kicks off a background full
/// parse via `spawn_blocking` so the next "load older" request hits a
/// fully populated cache without paying the parse cost again.
pub(super) fn try_tail_fast_path(
    state: &AppState,
    meta: &SessionMeta,
    offset: i64,
    limit: usize,
    session_id: &str,
) -> Result<Option<SessionMessagesWindow>> {
    if offset >= 0 {
        return Ok(None);
    }
    // Resolve the provider's tail parser up front. `None` means the
    // provider has no line-tail entry point (OpenCode is SQLite-backed;
    // Cursor isn't wired) — bail before paying for the stat below.
    // CC-Mirror reuses the Claude parser.
    let Some(parse_tail) = tail_parser_for(&meta.provider) else {
        return Ok(None);
    };
    if meta.source_path.is_empty() {
        return Ok(None);
    }

    // Cache hit means the full file has already been parsed — let the
    // existing slow path serve from `Arc<Vec<Message>>` rather than
    // re-running the tail mmap. Subagent `parent_id` / `project_path`
    // derivation lives in the full parser only; the caller already has
    // the correct values on `meta` (loaded from DB), so the fast path
    // doesn't need to redrive them here.
    let mtime = std::fs::metadata(&meta.source_path)
        .ok()
        .and_then(|m| m.modified().ok());
    let cache_key = session_cache_key(meta);
    if state.session_cache.get(&cache_key, mtime).is_some() {
        return Ok(None);
    }

    let target_messages = limit.max(offset.unsigned_abs() as usize).max(1);
    let path = PathBuf::from(&meta.source_path);
    // Run the provider's tail parser (resolved above). It returns the
    // common shape — messages + parse warnings — projected from each
    // provider's distinct result type by the adapter in `tail_parser_for`.
    let (tail_messages, parse_warning_count) = match parse_tail(&path, target_messages) {
        Some(tail) => tail,
        None => return Ok(None),
    };
    if crate::services::load_cancel::is_canceled() {
        return Err(canceled_error());
    }

    // Trust the DB's stored message count when we have it — it's what
    // the indexer wrote after the last full parse. Falls back to the
    // tail-length so the window slicing math still works for sessions
    // that haven't been indexed yet.
    let stored_total = meta.message_count as usize;
    let tail_len = tail_messages.len();
    let total = stored_total.max(tail_len);

    let from_end = offset.unsigned_abs() as usize;
    let want = from_end.max(limit);
    let visible_in_tail = want.min(tail_len);
    let slice_start = tail_len.saturating_sub(visible_in_tail);
    let slice = tail_messages[slice_start..tail_len].to_vec();
    let window_start = total.saturating_sub(visible_in_tail);

    // Token totals from the indexer are authoritative for the visible
    // window; the tail parse doesn't see the historical token-usage
    // entries that may have arrived earlier in the file.
    let token_totals =
        indexed_or_loaded_token_totals(&state.db, session_id, TokenTotals::default())?;

    schedule_full_parse_promote(state.clone(), meta.clone(), cache_key, mtime);

    Ok(Some(SessionMessagesWindow {
        total,
        start: window_start,
        messages: slice,
        parse_warning_count,
        token_totals,
    }))
}

/// A provider's tail parser projected onto the shape `try_tail_fast_path`
/// needs: given the source path and a target window size, return the
/// trailing messages plus the per-record parse-warning count, or `None`
/// when the tail came up empty / unreadable (caller falls back to the
/// full parse).
type TailParseFn = fn(&Path, usize) -> Option<(Vec<Message>, u32)>;

/// Map a provider to its tail-parser adapter, or `None` for providers
/// that don't expose a line-tail entry point (OpenCode is SQLite-backed;
/// Cursor is JSONL but has no tail parser wired yet). CC-Mirror reuses
/// the Claude parser. The match is exhaustive so adding a `Provider`
/// variant forces a decision here.
fn tail_parser_for(provider: &Provider) -> Option<TailParseFn> {
    match provider {
        Provider::Claude | Provider::CcMirror => Some(claude_tail),
        Provider::Codex => Some(codex_tail),
        Provider::Antigravity => Some(antigravity_tail),
        Provider::Kimi => Some(kimi_tail),
        Provider::OpenCode | Provider::Cursor | Provider::Pi => None,
    }
}

fn claude_tail(path: &Path, target_messages: usize) -> Option<(Vec<Message>, u32)> {
    crate::providers::claude::parser::parse_session_tail(path, target_messages)
        .map(|tail| (tail.messages, tail.parse_warning_count))
}

fn codex_tail(path: &Path, target_messages: usize) -> Option<(Vec<Message>, u32)> {
    crate::providers::codex::parser::parse_session_tail(path, target_messages)
        .map(|tail| (tail.messages, tail.parse_warning_count))
}

fn antigravity_tail(path: &Path, target_messages: usize) -> Option<(Vec<Message>, u32)> {
    crate::providers::antigravity::parser::parse_session_tail(path, target_messages)
        .map(|tail| (tail.messages, tail.parse_warning_count))
}

fn kimi_tail(path: &Path, target_messages: usize) -> Option<(Vec<Message>, u32)> {
    crate::providers::kimi::parser::parse_session_tail(path, target_messages)
        .map(|tail| (tail.messages, tail.parse_warning_count))
}

/// Fire-and-forget background full parse that overwrites the in-memory
/// cache with the complete `Vec<Message>` once it lands. The next
/// `load_messages_cached` call hits the promoted entry and the fast
/// path is no longer needed for this session.
///
/// Skipped when another promote for the same cache key is already in
/// flight (e.g. the user clicked the same session twice in rapid
/// succession) — `AppState.promote_in_flight` is the gating set.
///
/// Failures are logged at warn level. The user already has a usable
/// tail window in hand, so a stale cache is the worst outcome.
fn schedule_full_parse_promote(
    state: AppState,
    meta: SessionMeta,
    cache_key: String,
    mtime: Option<std::time::SystemTime>,
) {
    // Try to claim the promote slot before paying for `spawn_blocking`.
    // A racing fast-path that already spawned a promote for this cache
    // key wins; we silently no-op.
    {
        let mut guard = match state.promote_in_flight.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !guard.insert(cache_key.clone()) {
            return;
        }
    }

    tokio::task::spawn_blocking(move || {
        let result = load_messages_from_provider(&meta.provider, &meta.id, &meta.source_path);
        match result {
            Ok(loaded) => {
                let total_messages = loaded.messages.len();
                state.session_cache.insert(
                    cache_key.clone(),
                    meta.source_path.clone(),
                    loaded.messages,
                    loaded.parse_warning_count,
                    loaded.token_totals,
                    mtime,
                    false,
                    Some(total_messages),
                );
            }
            Err(error) => {
                log::warn!(
                    "background full parse failed for session {}: {error:#}",
                    meta.id
                );
            }
        }
        // Release the in-flight slot last so a subsequent fast-path that
        // wants to re-promote (after a file change, for instance) can
        // proceed cleanly.
        if let Ok(mut guard) = state.promote_in_flight.lock() {
            guard.remove(&cache_key);
        }
    });
}
