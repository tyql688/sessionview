use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

/// Cached `<persisted-output>` referenced file content.
///
/// Claude tool messages may carry a `Full output saved to: <path>` payload.
/// Resolving that path used to happen at session-open time for every
/// message, which on big sessions stalled the load. We now resolve lazily
/// on demand and cache by canonical path.
#[derive(Clone)]
struct CachedOutput {
    content: String,
    mtime: Option<SystemTime>,
    last_access: u64,
}

pub struct PersistedOutputCache {
    inner: Mutex<Inner>,
    counter: AtomicU64,
    max_bytes: usize,
}

struct Inner {
    map: HashMap<PathBuf, CachedOutput>,
    capacity_entries: usize,
    total_bytes: usize,
}

impl PersistedOutputCache {
    pub fn new(capacity_entries: usize, max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                capacity_entries: capacity_entries.max(1),
                total_bytes: 0,
            }),
            counter: AtomicU64::new(0),
            max_bytes,
        }
    }

    /// Read `canonical_path` content, returning a cached copy when fresh.
    /// Empty mtime triggers a fresh read (we cannot detect staleness).
    pub(crate) fn get_or_load(&self, canonical_path: &Path) -> std::io::Result<String> {
        let mtime = std::fs::metadata(canonical_path)
            .ok()
            .and_then(|m| m.modified().ok());

        // Fast path: cache hit with matching mtime.
        if mtime.is_some() {
            let mut inner = match self.inner.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if let Some(entry) = inner.map.get_mut(canonical_path) {
                if entry.mtime == mtime {
                    let access = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
                    entry.last_access = access;
                    return Ok(entry.content.clone());
                }
            }
            // Stale entry — drop it; we'll reinsert below. Absent is a no-op.
            if let Some(removed) = inner.map.remove(canonical_path) {
                inner.total_bytes = inner.total_bytes.saturating_sub(removed.content.len());
            }
        }

        // Read from disk. Drop the lock during IO so other resolves can
        // proceed in parallel.
        let content = std::fs::read_to_string(canonical_path)?;

        // Skip caching when:
        // - we couldn't read mtime (no way to validate freshness on the
        //   next call — caching would just leak entries); or
        // - a single entry would dominate the cache.
        if mtime.is_none() || content.len() > self.max_bytes / 2 {
            return Ok(content);
        }

        let access = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = CachedOutput {
            content: content.clone(),
            mtime,
            last_access: access,
        };

        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };

        let new_len = entry.content.len();
        let prev_len = inner
            .map
            .get(canonical_path)
            .map(|e| e.content.len())
            .unwrap_or(0);
        inner.total_bytes = inner.total_bytes.saturating_sub(prev_len) + new_len;
        inner.map.insert(canonical_path.to_path_buf(), entry);

        // Evict LRU until under both entry-count and byte caps.
        while inner.map.len() > inner.capacity_entries || inner.total_bytes > self.max_bytes {
            let Some((victim_key, victim_len)) = inner
                .map
                .iter()
                .min_by_key(|(_, v)| v.last_access)
                .map(|(k, v)| (k.clone(), v.content.len()))
            else {
                break;
            };
            inner.map.remove(&victim_key);
            inner.total_bytes = inner.total_bytes.saturating_sub(victim_len);
        }

        Ok(content)
    }

    pub fn clear(&self) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.map.clear();
        inner.total_bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn caches_content_and_returns_same_on_second_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"hello").unwrap();
        let canonical = path.canonicalize().unwrap();

        let cache = PersistedOutputCache::new(4, 1_000_000);
        let first = cache.get_or_load(&canonical).unwrap();
        assert_eq!(first, "hello");

        // Mutate the underlying file but DON'T touch mtime in a way the cache
        // can't see — instead, re-write so mtime advances; cache should
        // refresh.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"world").unwrap();
        drop(f);

        let second = cache.get_or_load(&canonical).unwrap();
        assert_eq!(second, "world", "stale cache must refresh after mtime bump");
    }

    #[test]
    fn evicts_when_over_byte_cap() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "x".repeat(100)).unwrap();
        std::fs::write(&b, "y".repeat(100)).unwrap();
        let ca = a.canonicalize().unwrap();
        let cb = b.canonicalize().unwrap();

        // 150 byte cap forces eviction once both are loaded.
        let cache = PersistedOutputCache::new(8, 150);
        let _ = cache.get_or_load(&ca).unwrap();
        let _ = cache.get_or_load(&cb).unwrap();

        let inner = cache.inner.lock().unwrap();
        assert!(
            inner.total_bytes <= 150,
            "byte cap must be enforced, got {}",
            inner.total_bytes
        );
    }
}
