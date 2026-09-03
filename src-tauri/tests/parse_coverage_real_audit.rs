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
//! it prints only provider-level counts and static logger call sites. Session
//! ids, paths, titles, record text, and raw warning messages stay private.
//! The Codex pass additionally asserts that every non-zero top-level
//! `token_usage_record` in the scanned file prefix became a keyed usage event.

#![cfg(test)]

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::sync::Mutex;

use log::{Level, Metadata, Record};
use serde_json::Value;
use sessionview_lib::models::Provider;
use sessionview_lib::provider::{ParsedSession, all_runtimes};

static WARN_SITES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct CollectingLogger;

impl log::Log for CollectingLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record) {
        if record.level() <= Level::Warn {
            let site = match (record.file(), record.line()) {
                (Some(file), Some(line)) => format!("{}@{file}:{line}", record.target()),
                _ => record.target().to_string(),
            };
            WARN_SITES.lock().unwrap().push(site);
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
        WARN_SITES.lock().unwrap().clear();
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
        if provider.provider() == Provider::Codex {
            audit_codex_token_usage_materialization(&parsed);
        }

        let mut targets: BTreeMap<String, usize> = BTreeMap::new();
        for site in WARN_SITES.lock().unwrap().iter() {
            *targets.entry(site.clone()).or_default() += 1;
        }
        let mut targets: Vec<_> = targets.into_iter().collect();
        targets.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.1));
        for (site, count) in targets.iter().take(10) {
            eprintln!("  {count:>5}x site={site}");
        }
    }
}

fn audit_codex_token_usage_materialization(sessions: &[ParsedSession]) {
    let mut records = 0usize;
    let mut materialized = 0usize;
    let mut zero_usage = 0usize;
    let mut unreadable_sources = 0usize;
    for session in sessions {
        let hashes: HashSet<&str> = session
            .usage_events
            .iter()
            .filter_map(|event| event.usage_hash.as_deref())
            .collect();
        let mut content = String::new();
        let read_result = File::open(&session.meta.source_path).and_then(|file| {
            file.take(session.meta.file_size_bytes)
                .read_to_string(&mut content)
        });
        match read_result {
            Ok(_) => {}
            Err(_) => {
                unreadable_sources += 1;
                continue;
            }
        }
        for line in content.lines() {
            let Ok(row) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if row.get("type").and_then(Value::as_str) != Some("token_usage_record") {
                continue;
            }
            let Some(payload) = row.get("payload") else {
                continue;
            };
            let Some(response_id) = payload.get("response_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(usage) = payload.get("usage") else {
                continue;
            };
            records += 1;
            let has_usage = [
                "input_tokens",
                "cached_input_tokens",
                "cache_write_input_tokens",
                "output_tokens",
                "reasoning_output_tokens",
                "total_tokens",
            ]
            .into_iter()
            .any(|field| {
                usage
                    .get(field)
                    .and_then(Value::as_u64)
                    .is_some_and(|n| n > 0)
            });
            if !has_usage {
                zero_usage += 1;
                continue;
            }
            let expected_hash = format!("codex-response:{response_id}");
            if hashes.contains(expected_hash.as_str()) {
                materialized += 1;
            }
        }
    }
    eprintln!(
        "  token_usage_record: {records} parsed, {materialized} materialized, {zero_usage} zero-usage"
    );
    assert_eq!(
        unreadable_sources, 0,
        "Codex audit source became unreadable"
    );
    assert_eq!(
        materialized + zero_usage,
        records,
        "some Codex token_usage_record values were not materialized"
    );
}
