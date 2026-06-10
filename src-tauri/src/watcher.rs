use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::provider::SessionProvider;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};
use tauri::AppHandle;
use tauri::Emitter;

/// How long to wait after the last file-change event before emitting a
/// batched `sessions-changed` event to the frontend.
const DEBOUNCE_MS: u64 = 500;

pub fn start_watcher(
    app: AppHandle,
    providers: &[Box<dyn SessionProvider>],
) -> Result<RecommendedWatcher, String> {
    let watch_paths: Vec<PathBuf> = providers
        .iter()
        .flat_map(|p| p.watch_paths())
        .filter(|p| p.exists())
        .collect();
    let shallow_watch_paths: Vec<PathBuf> = providers
        .iter()
        .flat_map(|p| p.watch_paths_shallow())
        .filter(|p| p.exists())
        .collect();

    // Channel for forwarding changed paths from the notify callback to the
    // debounce thread. The notify callback must be non-blocking, so we just
    // send paths and let the background thread accumulate them.
    let (tx, rx) = mpsc::channel::<Vec<String>>();

    // Background thread: collect changed paths and flush them as a single
    // batched event once no new changes arrive within the debounce window.
    std::thread::Builder::new()
        .name("watcher-debounce".into())
        .spawn(move || {
            let debounce = Duration::from_millis(DEBOUNCE_MS);
            let mut pending = HashSet::<String>::new();
            // Per-path mtime cache: skip emitting paths whose mtime is the
            // same as the last batch we already published. Catches editor
            // save-then-touch storms and notify firing for metadata changes
            // (atime, permissions) that don't actually mutate content.
            let mut last_emitted_mtime: HashMap<String, SystemTime> = HashMap::new();

            loop {
                // If nothing is pending, block until the first change arrives.
                // If something IS pending, wait up to `debounce` for more.
                let recv_result = if pending.is_empty() {
                    rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                } else {
                    rx.recv_timeout(debounce)
                };

                match recv_result {
                    Ok(paths) => {
                        pending.extend(paths);
                        // Drain any other paths that arrived in the meantime.
                        while let Ok(more) = rx.try_recv() {
                            pending.extend(more);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Debounce window elapsed — flush the batch.
                        if !pending.is_empty() {
                            let batch =
                                drain_with_mtime_dedup(&mut pending, &mut last_emitted_mtime);
                            if !batch.is_empty() {
                                let _ = app.emit("sessions-changed", batch);
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // Watcher was dropped; flush remaining and exit.
                        if !pending.is_empty() {
                            let batch =
                                drain_with_mtime_dedup(&mut pending, &mut last_emitted_mtime);
                            if !batch.is_empty() {
                                let _ = app.emit("sessions-changed", batch);
                            }
                        }
                        break;
                    }
                }
            }
        })
        .map_err(|e| format!("failed to spawn debounce thread: {e}"))?;

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let changed_paths: Vec<String> = event
                    .paths
                    .iter()
                    .filter(|p| {
                        p.extension()
                            .is_some_and(|ext| ext == "jsonl" || ext == "json")
                    })
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();

                if !changed_paths.is_empty() {
                    let _ = tx.send(changed_paths);
                }
            }
        },
        Config::default(),
    )
    .map_err(|e| format!("failed to create file watcher: {e}"))?;

    let mut watched_count = 0usize;
    for path in &watch_paths {
        match watcher.watch(path, RecursiveMode::Recursive) {
            Ok(()) => watched_count += 1,
            Err(e) => {
                log::warn!("failed to watch {}: {e}", path.display());
            }
        }
    }
    let mut shallow_watched_count = 0usize;
    for path in &shallow_watch_paths {
        // NonRecursive: only the top-level dir's fd is opened. Children
        // are not registered, so unrelated WAL/SHM churn from external
        // processes inside child dirs can't race the watcher's internal
        // file-ident map.
        match watcher.watch(path, RecursiveMode::NonRecursive) {
            Ok(()) => shallow_watched_count += 1,
            Err(e) => {
                log::warn!("failed to watch (shallow) {}: {e}", path.display());
            }
        }
    }

    if !watch_paths.is_empty() && watched_count == 0 && shallow_watched_count == 0 {
        return Err("failed to watch any provider directory".to_string());
    }

    log::info!(
        "Watching {watched_count}/{} directories recursively, {shallow_watched_count}/{} shallow",
        watch_paths.len(),
        shallow_watch_paths.len()
    );
    Ok(watcher)
}

/// Drain `pending` into a `Vec<String>`, skipping any path whose current
/// mtime matches what was already emitted last time. Paths that can't be
/// stat-ed (deleted, permission issue) are still emitted — they represent
/// real state changes downstream consumers must see.
fn drain_with_mtime_dedup(
    pending: &mut HashSet<String>,
    last_emitted_mtime: &mut HashMap<String, SystemTime>,
) -> Vec<String> {
    let mut batch = Vec::with_capacity(pending.len());
    for path in pending.drain() {
        let current_mtime = std::fs::metadata(Path::new(&path))
            .ok()
            .and_then(|m| m.modified().ok());
        match (current_mtime, last_emitted_mtime.get(&path)) {
            (Some(now), Some(prev)) if now == *prev => {
                // Same content as last emission — skip to avoid downstream re-parse.
                continue;
            }
            (Some(now), _) => {
                last_emitted_mtime.insert(path.clone(), now);
            }
            (None, _) => {
                // Path vanished or unreadable; drop any stale entry so a later
                // recreate with matching mtime still emits once.
                last_emitted_mtime.remove(&path);
            }
        }
        batch.push(path);
    }
    batch
}

#[cfg(test)]
mod tests {
    use super::drain_with_mtime_dedup;
    use std::collections::{HashMap, HashSet};
    use std::time::SystemTime;
    use tempfile::TempDir;

    #[test]
    fn drain_skips_paths_with_unchanged_mtime() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("session.jsonl");
        std::fs::write(&file, b"hello").unwrap();
        let path_str = file.to_string_lossy().to_string();

        let mut pending: HashSet<String> = [path_str.clone()].into_iter().collect();
        let mut cache: HashMap<String, SystemTime> = HashMap::new();

        let first = drain_with_mtime_dedup(&mut pending, &mut cache);
        assert_eq!(first, vec![path_str.clone()]);
        assert!(cache.contains_key(&path_str));

        // Second drain with the same mtime: should skip.
        pending.insert(path_str.clone());
        let second = drain_with_mtime_dedup(&mut pending, &mut cache);
        assert!(
            second.is_empty(),
            "path with unchanged mtime must be deduped"
        );
    }

    #[test]
    fn drain_emits_missing_paths_so_deletion_propagates() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing.jsonl");
        let path_str = missing.to_string_lossy().to_string();

        let mut pending: HashSet<String> = [path_str.clone()].into_iter().collect();
        let mut cache: HashMap<String, SystemTime> = HashMap::new();
        // Seed the cache with a stale entry; missing-file drain should clear it.
        cache.insert(path_str.clone(), SystemTime::UNIX_EPOCH);

        let batch = drain_with_mtime_dedup(&mut pending, &mut cache);
        assert_eq!(batch, vec![path_str.clone()]);
        assert!(!cache.contains_key(&path_str));
    }
}
