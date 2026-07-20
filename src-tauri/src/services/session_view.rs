//! Pure session-view business logic shared by the session commands:
//! load guards, message-window bounds, turn outlines, and subagent
//! meta titles. Command handlers in `commands/sessions.rs` stay thin
//! and delegate here.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::commands::{AppState, LoadToken};
use crate::models::Message;
use crate::services::load_cancel::{self, CancelFlag};

/// Identity of one frontend load request: the cancel-matching id plus a
/// client-issued monotonic sequence that orders concurrent loads for the
/// same session key (see `LoadToken::seq`).
#[derive(Clone, Copy, Default)]
pub(crate) struct LoadRequest<'a> {
    pub id: Option<&'a str>,
    pub seq: Option<u64>,
}

/// RAII guard that registers a cancel flag for `session_id` on
/// construction and removes it on drop.
///
/// Supersession is decided by the client sequence, never by registration
/// order: task scheduling can start an older request's blocking task after
/// a newer one's, and "insert cancels previous" would then kill the load
/// the user is actually looking at. So:
/// - incoming seq newer (or ordering unknown) → replace the entry and trip
///   the previous flag;
/// - incoming seq older than the registered one → the incoming request is
///   stale: its own flag starts tripped and the newer token stays in place.
///
/// The drop pass only removes the entry if it is still ours (a newer load
/// may have replaced it; a stale guard never owned it).
struct CancelFlagGuard<'a> {
    tokens: &'a Mutex<HashMap<String, LoadToken>>,
    session_id: String,
    flag: CancelFlag,
}

impl<'a> CancelFlagGuard<'a> {
    fn new(
        tokens: &'a Mutex<HashMap<String, LoadToken>>,
        session_id: &str,
        request: LoadRequest<'_>,
    ) -> Self {
        let flag = load_cancel::fresh();
        {
            let mut map = match tokens.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let registered_newer = matches!(
                (map.get(session_id).and_then(|t| t.seq), request.seq),
                (Some(registered), Some(incoming)) if registered > incoming
            );
            if registered_newer {
                load_cancel::cancel(&flag);
            } else if let Some(prev) = map.insert(
                session_id.to_string(),
                LoadToken {
                    request_id: request.id.map(str::to_string),
                    seq: request.seq,
                    flag: Arc::clone(&flag),
                },
            ) {
                load_cancel::cancel(&prev.flag);
            }
        }
        Self {
            tokens,
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
        let mut map = match self.tokens.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(existing) = map.get(&self.session_id)
            && Arc::ptr_eq(&existing.flag, &self.flag)
        {
            map.remove(&self.session_id);
        }
    }
}

/// Run `work` with a cancel guard installed. Panics in `work` correctly
/// drop the guard via stack unwinding so the AppState maps don't leak
/// entries on a failed parse.
pub(crate) fn with_load_guard<F, R>(
    state: &AppState,
    session_id: &str,
    _source_path: &str,
    request: LoadRequest<'_>,
    work: F,
) -> R
where
    F: FnOnce(CancelFlag) -> R,
{
    let cancel_guard = CancelFlagGuard::new(&state.load_tokens, session_id, request);
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

/// Whole-session renderable-message counts per role, for the filter toolbar.
/// The frontend's windowed loading only ever sees a slice, so its own counts
/// grow as pages land — these are the authoritative session-wide numbers,
/// computed on the same full parse the outline already pays for.
#[derive(Serialize, Clone, Default, PartialEq, Eq, Debug)]
pub struct SessionRoleCounts {
    pub user: usize,
    pub assistant: usize,
    pub tool: usize,
    pub system: usize,
}

/// Turn outline plus session-wide role counts — one full-parse pass feeds both.
#[derive(Serialize, Clone)]
pub struct SessionTurnOutline {
    pub turns: Vec<SessionTurnOutlineEntry>,
    pub role_counts: SessionRoleCounts,
}

/// Mirror of the frontend's `isRenderableMessage` (session/hooks.ts): the
/// counts must describe the rows the role filter actually controls, so both
/// sides skip the same non-renderable messages.
fn is_renderable_message(message: &Message) -> bool {
    if matches!(message.role, crate::models::MessageRole::Tool) {
        let orphan_tool_result = message
            .tool_name
            .as_deref()
            .is_some_and(|name| name.starts_with("toolu_"))
            && message.tool_metadata.is_none();
        if orphan_tool_result {
            return false;
        }
        // Truthiness, not presence: the TS side treats "" as absent.
        return !message.content.is_empty()
            || message
                .tool_input
                .as_deref()
                .is_some_and(|input| !input.is_empty())
            || message
                .tool_name
                .as_deref()
                .is_some_and(|name| !name.is_empty());
    }
    !message.content.trim().is_empty()
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

pub(crate) fn build_session_turn_outline(messages: &[Message]) -> SessionTurnOutline {
    let mut outline: Vec<SessionTurnOutlineEntry> = Vec::new();
    let mut role_counts = SessionRoleCounts::default();
    let mut ordinal = 0;
    for (message_index, message) in messages.iter().enumerate() {
        if is_renderable_message(message) {
            match message.role {
                crate::models::MessageRole::User => role_counts.user += 1,
                crate::models::MessageRole::Assistant => role_counts.assistant += 1,
                crate::models::MessageRole::Tool => role_counts.tool += 1,
                crate::models::MessageRole::System => role_counts.system += 1,
            }
        }
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
    SessionTurnOutline {
        turns: outline,
        role_counts,
    }
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
