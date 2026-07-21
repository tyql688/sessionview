//! call_id → message-index pairing shared by the per-provider parsers.
//!
//! Every file-format parser that emits tool calls as standalone
//! `MessageRole::Tool` messages later has to merge the tool *result*
//! (a separate line/event) back into that message. The shape is always
//! the same: remember the message index when the call is pushed, then
//! look the index back up by `call_id` when the result arrives.

use std::collections::HashMap;

use crate::models::Message;

/// Pairs tool-call messages with their later tool results by `call_id`.
///
/// `register` when the tool-call message is pushed; `message_mut` (or
/// `index_of`) when its result arrives. Lookups are bounds-checked
/// against the message list, so parsers that truncate messages (turn
/// rollback) can also call `retain_below` to drop stale registrations.
#[derive(Default)]
pub(crate) struct ToolCallPairer {
    by_call_id: HashMap<String, usize>,
}

impl ToolCallPairer {
    /// Remember that the tool call identified by `call_id` lives at
    /// message index `idx`. A `None` call_id is a no-op — the result
    /// can never be paired, callers keep their own fallback.
    pub(crate) fn register(&mut self, call_id: Option<&str>, idx: usize) {
        if let Some(cid) = call_id {
            self.by_call_id.insert(cid.to_string(), idx);
        }
    }

    /// Message index registered for `call_id`, if any.
    pub(crate) fn index_of(&self, call_id: Option<&str>) -> Option<usize> {
        call_id.and_then(|cid| self.by_call_id.get(cid)).copied()
    }

    /// The registered tool-call message for `call_id`, bounds-checked
    /// against `messages`.
    pub(crate) fn message_mut<'a>(
        &self,
        call_id: Option<&str>,
        messages: &'a mut [Message],
    ) -> Option<&'a mut Message> {
        messages.get_mut(self.index_of(call_id)?)
    }

    /// Drop registrations pointing at or beyond `len` — used when the
    /// caller truncates its message list (e.g. turn-cancel rollback).
    pub(crate) fn retain_below(&mut self, len: usize) {
        self.by_call_id.retain(|_, idx| *idx < len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MessageRole;

    #[test]
    fn register_then_index_of_returns_registered_index() {
        let mut pairer = ToolCallPairer::default();
        pairer.register(Some("call_1"), 3);
        assert_eq!(pairer.index_of(Some("call_1")), Some(3));
        assert_eq!(pairer.index_of(Some("call_2")), None);
        assert_eq!(pairer.index_of(None), None);
    }

    #[test]
    fn register_without_call_id_is_noop() {
        let mut pairer = ToolCallPairer::default();
        pairer.register(None, 0);
        assert_eq!(pairer.index_of(None), None);
    }

    #[test]
    fn message_mut_bounds_checks_against_messages() {
        let mut pairer = ToolCallPairer::default();
        let mut messages = vec![Message::new(MessageRole::Tool, String::new())];
        pairer.register(Some("ok"), 0);
        pairer.register(Some("stale"), 5);
        assert!(pairer.message_mut(Some("ok"), &mut messages).is_some());
        assert!(pairer.message_mut(Some("stale"), &mut messages).is_none());
        assert!(pairer.message_mut(Some("unknown"), &mut messages).is_none());
    }

    #[test]
    fn retain_below_drops_rolled_back_registrations() {
        let mut pairer = ToolCallPairer::default();
        pairer.register(Some("kept"), 1);
        pairer.register(Some("dropped"), 2);
        pairer.retain_below(2);
        assert_eq!(pairer.index_of(Some("kept")), Some(1));
        assert_eq!(pairer.index_of(Some("dropped")), None);
    }
}
