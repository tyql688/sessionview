//! Command Code v3 transcript parser.
//!
//! The transcript is an append-only tree. The last entry is the active leaf;
//! display messages follow its typed `parentId` chain, while usage is folded
//! from every assistant call in the file because abandoned branches still
//! consumed tokens and cost. Typed `agent` calls on the active branch become
//! limited inline child sessions containing only the prompt and final result
//! that Command Code actually persisted.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::types::{CURRENT_SESSION_VERSION, Entry, EntryBase, MetaSidecar, SessionHeader};
use crate::models::{MessageRole, Provider, SessionMeta};
use crate::provider::util::{project_name_from_path, session_title};
use crate::provider::{
    ParsedSession, SourceState, UsageEvent, system_time_to_epoch_seconds,
    token_totals_from_usage_events,
};

struct StoredEntry {
    line_no: usize,
    entry: Entry,
}

struct ParsedWire {
    header: SessionHeader,
    entries: Vec<StoredEntry>,
    parse_warning_count: u32,
}

struct ActiveEntry<'a> {
    index: usize,
    stored: &'a StoredEntry,
    timestamp: Option<String>,
    epoch_seconds: Option<i64>,
}

#[derive(Clone)]
struct NormalizedUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost_usd: Option<f64>,
}

pub(crate) fn source_state(path: &Path) -> Option<SourceState> {
    let transcript = std::fs::metadata(path).ok()?;
    let transcript_mtime = transcript
        .modified()
        .ok()
        .and_then(system_time_to_epoch_seconds)?;
    let sidecar = std::fs::metadata(meta_path(path)).ok();
    let sidecar_size = sidecar.as_ref().map_or(0, std::fs::Metadata::len);
    let sidecar_mtime = sidecar
        .and_then(|metadata| metadata.modified().ok())
        .and_then(system_time_to_epoch_seconds)
        .unwrap_or(0);

    Some(SourceState {
        size: transcript.len() + sidecar_size,
        mtime: transcript_mtime.max(sidecar_mtime),
        title: None,
    })
}

pub(crate) fn parse_session_file(path: &Path) -> Vec<ParsedSession> {
    let Some(ParsedWire {
        header,
        entries,
        mut parse_warning_count,
    }) = read_transcript(path)
    else {
        return Vec::new();
    };
    let source = match source_state(path) {
        Some(source) => source,
        None => {
            log::warn!("failed to stat Command Code session '{}'", path.display());
            return Vec::new();
        }
    };
    let sidecar = read_meta(path, &mut parse_warning_count);
    let entry_index = index_entries(&entries, path, &mut parse_warning_count);
    let branch = build_active_branch(&entries, &entry_index, path, &mut parse_warning_count);
    let active = materialize_active_entries(&entries, &branch, path, &mut parse_warning_count);
    let inline_subagents =
        subagents::collect_inline_subagents(&active, path, &mut parse_warning_count);
    let (usage_events, usage_by_entry) =
        extract_usage_events(&entries, &entry_index, path, &mut parse_warning_count);
    let messages = convert_messages(
        &entries,
        &entry_index,
        &active,
        &usage_by_entry,
        &inline_subagents.link_by_call_id,
        path,
        &mut parse_warning_count,
    );
    if messages.is_empty() {
        log::warn!(
            "skipping Command Code session '{}': no displayable messages",
            path.display()
        );
        return Vec::new();
    }

    let Some(created_at) = parse_header_timestamp(&header, path) else {
        return Vec::new();
    };
    let updated_at = active
        .iter()
        .filter_map(|entry| entry.epoch_seconds)
        .fold(created_at, i64::max);
    let first_user = messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .map(|message| message.content.as_str());
    let title = resolve_title(&entries, &sidecar, first_user);
    let model = resolve_active_model(&active).or_else(|| nonempty(sidecar.model.as_deref()));
    let totals = token_totals_from_usage_events(&usage_events);
    let Some(session_id) = session_id_from_path(path) else {
        return Vec::new();
    };
    if session_id != header.id {
        log::warn!(
            "Command Code transcript id '{}' does not match filename id '{}' in '{}'",
            header.id,
            session_id,
            path.display()
        );
        parse_warning_count = parse_warning_count.saturating_add(1);
    }
    let content_text = messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let mut root = ParsedSession {
        meta: SessionMeta {
            // Command Code's own catalog and `--session <id>` resolve the id
            // from `<id>.jsonl`; the duplicated header id is validated above
            // but is not the resume identity.
            id: session_id,
            provider: Provider::CommandCode,
            title,
            project_name: project_name_from_path(&header.cwd),
            project_path: header.cwd,
            created_at,
            updated_at,
            message_count: messages.len() as u32,
            file_size_bytes: source.size,
            source_path: path.to_string_lossy().to_string(),
            // Command Code's parentSession / parentSessionId fields describe
            // user-created forks and clones, not delegated subagents.
            is_sidechain: false,
            variant_name: None,
            model,
            cc_version: None,
            git_branch: nonempty(sidecar.git_branch.as_deref()),
            parent_id: None,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cache_read_tokens: totals.cache_read_tokens,
            cache_write_tokens: totals.cache_write_tokens,
        },
        messages,
        content_text,
        parse_warning_count,
        child_session_ids: Vec::new(),
        usage_events,
        source_mtime: source.mtime,
    };
    let mut children = inline_subagents.into_sessions(&root.meta, source.mtime);
    root.child_session_ids = children.iter().map(|child| child.meta.id.clone()).collect();
    let mut sessions = Vec::with_capacity(children.len() + 1);
    sessions.push(root);
    sessions.append(&mut children);
    sessions
}

fn read_transcript(path: &Path) -> Option<ParsedWire> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "failed to open Command Code session '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let mut records = Vec::new();
    let stats = crate::provider::util::for_each_jsonl_record(
        BufReader::new(file),
        path,
        |line_no, value: Value| {
            records.push((line_no, value));
            std::ops::ControlFlow::Continue(())
        },
    );
    let mut parse_warning_count = stats
        .read_error_count
        .saturating_add(stats.parse_error_count);
    let (header_index, header) = find_header(&records, path, &mut parse_warning_count)?;
    if header.version != CURRENT_SESSION_VERSION {
        log::warn!(
            "skipping unsupported Command Code session v{} in '{}' (supports v{})",
            header.version,
            path.display(),
            CURRENT_SESSION_VERSION
        );
        return None;
    }

    let entries = records
        .into_iter()
        .skip(header_index + 1)
        .filter_map(|(line_no, value)| parse_entry(value, line_no, path, &mut parse_warning_count))
        .collect();
    Some(ParsedWire {
        header,
        entries,
        parse_warning_count,
    })
}

fn find_header(
    records: &[(usize, Value)],
    path: &Path,
    parse_warning_count: &mut u32,
) -> Option<(usize, SessionHeader)> {
    for (index, (line_no, value)) in records.iter().enumerate() {
        if value.get("type").and_then(Value::as_str) != Some("session") {
            log::warn!(
                "skipping pre-header Command Code record at line {line_no} in '{}'",
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            continue;
        }
        let header: SessionHeader = match serde_json::from_value(value.clone()) {
            Ok(header) => header,
            Err(error) => {
                log::warn!(
                    "failed to parse Command Code session header at line {line_no} in '{}': {error}",
                    path.display()
                );
                return None;
            }
        };
        if header.kind != "session" {
            log::warn!(
                "invalid Command Code session header type at line {line_no} in '{}'",
                path.display()
            );
            return None;
        }
        return Some((index, header));
    }
    log::warn!(
        "skipping Command Code transcript without a session header: '{}'",
        path.display()
    );
    None
}

fn parse_entry(
    value: Value,
    line_no: usize,
    path: &Path,
    parse_warning_count: &mut u32,
) -> Option<StoredEntry> {
    let base: EntryBase = match serde_json::from_value(value.clone()) {
        Ok(base) => base,
        Err(error) => {
            log::warn!(
                "skipping malformed Command Code entry at line {line_no} in '{}': {error}",
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            return None;
        }
    };
    let kind = base.kind.clone();
    let entry = match kind.as_str() {
        "message" => decode_entry(
            value,
            base,
            Entry::Message,
            line_no,
            path,
            parse_warning_count,
        ),
        "model_change" => decode_entry(
            value,
            base,
            Entry::ModelChange,
            line_no,
            path,
            parse_warning_count,
        ),
        "compaction" => decode_entry(
            value,
            base,
            Entry::Compaction,
            line_no,
            path,
            parse_warning_count,
        ),
        "branch_summary" => decode_entry(
            value,
            base,
            Entry::BranchSummary,
            line_no,
            path,
            parse_warning_count,
        ),
        "custom_message" => decode_entry(
            value,
            base,
            Entry::CustomMessage,
            line_no,
            path,
            parse_warning_count,
        ),
        "session_info" => decode_entry(
            value,
            base,
            Entry::SessionInfo,
            line_no,
            path,
            parse_warning_count,
        ),
        "effort_change" | "custom" | "label" => Entry::Metadata(base),
        _ => {
            log::warn!(
                "skipping unsupported Command Code entry type '{kind}' at line {line_no} in '{}'",
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            Entry::Metadata(base)
        }
    };
    Some(StoredEntry { line_no, entry })
}

fn decode_entry<T: DeserializeOwned>(
    value: Value,
    base: EntryBase,
    wrap: impl FnOnce(T) -> Entry,
    line_no: usize,
    path: &Path,
    parse_warning_count: &mut u32,
) -> Entry {
    match serde_json::from_value(value) {
        Ok(entry) => wrap(entry),
        Err(error) => {
            log::warn!(
                "preserving malformed Command Code '{}' entry link at line {line_no} in '{}': {error}",
                base.kind,
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            Entry::Metadata(base)
        }
    }
}

fn index_entries(
    entries: &[StoredEntry],
    path: &Path,
    parse_warning_count: &mut u32,
) -> HashMap<String, usize> {
    let mut by_id = HashMap::with_capacity(entries.len());
    for (index, stored) in entries.iter().enumerate() {
        let base = stored.entry.base();
        if let Some(previous) = by_id.insert(base.id.clone(), index) {
            log::warn!(
                "duplicate Command Code entry id '{}' at lines {} and {} in '{}'",
                base.id,
                entries[previous].line_no,
                stored.line_no,
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
        }
    }
    by_id
}

fn build_active_branch(
    entries: &[StoredEntry],
    by_id: &HashMap<String, usize>,
    path: &Path,
    parse_warning_count: &mut u32,
) -> Vec<usize> {
    let Some(last) = entries.last() else {
        return Vec::new();
    };
    let mut current = Some(last.entry.base().id.as_str());
    let mut seen = HashSet::new();
    let mut branch = Vec::new();
    while let Some(id) = current {
        if !seen.insert(id.to_string()) {
            log::warn!(
                "cycle in Command Code entry tree at id '{id}' in '{}'",
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            break;
        }
        let Some(&index) = by_id.get(id) else {
            log::warn!(
                "missing Command Code parent entry '{id}' in '{}'",
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            break;
        };
        branch.push(index);
        current = entries[index].entry.base().parent_id.as_deref();
    }
    branch.reverse();
    branch
}

fn materialize_active_entries<'a>(
    entries: &'a [StoredEntry],
    branch: &[usize],
    path: &Path,
    parse_warning_count: &mut u32,
) -> Vec<ActiveEntry<'a>> {
    branch
        .iter()
        .map(|&index| {
            let stored = &entries[index];
            let parsed = DateTime::parse_from_rfc3339(&stored.entry.base().timestamp);
            let (timestamp, epoch_seconds) = match parsed {
                Ok(timestamp) => {
                    let timestamp = timestamp.with_timezone(&Utc);
                    (Some(timestamp.to_rfc3339()), Some(timestamp.timestamp()))
                }
                Err(error) => {
                    log::warn!(
                        "invalid Command Code entry timestamp at line {} in '{}': {error}",
                        stored.line_no,
                        path.display()
                    );
                    *parse_warning_count = parse_warning_count.saturating_add(1);
                    (None, None)
                }
            };
            ActiveEntry {
                index,
                stored,
                timestamp,
                epoch_seconds,
            }
        })
        .collect()
}

fn extract_usage_events(
    entries: &[StoredEntry],
    by_id: &HashMap<String, usize>,
    path: &Path,
    parse_warning_count: &mut u32,
) -> (Vec<UsageEvent>, HashMap<usize, NormalizedUsage>) {
    let mut events = Vec::new();
    let mut by_entry = HashMap::new();
    for (index, stored) in entries.iter().enumerate() {
        let Entry::Message(entry) = &stored.entry else {
            continue;
        };
        if entry.message.role != "assistant" {
            continue;
        }
        let Some(value) = entry.usage.as_ref() else {
            continue;
        };
        let Some(usage) = normalize_usage(value) else {
            log::warn!(
                "skipping malformed Command Code usage at line {} in '{}'",
                stored.line_no,
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            continue;
        };
        by_entry.insert(index, usage.clone());
        let Some(model) = resolve_model_for_entry(entries, by_id, index) else {
            log::warn!(
                "skipping Command Code usage without a typed model at line {} in '{}'",
                stored.line_no,
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            continue;
        };
        events.push(UsageEvent {
            timestamp: entry.base.timestamp.clone(),
            model,
            turn_count: 1,
            input_tokens: usage.input,
            output_tokens: usage.output,
            cache_read_input_tokens: usage.cache_read,
            cache_creation_input_tokens: usage.cache_write,
            usage_hash: None,
            cost_usd: usage.cost_usd,
        });
    }
    (events, by_entry)
}

fn normalize_usage(value: &Value) -> Option<NormalizedUsage> {
    let object = value.as_object()?;
    let input = object.get("inputTokens")?.as_u64()?;
    let output = object.get("outputTokens")?.as_u64()?;
    let cache_read = object.get("cacheReadTokens")?.as_u64()?;
    let cache_write = object.get("cacheWriteTokens")?.as_u64()?;
    let cost_usd = object.get("costUsd").and_then(Value::as_f64);
    Some(NormalizedUsage {
        input,
        output,
        cache_read,
        cache_write,
        cost_usd,
    })
}

fn resolve_model_for_entry(
    entries: &[StoredEntry],
    by_id: &HashMap<String, usize>,
    start: usize,
) -> Option<String> {
    let mut current = Some(start);
    let mut seen = HashSet::new();
    while let Some(index) = current {
        let entry = &entries[index].entry;
        if let Some(model) = explicit_model(entry) {
            return Some(model);
        }
        let parent = entry.base().parent_id.as_ref()?;
        if !seen.insert(parent.as_str()) {
            return None;
        }
        current = by_id.get(parent).copied();
    }
    None
}

fn explicit_model(entry: &Entry) -> Option<String> {
    match entry {
        Entry::Message(entry) if entry.message.role == "assistant" => {
            nonempty(entry.model.as_deref())
        }
        Entry::ModelChange(entry) => nonempty(Some(&entry.model)),
        _ => None,
    }
}

mod messages;
use messages::convert_messages;
mod subagents;
fn resolve_title(
    entries: &[StoredEntry],
    sidecar: &MetaSidecar,
    first_user: Option<&str>,
) -> String {
    // Session names are global metadata in Command Code, so use the most
    // recently appended session_info across the whole tree, not only the
    // currently selected conversation branch. An explicitly empty name means
    // "cleared"; do not resurrect a stale sidecar title in that case.
    for stored in entries.iter().rev() {
        if let Entry::SessionInfo(entry) = &stored.entry {
            return nonempty(entry.name.as_deref()).unwrap_or_else(|| session_title(first_user));
        }
    }
    nonempty(sidecar.title.as_deref()).unwrap_or_else(|| session_title(first_user))
}

fn resolve_active_model(active: &[ActiveEntry<'_>]) -> Option<String> {
    active
        .iter()
        .rev()
        .find_map(|active_entry| explicit_model(&active_entry.stored.entry))
}

fn read_meta(path: &Path, parse_warning_count: &mut u32) -> MetaSidecar {
    let path = meta_path(path);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return MetaSidecar::default();
        }
        Err(error) => {
            log::warn!(
                "failed to read Command Code meta sidecar '{}': {error}",
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            return MetaSidecar::default();
        }
    };
    match serde_json::from_str(&content) {
        Ok(meta) => meta,
        Err(error) => {
            log::warn!(
                "failed to parse Command Code meta sidecar '{}': {error}",
                path.display()
            );
            *parse_warning_count = parse_warning_count.saturating_add(1);
            MetaSidecar::default()
        }
    }
}

fn parse_header_timestamp(header: &SessionHeader, path: &Path) -> Option<i64> {
    match DateTime::parse_from_rfc3339(&header.timestamp) {
        Ok(timestamp) => Some(timestamp.timestamp()),
        Err(error) => {
            log::warn!(
                "skipping Command Code session with invalid header timestamp in '{}': {error}",
                path.display()
            );
            None
        }
    }
}

fn meta_path(path: &Path) -> PathBuf {
    path.with_extension("meta.json")
}

fn session_id_from_path(path: &Path) -> Option<String> {
    let Some(id) = path.file_stem().and_then(|id| id.to_str()) else {
        log::warn!(
            "skipping Command Code transcript without a UTF-8 filename id: '{}'",
            path.display()
        );
        return None;
    };
    nonempty(Some(id)).or_else(|| {
        log::warn!(
            "skipping Command Code transcript with an empty filename id: '{}'",
            path.display()
        );
        None
    })
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests;
