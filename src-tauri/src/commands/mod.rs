mod file_access;
#[cfg(feature = "gui")]
pub mod gui;
mod search;
mod session_tail;
mod sessions;
mod settings;
mod terminal;
mod usage;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::db::Database;
use crate::indexer::Indexer;
use crate::services::load_cancel::CancelFlag;
use crate::services::{EventBus, PersistedOutputCache, SessionCache};

#[derive(Clone)]
pub struct LoadToken {
    pub request_id: Option<String>,
    /// Client-issued monotonic sequence. Command handlers run on independently
    /// scheduled tasks, so token registration order does NOT reflect the order
    /// requests were issued in — this is the only trustworthy "which load is
    /// newer" signal for a session key.
    pub seq: Option<u64>,
    pub flag: CancelFlag,
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub indexer: Indexer,
    /// Backend→frontend event channel; shell-specific (Tauri emit vs SSE).
    pub events: Arc<dyn EventBus>,
    pub maintenance_running: Arc<AtomicBool>,
    /// In-memory LRU of parsed message vectors. Populated by paged session
    /// loaders and checked against source metadata before reuse.
    pub session_cache: Arc<SessionCache>,
    /// LRU of resolved `<persisted-output>` referenced files. Replaces
    /// per-message synchronous resolution at parse time.
    pub persisted_output_cache: Arc<PersistedOutputCache>,
    /// Live cancel flags keyed by session_id. Each flag also carries a
    /// frontend request identity so stale cleanup IPC cannot cancel a newer
    /// load for the same session.
    pub load_tokens: Arc<Mutex<HashMap<String, LoadToken>>>,
}

/// Holds `AppState::maintenance_running` for one maintenance pass (index,
/// usage or pricing refresh) and clears it on drop. Move it into the task
/// doing the work, so a dropped command future (HTTP client disconnect)
/// can neither release it early nor leak it set.
pub(crate) struct MaintenanceGuard(Arc<AtomicBool>);

impl MaintenanceGuard {
    /// `None` while another pass holds the flag.
    pub(crate) fn try_acquire(state: &AppState) -> Option<Self> {
        let flag = &state.maintenance_running;
        (!flag.swap(true, Ordering::SeqCst)).then(|| Self(Arc::clone(flag)))
    }
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Run blocking DB/file work off the async runtime and fold a join failure
/// into the command error chain. Nearly every command core wraps its body in
/// this; accepts closures returning either `anyhow::Result` or
/// `CommandResult`.
pub(crate) async fn blocking<T, E>(
    f: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> crate::error::CommandResult<T>
where
    T: Send + 'static,
    E: Send + 'static,
    crate::error::CommandError: From<E>,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result.map_err(crate::error::CommandError::from),
        Err(e) => Err(crate::error::CommandError(anyhow::anyhow!(
            "task join error: {e:#}"
        ))),
    }
}

pub use file_access::*;
pub use search::*;
pub use sessions::*;
pub use settings::*;
pub use terminal::*;
pub use usage::*;

pub(crate) fn load_session_detail_for_tests(
    db: &crate::db::Database,
    session_id: &str,
) -> anyhow::Result<crate::models::SessionDetail> {
    sessions::load_detail(session_id, db)
}

pub(crate) fn get_resume_command_for_tests(
    db: &crate::db::Database,
    session_id: &str,
) -> anyhow::Result<String> {
    terminal::get_resume_command_for_db(db, session_id)
}
