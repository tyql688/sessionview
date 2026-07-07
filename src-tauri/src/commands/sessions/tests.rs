use super::{build_session_messages_window, canceled_error, CANCEL_ERROR};
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
