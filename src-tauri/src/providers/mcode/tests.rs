use super::*;
use crate::models::MessageRole;
use crate::provider::ProviderDescriptor;
use std::fs;

fn write_minimal_db(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        r#"
            CREATE TABLE local_runtime_sessions (
                session_id TEXT PRIMARY KEY,
                record_json TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                columnar_version INTEGER NOT NULL DEFAULT 0,
                agent_name TEXT, runtime TEXT, session_type TEXT,
                status TEXT, archived INTEGER NOT NULL DEFAULT 0,
                visibility TEXT NOT NULL DEFAULT 'visible',
                session_kind TEXT NOT NULL DEFAULT 'conversation',
                parent_session_id TEXT, workspace_dir TEXT,
                project_workspace_dir TEXT,
                is_default_workspace INTEGER NOT NULL DEFAULT 0,
                title TEXT, created_at_ms INTEGER,
                extra_data_json TEXT NOT NULL DEFAULT '{}',
                project_id INTEGER
            );
            "#,
    )
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
fn insert_session(
    path: &Path,
    id: &str,
    agent: &str,
    session_type: &str,
    parent: Option<&str>,
    title: Option<&str>,
    workspace: &str,
    updated_at_ms: i64,
    extra: &str,
) {
    let conn = Connection::open(path).unwrap();
    conn.execute(
        "INSERT INTO local_runtime_sessions
                (session_id, record_json, updated_at_ms, agent_name, runtime,
                 session_type, archived, visibility, session_kind,
                 parent_session_id, workspace_dir, project_workspace_dir,
                 is_default_workspace, title, created_at_ms, extra_data_json)
             VALUES (?1, '{}', ?2, ?3, 'pi-agent', ?4, 0, 'visible',
                     'conversation', ?5, ?6, ?6, 0, ?7, ?2, ?8)",
        rusqlite::params![
            id,
            updated_at_ms,
            agent,
            session_type,
            parent,
            workspace,
            title,
            extra
        ],
    )
    .unwrap();
}

fn write_messages(dir: &Path, name: &str, body: &str) {
    let sub = dir.join(name);
    fs::create_dir_all(&sub).unwrap();
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "sessionId": name,
        "paths": {
            "messages": sub.join("messages.jsonl").to_string_lossy().to_string()
        }
    })
    .to_string();
    fs::write(sub.join("manifest.json"), manifest).unwrap();
    fs::write(sub.join("messages.jsonl"), body).unwrap();
}

fn known_state(parsed: &ParsedSession) -> HashMap<String, SourceState> {
    HashMap::from([(
        parsed.meta.source_path.clone(),
        SourceState {
            size: parsed.meta.file_size_bytes,
            mtime: parsed.source_mtime,
            title: Some(parsed.meta.title.clone()),
        },
    )])
}

#[test]
fn resume_command_includes_session_id() {
    let descriptor = Descriptor;
    assert_eq!(
        descriptor.resume_command("mvs_abc", None),
        Some("mcode --session mvs_abc".to_string())
    );
}

#[test]
fn descriptor_static_metadata() {
    let descriptor = Descriptor;
    assert_eq!(descriptor.display_key(None), "mcode");
    assert_eq!(descriptor.sort_order(), 13);
    assert_eq!(descriptor.cli_command(), "mcode");
    assert!(descriptor.color().starts_with('#'));
}

#[test]
fn scan_all_returns_empty_when_db_missing() {
    let dir = tempfile::tempdir().unwrap();
    let provider = McodeProvider::with_data_root(dir.path().to_path_buf());
    let sessions = provider.scan_all().unwrap();
    assert!(sessions.is_empty());
}

#[test]
fn scan_all_indexes_user_sessions_and_skips_hidden() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("sqlite").join("runtime-state.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    write_minimal_db(&db);

    // Two visible sessions; one is a `peek` bookkeeping row that
    // must NOT show up. session_kind is the right filter.
    insert_session(
        &db,
        "mvs_user1",
        "mavis",
        "root",
        None,
        Some("list /tmp"),
        "/private/tmp",
        1_787_049_058_444,
        r#"{"effectiveModel":"minimax/MiniMax-M3","effectiveModelVariant":"thinking"}"#,
    );
    insert_session(
        &db,
        "mvs_sub1",
        "worker",
        "branch",
        Some("mvs_user1"),
        Some("Main"),
        "/private/tmp",
        1_787_049_060_000,
        "{}",
    );
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO local_runtime_sessions
                (session_id, record_json, updated_at_ms, agent_name, runtime,
                 session_type, archived, visibility, session_kind,
                 parent_session_id, workspace_dir, project_workspace_dir,
                 is_default_workspace, title, created_at_ms, extra_data_json)
             VALUES ('mvs_bookkeeping', '{}', 1, 'mavis', 'pi-agent', 'root',
                     0, 'visible', 'peek', NULL, '/tmp', '/tmp', 0,
                     'Bookkeeping', 1, '{}')",
        [],
    )
    .unwrap();
    // Scaffolding the CLI writes at first launch — hidden by origin.
    conn.execute(
        "INSERT INTO local_runtime_sessions
                (session_id, record_json, updated_at_ms, agent_name, runtime,
                 session_type, archived, visibility, session_kind,
                 parent_session_id, workspace_dir, project_workspace_dir,
                 is_default_workspace, title, created_at_ms, extra_data_json)
             VALUES ('mvs_repair', '{}', 2, 'explore', 'pi-agent', 'root',
                     0, 'visible', 'conversation', NULL, '/tmp', '/tmp', 0,
                     'Main', 2, '{\"origin\":\"root-repair\"}')",
        [],
    )
    .unwrap();
    // Real delegated child: session_kind='task' must stay visible.
    conn.execute(
        "INSERT INTO local_runtime_sessions
                (session_id, record_json, updated_at_ms, agent_name, runtime,
                 session_type, archived, visibility, session_kind,
                 parent_session_id, workspace_dir, project_workspace_dir,
                 is_default_workspace, title, created_at_ms, extra_data_json)
             VALUES ('mvs_task1', '{}', 3, 'explore', 'pi-agent', 'branch',
                     0, 'visible', 'task', 'mvs_user1', '/private/tmp',
                     '/private/tmp', 0, 'Inspect workspace', 3, '{}')",
        [],
    )
    .unwrap();

    // Write minimal jsonl payloads so message_count and file_size work.
    let sessions_dir = root.path().join("sessions");
    write_messages(
        &sessions_dir,
        "mvs_user1",
        "{\"message_id\":\"u1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"<system-reminder>ctx</system-reminder>\\nhi\"}],\"canonicalTextRange\":{\"startOffset\":39,\"endOffset\":41},\"timestamp\":1}}\n{\"message_id\":\"a1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}],\"model\":\"MiniMax-M3\",\"usage\":{\"input\":7,\"output\":3,\"cacheRead\":2,\"cacheWrite\":0},\"timestamp\":1000}}\n",
    );
    write_messages(&sessions_dir, "mvs_sub1", "");
    write_messages(&sessions_dir, "mvs_task1", "");

    let provider = McodeProvider::with_data_root(root.path().to_path_buf());
    let parsed = provider.scan_all().unwrap();
    assert_eq!(
        parsed.len(),
        3,
        "peek and root-repair filtered; task child kept"
    );
    assert!(parsed.iter().all(|p| p.meta.id != "mvs_repair"));
    assert!(parsed.iter().all(|p| p.meta.id != "mvs_bookkeeping"));

    let user = parsed.iter().find(|p| p.meta.id == "mvs_user1").unwrap();
    assert_eq!(user.meta.provider, Provider::Mcode);
    assert_eq!(user.meta.title, "list /tmp");
    assert_eq!(user.meta.project_path, "/private/tmp");
    assert!(!user.meta.is_sidechain, "root session is not a sidechain");
    assert_eq!(user.meta.model.as_deref(), Some("minimax/MiniMax-M3"));
    assert_eq!(user.meta.message_count, 2);
    assert_eq!(user.content_text, "hi\nhello");
    assert_eq!(user.usage_events.len(), 1);
    assert_eq!(user.usage_events[0].input_tokens, 7);
    assert_eq!(user.usage_events[0].cache_read_input_tokens, 2);
    assert_eq!(user.meta.input_tokens, 7);
    assert_eq!(user.meta.output_tokens, 3);
    assert_eq!(user.meta.cache_read_tokens, 2);
    assert_eq!(user.meta.variant_name.as_deref(), Some("mavis"));
    assert_eq!(
        user.child_session_ids,
        vec!["mvs_sub1".to_string(), "mvs_task1".to_string()]
    );

    let sub = parsed.iter().find(|p| p.meta.id == "mvs_sub1").unwrap();
    assert!(sub.meta.is_sidechain, "branch+parent → sidechain");
    assert_eq!(sub.meta.parent_id.as_deref(), Some("mvs_user1"));
    assert_eq!(sub.meta.variant_name.as_deref(), Some("worker"));

    let task = parsed.iter().find(|p| p.meta.id == "mvs_task1").unwrap();
    assert!(task.meta.is_sidechain);
    assert_eq!(task.meta.parent_id.as_deref(), Some("mvs_user1"));
    assert_eq!(task.meta.variant_name.as_deref(), Some("explore"));
    assert_eq!(task.meta.title, "Inspect workspace");
}

#[test]
fn load_messages_reads_jsonl_payload() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("sqlite").join("runtime-state.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    write_minimal_db(&db);
    insert_session(
        &db,
        "mvs_xyz",
        "mavis",
        "root",
        None,
        Some("hi"),
        "/tmp",
        1,
        "{}",
    );

    let session_dir = root.path().join("sessions").join("mvs_xyz");
    fs::create_dir_all(&session_dir).unwrap();
    let body = "{\"message_id\":\"u1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1000}}\n\
                    {\"message_id\":\"a1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}],\"model\":\"MiniMax-M3\",\"usage\":{\"input\":7,\"output\":3,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":10},\"timestamp\":2000}}\n";
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "sessionId": "mvs_xyz",
        "paths": {
            "messages": session_dir.join("messages.jsonl").to_string_lossy().to_string()
        }
    })
    .to_string();
    fs::write(session_dir.join("manifest.json"), manifest).unwrap();
    fs::write(session_dir.join("messages.jsonl"), body).unwrap();

    let provider = McodeProvider::with_data_root(root.path().to_path_buf());
    // Force path-map build via scan_all
    let _ = provider.scan_all().unwrap();
    let loaded = provider
        .load_messages("mvs_xyz", "ignored")
        .expect("load ok");
    assert_eq!(loaded.messages.len(), 2);
    let assistant = &loaded.messages[1];
    assert_eq!(assistant.role, MessageRole::Assistant);
    assert_eq!(assistant.content, "hello");
    assert_eq!(assistant.model.as_deref(), Some("MiniMax-M3"));
    assert_eq!(assistant.token_usage.as_ref().unwrap().input_tokens, 7);
    assert_eq!(loaded.token_totals.input_tokens, 7);
    assert_eq!(loaded.token_totals.output_tokens, 3);
}

#[test]
fn load_messages_resolves_dated_session_dir_without_scan() {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let root = tempfile::tempdir().unwrap();
    let session_id = "mvs_dated1";
    let encoded = URL_SAFE_NO_PAD.encode(session_id.as_bytes());
    let session_dir = root
        .path()
        .join("sessions")
        .join("2026")
        .join("08")
        .join("18")
        .join(format!("18-30-38-471-session_{encoded}"));
    fs::create_dir_all(&session_dir).unwrap();
    let body = "{\"message_id\":\"u1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1}}\n";
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "sessionId": session_id,
        "paths": {
            "messages": session_dir.join("messages.jsonl").to_string_lossy().to_string()
        }
    })
    .to_string();
    fs::write(session_dir.join("manifest.json"), manifest).unwrap();
    fs::write(session_dir.join("messages.jsonl"), body).unwrap();

    let provider = McodeProvider::with_data_root(root.path().to_path_buf());
    let loaded = provider
        .load_messages(session_id, "ignored")
        .expect("load without scan_all");
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(loaded.messages[0].content, "hi");
}

#[test]
fn scan_incremental_tracks_manifest_and_jsonl_changes() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("sqlite").join("runtime-state.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    write_minimal_db(&db);
    insert_session(
        &db,
        "mvs_user1",
        "mavis",
        "root",
        None,
        Some("hi"),
        "/tmp",
        1,
        "{}",
    );
    write_messages(
        &root.path().join("sessions"),
        "mvs_user1",
        "{\"message_id\":\"u1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1}}\n",
    );

    let provider = McodeProvider::with_data_root(root.path().to_path_buf());
    let first = provider.scan_incremental(&HashMap::new()).unwrap().parsed;
    assert_eq!(first.len(), 1);
    let sources = provider.discover_session_sources();
    let source_state = provider.source_graph_state(&sources).unwrap();
    assert_eq!(first[0].meta.file_size_bytes, source_state.size);
    assert_eq!(first[0].source_mtime, source_state.mtime);

    let second = provider.scan_incremental(&known_state(&first[0])).unwrap();
    assert!(
        second.parsed.is_empty(),
        "unchanged source graph must not reparse"
    );
    assert_eq!(
        second.unchanged_source_paths,
        vec![provider.db_path().to_string_lossy().to_string()]
    );

    Connection::open(&db)
            .unwrap()
            .execute(
                "UPDATE local_runtime_sessions SET title = 'db changed', updated_at_ms = 2 WHERE session_id = 'mvs_user1'",
                [],
            )
            .unwrap();
    let db_changed = provider
        .scan_incremental(&known_state(&first[0]))
        .unwrap()
        .parsed;
    assert_eq!(db_changed.len(), 1);
    assert_eq!(db_changed[0].meta.title, "db changed");

    let session_dir = root.path().join("sessions").join("mvs_user1");
    let manifest_path = session_dir.join("manifest.json");
    let mut manifest = fs::read_to_string(&manifest_path).unwrap();
    manifest.push('\n');
    fs::write(&manifest_path, manifest).unwrap();
    let manifest_changed = provider
        .scan_incremental(&known_state(&db_changed[0]))
        .unwrap()
        .parsed;
    assert_eq!(manifest_changed.len(), 1);

    let messages_path = session_dir.join("messages.jsonl");
    let mut messages = fs::read_to_string(&messages_path).unwrap();
    messages.push_str(
            "{\"message_id\":\"a2\",\"turn_id\":\"t2\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"updated\"}],\"model\":\"MiniMax-M3\",\"timestamp\":2}}\n",
        );
    fs::write(&messages_path, messages).unwrap();
    let jsonl_changed = provider
        .scan_incremental(&known_state(&manifest_changed[0]))
        .unwrap()
        .parsed;
    assert_eq!(jsonl_changed.len(), 1);
    assert_eq!(jsonl_changed[0].meta.message_count, 2);
}

#[test]
fn scan_incremental_tracks_wal_only_changes() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("sqlite").join("runtime-state.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    write_minimal_db(&db);
    insert_session(
        &db,
        "mvs_user1",
        "mavis",
        "root",
        None,
        Some("before"),
        "/tmp",
        1,
        "{}",
    );
    write_messages(
        &root.path().join("sessions"),
        "mvs_user1",
        "{\"message_id\":\"u1\",\"turn_id\":\"t1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"timestamp\":1}}\n",
    );

    let writer = Connection::open(&db).unwrap();
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    writer
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();

    let provider = McodeProvider::with_data_root(root.path().to_path_buf());
    let first = provider.scan_incremental(&HashMap::new()).unwrap().parsed;
    assert_eq!(first.len(), 1);
    let db_before = fs::metadata(&db).unwrap();
    let db_mtime_before = db_before.modified().unwrap();

    writer
            .execute(
                "UPDATE local_runtime_sessions SET title = 'after', updated_at_ms = 2 WHERE session_id = 'mvs_user1'",
                [],
            )
            .unwrap();
    let wal_path = PathBuf::from(format!("{}-wal", db.to_string_lossy()));
    assert!(fs::metadata(&wal_path).unwrap().len() > 0);
    let db_after = fs::metadata(&db).unwrap();
    assert_eq!(db_before.len(), db_after.len());
    assert_eq!(db_mtime_before, db_after.modified().unwrap());

    let changed = provider
        .scan_incremental(&known_state(&first[0]))
        .unwrap()
        .parsed;
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].meta.title, "after");
}

#[test]
fn resolve_data_root_prefers_minimax_env() {
    assert_eq!(
        resolve_data_root(
            Some("/custom/minimax".into()),
            Some("/legacy/mavis".into()),
            Some(PathBuf::from("/home/u")),
        ),
        Some(PathBuf::from("/custom/minimax/v2"))
    );
    assert_eq!(
        resolve_data_root(
            None,
            Some("/legacy/mavis".into()),
            Some(PathBuf::from("/home/u"))
        ),
        Some(PathBuf::from("/legacy/mavis/v2"))
    );
    assert_eq!(
        resolve_data_root(None, None, Some(PathBuf::from("/home/u"))),
        Some(PathBuf::from("/home/u/.minimax/v2"))
    );
}

/// End-to-end smoke test against the real mcode runtime state on this
/// machine. Skipped when `~/.minimax/v2/` is absent so the test stays
/// green in CI / fresh checkouts. Run with:
///   cargo test --lib mcode::tests::smoke_against_real_runtime -- --ignored
#[test]
#[ignore = "hits the real ~/.minimax/v2/ tree; run with --ignored"]
fn smoke_against_real_runtime() {
    let Some(provider) = McodeProvider::new() else {
        return;
    };
    if provider.source_roots().is_empty() {
        return;
    }
    let sessions = provider
        .scan_all()
        .unwrap_or_else(|_| panic!("mcode real-data scan failed"));
    assert!(
        sessions
            .iter()
            .any(|s| !s.meta.is_sidechain && s.meta.message_count > 0),
        "expected at least one non-sidechain user session with messages"
    );
    for parsed in &sessions {
        assert_eq!(parsed.meta.provider, Provider::Mcode);
        assert!(!parsed.meta.id.is_empty());
        assert!(!parsed.meta.source_path.is_empty());
        if parsed.meta.is_sidechain {
            assert!(parsed.meta.parent_id.is_some());
        }
    }
    if let Some(first) = sessions
        .iter()
        .find(|session| session.meta.message_count > 0)
    {
        let fresh = McodeProvider::new().unwrap();
        let loaded = fresh
            .load_messages(&first.meta.id, "ignored")
            .unwrap_or_else(|_| panic!("mcode real-data load failed"));
        assert_eq!(loaded.messages.len(), first.meta.message_count as usize);
        assert_eq!(loaded.token_totals.input_tokens, first.meta.input_tokens);
        assert_eq!(loaded.token_totals.output_tokens, first.meta.output_tokens);
    }
}
