// Test code: clippy's allow-*-in-tests only covers `#[cfg(test)]` modules.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! One-shot maintenance test: wipe the stale parent_id / is_sidechain values
//! the old `db/sync.rs::find_uuids` heuristic wrote (cross-provider claims,
//! UUID false positives), then rerun the indexer so each provider's current
//! structured signals repopulate the links.
//!
//! Intended to be invoked manually against a real local install:
//!   cargo test --test cleanup_stale_links -- --include-ignored --nocapture
//!
//! `#[ignore]` so it never runs in normal `cargo test`. **Destructive** —
//! it edits the developer's production sessions.db at `data_local_dir`.
//! The assertions are *structural only* (no hardcoded session/project IDs)
//! so the test works on any developer's machine.

#![cfg(test)]

use std::path::PathBuf;
use std::sync::Arc;

use sessionview_lib::db::Database;
use sessionview_lib::indexer::Indexer;
use sessionview_lib::provider;

#[test]
#[ignore]
fn wipe_stale_parent_links_and_reindex() {
    let data_dir = dirs::data_local_dir()
        .expect("data_local_dir")
        .join("sessionview");
    if !data_dir.is_dir() {
        eprintln!("skip: no sessionview data dir at {data_dir:?}");
        return;
    }

    let db_path: PathBuf = data_dir.join("sessions.db");
    if !db_path.is_file() {
        eprintln!("skip: no sessions.db at {db_path:?}");
        return;
    }

    let db = Arc::new(Database::open(&data_dir).expect("open db"));

    let cross_provider_count = |label: &str| -> i64 {
        let n: i64 = db
            .with_transaction(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM sessions s JOIN sessions p ON s.parent_id = p.id \
                     WHERE s.provider != p.provider",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("count");
        eprintln!("[{label}] cross-provider parent links: {n}");
        n
    };

    let dangling_parent_count = |label: &str| -> i64 {
        let n: i64 = db
            .with_transaction(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM sessions s \
                     WHERE s.parent_id IS NOT NULL AND s.parent_id != '' \
                       AND NOT EXISTS (SELECT 1 FROM sessions p WHERE p.id = s.parent_id)",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("count");
        eprintln!("[{label}] dangling parent_id references: {n}");
        n
    };

    cross_provider_count("before");
    dangling_parent_count("before");

    // Wipe ALL parent links + sidechain flags. Each provider's parser will
    // re-derive the correct values from its structured signal during reindex
    // (Claude file path + isSidechain, Codex thread_spawn, Kimi SubagentEvent,
    // OpenCode parent_id column, Antigravity INVOKE_SUBAGENT + send_message).
    db.with_transaction(|conn| {
        conn.execute("UPDATE sessions SET parent_id = NULL, is_sidechain = 0", [])?;
        Ok(())
    })
    .expect("wipe");

    // Reindex via the real production code path so every provider runs
    // scan_all + sync_provider_snapshot. This is exactly what the
    // `start_rebuild_index` Tauri command does.
    let providers = provider::all_runtimes();
    let indexer = Indexer::new(Arc::clone(&db), providers, data_dir.clone());
    let synced = indexer.reindex().expect("reindex");
    eprintln!("[reindex] total sessions synced: {synced}");

    // Post-conditions:
    //  * no cross-provider links (no parser produces those under any signal)
    //  * every non-null parent_id points at an existing session row
    //  * every is_sidechain=1 row has a non-null parent_id (no orphans)
    assert_eq!(
        cross_provider_count("after"),
        0,
        "cross-provider links must be 0 after cleanup"
    );
    assert_eq!(
        dangling_parent_count("after"),
        0,
        "every parent_id must reference an existing session after reindex"
    );

    let orphan_sidechain: i64 = db
        .with_transaction(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sessions \
                 WHERE is_sidechain = 1 AND (parent_id IS NULL OR parent_id = '')",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .expect("count");
    eprintln!("[after] is_sidechain=1 rows with no parent_id: {orphan_sidechain}");
    assert_eq!(
        orphan_sidechain, 0,
        "sidechain sessions must have a parent_id"
    );
}
