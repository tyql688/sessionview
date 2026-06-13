pub mod commands;
pub mod db;
pub mod error;
mod exporter;
pub mod indexer;
pub mod models;
pub mod pricing;
pub mod provider;
pub mod provider_utils;
pub mod providers;
pub mod services;
pub mod tool_metadata;
pub mod trash_state;
mod watcher;

use std::sync::Arc;

/// Test helpers — exposes private functions for integration tests.
#[doc(hidden)]
pub mod exporter_test_helpers {
    pub fn render_session_html_pub(detail: &crate::models::SessionDetail) -> String {
        crate::exporter::html::render(detail)
    }

    pub fn render_session_markdown_pub(detail: &crate::models::SessionDetail) -> String {
        crate::exporter::markdown::render(detail)
    }

    pub fn render_tool_detail_pub(tool_name: &str, tool_input: &str) -> String {
        crate::exporter::html::render_tool_detail(tool_name, tool_input)
    }
}

#[doc(hidden)]
pub mod command_test_helpers {
    use crate::commands::{get_resume_command_for_tests, load_session_detail_for_tests};
    use crate::db::Database;
    use crate::models::{ProviderSnapshot, SessionDetail, TrashMeta};
    use crate::services::{ProviderSnapshotService, SessionLifecycleService};

    pub fn get_session_detail(db: &Database, session_id: &str) -> Result<SessionDetail, String> {
        load_session_detail_for_tests(db, session_id)
    }

    pub fn get_provider_snapshots(db: &Database) -> Result<Vec<ProviderSnapshot>, String> {
        ProviderSnapshotService::new(db)
            .list()
            .map_err(|e| e.to_string())
    }

    pub fn get_resume_command(db: &Database, session_id: &str) -> Result<String, String> {
        get_resume_command_for_tests(db, session_id)
    }

    pub fn trash_session(db: &Database, session_id: &str) -> Result<(), String> {
        SessionLifecycleService::new(db)
            .trash_session(session_id)
            .map_err(|e| e.to_string())
    }

    pub fn list_trash() -> Result<Vec<TrashMeta>, String> {
        SessionLifecycleService::list_trash().map_err(|e| e.to_string())
    }

    pub fn restore_session(db: &Database, trash_id: &str) -> Result<(), String> {
        SessionLifecycleService::new(db)
            .restore_session(trash_id)
            .map_err(|e| e.to_string())
    }

    pub fn delete_session(db: &Database, session_id: &str) -> Result<(), String> {
        SessionLifecycleService::new(db)
            .purge_session(session_id)
            .map_err(|e| e.to_string())
    }
}

use commands::AppState;
use db::Database;
use indexer::Indexer;
use tauri::Manager;

#[cfg(target_os = "macos")]
const MACOS_MIN_NOFILE_LIMIT: u64 = 65_536;
#[cfg(target_os = "macos")]
const MACOS_OPEN_MAX_COMPAT_LIMIT: u64 = 10_240;
#[cfg(target_os = "macos")]
const MACOS_RLIMIT_NOFILE: i32 = 8;
#[cfg(target_os = "macos")]
const MACOS_RLIM_INFINITY: u64 = i64::MAX as u64;

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct MacosRLimit {
    rlim_cur: u64,
    rlim_max: u64,
}

#[cfg(target_os = "macos")]
extern "C" {
    fn getrlimit(resource: i32, rlp: *mut MacosRLimit) -> i32;
    fn setrlimit(resource: i32, rlp: *const MacosRLimit) -> i32;
}

#[cfg(target_os = "macos")]
fn desired_nofile_soft_limit(current: u64, maximum: u64, target: u64) -> u64 {
    if current >= target {
        return current;
    }

    if maximum == MACOS_RLIM_INFINITY {
        return target;
    }

    target.min(maximum).max(current)
}

#[cfg(target_os = "macos")]
fn raise_file_descriptor_limit_for_macos_watchers() {
    let mut limit = MacosRLimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    // SAFETY: getrlimit/setrlimit are called with RLIMIT_NOFILE and a valid
    // MacosRLimit pointer matching Darwin's rlimit layout.
    unsafe {
        if getrlimit(MACOS_RLIMIT_NOFILE, &mut limit) != 0 {
            log::warn!(
                "failed to inspect macOS file descriptor limit: {}",
                std::io::Error::last_os_error()
            );
            return;
        }

        let original = limit;
        let next =
            desired_nofile_soft_limit(original.rlim_cur, original.rlim_max, MACOS_MIN_NOFILE_LIMIT);
        if next <= original.rlim_cur {
            return;
        }

        limit.rlim_cur = next;
        if setrlimit(MACOS_RLIMIT_NOFILE, &limit) == 0 {
            return;
        }

        let first_error = std::io::Error::last_os_error();
        let fallback = desired_nofile_soft_limit(
            original.rlim_cur,
            original.rlim_max,
            MACOS_OPEN_MAX_COMPAT_LIMIT,
        );
        if fallback > original.rlim_cur && fallback != next {
            limit.rlim_cur = fallback;
            if setrlimit(MACOS_RLIMIT_NOFILE, &limit) == 0 {
                log::warn!(
                    "raised macOS file descriptor limit to fallback {fallback} after {next} failed: {first_error}");
                return;
            }
        }

        log::warn!("failed to raise macOS file descriptor limit to {next}: {first_error}");
    }
}

#[cfg(not(target_os = "macos"))]
fn raise_file_descriptor_limit_for_macos_watchers() {}

/// Detect and fix inconsistencies left by interrupted trash operations.
/// Called once at app startup, after DB is opened.
fn audit_trash_consistency(db: &db::Database) {
    let Ok(trash_dir) = trash_state::trash_dir() else {
        return;
    };
    let entries = match services::SessionLifecycleService::list_trash() {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("trash audit: failed to list trash metadata: {e}");
            return;
        }
    };
    if entries.is_empty() {
        return;
    }

    for entry in &entries {
        // Auto-fix: session in both trash_meta AND DB → complete interrupted trash
        let session_exists_in_db = match db.get_session(&entry.id) {
            Ok(session) => session.is_some(),
            Err(e) => {
                log::warn!(
                    "trash audit: failed to query session {} in DB: {e}",
                    entry.id
                );
                false
            }
        };
        if session_exists_in_db {
            log::warn!(
                "trash audit: session {} found in both trash and DB — completing interrupted trash",
                entry.id
            );
            if let Err(e) = db.delete_session(&entry.id) {
                log::warn!(
                    "trash audit: failed to delete session {} from DB: {e}",
                    entry.id
                );
            }
        }

        // Log: trash file referenced but missing
        if !entry.trash_file.is_empty() {
            let trash_file_path = trash_dir.join(&entry.trash_file);
            if !trash_file_path.exists() {
                log::warn!(
                    "trash audit: session {} references missing trash file: {}",
                    entry.id,
                    entry.trash_file
                );
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    raise_file_descriptor_limit_for_macos_watchers();

    let data_dir = match dirs::data_local_dir() {
        Some(d) => d.join("cc-session"),
        None => {
            log::error!("failed to resolve local data dir");
            std::process::exit(1);
        }
    };

    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        log::error!("failed to create data dir: {e}");
        std::process::exit(1);
    }

    let db = match Database::open(&data_dir) {
        Ok(db) => Arc::new(db),
        Err(e) => {
            log::error!("failed to open database: {e}");
            std::process::exit(1);
        }
    };

    audit_trash_consistency(&db);

    let providers = provider::all_runtimes();

    let indexer = Indexer::new(Arc::clone(&db), providers, data_dir.clone());

    let state = AppState {
        db: Arc::clone(&db),
        indexer,
        maintenance_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        session_cache: Arc::new(crate::services::SessionCache::new(8)),
        // 16 entries / 32 MiB cap — covers a typical viewing burst without
        // blowing memory on multi-MB persisted outputs.
        persisted_output_cache: Arc::new(crate::services::PersistedOutputCache::new(
            16,
            32 * 1024 * 1024,
        )),
        load_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        loading_paths: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        promote_in_flight: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::reindex,
            commands::reindex_providers,
            commands::sync_sources,
            commands::get_tree,
            commands::get_session_detail,
            commands::get_session_meta,
            commands::get_session_open_window,
            commands::get_session_messages_window,
            commands::cancel_session_load,
            commands::get_child_sessions,
            commands::get_child_session_counts,
            commands::search_sessions,
            commands::rename_session,
            commands::delete_session,
            commands::get_session_count,
            commands::export_session,
            commands::get_index_stats,
            commands::get_pricing_catalog_status,
            commands::start_rebuild_index,
            commands::refresh_pricing_catalog,
            commands::clear_index,
            commands::clear_usage_stats,
            commands::start_refresh_usage,
            commands::get_provider_snapshots,
            commands::get_resume_command,
            commands::detect_terminal,
            commands::resume_session,
            commands::trash_session,
            commands::trash_sessions_batch,
            commands::list_trash,
            commands::restore_session,
            commands::restore_sessions_batch,
            commands::empty_trash,
            commands::permanent_delete_trash,
            commands::permanent_delete_trash_batch,
            commands::export_sessions_batch,
            commands::toggle_favorite,
            commands::list_recent_sessions,
            commands::list_favorites,
            commands::is_favorite,
            commands::read_image_base64,
            commands::read_tool_result_text,
            commands::resolve_persisted_output,
            commands::open_in_folder,
            commands::open_external,
            commands::get_usage_stats,
            commands::get_activity_calendar,
            commands::get_today_cost,
            commands::get_today_tokens,
        ])
        .setup(|app| {
            // On Windows, hide native decorations so the custom titlebar is the only one.
            #[cfg(target_os = "windows")]
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_decorations(false);
            }

            // On Linux, the WM also draws its own title bar, which would stack on
            // top of our custom one — hide native decorations like on Windows.
            #[cfg(target_os = "linux")]
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_decorations(false);
            }

            // Provider instances are lightweight (just PathBuf); create a separate
            // set for the watcher since Indexer consumed the first set.
            let watcher_providers = provider::all_runtimes();
            match watcher::start_watcher(app.handle().clone(), &watcher_providers) {
                Ok(fs_watcher) => {
                    app.manage(fs_watcher);
                }
                Err(e) => log::warn!("failed to start file watcher: {e}"),
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            log::error!("failed to run tauri application: {e}");
            std::process::exit(1);
        });
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn nofile_soft_limit_is_raised_without_exceeding_finite_hard_limit() {
        assert_eq!(
            super::desired_nofile_soft_limit(256, 1024, super::MACOS_MIN_NOFILE_LIMIT),
            1024
        );
        assert_eq!(
            super::desired_nofile_soft_limit(
                256,
                super::MACOS_RLIM_INFINITY,
                super::MACOS_MIN_NOFILE_LIMIT
            ),
            super::MACOS_MIN_NOFILE_LIMIT
        );
        assert_eq!(
            super::desired_nofile_soft_limit(
                super::MACOS_MIN_NOFILE_LIMIT + 1,
                super::MACOS_RLIM_INFINITY,
                super::MACOS_MIN_NOFILE_LIMIT
            ),
            super::MACOS_MIN_NOFILE_LIMIT + 1
        );
    }
}
