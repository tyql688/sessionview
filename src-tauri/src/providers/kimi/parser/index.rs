//! Path → identity and on-disk index resolution for the Kimi parser.
//!
//! Holds `session_index.jsonl` lookup (`SessionIndex`), the `state.json`
//! companion reader (`StateJson`), and the wire.jsonl path-shape helpers
//! (`split_session_path` / `session_id_for_path`).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

// ---------------------------------------------------------------------------
// session_index.jsonl: sessionId → workDir map written by kimi-code itself.
// Each line is `{"sessionId":"...","sessionDir":"...","workDir":"..."}`.
// We key by both sessionId (e.g. `session_<uuid>`) and sessionDir absolute
// path so a lookup can succeed from either side.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct SessionIndex {
    pub(super) by_id: HashMap<String, String>,
    by_dir: HashMap<String, String>,
}

impl SessionIndex {
    pub(crate) fn load(path: &Path) -> Self {
        let mut index = Self::default();
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) => {
                // Missing file is fine on first run; log at debug so it
                // doesn't clutter normal operation.
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "failed to read Kimi session index '{}': {error}",
                        path.display()
                    );
                }
                return index;
            }
        };
        for (line_no, line) in BufReader::new(file).lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(error) => {
                    log::warn!(
                        "failed to read Kimi session_index.jsonl line {} from '{}': {error}",
                        line_no + 1,
                        path.display()
                    );
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(error) => {
                    log::warn!(
                        "skipping malformed Kimi session_index.jsonl line {} in '{}': {error}",
                        line_no + 1,
                        path.display()
                    );
                    continue;
                }
            };
            let work_dir = value
                .get("workDir")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let Some(work_dir) = work_dir else {
                continue;
            };
            if let Some(id) = value.get("sessionId").and_then(|v| v.as_str()) {
                index.by_id.insert(id.to_string(), work_dir.clone());
            }
            if let Some(dir) = value.get("sessionDir").and_then(|v| v.as_str()) {
                index.by_dir.insert(dir.to_string(), work_dir);
            }
        }
        index
    }

    pub(super) fn lookup_workdir(&self, session_id: &str, session_dir: &Path) -> Option<String> {
        if let Some(wd) = self.by_id.get(session_id) {
            return Some(wd.clone());
        }
        // Try canonicalised first so `/var/...` ↔ `/private/var/...`
        // symlinks (macOS `/tmp` etc.) and trailing-slash mismatches
        // resolve; fall back to the raw path string for the common
        // case where both sides already match.
        let canon = std::fs::canonicalize(session_dir)
            .ok()
            .map(|p| p.to_string_lossy().to_string());
        if let Some(c) = &canon
            && let Some(wd) = self.by_dir.get(c)
        {
            return Some(wd.clone());
        }
        let raw = session_dir.to_string_lossy().to_string();
        let trimmed = raw.trim_end_matches('/');
        self.by_dir
            .get(trimmed)
            .or_else(|| self.by_dir.get(&raw))
            .cloned()
    }
}

// ---------------------------------------------------------------------------
// Path → identity helpers
// ---------------------------------------------------------------------------

/// Extract `(session_dir, agent_name)` from a wire.jsonl path.
/// Returns None if the path doesn't match the expected layout
/// `<session_dir>/agents/<agent>/wire.jsonl`.
pub(super) fn split_session_path(path: &Path) -> Option<(PathBuf, String)> {
    let agent_dir = path.parent()?; // <session_dir>/agents/<agent>
    let agents_dir = agent_dir.parent()?; // <session_dir>/agents
    if agents_dir.file_name() != Some(std::ffi::OsStr::new("agents")) {
        return None;
    }
    let session_dir = agents_dir.parent()?.to_path_buf();
    let agent_name = agent_dir.file_name()?.to_string_lossy().to_string();
    Some((session_dir, agent_name))
}

/// Derive the on-disk session id (e.g. `session_<uuid>` or `ses_<uuid>`)
/// from a wire.jsonl path. Used by mod.rs to assemble parent ids for
/// subagents and by the source-sync layer to look up DB rows by path.
pub fn session_id_for_path(path: &Path) -> Option<String> {
    let (session_dir, _agent) = split_session_path(path)?;
    Some(session_dir.file_name()?.to_string_lossy().to_string())
}

/// state.json companion file produced by kimi-code alongside each session.
/// We only consume a few fields here; the schema may grow.
#[derive(Debug, Default)]
pub(super) struct StateJson {
    /// Display title kimi-code stores after the first prompt.
    pub(super) title: Option<String>,
    /// ISO-8601 (UTC) creation time, e.g. `"2026-05-25T09:26:36.474Z"`.
    pub(super) created_at: Option<String>,
    /// ISO-8601 (UTC) last-update time.
    pub(super) updated_at: Option<String>,
    /// Map of agent-name → parent-agent-name (None for `main`).
    /// Used to identify which wire.jsonl is the parent vs. subagent.
    pub(super) agents: HashMap<String, Option<String>>,
    /// Map of swarm subagent-name → item assigned by AgentSwarm.
    pub(super) swarm_items: HashMap<String, String>,
}

impl StateJson {
    pub(super) fn load(session_dir: &Path) -> Self {
        let path = session_dir.join("state.json");
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "failed to read Kimi state.json '{}': {error}",
                        path.display()
                    );
                }
                return Self::default();
            }
        };
        let value: Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(error) => {
                log::warn!(
                    "failed to parse Kimi state.json '{}': {error}",
                    path.display()
                );
                return Self::default();
            }
        };
        let title = value
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let created_at = value
            .get("createdAt")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let updated_at = value
            .get("updatedAt")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let mut agents = HashMap::new();
        let mut swarm_items = HashMap::new();
        if let Some(map) = value.get("agents").and_then(|v| v.as_object()) {
            for (name, entry) in map {
                let parent = entry
                    .get("parentAgentId")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                agents.insert(name.clone(), parent);
                if let Some(item) = entry
                    .get("swarmItem")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    swarm_items.insert(name.clone(), item.to_string());
                }
            }
        }
        Self {
            title,
            created_at,
            updated_at,
            agents,
            swarm_items,
        }
    }
}
