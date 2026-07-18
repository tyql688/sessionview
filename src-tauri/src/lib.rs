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
#[cfg(feature = "headless")]
pub mod server;
pub mod services;
pub mod tool_metadata;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

/// Test helpers — exposes private functions for integration tests.
#[doc(hidden)]
pub mod exporter_test_helpers {
    pub fn render_session_markdown_pub(detail: &crate::models::SessionDetail) -> String {
        crate::exporter::markdown::render(detail)
    }
}

#[doc(hidden)]
pub mod command_test_helpers {
    use crate::commands::{get_resume_command_for_tests, load_session_detail_for_tests};
    use crate::db::Database;
    use crate::models::{ProviderSnapshot, SessionDetail};
    use crate::services::ProviderSnapshotService;

    pub fn get_session_detail(db: &Database, session_id: &str) -> anyhow::Result<SessionDetail> {
        load_session_detail_for_tests(db, session_id)
    }

    pub fn get_provider_snapshots(db: &Database) -> anyhow::Result<Vec<ProviderSnapshot>> {
        Ok(ProviderSnapshotService::new(db).list()?)
    }

    pub fn get_resume_command(db: &Database, session_id: &str) -> anyhow::Result<String> {
        get_resume_command_for_tests(db, session_id)
    }
}

use commands::AppState;
use db::Database;
use indexer::Indexer;
use services::EventBus;

/// Per-user data directory shared by the GUI and headless shells — pointing
/// both at the same SQLite index is what makes them interchangeable without
/// re-indexing or duplicated storage. Fixed at `~/.sessionview` on every
/// platform (no migration from older platform-specific dirs — a fresh index
/// is rebuilt incrementally on first run).
pub fn default_data_dir() -> anyhow::Result<PathBuf> {
    dirs::home_dir()
        .map(|d| d.join(".sessionview"))
        .context("failed to resolve home dir")
}

/// Build the shared application state (database, indexer, caches). Both
/// shells call this with their own `EventBus` implementation.
pub fn build_app_state(data_dir: &Path, events: Arc<dyn EventBus>) -> anyhow::Result<AppState> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("failed to create data dir {}", data_dir.display()))?;

    let db = Arc::new(Database::open(data_dir).context("failed to open database")?);
    let providers = provider::all_runtimes();
    let indexer = Indexer::new(Arc::clone(&db), providers, data_dir.to_path_buf());

    Ok(AppState {
        db,
        indexer,
        events,
        maintenance_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        session_cache: Arc::new(crate::services::SessionCache::new(8)),
        // 16 entries / 32 MiB cap — covers a typical viewing burst without
        // blowing memory on multi-MB persisted outputs.
        persisted_output_cache: Arc::new(crate::services::PersistedOutputCache::new(
            16,
            32 * 1024 * 1024,
        )),
        load_tokens: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        promote_in_flight: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
    })
}

/// Forwards backend events to the Tauri webview.
#[cfg(feature = "gui")]
struct TauriEventBus(tauri::AppHandle);

#[cfg(feature = "gui")]
impl EventBus for TauriEventBus {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.0.emit(event, payload);
    }
}

#[cfg(feature = "gui")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            commands::gui::reindex,
            commands::gui::reindex_providers,
            commands::gui::get_tree,
            commands::gui::get_session_detail,
            commands::gui::get_session_meta,
            commands::gui::get_session_open_window,
            commands::gui::get_session_messages_window,
            commands::gui::get_session_turn_outline,
            commands::gui::cancel_session_load,
            commands::gui::get_child_sessions,
            commands::gui::get_child_session_counts,
            commands::gui::search_sessions,
            commands::gui::rename_session,
            commands::gui::get_session_count,
            commands::gui::export_session,
            commands::gui::get_index_stats,
            commands::gui::get_pricing_catalog_status,
            commands::gui::start_rebuild_index,
            commands::gui::refresh_pricing_catalog,
            commands::gui::clear_index,
            commands::gui::clear_usage_stats,
            commands::gui::start_refresh_usage,
            commands::gui::get_provider_snapshots,
            commands::gui::get_resume_command,
            commands::gui::detect_terminal,
            commands::gui::resume_session,
            commands::gui::export_sessions_batch,
            commands::gui::toggle_favorite,
            commands::gui::list_recent_sessions,
            commands::gui::list_favorites,
            commands::gui::is_favorite,
            commands::gui::read_image_base64,
            commands::gui::read_tool_result_text,
            commands::gui::resolve_persisted_output,
            commands::gui::open_in_folder,
            commands::gui::open_external,
            commands::gui::get_usage_stats,
            commands::gui::get_activity_calendar,
            commands::gui::get_project_tool_usage,
            commands::gui::get_project_daily_usage,
            commands::gui::get_today_cost,
            commands::gui::get_today_tokens,
        ])
        .setup(|app| {
            let data_dir = default_data_dir()?;
            let events: Arc<dyn EventBus> = Arc::new(TauriEventBus(app.handle().clone()));
            let state = build_app_state(&data_dir, events)?;
            app.manage(state);

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

            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            log::error!("failed to run tauri application: {e}");
            std::process::exit(1);
        });
}
