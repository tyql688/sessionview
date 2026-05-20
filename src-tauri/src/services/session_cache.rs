use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::models::{Message, TokenTotals};

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
/// cache before re-parsing; the watcher invalidates entries when source
/// files change so window reads always observe a coherent snapshot.
pub struct SessionCache {
    inner: Mutex<Inner>,
    counter: AtomicU64,
}

struct Inner {
    map: HashMap<String, CachedMessages>,
    capacity: usize,
}

impl SessionCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                capacity: capacity.max(1),
            }),
            counter: AtomicU64::new(0),
        }
    }

    /// Look up a cached entry whose stored mtime matches `current_mtime`.
    /// Updates LRU order on hit. Returns `None` if missing or stale.
    pub fn get(&self, key: &str, current_mtime: Option<SystemTime>) -> Option<CachedMessages> {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = inner.map.get_mut(key)?;
        if entry.mtime != current_mtime {
            inner.map.remove(key);
            return None;
        }
        let access = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        entry.last_access = access;
        Some(entry.clone())
    }

    /// Insert a freshly parsed entry, evicting the least-recently-accessed
    /// entry when over capacity.
    // Cache-row fields are all small Copy / String values consumed by the
    // CachedMessages constructor below; bundling them into a struct just
    // to satisfy clippy would force every caller to build the struct
    // before calling, with no real readability gain.
    #[allow(clippy::too_many_arguments)]
    pub fn insert(
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

        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
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

    /// Drop a cache entry by source path. Used when the watcher detects a
    /// change so the next read re-parses.
    pub fn invalidate(&self, key: &str) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        inner.map.remove(key);
    }

    pub fn invalidate_source(&self, source_path: &str) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        inner
            .map
            .retain(|_, entry| entry.source_path != source_path);
    }

    pub fn clear(&self) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        inner.map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_messages(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| Message {
                role: crate::models::MessageRole::User,
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
    fn invalidate_source_removes_all_entries_for_source() {
        let cache = SessionCache::new(4);
        insert_dummy(
            &cache,
            "codex:s1:/tmp/shared.jsonl",
            "/tmp/shared.jsonl",
            None,
        );
        insert_dummy(
            &cache,
            "codex:s2:/tmp/shared.jsonl",
            "/tmp/shared.jsonl",
            None,
        );
        insert_dummy(
            &cache,
            "codex:s3:/tmp/other.jsonl",
            "/tmp/other.jsonl",
            None,
        );

        cache.invalidate_source("/tmp/shared.jsonl");

        assert!(cache.get("codex:s1:/tmp/shared.jsonl", None).is_none());
        assert!(cache.get("codex:s2:/tmp/shared.jsonl", None).is_none());
        assert!(cache.get("codex:s3:/tmp/other.jsonl", None).is_some());
    }
}
