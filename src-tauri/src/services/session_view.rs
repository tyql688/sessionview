//! Pure session-view business logic shared by the session commands:
//! load guards, message-window bounds, turn outlines, and subagent
//! meta titles. Command handlers in `commands/sessions.rs` stay thin
//! and delegate here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::commands::AppState;
use crate::models::Message;
use crate::services::load_cancel::{self, CancelFlag};

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
pub(crate) fn with_load_guard<F, R>(
    state: &AppState,
    session_id: &str,
    source_path: &str,
    work: F,
) -> R
where
    F: FnOnce(CancelFlag) -> R,
{
    let cancel_guard = CancelFlagGuard::new(state, session_id);
    let _path_guard = LoadingPathGuard::new(state, source_path);
    let flag = cancel_guard.flag().clone();
    load_cancel::run_with(flag.clone(), move || work(flag))
}

#[derive(Serialize, Clone)]
pub struct SessionTurnOutlineEntry {
    pub ordinal: usize,
    pub message_index: usize,
    pub user_text: String,
    pub reply_text: String,
}

const OUTLINE_PREVIEW_CHARS: usize = 240;
const OUTLINE_PREVIEW_SCAN_CHARS: usize = OUTLINE_PREVIEW_CHARS * 4;

fn outline_preview(content: &str) -> String {
    content
        .chars()
        .take(OUTLINE_PREVIEW_SCAN_CHARS)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(OUTLINE_PREVIEW_CHARS)
        .collect()
}

pub(crate) fn build_session_turn_outline(messages: &[Message]) -> Vec<SessionTurnOutlineEntry> {
    let mut outline: Vec<SessionTurnOutlineEntry> = Vec::new();
    let mut ordinal = 0;
    for (message_index, message) in messages.iter().enumerate() {
        match message.role {
            crate::models::MessageRole::User => {
                let user_text = outline_preview(&message.content);
                if user_text.is_empty() {
                    continue;
                }
                outline.push(SessionTurnOutlineEntry {
                    ordinal,
                    message_index,
                    user_text,
                    reply_text: String::new(),
                });
                ordinal += 1;
            }
            crate::models::MessageRole::Assistant => {
                let Some(last) = outline.last_mut() else {
                    continue;
                };
                if !last.reply_text.is_empty() {
                    continue;
                }
                let reply_text = outline_preview(&message.content);
                if !reply_text.is_empty() {
                    last.reply_text = reply_text;
                }
            }
            crate::models::MessageRole::Tool | crate::models::MessageRole::System => {}
        }
    }
    outline
}

pub(crate) fn session_window_bounds(total: usize, offset: i64, limit: usize) -> (usize, usize) {
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

pub(crate) fn subagent_meta_title(source_path: &str) -> Option<String> {
    let meta_path = Path::new(source_path).with_extension("meta.json");
    if !meta_path.exists() {
        return None;
    }
    let content = match std::fs::read_to_string(&meta_path) {
        Ok(content) => content,
        Err(error) => {
            log::warn!(
                "failed to read Claude subagent meta '{}': {error}",
                meta_path.display()
            );
            return None;
        }
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(json) => json,
        Err(error) => {
            log::warn!(
                "failed to parse Claude subagent meta '{}': {error}",
                meta_path.display()
            );
            return None;
        }
    };
    json.get("description")
        .or_else(|| json.get("agentType"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests;
