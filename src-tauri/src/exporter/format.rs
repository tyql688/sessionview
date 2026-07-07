//! Shared formatting helpers for the markdown exporter.

use crate::models::Message;

/// Format an epoch-seconds timestamp as local `YYYY-MM-DD HH:MM`,
/// returning `fallback` when the epoch is out of range.
pub(super) fn format_epoch(epoch: i64, fallback: &str) -> String {
    chrono::DateTime::from_timestamp(epoch, 0).map_or_else(
        || fallback.to_string(),
        |d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}

/// Compact token count: `1.2M` / `3.4k` / `512`.
pub(super) fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Sum `(input, output, cache_read, cache_write)` token usage across messages.
pub(super) fn aggregate_token_usage(messages: &[Message]) -> (u64, u64, u64, u64) {
    messages
        .iter()
        .fold((0u64, 0u64, 0u64, 0u64), |(inp, out, cr, cw), msg| {
            if let Some(u) = &msg.token_usage {
                (
                    inp + u.input_tokens as u64,
                    out + u.output_tokens as u64,
                    cr + u.cache_read_input_tokens as u64,
                    cw + u.cache_creation_input_tokens as u64,
                )
            } else {
                (inp, out, cr, cw)
            }
        })
}
