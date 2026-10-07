use std::fs;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::provider::{SessionProvider, SourceState};

const THREAD_ID: &str = "11111111-1111-4111-a111-111111111111";
const SEGMENT_ID: &str = "22222222-2222-4222-a222-222222222222";
const NEXT_SEGMENT_ID: &str = "33333333-3333-4333-a333-333333333333";

fn rows(start: u64, base: Option<(&str, u64, u64)>, text: &str) -> String {
    rows_with_id(THREAD_ID, start, base, text)
}

fn rows_with_id(id: &str, start: u64, base: Option<(&str, u64, u64)>, text: &str) -> String {
    let mut header = json!({
        "id": id, "cwd": "/tmp/project", "cli_version": "fixture",
    });
    if let Some((rollout_id, ordinal, bytes)) = base {
        header["history_base"] = json!({
            "thread_id": rollout_id, "end_ordinal_exclusive": ordinal, "end_byte_offset": bytes,
        });
    }
    let usage = json!({"input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 10, "total_tokens": 110});
    let multiplier = start / 4 + 1;
    let total = json!({"input_tokens": 100 * multiplier, "cached_input_tokens": 40 * multiplier,
        "output_tokens": 10 * multiplier, "total_tokens": 110 * multiplier});
    [
        json!({"ordinal": start, "type": "session_meta", "payload": header}),
        json!({"ordinal": start + 1, "type": "turn_context", "payload": {"turn_id": format!("turn-{start}"), "model": "model-test"}}),
        json!({"ordinal": start + 2, "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}}),
        json!({"ordinal": start + 3, "type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": usage, "total_token_usage": total}}}),
    ].into_iter().enumerate().map(|(i, mut row)| {
        row["timestamp"] = json!(format!("2026-09-01T{:02}:{:02}:{i:02}Z", start / 60, start % 60));
        format!("{row}\n")
    }).collect()
}

fn fixture() -> (TempDir, CodexProvider, PathBuf, PathBuf) {
    let home = TempDir::new().unwrap();
    let dir = home.path().join(".codex/sessions");
    fs::create_dir_all(&dir).unwrap();
    let root = dir.join(format!("rollout-2026-09-01T00-00-00-{THREAD_ID}.jsonl"));
    let leaf = dir.join(format!(
        "rollout-2026-09-01T00-00-00-{THREAD_ID}_{SEGMENT_ID}.jsonl"
    ));
    let prefix = rows(0, None, "retained history");
    // A discarded branch after the retained boundary must not leak into the
    // new timeline or token totals.
    fs::write(
        &root,
        format!("{prefix}{}", rows(4, None, "discarded branch")),
    )
    .unwrap();
    fs::write(
        &leaf,
        rows(4, Some((THREAD_ID, 4, prefix.len() as u64)), "new history"),
    )
    .unwrap();
    let provider = CodexProvider {
        home_dir: home.path().to_path_buf(),
    };
    (home, provider, root, leaf)
}

#[test]
fn pagination_preserves_prefix_and_continuation_once_across_scans_and_loads() {
    let (_home, provider, _root, leaf) = fixture();
    let sessions = provider.scan_all().unwrap();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    // Directory walking can normalize mixed Windows path separators.
    assert_eq!(Path::new(&session.meta.source_path), leaf.as_path());
    assert_eq!(
        session.meta.file_size_bytes,
        fs::metadata(&leaf).unwrap().len()
    );
    assert_eq!(session.usage_events.len(), 2);
    assert_eq!(
        session
            .usage_events
            .iter()
            .map(|event| event.input_tokens)
            .sum::<u64>(),
        120
    );
    assert!(session.content_text.contains("retained history"));
    assert!(session.content_text.contains("new history"));
    assert!(!session.content_text.contains("discarded branch"));
    assert!(super::super::parser::parse_session_tail(&leaf, 100).is_none());
    let known = HashMap::from([(
        session.meta.source_path.clone(),
        SourceState {
            size: session.meta.file_size_bytes,
            mtime: session.source_mtime,
            title: Some(session.meta.title.clone()),
        },
    )]);
    for _ in 0..2 {
        let next = provider.scan_incremental(&known).unwrap();
        assert!(next.parsed.is_empty());
        assert_eq!(next.unchanged_source_paths.len(), 1);
        assert_eq!(Path::new(&next.unchanged_source_paths[0]), leaf.as_path());
    }
    let loaded = provider
        .load_messages(&session.meta.id, &session.meta.source_path)
        .unwrap();
    assert_eq!(loaded.messages.len(), session.messages.len());
}

#[test]
fn pagination_revert_resolves_physical_rollout_id_and_preserves_logical_thread() {
    let (home, provider, _root, leaf) = fixture();
    let next = home.path().join(format!(
        ".codex/sessions/rollout-2026-09-01T00-00-00-{THREAD_ID}_{NEXT_SEGMENT_ID}.jsonl"
    ));
    fs::write(
        &next,
        rows(
            8,
            Some((SEGMENT_ID, 8, fs::metadata(&leaf).unwrap().len())),
            "third segment",
        ),
    )
    .unwrap();
    let sessions = provider.scan_all().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].meta.id, THREAD_ID);
    assert_eq!(Path::new(&sessions[0].meta.source_path), next.as_path());
    assert_eq!(sessions[0].usage_events.len(), 3);
    assert!(sessions[0].content_text.contains("retained history"));
    assert!(sessions[0].content_text.contains("new history"));
    assert!(sessions[0].content_text.contains("third segment"));
    assert!(!sessions[0].content_text.contains("discarded branch"));
    let loaded = provider
        .load_messages(THREAD_ID, &sessions[0].meta.source_path)
        .unwrap();
    assert_eq!(loaded.messages.len(), sessions[0].messages.len());
}

#[test]
fn pagination_rejects_missing_or_misaligned_base_instead_of_indexing_a_fragment() {
    let (_home, provider, root, leaf) = fixture();
    let original = fs::read(&leaf).unwrap();
    fs::write(&leaf, rows(4, Some((THREAD_ID, 4, 1)), "bad boundary")).unwrap();
    assert!(provider.scan_all().is_err());
    assert!(provider.parse_session_file(&leaf).is_none());
    fs::write(&leaf, original).unwrap();
    fs::remove_file(root).unwrap();
    assert!(provider.scan_all().is_err());
    assert!(provider.parse_session_file(&leaf).is_none());
}

#[test]
fn pagination_rejects_a_different_rollout_with_matching_thread_and_boundary() {
    let (_home, provider, _root, leaf) = fixture();
    let prefix = rows(0, None, "retained history");
    fs::write(
        &leaf,
        rows(
            4,
            Some((NEXT_SEGMENT_ID, 4, prefix.len() as u64)),
            "unresolved history",
        ),
    )
    .unwrap();
    assert!(provider.scan_all().is_err());
    assert!(provider.parse_session_file(&leaf).is_none());
}

#[test]
fn pagination_rename_uses_header_identity_instead_of_segment_filename() {
    let (home, provider, _root, _leaf) = fixture();
    let session = provider.scan_all().unwrap().remove(0);
    let known = HashMap::from([(
        session.meta.source_path,
        SourceState {
            size: session.meta.file_size_bytes,
            mtime: session.source_mtime,
            title: Some(session.meta.title),
        },
    )]);
    fs::write(
        home.path().join(".codex/session_index.jsonl"),
        format!("{}\n", json!({"id": THREAD_ID, "thread_name": "renamed"})),
    )
    .unwrap();
    let next = provider.scan_incremental(&known).unwrap();
    assert_eq!(next.parsed.len(), 1);
    assert_eq!(next.parsed[0].meta.title, "renamed");
}

#[test]
fn pagination_resolves_continuation_with_distinct_rollout_id() {
    let home = TempDir::new().unwrap();
    let dir = home.path().join(".codex/sessions");
    fs::create_dir_all(&dir).unwrap();
    let root = dir.join(format!("rollout-2026-09-01T00-00-00-{THREAD_ID}.jsonl"));
    let leaf = dir.join(format!("rollout-2026-09-01T00-00-00-{SEGMENT_ID}.jsonl"));
    let prefix = rows_with_id(THREAD_ID, 0, None, "retained history");
    fs::write(
        &root,
        format!(
            "{prefix}{}",
            rows_with_id(THREAD_ID, 4, None, "discarded branch")
        ),
    )
    .unwrap();
    fs::write(
        &leaf,
        rows_with_id(
            SEGMENT_ID,
            4,
            Some((THREAD_ID, 4, prefix.len() as u64)),
            "new history",
        ),
    )
    .unwrap();
    let provider = CodexProvider {
        home_dir: home.path().to_path_buf(),
    };
    let sessions = provider.scan_all().unwrap();
    assert_eq!(
        sessions.len(),
        2,
        "a referenced fork must keep its parent indexed"
    );
    let child = sessions
        .iter()
        .find(|session| session.meta.id == SEGMENT_ID)
        .unwrap();
    assert_eq!(Path::new(&child.meta.source_path), leaf.as_path());
    assert!(child.content_text.contains("retained history"));
    assert!(child.content_text.contains("new history"));
    assert!(!child.content_text.contains("discarded branch"));
    assert_eq!(
        child.usage_events.len(),
        1,
        "inherited parent requests are not child spending"
    );
    assert_eq!(child.usage_events[0].input_tokens, 60);
    let loaded = provider
        .load_messages(SEGMENT_ID, &child.meta.source_path)
        .unwrap();
    assert_eq!(loaded.messages.len(), child.messages.len());
}

#[test]
fn thread_replacement_selects_newest_filename_without_history_base() {
    let home = TempDir::new().unwrap();
    let dir = home.path().join(".codex/sessions");
    fs::create_dir_all(&dir).unwrap();
    let root = dir.join(format!("rollout-2026-09-01T00-00-00-{THREAD_ID}.jsonl"));
    let replacement = dir.join(format!(
        "rollout-2026-09-01T00-01-00-{THREAD_ID}_{SEGMENT_ID}.jsonl"
    ));
    fs::write(&root, rows(0, None, "obsolete history")).unwrap();
    fs::write(&replacement, rows(0, None, "current history")).unwrap();
    let provider = CodexProvider::with_home(home.path().to_path_buf());
    let sessions = provider.scan_all().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].meta.id, THREAD_ID);
    assert_eq!(Path::new(&sessions[0].meta.source_path), replacement);
    assert!(sessions[0].content_text.contains("current history"));
    assert!(!sessions[0].content_text.contains("obsolete history"));
}

#[test]
fn history_base_requires_physical_identity_even_when_logical_id_matches() {
    let home = TempDir::new().unwrap();
    let dir = home.path().join(".codex/sessions");
    fs::create_dir_all(&dir).unwrap();
    let wrong = dir.join(format!("rollout-2026-09-01T00-00-00-{SEGMENT_ID}.jsonl"));
    let child = dir.join(format!(
        "rollout-2026-09-01T00-01-00-{NEXT_SEGMENT_ID}.jsonl"
    ));
    let prefix = rows(0, None, "wrong physical file");
    fs::write(wrong, &prefix).unwrap();
    fs::write(
        child,
        rows_with_id(
            NEXT_SEGMENT_ID,
            4,
            Some((THREAD_ID, 4, prefix.len() as u64)),
            "child",
        ),
    )
    .unwrap();
    let provider = CodexProvider::with_home(home.path().to_path_buf());
    assert!(provider.scan_all().is_err());
}

#[test]
fn same_second_replacement_uses_physical_uuid_order_independent_of_input_order() {
    let (_home, _provider, root, leaf) = fixture();
    fs::write(&leaf, rows(0, None, "replacement")).unwrap();
    for paths in [
        vec![root.clone(), leaf.clone()],
        vec![leaf.clone(), root.clone()],
    ] {
        assert_eq!(leaf_paths(paths).unwrap(), vec![leaf.clone()]);
    }
}

#[test]
fn obsolete_broken_chain_does_not_invalidate_a_fresh_replacement() {
    let (home, provider, root, leaf) = fixture();
    fs::write(
        &root,
        rows(4, Some((NEXT_SEGMENT_ID, 4, 1)), "broken old branch"),
    )
    .unwrap();
    let replacement = home.path().join(format!(
        ".codex/sessions/rollout-2026-09-01T00-01-00-{THREAD_ID}_{NEXT_SEGMENT_ID}.jsonl"
    ));
    fs::write(&replacement, rows(0, None, "fresh replacement")).unwrap();
    let sessions = provider.scan_all().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(Path::new(&sessions[0].meta.source_path), replacement);
    assert!(provider.parse_session_file(&leaf).is_none());
}

#[test]
fn duplicate_nonstandard_filenames_remain_an_explicit_error() {
    let home = TempDir::new().unwrap();
    let one = home.path().join("one.jsonl");
    let two = home.path().join("two.jsonl");
    fs::write(&one, rows(0, None, "one")).unwrap();
    fs::write(&two, rows(0, None, "two")).unwrap();
    assert!(leaf_paths(vec![one, two]).is_err());
}

#[test]
fn typed_subagent_boundary_is_enforced_in_full_and_partial_tail_reads() {
    for inherited_messages in [0, 256] {
        let home = TempDir::new().unwrap();
        let dir = home.path().join(".codex/sessions");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("rollout-2026-09-01T00-01-00-{SEGMENT_ID}.jsonl"));
        let start = 5 + inherited_messages;
        let header = json!({
            "ordinal": 0, "timestamp": "2026-09-01T00:01:00Z", "type": "session_meta",
            "payload": {"id": SEGMENT_ID, "thread_source": "subagent", "parent_thread_id": THREAD_ID,
                "subagent_history_start_ordinal": start},
        });
        let mut content = format!("{header}\n{}", rows(1, None, "inherited transcript"));
        for ordinal in 5..start {
            let row = json!({
                "ordinal": ordinal, "type": "response_item",
                "payload": {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "inherited transcript"}]},
            });
            content.push_str(&format!("{row}\n"));
        }
        let owned = rows_with_id(SEGMENT_ID, start - 1, None, "owned transcript");
        for line in owned.lines().skip(1) {
            let mut row: Value = serde_json::from_str(line).unwrap();
            if let Some(total) = row.pointer_mut("/payload/info/total_token_usage") {
                *total = json!({"input_tokens": 200, "cached_input_tokens": 80, "output_tokens": 20, "total_tokens": 220});
            }
            content.push_str(&format!("{row}\n"));
        }
        fs::write(&path, content).unwrap();
        let provider = CodexProvider::with_home(home.path().to_path_buf());
        let parsed = provider.parse_session_file(&path).unwrap();
        assert_eq!(parsed.meta.id, SEGMENT_ID);
        assert_eq!(parsed.meta.parent_id.as_deref(), Some(THREAD_ID));
        assert!(parsed.meta.is_sidechain);
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].content, "owned transcript");
        assert_eq!(parsed.usage_events.len(), 1);
        let tail = super::super::parser::parse_session_tail(&path, 1).unwrap();
        assert_eq!(tail.parse_warning_count, 0);
        assert_eq!(tail.messages.len(), 1);
        assert_eq!(tail.messages[0].content, "owned transcript");
    }
}

#[test]
fn continuation_keeps_retained_cumulative_baseline_for_snapshots_and_deltas() {
    for cumulative_only in [false, true] {
        let (_home, provider, _root, leaf) = fixture();
        let content = fs::read_to_string(&leaf).unwrap();
        let mut rewritten = String::new();
        for line in content.lines() {
            let mut row: Value = serde_json::from_str(line).unwrap();
            if let Some(info) = row.pointer_mut("/payload/info") {
                if cumulative_only {
                    info.as_object_mut().unwrap().remove("last_token_usage");
                    info["total_token_usage"] = json!({"input_tokens": 200, "cached_input_tokens": 80, "output_tokens": 20, "total_tokens": 220});
                } else {
                    info["total_token_usage"] = info["last_token_usage"].clone();
                }
            }
            rewritten.push_str(&format!("{row}\n"));
        }
        fs::write(&leaf, rewritten).unwrap();
        let parsed = provider.parse_session_file(&leaf).unwrap();
        let expected = if cumulative_only { 2 } else { 1 };
        assert_eq!(parsed.usage_events.len(), expected);
        assert_eq!(
            parsed
                .usage_events
                .iter()
                .map(|event| event.input_tokens)
                .sum::<u64>(),
            60 * expected as u64,
        );
    }
}

#[test]
fn referenced_subagent_recovers_own_delta_from_retained_parent_totals() {
    let (_home, provider, root, _) = fixture();
    let prefix = rows(0, None, "retained history");
    let child = root.with_file_name(format!("rollout-2026-09-01T00-05-00-{SEGMENT_ID}.jsonl"));
    let content = rows_with_id(
        SEGMENT_ID,
        4,
        Some((THREAD_ID, 4, prefix.len() as u64)),
        "owned transcript",
    );
    let mut rewritten = String::new();
    for line in content.lines() {
        let mut row: Value = serde_json::from_str(line).unwrap();
        if row["type"] == "session_meta" {
            row["payload"]["thread_source"] = json!("subagent");
            row["payload"]["parent_thread_id"] = json!(THREAD_ID);
            row["payload"]["subagent_history_start_ordinal"] = json!(5);
        }
        if let Some(info) = row.pointer_mut("/payload/info") {
            info.as_object_mut().unwrap().remove("last_token_usage");
        }
        rewritten.push_str(&format!("{row}\n"));
    }
    fs::write(&child, rewritten).unwrap();
    let parsed = provider.parse_session_file(&child).unwrap();
    assert_eq!(parsed.parse_warning_count, 0);
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].content, "owned transcript");
    assert_eq!(parsed.usage_events.len(), 1);
    assert_eq!(parsed.usage_events[0].input_tokens, 60);
    assert_eq!(parsed.usage_events[0].output_tokens, 10);
    assert_eq!(parsed.usage_events[0].cache_read_input_tokens, 40);
}
