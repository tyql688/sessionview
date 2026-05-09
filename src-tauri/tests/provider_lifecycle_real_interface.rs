#![cfg(not(target_os = "windows"))]

//! Manual real-provider smoke test.
//!
//! This test generates fresh local sessions via the real provider CLIs, then
//! exercises the Tauri command interface end-to-end against an isolated temp DB:
//! rebuild -> snapshots -> recent/detail -> trash -> restore -> delete.
//!
//! It is ignored by default because it depends on locally installed/authenticated
//! CLIs and will create then delete real local provider session data.
//!
//! Run manually:
//! `cargo test --test provider_lifecycle_real_interface -- --ignored --nocapture`

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use cc_session_lib::commands::{self, AppState};
use cc_session_lib::db::Database;
use cc_session_lib::indexer::Indexer;
use cc_session_lib::models::{Provider, ProviderSnapshot, TrashMeta, TreeNode};
use cc_session_lib::provider::{self, WatchStrategy};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{
    get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY,
};
use tauri::webview::InvokeRequest;
use tauri::{App, Webview, WebviewWindowBuilder};
use tempfile::TempDir;

#[derive(Debug, Clone, Deserialize)]
struct SmokeSessionMeta {
    id: String,
    provider: Provider,
    title: String,
    project_name: String,
    project_path: String,
    source_path: String,
    #[serde(default)]
    variant_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SmokeSessionDetail {
    meta: SmokeSessionMeta,
    #[serde(default)]
    messages: Vec<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct SessionExpectation {
    provider: Provider,
    marker: String,
    expected_variant: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MirrorVariantMeta {
    name: Option<String>,
}

fn build_app() -> (TempDir, App<MockRuntime>, tauri::WebviewWindow<MockRuntime>) {
    let temp_dir = TempDir::new().expect("temp dir");
    let db = Arc::new(Database::open(temp_dir.path()).expect("open temp db"));
    let data_dir = temp_dir.path().to_path_buf();
    let indexer = Indexer::new(Arc::clone(&db), provider::all_runtimes(), data_dir);
    let state = AppState {
        db,
        indexer,
        maintenance_running: Arc::new(AtomicBool::new(false)),
        session_cache: Arc::new(cc_session_lib::services::SessionCache::new(4)),
        persisted_output_cache: Arc::new(cc_session_lib::services::PersistedOutputCache::new(
            4,
            1024 * 1024,
        )),
        load_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        loading_paths: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
    };

    let app = mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_provider_snapshots,
            commands::reindex,
            commands::get_tree,
            commands::list_recent_sessions,
            commands::get_session_detail,
            commands::trash_session,
            commands::list_trash,
            commands::restore_session,
            commands::permanent_delete_trash,
            commands::delete_session,
        ])
        .build(mock_context(noop_assets()))
        .expect("build test app");

    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build test webview");

    (temp_dir, app, webview)
}

fn invoke<T: DeserializeOwned, W: AsRef<Webview<MockRuntime>>>(
    webview: &W,
    cmd: &str,
    body: serde_json::Value,
) -> Result<T, serde_json::Value> {
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().expect("invoke url"),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|payload| payload.deserialize::<T>().expect("deserialize response"))
}

fn list_recent<W: AsRef<Webview<MockRuntime>>>(webview: &W) -> Vec<SmokeSessionMeta> {
    invoke(webview, "list_recent_sessions", json!({ "limit": 5000 })).expect("list recent")
}

fn list_trash_entries<W: AsRef<Webview<MockRuntime>>>(webview: &W) -> Vec<TrashMeta> {
    invoke(webview, "list_trash", json!({})).expect("list trash")
}

fn create_workspace(root: &Path, marker: &str) -> PathBuf {
    let path = root.join(marker);
    std::fs::create_dir_all(&path).expect("create workspace dir");
    path
}

fn run_cli(program: &str, args: &[String], current_dir: &Path) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(current_dir)
        .output()
        .unwrap_or_else(|error| panic!("failed to run {program}: {error}"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("== {program} ==");
    eprintln!("cwd: {}", current_dir.display());
    eprintln!("stdout:\n{stdout}");
    if !stderr.trim().is_empty() {
        eprintln!("stderr:\n{stderr}");
    }

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} failed with status {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status
        ))
    }
}

fn should_skip_provider(error: &str) -> bool {
    error.contains("You're out of usage")
        || error.contains("No auth method configured")
        || error.contains("Please authenticate")
        || error.contains("Please login")
        || error.contains("oauth")
}

fn sanitize_cc_mirror_command(raw: &str) -> String {
    raw.chars()
        .filter(|ch| ch.is_alphanumeric() || *ch == '-' || *ch == '_')
        .collect()
}

fn discover_cc_mirror_commands() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mirror_root = home.join(".cc-mirror");
    let Ok(entries) = std::fs::read_dir(&mirror_root) else {
        return Vec::new();
    };

    let mut commands = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }

        let dir_name = dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(sanitize_cc_mirror_command)
            .unwrap_or_default();
        if dir_name.is_empty() {
            continue;
        }

        let command = std::fs::read_to_string(dir.join("variant.json"))
            .ok()
            .and_then(|content| serde_json::from_str::<MirrorVariantMeta>(&content).ok())
            .and_then(|meta| meta.name)
            .map(|name| sanitize_cc_mirror_command(&name))
            .filter(|name| !name.is_empty())
            .unwrap_or(dir_name);

        if !commands.contains(&command) {
            commands.push(command);
        }
    }

    commands
}

fn generate_real_sessions(root: &Path, run_id: &str) -> (Vec<SessionExpectation>, Vec<String>) {
    let claude_marker = format!("ccsession-real-{run_id}-claude");
    let codex_marker = format!("ccsession-real-{run_id}-codex");
    let opencode_marker = format!("ccsession-real-{run_id}-opencode");
    let gemini_marker = format!("ccsession-real-{run_id}-gemini");
    let kimi_marker = format!("ccsession-real-{run_id}-kimi");
    let qwen_marker = format!("ccsession-real-{run_id}-qwen");
    let cc_mirror_restore_marker = format!("ccsession-real-{run_id}-cc-mirror-restore");
    let cc_mirror_delete_marker = format!("ccsession-real-{run_id}-cc-mirror-delete");
    let cc_mirror_purge_marker = format!("ccsession-real-{run_id}-cc-mirror-purge");

    let claude_dir = create_workspace(root, &claude_marker);
    let codex_dir = create_workspace(root, &codex_marker);
    let opencode_dir = create_workspace(root, &opencode_marker);
    let gemini_dir = create_workspace(root, &gemini_marker);
    let kimi_dir = create_workspace(root, &kimi_marker);
    let qwen_dir = create_workspace(root, &qwen_marker);
    let cc_mirror_restore_dir = create_workspace(root, &cc_mirror_restore_marker);
    let cc_mirror_delete_dir = create_workspace(root, &cc_mirror_delete_marker);
    let cc_mirror_purge_dir = create_workspace(root, &cc_mirror_purge_marker);

    let markers = vec![
        claude_marker.clone(),
        codex_marker.clone(),
        opencode_marker.clone(),
        gemini_marker.clone(),
        kimi_marker.clone(),
        qwen_marker.clone(),
        cc_mirror_restore_marker.clone(),
        cc_mirror_delete_marker.clone(),
        cc_mirror_purge_marker.clone(),
    ];

    let mut expectations = Vec::new();
    let mut register = |label: &str,
                        provider: Provider,
                        marker: String,
                        expected_variant: Option<String>,
                        result: Result<(), String>| {
        match result {
            Ok(()) => expectations.push(SessionExpectation {
                provider,
                marker,
                expected_variant,
            }),
            Err(error) if should_skip_provider(&error) => {
                eprintln!("SKIP {label}: {error}");
            }
            Err(error) => panic!("{label} generation failed: {error}"),
        }
    };

    register(
        "claude",
        Provider::Claude,
        claude_marker.clone(),
        None,
        run_cli(
            "claude",
            &[
                "-p".into(),
                "--output-format".into(),
                "json".into(),
                "--permission-mode".into(),
                "dontAsk".into(),
                "--tools".into(),
                String::new(),
                "-n".into(),
                format!("smoke-{run_id}-claude"),
                format!("{claude_marker} Reply with OK only."),
            ],
            &claude_dir,
        ),
    );

    register(
        "codex",
        Provider::Codex,
        codex_marker.clone(),
        None,
        run_cli(
            "codex",
            &[
                "exec".into(),
                "--skip-git-repo-check".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--json".into(),
                format!("{codex_marker} Reply with OK only."),
            ],
            &codex_dir,
        ),
    );

    register(
        "opencode",
        Provider::OpenCode,
        opencode_marker.clone(),
        None,
        run_cli(
            "opencode",
            &[
                "run".into(),
                "--dir".into(),
                opencode_dir.to_string_lossy().to_string(),
                "--format".into(),
                "json".into(),
                format!("{opencode_marker} Reply with OK only."),
            ],
            root,
        ),
    );

    register(
        "gemini",
        Provider::Gemini,
        gemini_marker.clone(),
        None,
        run_cli(
            "gemini",
            &[
                "--prompt".into(),
                format!("{gemini_marker} Reply with OK only."),
                "--approval-mode".into(),
                "yolo".into(),
                "--output-format".into(),
                "json".into(),
            ],
            &gemini_dir,
        ),
    );

    register(
        "kimi",
        Provider::Kimi,
        kimi_marker.clone(),
        None,
        run_cli(
            "kimi",
            &[
                "--print".into(),
                "-m".into(),
                "moonshot-cn/kimi-k2.5".into(),
                "--prompt".into(),
                format!("{kimi_marker} Reply with OK only."),
            ],
            &kimi_dir,
        ),
    );

    register(
        "qwen",
        Provider::Qwen,
        qwen_marker.clone(),
        None,
        run_cli(
            "qwen",
            &[
                "--approval-mode".into(),
                "yolo".into(),
                format!("{qwen_marker} Reply with OK only."),
            ],
            &qwen_dir,
        ),
    );

    let cc_mirror_commands = discover_cc_mirror_commands();
    let cc_mirror_restore_command = cc_mirror_commands.first().cloned();
    let cc_mirror_delete_command = cc_mirror_commands
        .get(1)
        .cloned()
        .or_else(|| cc_mirror_restore_command.clone());
    let cc_mirror_purge_command = cc_mirror_commands
        .get(2)
        .cloned()
        .or_else(|| cc_mirror_delete_command.clone())
        .or_else(|| cc_mirror_restore_command.clone());

    if let Some(command) = cc_mirror_restore_command {
        register(
            "cc-mirror-restore",
            Provider::CcMirror,
            cc_mirror_restore_marker.clone(),
            Some(command.clone()),
            run_cli(
                &command,
                &[
                    "-p".into(),
                    "--output-format".into(),
                    "json".into(),
                    "--permission-mode".into(),
                    "dontAsk".into(),
                    "--tools".into(),
                    String::new(),
                    "-n".into(),
                    format!("smoke-{run_id}-cc-mirror-restore"),
                    format!("{cc_mirror_restore_marker} Reply with OK only."),
                ],
                &cc_mirror_restore_dir,
            ),
        );
    } else {
        eprintln!("SKIP cc-mirror-restore: no discovered variants");
    }

    if let Some(command) = cc_mirror_delete_command {
        register(
            "cc-mirror-delete",
            Provider::CcMirror,
            cc_mirror_delete_marker.clone(),
            Some(command.clone()),
            run_cli(
                &command,
                &[
                    "-p".into(),
                    "--output-format".into(),
                    "json".into(),
                    "--permission-mode".into(),
                    "dontAsk".into(),
                    "--tools".into(),
                    String::new(),
                    "-n".into(),
                    format!("smoke-{run_id}-cc-mirror-delete"),
                    format!("{cc_mirror_delete_marker} Reply with OK only."),
                ],
                &cc_mirror_delete_dir,
            ),
        );
    } else {
        eprintln!("SKIP cc-mirror-delete: no discovered variants");
    }

    if let Some(command) = cc_mirror_purge_command {
        register(
            "cc-mirror-purge",
            Provider::CcMirror,
            cc_mirror_purge_marker.clone(),
            Some(command.clone()),
            run_cli(
                &command,
                &[
                    "-p".into(),
                    "--output-format".into(),
                    "json".into(),
                    "--permission-mode".into(),
                    "dontAsk".into(),
                    "--tools".into(),
                    String::new(),
                    "-n".into(),
                    format!("smoke-{run_id}-cc-mirror-purge"),
                    format!("{cc_mirror_purge_marker} Reply with OK only."),
                ],
                &cc_mirror_purge_dir,
            ),
        );
    } else {
        eprintln!("SKIP cc-mirror-purge: no discovered variants");
    }

    (expectations, markers)
}

fn session_matches_marker(session: &SmokeSessionMeta, marker: &str) -> bool {
    session.title.contains(marker)
        || session.project_name.contains(marker)
        || session.project_path.contains(marker)
        || session.source_path.contains(marker)
}

fn trash_matches_marker(entry: &TrashMeta, marker: &str) -> bool {
    entry.title.contains(marker)
        || entry.project_name.contains(marker)
        || entry.original_path.contains(marker)
}

fn find_session_by_marker(
    sessions: &[SmokeSessionMeta],
    expectation: &SessionExpectation,
) -> SmokeSessionMeta {
    sessions
        .iter()
        .find(|session| {
            session.provider == expectation.provider
                && session_matches_marker(session, &expectation.marker)
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "missing {:?} session for marker {}",
                expectation.provider, expectation.marker
            )
        })
}

fn find_session_by_id(sessions: &[SmokeSessionMeta], session_id: &str) -> Option<SmokeSessionMeta> {
    sessions
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
}

fn assert_detail<W: AsRef<Webview<MockRuntime>>>(
    webview: &W,
    session_id: &str,
    provider: Provider,
    variant_name: Option<&str>,
) {
    let detail: SmokeSessionDetail = invoke(
        webview,
        "get_session_detail",
        json!({ "sessionId": session_id }),
    )
    .unwrap_or_else(|error| panic!("detail read failed for {session_id}: {error}"));
    assert_eq!(detail.meta.provider, provider);
    assert_eq!(detail.meta.variant_name.as_deref(), variant_name);
    assert!(!detail.messages.is_empty(), "expected non-empty messages");
}

fn trash_and_restore<W: AsRef<Webview<MockRuntime>>>(
    webview: &W,
    session: &SmokeSessionMeta,
    expected_variant: Option<&str>,
) {
    invoke::<(), _>(webview, "trash_session", json!({ "sessionId": session.id }))
        .unwrap_or_else(|error| panic!("trash failed for {}: {error}", session.id));

    let trash_entries = list_trash_entries(webview);
    let trash_entry = trash_entries
        .iter()
        .find(|entry| entry.id == session.id)
        .unwrap_or_else(|| panic!("missing trash entry for {}", session.id));
    assert_eq!(trash_entry.project_name, session.project_name);
    assert_eq!(trash_entry.title, session.title);

    assert!(
        invoke::<SmokeSessionDetail, _>(
            webview,
            "get_session_detail",
            json!({ "sessionId": session.id })
        )
        .is_err(),
        "trashed session should disappear from index"
    );

    invoke::<(), _>(webview, "restore_session", json!({ "trashId": session.id }))
        .unwrap_or_else(|error| panic!("restore failed for {}: {error}", session.id));

    assert!(
        list_trash_entries(webview)
            .iter()
            .all(|entry| entry.id != session.id),
        "restored session should be removed from trash"
    );

    let recent = list_recent(webview);
    let restored = find_session_by_id(&recent, &session.id)
        .unwrap_or_else(|| panic!("restored session missing from recent list: {}", session.id));
    assert_eq!(restored.project_name, session.project_name);
    assert_detail(
        webview,
        &session.id,
        session.provider.clone(),
        expected_variant,
    );
}

fn delete_session_via_command<W: AsRef<Webview<MockRuntime>>>(webview: &W, session_id: &str) {
    invoke::<(), _>(
        webview,
        "delete_session",
        json!({ "sessionId": session_id }),
    )
    .unwrap_or_else(|error| panic!("delete failed for {session_id}: {error}"));
    assert!(
        invoke::<SmokeSessionDetail, _>(
            webview,
            "get_session_detail",
            json!({ "sessionId": session_id })
        )
        .is_err(),
        "deleted session should be unavailable"
    );
}

fn cleanup_generated_sessions<W: AsRef<Webview<MockRuntime>>>(
    webview: &W,
    markers: &[String],
    known_ids: &[String],
) {
    let _ = invoke::<usize, _>(webview, "reindex", json!({}));

    let matching_recent: Vec<String> = list_recent(webview)
        .into_iter()
        .filter(|session| {
            known_ids.contains(&session.id)
                || markers
                    .iter()
                    .any(|marker| session_matches_marker(session, marker))
        })
        .map(|session| session.id)
        .collect();

    for session_id in matching_recent {
        let _ = invoke::<(), _>(
            webview,
            "delete_session",
            json!({ "sessionId": session_id }),
        );
    }

    for entry in list_trash_entries(webview).into_iter().filter(|entry| {
        known_ids.contains(&entry.id)
            || markers
                .iter()
                .any(|marker| trash_matches_marker(entry, marker))
    }) {
        let _ = invoke::<(), _>(
            webview,
            "permanent_delete_trash",
            json!({ "trashId": entry.id }),
        );
    }
}

fn assert_provider_snapshots<W: AsRef<Webview<MockRuntime>>>(webview: &W) {
    let snapshots: Vec<ProviderSnapshot> =
        invoke(webview, "get_provider_snapshots", json!({})).expect("provider snapshots");
    let keys: Vec<Provider> = snapshots
        .iter()
        .map(|snapshot| snapshot.key.clone())
        .collect();

    assert_eq!(
        keys,
        vec![
            Provider::Claude,
            Provider::CcMirror,
            Provider::Codex,
            Provider::Gemini,
            Provider::OpenCode,
            Provider::Kimi,
            Provider::Qwen,
        ]
    );

    let cc_mirror = snapshots
        .iter()
        .find(|snapshot| snapshot.key == Provider::CcMirror)
        .expect("cc-mirror snapshot");
    assert!(
        cc_mirror.path.contains(".cc-mirror"),
        "cc-mirror snapshot should point at the common root: {}",
        cc_mirror.path
    );

    let gemini = snapshots
        .iter()
        .find(|snapshot| snapshot.key == Provider::Gemini)
        .expect("gemini snapshot");
    let opencode = snapshots
        .iter()
        .find(|snapshot| snapshot.key == Provider::OpenCode)
        .expect("opencode snapshot");

    assert!(matches!(gemini.watch_strategy, WatchStrategy::Poll));
    assert!(matches!(opencode.watch_strategy, WatchStrategy::Poll));
}

#[test]
#[ignore]
fn real_provider_sessions_support_interface_semantics() {
    let (_temp_dir, _app, webview) = build_app();
    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_millis()
        .to_string();
    let workspaces = TempDir::new().expect("workspace temp dir");
    let (expectations, markers) = generate_real_sessions(workspaces.path(), &run_id);
    assert!(
        !expectations.is_empty(),
        "no providers generated sessions successfully"
    );
    let known_ids = Mutex::new(Vec::<String>::new());

    let result = catch_unwind(AssertUnwindSafe(|| {
        let indexed: usize = invoke(&webview, "reindex", json!({})).expect("reindex");
        assert!(indexed > 0, "expected sessions to be indexed");

        assert_provider_snapshots(&webview);

        let tree: Vec<TreeNode> = invoke(&webview, "get_tree", json!({})).expect("get_tree");
        assert!(!tree.is_empty(), "tree should not be empty after reindex");

        let recent = list_recent(&webview);
        let mut discovered = Vec::new();
        for expectation in &expectations {
            let session = find_session_by_marker(&recent, expectation);
            assert_eq!(session.project_name, expectation.marker);
            known_ids
                .lock()
                .expect("known ids lock")
                .push(session.id.clone());
            discovered.push((expectation.clone(), session));
        }

        for (expectation, session) in &discovered {
            assert_detail(
                &webview,
                &session.id,
                expectation.provider.clone(),
                expectation.expected_variant.as_deref(),
            );
        }

        for (expectation, session) in &discovered {
            match expectation.marker.as_str() {
                marker if marker.contains("cc-mirror-purge") => {}
                marker if marker.contains("cc-mirror-delete") => {}
                _ => trash_and_restore(&webview, session, expectation.expected_variant.as_deref()),
            }
        }

        let purge_target = discovered
            .iter()
            .find(|(expectation, _)| expectation.marker.contains("cc-mirror-purge"))
            .map(|(_, session)| session.clone())
            .expect("cc-mirror purge target");
        invoke::<(), _>(
            &webview,
            "trash_session",
            json!({ "sessionId": purge_target.id }),
        )
        .expect("trash purge target");
        let purge_entry = list_trash_entries(&webview)
            .into_iter()
            .find(|entry| entry.id == purge_target.id)
            .expect("purge target should appear in trash");
        assert_eq!(purge_entry.project_name, purge_target.project_name);
        invoke::<(), _>(
            &webview,
            "permanent_delete_trash",
            json!({ "trashId": purge_target.id }),
        )
        .expect("permanent delete purge target");
        assert!(
            list_trash_entries(&webview)
                .iter()
                .all(|entry| entry.id != purge_target.id),
            "purged target should be removed from trash"
        );

        let delete_target = discovered
            .iter()
            .find(|(expectation, _)| expectation.marker.contains("cc-mirror-delete"))
            .map(|(_, session)| session.clone())
            .expect("cc-mirror delete target");
        delete_session_via_command(&webview, &delete_target.id);

        let kimi = discovered
            .iter()
            .find(|(expectation, _)| expectation.provider == Provider::Kimi)
            .map(|(_, session)| session.clone());
        if let Some(kimi) = kimi {
            let kimi_dir = PathBuf::from(&kimi.source_path)
                .parent()
                .expect("kimi session dir")
                .to_path_buf();
            delete_session_via_command(&webview, &kimi.id);
            assert!(
                !kimi_dir.exists(),
                "kimi session dir should be removed on direct delete: {}",
                kimi_dir.display()
            );
        }

        let codex = discovered
            .iter()
            .find(|(expectation, _)| expectation.provider == Provider::Codex)
            .map(|(_, session)| session.clone())
            .expect("codex target");
        delete_session_via_command(&webview, &codex.id);
    }));

    cleanup_generated_sessions(
        &webview,
        &markers,
        &known_ids.lock().expect("known ids lock"),
    );

    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
