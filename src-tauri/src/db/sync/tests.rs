use super::{Database, SessionMeta, TokenStatRow};
use crate::models::Provider;
use crate::provider::ParsedSession;
use tempfile::TempDir;

fn sample_meta(session_id: &str) -> SessionMeta {
    SessionMeta {
        id: session_id.to_string(),
        provider: Provider::Claude,
        title: "Test".into(),
        project_path: "/tmp/project".into(),
        project_name: "project".into(),
        created_at: 1_775_635_200,
        updated_at: 1_775_635_200,
        message_count: 1,
        file_size_bytes: 0,
        source_path: "/tmp/source.jsonl".into(),
        is_sidechain: false,
        variant_name: None,
        model: Some("claude-opus-4-6".into()),
        cc_version: None,
        git_branch: None,
        parent_id: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    }
}

#[test]
fn provider_snapshot_delete_guard_protects_empty_non_aggressive_scan() {
    assert!(!super::should_delete_provider_snapshot(
        &Provider::Claude,
        false,
        100,
        0,
        0,
    ));
}

#[test]
fn provider_snapshot_delete_guard_allows_empty_aggressive_scan() {
    assert!(super::should_delete_provider_snapshot(
        &Provider::Claude,
        true,
        100,
        0,
        0,
    ));
}

#[test]
fn provider_snapshot_delete_guard_counts_preserved_sources_as_alive() {
    assert!(super::should_delete_provider_snapshot(
        &Provider::Claude,
        false,
        100,
        0,
        75,
    ));
    assert!(!super::should_delete_provider_snapshot(
        &Provider::Claude,
        false,
        100,
        0,
        50,
    ));
}

#[test]
fn source_snapshot_delete_guard_treats_empty_scan_as_deleted_file() {
    assert!(super::should_delete_source_snapshot(100, 0));
}

#[test]
fn source_snapshot_delete_guard_uses_ratio_for_non_empty_scans() {
    assert!(super::should_delete_source_snapshot(100, 51));
    assert!(!super::should_delete_source_snapshot(100, 50));
}

#[test]
fn replace_token_stats_clears_existing_rows_when_empty() {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let meta = sample_meta("session-1");
    db.sync_provider_snapshot(
        &Provider::Claude,
        &[ParsedSession {
            meta: meta.clone(),
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 0,
        }],
        true,
        &[],
    )
    .unwrap();

    db.replace_token_stats(
        &meta.id,
        &[TokenStatRow {
            date: "2026-04-09".into(),
            model: "claude-opus-4-6".into(),
            turn_count: 1,
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.01,
        }],
    )
    .unwrap();

    db.replace_token_stats(&meta.id, &[]).unwrap();

    let conn = db.lock_read().unwrap();
    let count: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_token_stats WHERE session_id = ?1",
            [meta.id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn clear_usage_stats_preserves_sessions() {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let meta = sample_meta("session-2");
    db.sync_provider_snapshot(
        &Provider::Claude,
        &[ParsedSession {
            meta: meta.clone(),
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 0,
        }],
        true,
        &[],
    )
    .unwrap();
    db.replace_token_stats(
        &meta.id,
        &[TokenStatRow {
            date: "2026-04-10".into(),
            model: "claude-opus-4-6".into(),
            turn_count: 1,
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.001,
        }],
    )
    .unwrap();

    db.clear_usage_stats().unwrap();

    assert!(db.get_session(&meta.id).unwrap().is_some());
    let conn = db.lock_read().unwrap();
    let usage_rows: u64 = conn
        .query_row("SELECT COUNT(*) FROM session_token_stats", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(usage_rows, 0);
}

#[test]
fn clear_usage_stats_resets_source_mtime_so_next_scan_reparses() {
    // Regression: `clear_usage_stats` used to leave `sessions.source_mtime`
    // intact. The next `Indexer::reindex` would then call
    // `scan_incremental(&known)`, every file's `(size, mtime)` would still
    // match the snapshot, and `partition_files_by_freshness` would route
    // them all into `unchanged_source_paths` — skipping the parse pass
    // that rewrites `session_token_stats`. Result: all token stats
    // permanently zero until the user manually edited every transcript.
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let meta = sample_meta("session-mtime");
    db.sync_provider_snapshot(
        &Provider::Claude,
        &[ParsedSession {
            meta: meta.clone(),
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 1_775_635_200,
        }],
        true,
        &[],
    )
    .unwrap();

    let known_before = db
        .source_states_for_provider(Provider::Claude.key())
        .unwrap();
    assert_eq!(
        known_before.get(&meta.source_path).map(|s| s.mtime),
        Some(1_775_635_200),
        "precondition: sync recorded the source mtime",
    );

    db.clear_usage_stats().unwrap();

    let known_after = db
        .source_states_for_provider(Provider::Claude.key())
        .unwrap();
    assert_eq!(
        known_after.get(&meta.source_path).map(|s| s.mtime),
        Some(0),
        "clear_usage_stats must invalidate the freshness snapshot \
         so scan_incremental reparses every file",
    );
}

#[test]
fn child_session_counts_returns_counts_for_requested_parents() {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path()).unwrap();
    let parent_a = sample_meta("parent-a");
    let parent_b = sample_meta("parent-b");
    let mut child_a1 = sample_meta("child-a-1");
    child_a1.parent_id = Some(parent_a.id.clone());
    child_a1.is_sidechain = true;
    let mut child_a2 = sample_meta("child-a-2");
    child_a2.parent_id = Some(parent_a.id.clone());
    child_a2.is_sidechain = true;
    let mut child_b1 = sample_meta("child-b-1");
    child_b1.parent_id = Some(parent_b.id.clone());
    child_b1.is_sidechain = true;

    let parsed = [
        parent_a.clone(),
        parent_b.clone(),
        child_a1,
        child_a2,
        child_b1,
    ]
    .into_iter()
    .map(|meta| ParsedSession {
        meta,
        messages: Vec::new(),
        content_text: String::new(),
        parse_warning_count: 0,
        child_session_ids: Vec::new(),
        usage_events: Vec::new(),
        source_mtime: 0,
    })
    .collect::<Vec<_>>();
    db.sync_provider_snapshot(&Provider::Claude, &parsed, true, &[])
        .unwrap();

    let counts = db
        .child_session_counts(&[parent_a.id.clone(), parent_b.id.clone()])
        .unwrap();
    assert_eq!(counts.get(&parent_a.id), Some(&2));
    assert_eq!(counts.get(&parent_b.id), Some(&1));
}

#[test]
fn parent_backfills_child_when_parser_declares_child_ids() {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let child_id = "22222222-2222-4222-a222-222222222222";
    let parent_id = "11111111-1111-4111-a111-111111111111";

    // 1. Child indexed first with no parent / Unknown project (mirrors the
    //    watcher firing on the child transcript before the parent's).
    let mut child_meta = sample_meta(child_id);
    child_meta.project_path = String::new();
    child_meta.project_name = "Unknown Project".into();
    child_meta.parent_id = None;
    child_meta.is_sidechain = false;

    db.sync_provider_snapshot(
        &Provider::Antigravity,
        &[ParsedSession {
            meta: child_meta.clone(),
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 0,
        }],
        true,
        &[],
    )
    .unwrap();

    let loaded_child = db.get_session(child_id).unwrap().unwrap();
    assert_eq!(loaded_child.parent_id, None);
    assert!(!loaded_child.is_sidechain);
    assert_eq!(loaded_child.project_name, "Unknown Project");

    // 2. Parent indexed with explicit child id list (the antigravity parser
    //    populates this from INVOKE_SUBAGENT step content). The child row
    //    must be back-filled with parent_id + inherited project metadata.
    let mut parent_meta = sample_meta(parent_id);
    parent_meta.project_path = "/tmp/ccsession".into();
    parent_meta.project_name = "ccsession".into();

    db.sync_provider_snapshot(
        &Provider::Antigravity,
        &[ParsedSession {
            meta: parent_meta.clone(),
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: vec![child_id.to_string()],
            usage_events: Vec::new(),
            source_mtime: 0,
        }],
        true,
        &[],
    )
    .unwrap();

    let loaded_child_after = db.get_session(child_id).unwrap().unwrap();
    assert_eq!(loaded_child_after.parent_id, Some(parent_id.to_string()));
    assert!(loaded_child_after.is_sidechain);
    assert_eq!(loaded_child_after.project_path, "/tmp/ccsession");
    assert_eq!(loaded_child_after.project_name, "ccsession");

    // 3. A later incremental sync of the child (no parent info) must not
    //    clobber parent_id / is_sidechain / project metadata.
    db.sync_provider_snapshot(
        &Provider::Antigravity,
        &[ParsedSession {
            meta: child_meta.clone(),
            messages: Vec::new(),
            content_text: "Child updated content".into(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 0,
        }],
        true,
        &[],
    )
    .unwrap();

    let loaded_child_final = db.get_session(child_id).unwrap().unwrap();
    assert_eq!(loaded_child_final.parent_id, Some(parent_id.to_string()));
    assert!(loaded_child_final.is_sidechain);
    assert_eq!(loaded_child_final.project_path, "/tmp/ccsession");
    assert_eq!(loaded_child_final.project_name, "ccsession");
}

#[test]
fn pi_upsert_clears_stale_parent_when_parser_no_longer_resolves_it() {
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let child_id = "22222222-2222-4222-a222-222222222222";
    let parent_id = "11111111-1111-4111-a111-111111111111";

    let mut child_meta = sample_meta(child_id);
    child_meta.provider = Provider::Pi;
    child_meta.parent_id = Some(parent_id.to_string());
    child_meta.is_sidechain = true;

    db.sync_provider_snapshot(
        &Provider::Pi,
        &[ParsedSession {
            meta: child_meta.clone(),
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 0,
        }],
        true,
        &[],
    )
    .unwrap();

    let loaded_child = db.get_session(child_id).unwrap().unwrap();
    assert_eq!(loaded_child.parent_id, Some(parent_id.to_string()));
    assert!(loaded_child.is_sidechain);

    child_meta.parent_id = None;
    child_meta.is_sidechain = false;

    db.sync_provider_snapshot(
        &Provider::Pi,
        &[ParsedSession {
            meta: child_meta,
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: Vec::new(),
            usage_events: Vec::new(),
            source_mtime: 0,
        }],
        true,
        &[],
    )
    .unwrap();

    let loaded_child_after = db.get_session(child_id).unwrap().unwrap();
    assert_eq!(loaded_child_after.parent_id, None);
    assert!(!loaded_child_after.is_sidechain);
}

#[test]
fn upsert_does_not_relink_when_child_already_has_parent() {
    // Regression guard: random UUIDs that happen to match an existing
    // session id must NOT steal it away from its real parent.
    let dir = TempDir::new().unwrap();
    let db = Database::open(dir.path()).unwrap();

    let child_id = "22222222-2222-4222-a222-222222222222";
    let true_parent = "11111111-1111-4111-a111-111111111111";
    let other = "44444444-4444-4444-a444-444444444444";

    // Real parent claims the child.
    let mut child_meta = sample_meta(child_id);
    child_meta.parent_id = Some(true_parent.into());
    child_meta.is_sidechain = true;
    let mut true_parent_meta = sample_meta(true_parent);
    true_parent_meta.project_path = "/tmp/real".into();
    true_parent_meta.project_name = "real".into();

    db.sync_provider_snapshot(
        &Provider::Antigravity,
        &[
            ParsedSession {
                meta: child_meta,
                messages: Vec::new(),
                content_text: String::new(),
                parse_warning_count: 0,
                child_session_ids: Vec::new(),
                usage_events: Vec::new(),
                source_mtime: 0,
            },
            ParsedSession {
                meta: true_parent_meta,
                messages: Vec::new(),
                content_text: String::new(),
                parse_warning_count: 0,
                child_session_ids: vec![child_id.into()],
                usage_events: Vec::new(),
                source_mtime: 0,
            },
        ],
        true,
        &[],
    )
    .unwrap();

    // An unrelated session that happens to mention the child id (e.g. a
    // long-running session whose transcript copy-pasted that uuid) must
    // not be allowed to override the existing parent.
    let other_meta = sample_meta(other);
    db.sync_provider_snapshot(
        &Provider::Antigravity,
        &[ParsedSession {
            meta: other_meta,
            messages: Vec::new(),
            content_text: String::new(),
            parse_warning_count: 0,
            child_session_ids: vec![child_id.into()],
            usage_events: Vec::new(),
            source_mtime: 0,
        }],
        true,
        &[],
    )
    .unwrap();

    let loaded = db.get_session(child_id).unwrap().unwrap();
    assert_eq!(loaded.parent_id, Some(true_parent.to_string()));
}

#[test]
fn indexable_content_indexes_only_user_and_assistant() {
    use crate::models::{Message, MessageRole};

    fn msg(
        role: MessageRole,
        content: &str,
        tool_name: Option<&str>,
        tool_input: Option<&str>,
    ) -> Message {
        Message {
            role,
            message_kind: None,
            content: content.to_string(),
            timestamp: None,
            tool_name: tool_name.map(str::to_string),
            tool_input: tool_input.map(str::to_string),
            tool_metadata: None,
            token_usage: None,
            model: None,
            usage_hash: None,
        }
    }

    let messages = vec![
        msg(MessageRole::User, "用户问题", None, None),
        msg(
            MessageRole::Assistant,
            "助手回复",
            Some("Bash"),
            Some("grep 配置 src/"),
        ),
        msg(MessageRole::Tool, "工具输出里有中文命中", None, None),
        msg(
            MessageRole::System,
            "[thinking]\n模型在思考问题",
            None,
            None,
        ),
    ];

    let text = super::indexable_content_text(&messages, "fallback");
    // Only user + assistant dialogue is indexed.
    assert!(text.contains("用户问题"));
    assert!(text.contains("助手回复"));
    // Tool name/input, tool result bodies, thinking, and system are excluded.
    assert!(!text.contains("Bash"), "tool name must NOT be indexed");
    assert!(
        !text.contains("grep 配置"),
        "tool input must NOT be indexed"
    );
    assert!(
        !text.contains("工具输出"),
        "tool result body must NOT be indexed"
    );
    assert!(
        !text.contains("模型在思考"),
        "thinking text must NOT be indexed"
    );
}

#[test]
fn indexable_content_retains_complete_dialogue() {
    use crate::models::{Message, MessageRole};

    // Long dialogue with a unique marker near the end. Global search should
    // index the full message text, not a capped prefix.
    let filler = "搜索内容 ".repeat(8000);
    let marker = "悬停飞出标记";
    let messages = vec![Message {
        role: MessageRole::Assistant,
        message_kind: None,
        content: format!("{filler}{marker}"),
        timestamp: None,
        tool_name: None,
        tool_input: None,
        tool_metadata: None,
        token_usage: None,
        model: None,
        usage_hash: None,
    }];

    let text = super::indexable_content_text(&messages, "");
    assert!(
        text.len() > 64 * 1024,
        "dialogue beyond the old 64 KiB cap must now be retained"
    );
    assert!(
        text.contains(marker),
        "a marker past the old cap must be indexable for global search"
    );
    assert_eq!(text.len(), filler.len() + marker.len());
}
