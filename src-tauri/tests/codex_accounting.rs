// Integration tests use synthetic on-disk rollouts and temporary provider homes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use sessionview_lib::indexer::Indexer;
use sessionview_lib::provider::{SessionProvider, SourceState, token_totals_from_usage_events};
use sessionview_lib::providers::codex::CodexProvider;
use sessionview_lib::services::events::NullEventBus;

#[test]
fn codex_accounting_fixture_reconciles_scan_load_stats_and_incremental_reads() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex_accounting/sessions");
    let home = tempfile::TempDir::new().unwrap();
    let sessions_dir = home.path().join(".codex/sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    for entry in fs::read_dir(fixture).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), sessions_dir.join(entry.file_name())).unwrap();
    }
    let provider = CodexProvider::with_home(home.path().to_path_buf());
    let sessions = provider.scan_all().unwrap();
    assert_eq!(sessions.len(), 5);
    // Input is disjoint from cache reads and writes; reasoning is part of output.
    let expected = HashMap::from([
        ("11111111-1111-4111-a111-111111111111", [100, 40, 80, 20]),
        ("22222222-2222-4222-a222-222222222222", [90, 20, 60, 0]),
        ("33333333-3333-4333-a333-333333333333", [20, 4, 10, 0]),
        ("44444444-4444-4444-a444-444444444444", [60, 10, 40, 0]),
        ("55555555-5555-4555-a555-555555555555", [15, 3, 5, 0]),
    ]);
    let mut seen = HashSet::new();
    for session in &sessions {
        assert_eq!(session.parse_warning_count, 0);
        let totals = token_totals_from_usage_events(&session.usage_events);
        let actual = [
            totals.input_tokens,
            totals.output_tokens,
            totals.cache_read_tokens,
            totals.cache_write_tokens,
        ];
        assert_eq!(actual, expected[session.meta.id.as_str()]);
        let loaded = provider
            .load_messages(&session.meta.id, &session.meta.source_path)
            .unwrap();
        assert_eq!(loaded.token_totals, totals);
        let rows = provider.compute_token_stats(session, None, Some(&mut seen));
        let aggregate = rows.iter().fold([0; 4], |mut counts, row| {
            for (total, amount) in counts.iter_mut().zip([
                row.input_tokens,
                row.output_tokens,
                row.cache_read_tokens,
                row.cache_write_tokens,
            ]) {
                *total += amount;
            }
            counts
        });
        assert_eq!(aggregate, actual);
    }
    let known = sessions
        .iter()
        .map(|session| {
            (
                session.meta.source_path.clone(),
                SourceState {
                    size: session.meta.file_size_bytes,
                    mtime: session.source_mtime,
                    title: Some(session.meta.title.clone()),
                },
            )
        })
        .collect();
    let second = provider.scan_incremental(&known).unwrap();
    assert!(second.parsed.is_empty());
    assert_eq!(second.unchanged_source_paths.len(), 5);

    let data = tempfile::TempDir::new().unwrap();
    let mut state = sessionview_lib::build_app_state(data.path(), Arc::new(NullEventBus)).unwrap();
    state.indexer = Indexer::new(
        Arc::clone(&state.db),
        vec![Box::new(CodexProvider::with_home(
            home.path().to_path_buf(),
        ))],
        data.path().to_path_buf(),
    );
    assert_eq!(state.indexer.reindex().unwrap(), 5);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for session in &sessions {
        let detail = runtime
            .block_on(sessionview_lib::commands::get_session_detail(
                session.meta.id.clone(),
                None,
                state.clone(),
            ))
            .unwrap();
        assert_eq!(
            [
                detail.meta.input_tokens,
                detail.meta.output_tokens,
                detail.meta.cache_read_tokens,
                detail.meta.cache_write_tokens
            ],
            expected[session.meta.id.as_str()],
        );
    }
    for _ in 0..2 {
        let usage = runtime
            .block_on(sessionview_lib::commands::get_usage_stats(
                vec!["codex".to_string()],
                None,
                None,
                None,
                Some("Asia/Shanghai".to_string()),
                state.clone(),
            ))
            .unwrap();
        assert_eq!(usage.total_sessions, 5);
        assert_eq!(usage.total_turns, 7);
        assert_eq!(
            [
                usage.total_input_tokens,
                usage.total_output_tokens,
                usage.total_cache_read_tokens,
                usage.total_cache_write_tokens
            ],
            [285, 77, 195, 20],
        );
        assert_eq!(state.indexer.reindex_providers(None, false).unwrap(), 0);
    }
}
