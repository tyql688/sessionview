use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use anyhow::anyhow;

use crate::models::{Message, TokenTotals};
use crate::provider::LoadedSession;
use crate::services::load_cancel;

/// How often a thread waiting on another thread's parse re-checks its own
/// cancel flag.
const FLIGHT_POLL: Duration = Duration::from_millis(50);

/// Snapshot of a parsed session's messages. `Arc` keeps clones cheap when
/// multiple windowed reads run concurrently.
///
/// `is_partial` is true when the entry came from a tail-only parse (we
/// only have the most recent N messages, not the whole file). A later
/// full parse will overwrite the entry via `insert` and `is_partial`
/// flips back to false. Callers needing entries older than what's in
/// `messages` must check this flag and trigger a full re-parse before
/// trusting the slice.
///
/// `total_messages` is the full file's message count when known —
/// populated from the DB meta on the fast path so the frontend can
/// render the correct totals even when only the tail is in memory.
#[derive(Clone)]
pub struct CachedMessages {
    pub source_path: String,
    pub messages: Arc<Vec<Message>>,
    pub parse_warning_count: u32,
    pub token_totals: TokenTotals,
    pub mtime: Option<SystemTime>,
    pub is_partial: bool,
    pub total_messages: Option<usize>,
    last_access: u64,
}

/// Lightweight LRU cache for parsed session message vectors.
///
/// Keyed by canonical `source_path`. Backend session loaders consult this
/// cache before re-parsing and compare source metadata before reusing an
/// entry.
pub struct SessionCache {
    inner: Mutex<Inner>,
    counter: AtomicU64,
}

struct Inner {
    map: HashMap<String, CachedMessages>,
    capacity: usize,
    /// Parses in progress, by key (see `get_or_load`).
    loading: HashMap<String, Arc<Flight>>,
}

/// One parse in progress that other callers for the same key wait on.
#[derive(Default)]
struct Flight {
    outcome: Mutex<Option<FlightOutcome>>,
    done: Condvar,
}

#[derive(Clone)]
enum FlightOutcome {
    Loaded(CachedMessages),
    Failed(String),
    /// The leading load was canceled before it finished: a waiter that still
    /// needs the session loads it itself.
    Canceled,
}

impl Flight {
    fn finish(&self, outcome: FlightOutcome) {
        *lock(&self.outcome) = Some(outcome);
        self.done.notify_all();
    }

    /// The leading load's outcome, or `None` once the waiting thread's own
    /// load is canceled.
    fn wait(&self) -> Option<FlightOutcome> {
        let mut outcome = lock(&self.outcome);
        loop {
            if let Some(outcome) = outcome.as_ref() {
                return Some(outcome.clone());
            }
            if load_cancel::is_canceled() {
                return None;
            }
            outcome = match self.done.wait_timeout(outcome, FLIGHT_POLL) {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
}

/// Publishes the leading load's outcome on drop — `Canceled` if the load
/// panicked — after unregistering the flight, so a waiter that retries can
/// take the lead.
struct Leader<'a> {
    cache: &'a SessionCache,
    key: &'a str,
    flight: Arc<Flight>,
    outcome: FlightOutcome,
}

impl Drop for Leader<'_> {
    fn drop(&mut self) {
        lock(&self.cache.inner).loading.remove(self.key);
        let outcome = std::mem::replace(&mut self.outcome, FlightOutcome::Canceled);
        self.flight.finish(outcome);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl SessionCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                capacity: capacity.max(1),
                loading: HashMap::new(),
            }),
            counter: AtomicU64::new(0),
        }
    }

    /// Look up a cached entry whose stored mtime matches `current_mtime`.
    /// Updates LRU order on hit. Returns `None` if missing or stale.
    pub fn get(&self, key: &str, current_mtime: Option<SystemTime>) -> Option<CachedMessages> {
        self.lookup(&mut lock(&self.inner), key, current_mtime)
    }

    fn lookup(
        &self,
        inner: &mut Inner,
        key: &str,
        current_mtime: Option<SystemTime>,
    ) -> Option<CachedMessages> {
        let entry = inner.map.get_mut(key)?;
        if entry.mtime != current_mtime {
            inner.map.remove(key);
            return None;
        }
        let access = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        entry.last_access = access;
        Some(entry.clone())
    }

    /// The cached entry for `key`, or the result of `load` stored under it.
    ///
    /// One parse per key at a time: a caller that finds one in progress waits
    /// for it and shares its result instead of parsing the same file again.
    /// A parse that finishes is kept even when its own request was canceled
    /// meanwhile; a waiter whose request is canceled stops waiting, and a
    /// waiter whose leader was canceled mid-parse loads the session itself.
    pub(crate) fn get_or_load(
        &self,
        key: &str,
        source_path: &str,
        mtime: Option<SystemTime>,
        load: impl FnOnce() -> anyhow::Result<LoadedSession>,
    ) -> anyhow::Result<CachedMessages> {
        let flight = loop {
            let in_progress = {
                let mut inner = lock(&self.inner);
                if let Some(hit) = self.lookup(&mut inner, key, mtime) {
                    return Ok(hit);
                }
                match inner.loading.get(key) {
                    Some(flight) => Arc::clone(flight),
                    None => {
                        let flight = Arc::new(Flight::default());
                        inner.loading.insert(key.to_string(), Arc::clone(&flight));
                        break flight;
                    }
                }
            };
            match in_progress.wait() {
                Some(FlightOutcome::Loaded(entry)) => return Ok(entry),
                Some(FlightOutcome::Failed(message)) => return Err(anyhow!(message)),
                Some(FlightOutcome::Canceled) => {}
                None => return Err(anyhow!("canceled while waiting for {key} to load")),
            }
        };

        let mut leader = Leader {
            cache: self,
            key,
            flight,
            outcome: FlightOutcome::Canceled,
        };
        match load() {
            Ok(loaded) => {
                let total_messages = loaded.messages.len();
                let entry = self.insert(
                    key.to_string(),
                    source_path.to_string(),
                    loaded.messages,
                    loaded.parse_warning_count,
                    loaded.token_totals,
                    mtime,
                    false,
                    Some(total_messages),
                );
                leader.outcome = FlightOutcome::Loaded(entry.clone());
                Ok(entry)
            }
            Err(error) => {
                if !load_cancel::is_canceled() {
                    leader.outcome = FlightOutcome::Failed(format!("{error:#}"));
                }
                Err(error)
            }
        }
    }

    /// Whether a parse for `key` is in progress.
    pub(crate) fn is_loading(&self, key: &str) -> bool {
        lock(&self.inner).loading.contains_key(key)
    }

    /// Insert a freshly parsed entry, evicting the least-recently-accessed
    /// entry when over capacity.
    // Cache-row fields are all small Copy / String values consumed by the
    // CachedMessages constructor below; bundling them into a struct just
    // to satisfy clippy would force every caller to build the struct
    // before calling, with no real readability gain.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert(
        &self,
        key: String,
        source_path: String,
        messages: Vec<Message>,
        parse_warning_count: u32,
        token_totals: TokenTotals,
        mtime: Option<SystemTime>,
        is_partial: bool,
        total_messages: Option<usize>,
    ) -> CachedMessages {
        let access = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = CachedMessages {
            source_path,
            messages: Arc::new(messages),
            parse_warning_count,
            token_totals,
            mtime,
            is_partial,
            total_messages,
            last_access: access,
        };

        let mut inner = lock(&self.inner);
        inner.map.insert(key, entry.clone());

        while inner.map.len() > inner.capacity {
            if let Some(oldest_key) = inner
                .map
                .iter()
                .min_by_key(|(_, v)| v.last_access)
                .map(|(k, _)| k.clone())
            {
                inner.map.remove(&oldest_key);
            } else {
                break;
            }
        }

        entry
    }

    /// Drop a cache entry by key so the next read re-parses.
    pub fn invalidate(&self, key: &str) {
        lock(&self.inner).map.remove(key);
    }

    pub fn clear(&self) {
        lock(&self.inner).map.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    use super::*;

    fn dummy_messages(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| Message {
                role: crate::models::MessageRole::User,
                message_kind: None,
                content: format!("msg {i}"),
                timestamp: None,
                tool_name: None,
                tool_input: None,
                tool_metadata: None,
                token_usage: None,
                model: None,
                usage_hash: None,
            })
            .collect()
    }

    fn insert_dummy(cache: &SessionCache, key: &str, source_path: &str, mtime: Option<SystemTime>) {
        cache.insert(
            key.into(),
            source_path.into(),
            dummy_messages(1),
            0,
            TokenTotals::default(),
            mtime,
            false,
            None,
        );
    }

    fn loaded(n: usize) -> anyhow::Result<LoadedSession> {
        Ok(LoadedSession::new(dummy_messages(n)))
    }

    /// Start a load of "a" on another thread that holds the lead until the
    /// returned sender fires, then returns `result`.
    fn blocked_leader(
        cache: &Arc<SessionCache>,
        result: impl FnOnce() -> anyhow::Result<LoadedSession> + Send + 'static,
    ) -> (
        std::thread::JoinHandle<anyhow::Result<CachedMessages>>,
        mpsc::Sender<()>,
    ) {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let cache = Arc::clone(cache);
        let leader = std::thread::spawn(move || {
            cache.get_or_load("a", "/tmp/a.jsonl", None, move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                result()
            })
        });
        started_rx.recv().unwrap();
        (leader, release_tx)
    }

    fn waiter(
        cache: &Arc<SessionCache>,
        loads: &Arc<AtomicUsize>,
        flag: Option<load_cancel::CancelFlag>,
    ) -> std::thread::JoinHandle<anyhow::Result<CachedMessages>> {
        let (cache, loads) = (Arc::clone(cache), Arc::clone(loads));
        std::thread::spawn(move || {
            let load = || {
                cache.get_or_load("a", "/tmp/a.jsonl", None, || {
                    loads.fetch_add(1, Ordering::SeqCst);
                    loaded(9)
                })
            };
            match flag {
                Some(flag) => load_cancel::run_with(flag, load),
                None => load(),
            }
        })
    }

    /// Long enough for spawned waiters to find the load in progress.
    fn let_waiters_join() {
        std::thread::sleep(Duration::from_millis(200));
    }

    #[test]
    fn evicts_least_recently_used() {
        let cache = SessionCache::new(2);
        insert_dummy(&cache, "a", "/tmp/a.jsonl", None);
        insert_dummy(&cache, "b", "/tmp/b.jsonl", None);
        // Touch "a" so "b" becomes LRU
        let _ = cache.get("a", None);
        insert_dummy(&cache, "c", "/tmp/c.jsonl", None);

        assert!(cache.get("a", None).is_some(), "a must remain (recent)");
        assert!(cache.get("b", None).is_none(), "b must be evicted");
        assert!(cache.get("c", None).is_some(), "c must remain (newest)");
    }

    #[test]
    fn mtime_mismatch_invalidates() {
        let cache = SessionCache::new(4);
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        insert_dummy(&cache, "a", "/tmp/a.jsonl", Some(t0));
        assert!(cache.get("a", Some(t0)).is_some());
        assert!(cache.get("a", Some(t1)).is_none());
        // After mismatch, entry must have been removed
        assert!(cache.get("a", Some(t0)).is_none());
    }

    #[test]
    fn invalidate_removes_entry() {
        let cache = SessionCache::new(4);
        insert_dummy(&cache, "a", "/tmp/a.jsonl", None);
        cache.invalidate("a");
        assert!(cache.get("a", None).is_none());
    }

    #[test]
    fn concurrent_loads_of_one_session_share_one_parse() {
        let cache = Arc::new(SessionCache::new(4));
        let loads = Arc::new(AtomicUsize::new(0));
        let (leader, release) = blocked_leader(&cache, || loaded(3));
        let waiters: Vec<_> = (0..4).map(|_| waiter(&cache, &loads, None)).collect();
        let_waiters_join();
        release.send(()).unwrap();

        let led = leader.join().unwrap().unwrap();
        for waiter in waiters {
            let shared = waiter.join().unwrap().unwrap();
            assert!(Arc::ptr_eq(&shared.messages, &led.messages));
        }
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        assert!(!cache.is_loading("a"));
    }

    #[test]
    fn a_finished_parse_is_kept_when_its_request_was_canceled_meanwhile() {
        let cache = SessionCache::new(4);
        let flag = load_cancel::fresh();
        let tripped_mid_parse = Arc::clone(&flag);
        let result = load_cancel::run_with(flag, || {
            cache.get_or_load("a", "/tmp/a.jsonl", None, move || {
                // Most parsers never check the flag and run to the end.
                load_cancel::cancel(&tripped_mid_parse);
                loaded(3)
            })
        });

        assert_eq!(result.unwrap().messages.len(), 3);
        assert!(cache.get("a", None).is_some());
    }

    #[test]
    fn a_waiter_loads_the_session_itself_when_the_leading_parse_is_canceled() {
        let cache = Arc::new(SessionCache::new(4));
        let loads = Arc::new(AtomicUsize::new(0));
        let leader_flag = load_cancel::fresh();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let leader = {
            let cache = Arc::clone(&cache);
            std::thread::spawn(move || {
                load_cancel::run_with(Arc::clone(&leader_flag), || {
                    cache.get_or_load("a", "/tmp/a.jsonl", None, || {
                        started_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        // A parser that checks the flag bails out mid-file.
                        load_cancel::cancel(&leader_flag);
                        Err(anyhow!("parse interrupted"))
                    })
                })
            })
        };
        started_rx.recv().unwrap();
        let waiter = waiter(&cache, &loads, None);
        let_waiters_join();
        release_tx.send(()).unwrap();

        assert!(leader.join().unwrap().is_err());
        assert_eq!(waiter.join().unwrap().unwrap().messages.len(), 9);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_canceled_waiter_stops_waiting_while_the_parse_continues() {
        let cache = Arc::new(SessionCache::new(4));
        let loads = Arc::new(AtomicUsize::new(0));
        let (leader, release) = blocked_leader(&cache, || loaded(1));
        let flag = load_cancel::fresh();
        let waiter = waiter(&cache, &loads, Some(Arc::clone(&flag)));
        let_waiters_join();
        load_cancel::cancel(&flag);

        assert!(waiter.join().unwrap().is_err());
        assert!(cache.is_loading("a"));
        release.send(()).unwrap();
        assert_eq!(leader.join().unwrap().unwrap().messages.len(), 1);
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        assert!(cache.get("a", None).is_some());
    }

    #[test]
    fn waiters_share_the_leading_parse_failure() {
        let cache = Arc::new(SessionCache::new(4));
        let loads = Arc::new(AtomicUsize::new(0));
        let (leader, release) = blocked_leader(&cache, || Err(anyhow!("unreadable source")));
        let waiter = waiter(&cache, &loads, None);
        let_waiters_join();
        release.send(()).unwrap();

        let led = leader.join().unwrap().err().unwrap();
        let shared = waiter.join().unwrap().err().unwrap();
        assert!(format!("{led:#}").contains("unreadable source"));
        assert!(format!("{shared:#}").contains("unreadable source"));
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        assert!(!cache.is_loading("a"));
    }
}
