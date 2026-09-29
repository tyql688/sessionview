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
//! It checks all four token components too, and reconciles complete modern
//! root sessions against the sum of unique response records. Mixed legacy
//! sessions are reported separately because their older calls have no ids.
//! Completed tool items in root sessions must likewise become tool messages.
//! Sidechains are excluded from that check because their forked parent prefix
//! intentionally does not belong to the child's visible transcript.

#![cfg(test)]

use std::collections::{BTreeMap, HashMap, HashSet};
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
    assert_eq!(
        sessions
            .iter()
            .map(|session| &session.meta.id)
            .collect::<HashSet<_>>()
            .len(),
        sessions.len(),
        "physical Codex history segments must not overwrite the same session id"
    );
    let mut records = 0usize;
    let mut materialized = 0usize;
    let mut zero_usage = 0usize;
    let mut unreadable_sources = 0usize;
    let mut completed_tools = 0usize;
    let mut reconciled_sessions = 0usize;
    let mut reconciled_children = 0usize;
    let mut missing_tools: BTreeMap<String, usize> = BTreeMap::new();
    for session in sessions {
        let tool_ids: HashSet<&str> = session
            .messages
            .iter()
            .filter_map(|message| message.tool_metadata.as_ref())
            .filter_map(|metadata| metadata.ids.get("tool_use_id").map(String::as_str))
            .collect();
        let keyed_usage: HashMap<_, _> = session
            .usage_events
            .iter()
            .filter_map(|event| event.usage_hash.as_deref().map(|hash| (hash, event)))
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
            let row_type = row.get("type").and_then(Value::as_str);
            if !session.meta.is_sidechain
                && row_type == Some("event_msg")
                && row.pointer("/payload/type").and_then(Value::as_str) == Some("item_completed")
                && let Some(item) = row.pointer("/payload/item")
                && let Some(kind) = item.get("type").and_then(Value::as_str)
                && matches!(
                    kind,
                    "CommandExecution"
                        | "FileChange"
                        | "McpToolCall"
                        | "WebSearch"
                        | "ImageView"
                        | "DynamicToolCall"
                        | "FunctionCallOutput"
                        | "SubAgentActivity"
                        | "CollabAgentToolCall"
                        | "Extension"
                )
                && let Some(id) = item.get("id").and_then(Value::as_str)
            {
                completed_tools += 1;
                if !tool_ids.contains(id) {
                    *missing_tools.entry(kind.to_string()).or_default() += 1;
                }
            }
            if row_type != Some("token_usage_record") {
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
            if let Some(event) = keyed_usage.get(expected_hash.as_str()) {
                materialized += 1;
                assert_eq!(
                    [
                        event.input_tokens
                            + event.cache_read_input_tokens
                            + event.cache_creation_input_tokens,
                        event.output_tokens,
                        event.cache_read_input_tokens,
                        event.cache_creation_input_tokens,
                    ],
                    raw_token_components(usage),
                    "Codex response token components changed during normalization"
                );
            }
        }
        if audit_complete_codex_response_totals(session, &content) {
            reconciled_sessions += 1;
        }
        if audit_fresh_codex_child_totals(session, &content) {
            reconciled_children += 1;
        }
    }
    eprintln!(
        "  token_usage_record: {records} parsed, {materialized} materialized, {zero_usage} zero-usage"
    );
    eprintln!(
        "  item_completed: {completed_tools} root-session tool records, missing by kind: {missing_tools:?}"
    );
    eprintln!("  token totals: {reconciled_sessions} complete modern root sessions reconciled");
    eprintln!("  token totals: {reconciled_children} fresh legacy subagents reconciled");
    assert!(
        missing_tools.is_empty(),
        "some completed Codex tools were not materialized"
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

fn raw_token_components(usage: &Value) -> [u64; 4] {
    [
        "input_tokens",
        "output_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
    ]
    .map(|field| usage.get(field).and_then(Value::as_u64).unwrap_or(0))
}

fn audit_complete_codex_response_totals(session: &ParsedSession, content: &str) -> bool {
    if session.meta.is_sidechain {
        return false;
    }
    let mut responses = HashMap::new();
    let mut legacy = HashMap::<[u64; 4], usize>::new();
    let mut previous_total = None;
    for line in content.lines() {
        let row: Value = serde_json::from_str(line).unwrap();
        if row.get("type").and_then(Value::as_str) == Some("token_usage_record") {
            let response_id = row
                .pointer("/payload/response_id")
                .and_then(Value::as_str)
                .unwrap();
            responses.insert(
                response_id.to_string(),
                raw_token_components(&row["payload"]["usage"]),
            );
        } else if row.pointer("/payload/type").and_then(Value::as_str) == Some("token_count")
            && let Some(info) = row.pointer("/payload/info").filter(|info| !info.is_null())
        {
            let Some(total) = info
                .get("total_token_usage")
                .filter(|total| total.is_object())
            else {
                return false; // This legacy format cannot prove response coverage.
            };
            if previous_total.as_ref() != Some(total) {
                let Some(last) = info.get("last_token_usage").filter(|last| last.is_object())
                else {
                    return false;
                };
                let counts = raw_token_components(last);
                if counts.iter().any(|count| *count != 0) {
                    *legacy.entry(counts).or_default() += 1;
                }
            }
            previous_total = Some(total.clone());
        }
    }
    if responses.is_empty() {
        return false;
    }
    let mut expected = [0u64; 4];
    for counts in responses.values() {
        for (sum, count) in expected.iter_mut().zip(counts) {
            *sum += count;
        }
        if let Some(remaining) = legacy.get_mut(counts) {
            *remaining = remaining.saturating_sub(1);
        }
    }
    if legacy.values().any(|remaining| *remaining > 0) {
        return false; // Mixed legacy calls need their own accounting, not just response ids.
    }
    let actual = parsed_codex_components(session);
    assert_eq!(
        actual, expected,
        "Codex session totals include missing or duplicated response usage"
    );
    true
}

fn parsed_codex_components(session: &ParsedSession) -> [u64; 4] {
    let mut actual = [0u64; 4];
    for event in &session.usage_events {
        actual[0] +=
            event.input_tokens + event.cache_read_input_tokens + event.cache_creation_input_tokens;
        actual[1] += event.output_tokens;
        actual[2] += event.cache_read_input_tokens;
        actual[3] += event.cache_creation_input_tokens;
    }
    actual
}

fn audit_fresh_codex_child_totals(session: &ParsedSession, content: &str) -> bool {
    if !session.meta.is_sidechain {
        return false;
    }
    let mut metadata_count = 0;
    let mut final_total = [0u64; 4];
    for line in content.lines() {
        let row: Value = serde_json::from_str(line).unwrap();
        match row.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                metadata_count += 1;
                if metadata_count > 1 || row.pointer("/payload/forked_from_id").is_some() {
                    return false; // Inherited history needs replay-aware accounting.
                }
            }
            Some("token_usage_record") => return false,
            _ => {}
        }
        if row.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(info) = row.pointer("/payload/info").filter(|info| !info.is_null()) else {
            continue;
        };
        let (Some(total), Some(last)) =
            (info.get("total_token_usage"), info.get("last_token_usage"))
        else {
            return false;
        };
        let total = raw_token_components(total);
        if total == final_total {
            continue;
        }
        let last = raw_token_components(last);
        if (0..4).any(|i| final_total[i].checked_add(last[i]) != Some(total[i])) {
            return false; // Only a complete, unbroken cumulative chain is an oracle.
        }
        final_total = total;
    }
    assert_eq!(
        parsed_codex_components(session),
        final_total,
        "fresh Codex child lost usage"
    );
    true
}
