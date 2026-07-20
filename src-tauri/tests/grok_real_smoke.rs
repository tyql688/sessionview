// Test code: clippy's allow-*-in-tests only covers `#[cfg(test)]` modules.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Real-data smoke test for the Grok provider.
//!
//! Run manually against a logged-in `~/.grok/` install:
//!
//!   cargo test --test grok_real_smoke -- --include-ignored --nocapture
//!
//! `#[ignore]` so it never fires in normal `cargo test`. Read-only —
//! only scans the on-disk session files. Assertions are structural so
//! the test works on any developer's machine.

#![cfg(test)]

use sessionview_lib::models::{MessageRole, Provider};
use sessionview_lib::provider::SessionProvider;
use sessionview_lib::providers::grok::GrokProvider;

#[test]
#[ignore]
fn scan_real_grok_directory() {
    let provider = match GrokProvider::new() {
        Some(p) => p,
        None => {
            eprintln!("skip: no HOME dir");
            return;
        }
    };

    let grok_dir = dirs::home_dir().unwrap().join(".grok");
    if !grok_dir.is_dir() {
        eprintln!("skip: no ~/.grok");
        return;
    }

    let parsed = provider.scan_all().expect("scan_all");
    eprintln!("Parsed {} grok sessions", parsed.len());

    for s in &parsed {
        eprintln!(
            "  id={:?} msgs={} usage_events={} warnings={} title={:?} model={:?}",
            s.meta.id,
            s.meta.message_count,
            s.usage_events.len(),
            s.parse_warning_count,
            s.meta.title,
            s.meta.model,
        );

        assert_eq!(s.meta.provider, Provider::Grok);
        assert!(
            !s.meta.id.is_empty(),
            "session id must be populated for {:?}",
            s.meta.source_path
        );
        assert!(
            s.meta.created_at > 0 && s.meta.updated_at >= s.meta.created_at,
            "timestamps invariant violated for {:?}",
            s.meta.id,
        );
        assert_eq!(
            s.meta.message_count,
            s.messages.len() as u32,
            "message_count must match messages.len() for {:?}",
            s.meta.id,
        );
        assert!(
            s.meta.source_path.contains("/.grok/sessions/"),
            "source_path should live under ~/.grok/sessions: {:?}",
            s.meta.source_path,
        );
        assert!(
            s.messages.iter().any(|m| m.role == MessageRole::User),
            "session {:?} should contain at least one real user prompt",
            s.meta.id,
        );
        // Every tool message with a result-bearing content should have
        // metadata built by the shared registry.
        for m in &s.messages {
            if m.role == MessageRole::Tool {
                assert!(
                    m.tool_metadata.is_some(),
                    "tool message without metadata in {:?}",
                    s.meta.id
                );
            }
        }
        // Subagent children must link back to a parsed parent.
        if s.meta.is_sidechain {
            let parent_id = s
                .meta
                .parent_id
                .as_deref()
                .expect("sidechain session must resolve parent_id");
            assert!(
                parsed.iter().any(|p| p.meta.id == parent_id),
                "parent {parent_id} of {:?} must be a parsed session",
                s.meta.id,
            );
        }
        // Usage events must carry a model and parseable timestamps.
        for event in &s.usage_events {
            assert!(!event.model.is_empty());
            assert!(
                chrono::DateTime::parse_from_rfc3339(&event.timestamp).is_ok(),
                "usage event timestamp must be RFC3339: {:?}",
                event.timestamp
            );
        }
    }
}
