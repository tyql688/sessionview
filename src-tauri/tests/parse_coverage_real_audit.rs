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
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::{Level, Metadata, Record};
use serde_json::Value;
use sessionview_lib::models::Provider;
use sessionview_lib::provider::{ParsedSession, SessionProvider, all_runtimes};
use sessionview_lib::providers::codex::CodexProvider;

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
            audit_codex_token_usage_materialization(&parsed, &provider.source_roots());
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

#[test]
#[ignore]
fn audit_codex_real_history_usage() {
    log::set_logger(&LOGGER).ok();
    log::set_max_level(log::LevelFilter::Warn);
    let provider = CodexProvider::new().unwrap();
    let sessions = provider.scan_all().unwrap();
    eprintln!(
        "codex: {} sessions, {} parse warnings",
        sessions.len(),
        sessions
            .iter()
            .map(|session| u64::from(session.parse_warning_count))
            .sum::<u64>()
    );
    audit_codex_token_usage_materialization(&sessions, &provider.source_roots());
}

fn audit_codex_token_usage_materialization(sessions: &[ParsedSession], roots: &[PathBuf]) {
    let catalog = reference_rollout_catalog(roots);
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
        let content = match read_reference_history(
            Path::new(&session.meta.source_path),
            session.meta.file_size_bytes,
            &catalog,
        ) {
            Ok(content) => content,
            Err(_) => {
                unreadable_sources += 1;
                continue;
            }
        };
        let mut seen_response_ids = HashSet::new();
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
            if payload
                .get("thread_id")
                .and_then(Value::as_str)
                .is_some_and(|owner| owner != session.meta.id)
            {
                continue;
            }
            let Some(response_id) = payload.get("response_id").and_then(Value::as_str) else {
                continue;
            };
            if !seen_response_ids.insert(response_id.to_string()) {
                continue;
            }
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

fn reference_rollout_catalog(roots: &[PathBuf]) -> HashMap<String, Vec<PathBuf>> {
    let home = roots.first().and_then(|root| root.parent());
    assert!(roots.iter().all(|root| root.parent() == home));
    let mut catalog = HashMap::<String, Vec<PathBuf>>::new();
    let mut relative_paths = HashSet::new();
    for root in roots.iter().filter(|root| root.exists()) {
        let mut files = walkdir::WalkDir::new(root)
            .into_iter()
            .map(Result::unwrap)
            .filter(|entry| {
                entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|ext| ext == "jsonl")
            })
            .map(walkdir::DirEntry::into_path)
            .collect::<Vec<_>>();
        files.sort();
        for path in files {
            // Active storage precedes its archived copy at the same relative path.
            if !relative_paths.insert(path.strip_prefix(root).unwrap().to_path_buf()) {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
                && let Some(id) = stem.get(stem.len().saturating_sub(36)..)
            {
                catalog
                    .entry(id.to_ascii_lowercase())
                    .or_default()
                    .push(path);
            }
        }
    }
    catalog
}

fn read_reference_history(
    path: &Path,
    limit: u64,
    catalog: &HashMap<String, Vec<PathBuf>>,
) -> std::io::Result<String> {
    let mut path = path.to_path_buf();
    let mut limit = limit;
    let mut parts = Vec::new();
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(path.clone()) || visited.len() > 64 {
            return Err(std::io::Error::other("invalid history graph"));
        }
        let mut content = String::new();
        File::open(&path)?
            .take(limit)
            .read_to_string(&mut content)?;
        let header: Value = serde_json::from_str(content.lines().next().unwrap_or_default())?;
        let base = header
            .pointer("/payload/history_base")
            .filter(|base| !base.is_null());
        let Some(base) = base else {
            parts.push(content);
            break;
        };
        let id = base
            .get("thread_id")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("missing physical identity"))?;
        let candidates = catalog
            .get(&id.to_ascii_lowercase())
            .ok_or_else(|| std::io::Error::other("missing history prefix"))?;
        let [parent] = candidates.as_slice() else {
            return Err(std::io::Error::other("ambiguous physical history identity"));
        };
        let end_ordinal = base
            .get("end_ordinal_exclusive")
            .and_then(Value::as_u64)
            .ok_or_else(|| std::io::Error::other("missing ordinal boundary"))?;
        if header.get("ordinal").and_then(Value::as_u64) != Some(end_ordinal) {
            return Err(std::io::Error::other("invalid continuation ordinal"));
        }
        limit = base
            .get("end_byte_offset")
            .and_then(Value::as_u64)
            .ok_or_else(|| std::io::Error::other("missing byte boundary"))?;
        let mut prefix = String::new();
        File::open(parent)?
            .take(limit)
            .read_to_string(&mut prefix)?;
        let last: Value = serde_json::from_str(prefix.lines().last().unwrap_or_default())?;
        if prefix.len() as u64 != limit
            || !prefix.ends_with('\n')
            || last
                .get("ordinal")
                .and_then(Value::as_u64)
                .and_then(|ordinal| ordinal.checked_add(1))
                != Some(end_ordinal)
        {
            return Err(std::io::Error::other("invalid retained boundary"));
        }
        parts.push(content);
        path = parent.clone();
    }
    Ok(parts.into_iter().rev().collect())
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
            if row
                .pointer("/payload/thread_id")
                .and_then(Value::as_str)
                .is_some_and(|owner| owner != session.meta.id)
            {
                continue;
            }
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

#[test]
fn reference_history_reads_only_the_retained_prefix_and_rejects_duplicate_identity() {
    let home = tempfile::TempDir::new().unwrap();
    let root = home.path().join("sessions");
    std::fs::create_dir_all(&root).unwrap();
    let id = "11111111-1111-4111-a111-111111111111";
    let parent = root.join(format!("rollout-2026-09-01T00-00-00-{id}.jsonl"));
    let prefix = format!(
        "{}\n{}\n",
        serde_json::json!({"type": "session_meta", "ordinal": 0, "payload": {"id": id}}),
        serde_json::json!({"type": "response_item", "ordinal": 1, "payload": {"text": "retained"}}),
    );
    std::fs::write(
        &parent,
        format!(
            "{prefix}{}\n",
            serde_json::json!({"ordinal": 2, "text": "discarded"})
        ),
    )
    .unwrap();
    let child = root.join("child.jsonl");
    let content = format!(
        "{}\n",
        serde_json::json!({
            "type": "session_meta", "ordinal": 2, "payload": {"id": "child-test", "history_base": {
                "thread_id": id, "end_ordinal_exclusive": 2, "end_byte_offset": prefix.len(),
            }},
        })
    );
    std::fs::write(&child, &content).unwrap();
    let roots = vec![root.clone()];
    let catalog = reference_rollout_catalog(&roots);
    assert_eq!(
        read_reference_history(&child, content.len() as u64, &catalog).unwrap(),
        format!("{prefix}{content}"),
    );
    let duplicate_dir = root.join("duplicate");
    std::fs::create_dir_all(&duplicate_dir).unwrap();
    std::fs::copy(&parent, duplicate_dir.join(parent.file_name().unwrap())).unwrap();
    assert!(
        read_reference_history(
            &child,
            content.len() as u64,
            &reference_rollout_catalog(&roots)
        )
        .is_err()
    );
}

#[test]
fn reference_catalog_keeps_active_storage_at_the_same_relative_path() {
    let home = tempfile::TempDir::new().unwrap();
    let active = home.path().join("sessions");
    let archive = home.path().join("archived_sessions");
    let id = "11111111-1111-4111-a111-111111111111";
    let filename = format!("rollout-2026-09-01T00-00-00-{id}.jsonl");
    for root in [&active, &archive] {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join(&filename), "{}\n").unwrap();
    }
    let catalog = reference_rollout_catalog(&[active.clone(), archive]);
    assert_eq!(catalog[id], vec![active.join(filename)]);
}
