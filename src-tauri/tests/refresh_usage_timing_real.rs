// Test code: clippy's allow-*-in-tests only covers `#[cfg(test)]` modules.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Manual timing smoke test for the forced usage refresh, against a COPY of
//! the local live database and the machine's real provider files. Ignored by
//! default (depends on `~/.<provider>/` data); run explicitly with:
//!
//! ```bash
//! cargo test --release --test refresh_usage_timing_real -- --ignored --nocapture
//! ```
//!
//! Asserts structural invariants only (never hardcoded ids): the refresh must
//! repopulate token stats without ever leaving the copy empty, and must not
//! shrink the session table.

use std::sync::Arc;
use std::time::Instant;

use sessionview_lib::db::Database;
use sessionview_lib::indexer::Indexer;

fn live_data_dir() -> Option<std::path::PathBuf> {
    let dir = dirs::data_local_dir()?.join("sessionview");
    dir.join("sessions.db").exists().then_some(dir)
}

#[test]
#[ignore] // real local data; run manually with --ignored --nocapture
fn forced_usage_refresh_timing_on_db_copy() {
    let Some(live_dir) = live_data_dir() else {
        eprintln!("no live sessionview database found; skipping");
        return;
    };

    let temp = tempfile::TempDir::new().unwrap();
    // VACUUM INTO gives a consistent snapshot even while the app is running.
    {
        let src = rusqlite::Connection::open_with_flags(
            live_dir.join("sessions.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let dest = temp.path().join("sessions.db");
        src.execute("VACUUM INTO ?1", [dest.to_str().unwrap()])
            .unwrap();
    }

    let counts = || -> (u64, u64) {
        let conn = rusqlite::Connection::open_with_flags(
            temp.path().join("sessions.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        (
            conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
                .unwrap(),
            conn.query_row("SELECT COUNT(*) FROM session_token_stats", [], |r| r.get(0))
                .unwrap(),
        )
    };

    let db = Arc::new(Database::open(temp.path()).unwrap());
    let (sessions_before, stats_before) = counts();
    assert!(sessions_before > 0, "copy must contain indexed sessions");

    let indexer = Indexer::new(
        Arc::clone(&db),
        sessionview_lib::provider::all_runtimes(),
        temp.path().to_path_buf(),
    );

    let start = Instant::now();
    let parsed = indexer.refresh_usage().unwrap();
    let elapsed = start.elapsed();

    let (sessions_after, stats_after) = counts();
    drop(db);

    println!(
        "forced usage refresh: {parsed} sessions parsed in {:.1}s \
         (sessions {sessions_before} -> {sessions_after}, stats rows {stats_before} -> {stats_after})",
        elapsed.as_secs_f64()
    );

    assert!(stats_after > 0, "refresh must repopulate token stats");
    assert!(
        sessions_after >= sessions_before,
        "a forced refresh must never lose sessions ({sessions_before} -> {sessions_after})"
    );
}
