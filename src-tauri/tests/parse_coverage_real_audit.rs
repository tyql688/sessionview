// Test code: clippy's allow-*-in-tests only covers `#[cfg(test)]` modules.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Real-data parse-coverage audit.
//!
//! Scans every locally installed provider's real sessions and reports how
//! many records the parsers could not interpret — the number behind the
//! per-session "parse warning" badge. Run manually:
//!
//!   cargo test --test parse_coverage_real_audit -- --ignored --nocapture
//!
//! `#[ignore]` so it never fires in normal `cargo test`. Read-only.
//! It never fails on warning counts (they depend on the machine's data);
//! it prints only provider-level counts and static logger targets. Session
//! ids, paths, titles, record text, and raw warning messages stay private.

#![cfg(test)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use log::{Level, Metadata, Record};
use sessionview_lib::provider::all_runtimes;

static WARN_TARGETS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct CollectingLogger;

impl log::Log for CollectingLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record) {
        if record.level() <= Level::Warn {
            WARN_TARGETS
                .lock()
                .unwrap()
                .push(record.target().to_string());
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
        WARN_TARGETS.lock().unwrap().clear();
        let provider_key = provider.provider().key();
        let parsed = match provider.scan_all() {
            Ok(parsed) => parsed,
            Err(_) => {
                eprintln!("{provider_key}: scan failed or provider is not installed");
                continue;
            }
        };
        if parsed.is_empty() {
            continue;
        }

        let total_warnings: u64 = parsed
            .iter()
            .map(|session| u64::from(session.parse_warning_count))
            .sum();
        let flagged = parsed
            .iter()
            .filter(|session| session.parse_warning_count > 0)
            .count();
        eprintln!(
            "{provider_key}: {} sessions, {flagged} with warnings, {total_warnings} warnings total",
            parsed.len(),
        );

        let mut targets: BTreeMap<String, usize> = BTreeMap::new();
        for target in WARN_TARGETS.lock().unwrap().iter() {
            *targets.entry(target.clone()).or_default() += 1;
        }
        let mut targets: Vec<_> = targets.into_iter().collect();
        targets.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.1));
        for (target, count) in targets.iter().take(10) {
            eprintln!("  {count:>5}x target={target}");
        }
    }
}
