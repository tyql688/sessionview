use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::models::{token_totals_from_messages, Message, SessionMeta, TokenTotals};

use super::UsageEvent;

/// File-level snapshot the indexer uses to decide whether a session
/// needs to be re-parsed on the next scan. Mirrors what the `sessions`
/// table stores per-row in `(file_size_bytes, source_mtime)`.
///
/// `mtime` is epoch seconds (i64) — matches `SystemTime → UNIX_EPOCH`
/// duration and survives JSON / SQLite roundtrips without precision
/// loss for the resolution we care about (sub-second changes always
/// also bump file size on append-only JSONL).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceState {
    pub size: u64,
    pub mtime: i64,
}

/// Returned by `SessionProvider::scan_all`. `parsed` carries the
/// sessions the provider actually re-parsed; `unchanged_source_paths`
/// carries the file paths whose `(size, mtime)` matched the `known`
/// snapshot passed in and were therefore skipped. The indexer combines
/// both lists when deciding which DB rows are still live (so an
/// unchanged session isn't accidentally pruned just because it doesn't
/// appear in `parsed`).
#[derive(Default)]
pub struct ScanOutcome {
    pub parsed: Vec<ParsedSession>,
    pub unchanged_source_paths: Vec<String>,
}

/// Convert a `SystemTime` into the integer epoch-seconds form we store
/// in `sessions.source_mtime`. Returns `None` for clocks before the
/// UNIX epoch (effectively never on real systems).
pub fn system_time_to_epoch_seconds(t: std::time::SystemTime) -> Option<i64> {
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
}

/// Split a flat list of JSONL paths into "must reparse" vs "unchanged".
/// JSONL providers' `scan_incremental` calls this before kicking off the
/// rayon parse pipeline so only stale files actually hit the parser.
/// The unchanged list is returned as the path strings the DB stored,
/// matching what `Database::source_states_for_provider` keys on.
pub fn partition_files_by_freshness(
    files: Vec<PathBuf>,
    known: &HashMap<String, SourceState>,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut to_parse = Vec::with_capacity(files.len());
    let mut unchanged = Vec::new();
    for file in files {
        let path_str = file.to_string_lossy().to_string();
        match known.get(&path_str) {
            Some(state) if source_state_matches(&file, state) => unchanged.push(path_str),
            _ => to_parse.push(file),
        }
    }
    (to_parse, unchanged)
}

/// Stat `path` and decide whether the on-disk file still matches the
/// `(size, mtime)` we recorded in the DB. Used by JSONL providers'
/// `scan_all` override to skip parsing unchanged files.
pub fn source_state_matches(path: &Path, known: &SourceState) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() != known.size {
        return false;
    }
    let Some(mtime) = meta.modified().ok().and_then(system_time_to_epoch_seconds) else {
        return false;
    };
    mtime == known.mtime
}

#[derive(Clone)]
pub struct ParsedSession {
    pub meta: SessionMeta,
    pub messages: Vec<Message>,
    pub content_text: String,
    /// Number of per-line / per-record parse warnings (malformed JSONL lines,
    /// JSON fields that couldn't be decoded, etc.) encountered while parsing
    /// this session. File-level failures (can't open, file-wide JSON damage)
    /// are surfaced as `Err` instead. Zero when the parser hasn't been wired
    /// for per-record counting yet.
    pub parse_warning_count: u32,
    /// Conversation IDs of subagents this session explicitly invoked.
    ///
    /// Populated only by providers that surface structured parent→child links
    /// in the transcript itself (today: Antigravity's `INVOKE_SUBAGENT` step
    /// type). `db/sync.rs::upsert_session_on` uses this to back-fill
    /// `parent_id` / `is_sidechain` / inherited project metadata on already-
    /// indexed child rows. Empty for every other provider.
    pub child_session_ids: Vec<String>,
    /// Out-of-band token-usage events emitted by the provider's transcript
    /// (e.g. Codex `event_msg.token_count`). Consumed by the provider's
    /// own `compute_token_stats` override; empty for providers whose token
    /// counts are attached to messages directly.
    pub usage_events: Vec<UsageEvent>,
    /// Last-modified epoch seconds of the source file at parse time.
    /// 0 means "unknown" — the indexer treats that as "always reparse".
    /// Used together with `meta.file_size_bytes` to short-circuit
    /// unchanged files in `scan_incremental`. Not exposed to the
    /// frontend; never read outside the indexer / sync layer.
    pub source_mtime: i64,
}

/// Materialized session payload returned by `SessionProvider::load_messages`
/// at display time. Carries the parser's per-record warning count so the UI
/// can show a ⚠ badge when the session contains malformed lines the parser
/// had to skip.
///
/// `Deref<Target = Vec<Message>>` is implemented so tests and callers that
/// only care about the messages (e.g. `.iter()`, `.len()`, indexing) can treat
/// a `LoadedSession` as if it were a `Vec<Message>`. Call sites that need the
/// warning count access the field directly.
#[derive(Debug, Clone)]
pub struct LoadedSession {
    pub messages: Vec<Message>,
    pub parse_warning_count: u32,
    pub token_totals: TokenTotals,
}

impl LoadedSession {
    pub fn new(messages: Vec<Message>) -> Self {
        Self::from_messages(messages, 0)
    }

    pub fn from_messages(messages: Vec<Message>, parse_warning_count: u32) -> Self {
        let token_totals = token_totals_from_messages(&messages);
        Self {
            messages,
            parse_warning_count,
            token_totals,
        }
    }

    pub fn from_parsed(parsed: ParsedSession) -> Self {
        Self::from_messages(parsed.messages, parsed.parse_warning_count)
    }
}

impl std::ops::Deref for LoadedSession {
    type Target = Vec<Message>;

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}
