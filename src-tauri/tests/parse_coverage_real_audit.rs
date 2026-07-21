// Test code: clippy's allow-*-in-tests only covers `#[cfg(test)]` modules.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Real-data parse-coverage audit.
//!
//! Scans every locally installed provider's real sessions and reports how
//! many records the parsers could not interpret — the number behind the
//! per-session "parse warning" badge. Run manually:
//!
//!   cargo test --test parse_coverage_audit -- --include-ignored --nocapture
//!
//! `#[ignore]` so it never fires in normal `cargo test`. Read-only.
//! It never fails on warning counts (they depend on the machine's data);
//! it exists to print the coverage table and the distinct unknown-record
//! log lines so ignore-lists can be extended deliberately.

#![cfg(test)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use log::{Level, Metadata, Record};
use sessionview_lib::provider::all_runtimes;

static WARN_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct CollectingLogger;

impl log::Log for CollectingLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record) {
        if record.level() <= Level::Warn {
            WARN_LINES
                .lock()
                .unwrap()
                .push(format!("{}", record.args()));
        }
    }

    fn flush(&self) {}
}

static LOGGER: CollectingLogger = CollectingLogger;

#[test]
#[ignore]
fn audit_parse_warnings_across_all_local_providers() {
    log::set_logger(&LOGGER).ok();
    log::set_max_level(log::LevelFilter::Warn);

    for provider in all_runtimes() {
        WARN_LINES.lock().unwrap().clear();
        let parsed = match provider.scan_all() {
            Ok(parsed) => parsed,
            Err(error) => {
                eprintln!("provider scan failed (likely not installed): {error}");
                continue;
            }
        };
        if parsed.is_empty() {
            continue;
        }

        let label = parsed[0].meta.provider.label();
        let total_warnings: u64 = parsed
            .iter()
            .map(|session| u64::from(session.parse_warning_count))
            .sum();
        let flagged = parsed
            .iter()
            .filter(|session| session.parse_warning_count > 0)
            .count();
        eprintln!(
            "{label}: {} sessions, {flagged} with warnings, {total_warnings} warnings total",
            parsed.len(),
        );

        let mut worst: Vec<_> = parsed
            .iter()
            .filter(|session| session.parse_warning_count > 0)
            .map(|session| (session.parse_warning_count, session.meta.id.clone()))
            .collect();
        worst.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));
        for (count, id) in worst.iter().take(5) {
            eprintln!("  {count:>5}  {id}");
        }

        let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
        for line in WARN_LINES.lock().unwrap().iter() {
            *reasons.entry(line.clone()).or_default() += 1;
        }
        let mut reasons: Vec<_> = reasons.into_iter().collect();
        reasons.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.1));
        for (line, count) in reasons.iter().take(10) {
            eprintln!("  {count:>5}x {line}");
        }
    }
}
