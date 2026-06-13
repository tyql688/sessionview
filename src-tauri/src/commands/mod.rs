mod file_access;
mod search;
mod session_tail;
mod sessions;
mod settings;
mod terminal;
pub mod trash;
mod usage;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::db::Database;
use crate::indexer::Indexer;
use crate::services::load_cancel::CancelFlag;
use crate::services::{PersistedOutputCache, SessionCache};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub indexer: Indexer,
    pub maintenance_running: Arc<AtomicBool>,
    /// In-memory LRU of parsed message vectors. Populated by paged session
    /// loaders, invalidated when the watcher reports a source change.
    pub session_cache: Arc<SessionCache>,
    /// LRU of resolved `<persisted-output>` referenced files. Replaces
    /// per-message synchronous resolution at parse time.
    pub persisted_output_cache: Arc<PersistedOutputCache>,
    /// Live cancel flags keyed by session_id. Frontend cancels by id when
    /// the user closes / switches tabs mid-load.
    pub load_tokens: Arc<Mutex<HashMap<String, CancelFlag>>>,
    /// Source paths currently being parsed. Watcher events for these paths
    /// are dropped to avoid feedback-loop reparses while a load is in flight.
    pub loading_paths: Arc<Mutex<HashSet<PathBuf>>>,
    /// Cache keys whose background full-file parse is in flight. The tail
    /// fast-path consults this set to avoid spawning a duplicate promote
    /// when the user opens the same session twice in rapid succession.
    pub promote_in_flight: Arc<Mutex<HashSet<String>>>,
}

pub use file_access::*;
pub use search::*;
pub use sessions::*;
pub use settings::*;
pub use terminal::*;
pub use trash::*;
pub use usage::*;

pub(crate) fn load_session_detail_for_tests(
    db: &crate::db::Database,
    session_id: &str,
) -> Result<crate::models::SessionDetail, String> {
    sessions::load_detail(session_id, db).map_err(|e| format!("{e:#}"))
}

pub(crate) fn get_resume_command_for_tests(
    db: &crate::db::Database,
    session_id: &str,
) -> Result<String, String> {
    terminal::get_resume_command_for_db(db, session_id).map_err(|e| format!("{e:#}"))
}
