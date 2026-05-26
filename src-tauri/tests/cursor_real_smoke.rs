//! Real-data smoke test for the Cursor CLI provider.
//!
//! Run manually against a local `~/.cursor/` install:
//!
//!   cargo test --test cursor_real_smoke -- --include-ignored --nocapture
//!
//! `#[ignore]` so it never fires in normal `cargo test`. Read-only.
//! Assertions are structural so the test works on any developer
//! machine (no hardcoded session ids or project paths).

#![cfg(test)]

use cc_session_lib::models::{MessageRole, Provider};
use cc_session_lib::provider::SessionProvider;
use cc_session_lib::providers::cursor::CursorProvider;

#[test]
#[ignore]
fn scan_real_cursor_directory() {
    let provider = match CursorProvider::new() {
        Some(p) => p,
        None => {
            eprintln!("skip: no HOME dir");
            return;
        }
    };

    let cursor_home = dirs::home_dir().unwrap().join(".cursor");
    if !cursor_home.is_dir() {
        eprintln!("skip: no ~/.cursor");
        return;
    }

    let parsed = provider.scan_all().expect("scan_all");
    eprintln!("Parsed {} cursor sessions", parsed.len());

    let mut saw_user = false;
    let mut saw_tool = false;
    for s in &parsed {
        eprintln!(
            "  id={:?} side={} msgs={} model={:?} title={:?} project={:?}",
            s.meta.id,
            s.meta.is_sidechain,
            s.meta.message_count,
            s.meta.model,
            s.meta.title,
            s.meta.project_name,
        );

        assert_eq!(s.meta.provider, Provider::Cursor);
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
        assert!(
            s.meta.message_count == s.messages.len() as u32,
            "message_count mismatch for {:?}",
            s.meta.id,
        );
        assert!(
            s.meta.source_path.contains("/.cursor/projects/")
                && s.meta.source_path.contains("/agent-transcripts/"),
            "source_path should live under cursor projects: {}",
            s.meta.source_path,
        );
        if s.meta.is_sidechain {
            assert!(
                s.meta.parent_id.is_some(),
                "subagent must carry parent_id ({:?})",
                s.meta.id
            );
        }

        for m in &s.messages {
            match m.role {
                MessageRole::User => saw_user = true,
                MessageRole::Tool => saw_tool = true,
                _ => {}
            }
        }

        // Preview a couple of messages so visual sanity check is easy.
        for m in s.messages.iter().take(3) {
            let preview: String = m.content.chars().take(80).collect();
            eprintln!("    [{:?}] tool={:?} {:?}", m.role, m.tool_name, preview);
        }
    }

    if !parsed.is_empty() {
        eprintln!("Observed user msgs: {saw_user}, tool msgs: {saw_tool}");
    }
}
