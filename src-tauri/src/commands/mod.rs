mod file_access;
mod search;
mod session_tail;
mod sessions;
mod settings;
mod terminal;
pub mod trash;
mod usage;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::db::Database;
use crate::indexer::Indexer;
use crate::services::load_cancel::CancelFlag;
use crate::services::{PersistedOutputCache, SessionCache};

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
) -> anyhow::Result<crate::models::SessionDetail> {
    sessions::load_detail(session_id, db)
}

pub(crate) fn get_resume_command_for_tests(
    db: &crate::db::Database,
    session_id: &str,
) -> anyhow::Result<String> {
    terminal::get_resume_command_for_db(db, session_id)
}
