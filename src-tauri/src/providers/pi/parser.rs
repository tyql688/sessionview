use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{Provider, SessionMeta};
use crate::provider::{LoadedSession, ParsedSession, UsageEvent, token_totals_from_usage_events};

use super::types::*;

/// Parse a Pi session JSONL file
pub(crate) fn parse_session_file(path: &Path) -> Option<ParsedSession> {
    let (header, entries, mut parse_warning_count) = parse_entries(path)?;

    let active_branch = build_active_branch(&entries);
    let context_branch = build_context_branch(&entries, &active_branch, path);
    let messages = extract_messages(&entries, &context_branch);
    let title = extract_title(&entries, &active_branch, &header);
    let model = extract_model(&entries, &active_branch);
    let usage_events = extract_usage_events(&entries, path, &mut parse_warning_count);
    let token_totals = token_totals_from_usage_events(&usage_events);

    let created_at = match crate::provider_utils::parse_rfc3339_epoch_seconds(&header.timestamp) {
        Some(ts) => ts,
        None => {
            log::warn!(
                "Skipping Pi session with malformed header timestamp '{}': {}",
                header.timestamp,
                path.display()
            );
            return None;
        }
    };
    let updated_at = extract_modified_at(&entries, path).unwrap_or(created_at);
    let parent_id = resolve_parent_session_id(header.parent_session.as_deref(), path);
    let is_sidechain = parent_id.is_some();

    let meta = SessionMeta {
        id: header.id.clone(),
        provider: Provider::Pi,
        title,
        project_path: header.cwd.clone(),
        project_name: extract_project_name(&header.cwd),
        created_at,
        updated_at,
        message_count: messages.len() as u32,
        file_size_bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        source_path: path.to_string_lossy().to_string(),
        is_sidechain,
        variant_name: None,
        model,
        cc_version: None,
        git_branch: None,
        parent_id,
        input_tokens: token_totals.input_tokens,
        output_tokens: token_totals.output_tokens,
        cache_read_tokens: token_totals.cache_read_tokens,
        cache_write_tokens: token_totals.cache_write_tokens,
    };

    let content_text = messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Some(ParsedSession {
        meta,
        messages,
        content_text,
        parse_warning_count,
        child_session_ids: Vec::new(),
        usage_events,
        source_mtime: std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(crate::provider::system_time_to_epoch_seconds)
            .unwrap_or(0),
    })
}

/// Load messages from a Pi session file (for detail view)
pub(crate) fn load_messages(path: &Path) -> Option<LoadedSession> {
    parse_session_file(path).map(LoadedSession::from_parsed)
}

fn parse_entries(path: &Path) -> Option<(PiSessionHeader, Vec<PiEntry>, u32)> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            log::warn!("Failed to read Pi session '{}': {error}", path.display());
            return None;
        }
    };
    if content.trim().is_empty() {
        return None;
    }
    let (header_line, entry_lines) = content.split_once('\n').unwrap_or((content.as_str(), ""));

    let mut header_value: Value = match serde_json::from_str(header_line) {
        Ok(header) => header,
        Err(error) => {
            log::warn!(
                "Failed to parse Pi session header '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let original_version = header_value
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .unwrap_or(1);
    if original_version > 3 {
        log::warn!(
            "Skipping unsupported Pi session v{}: {}",
            original_version,
            path.display()
        );
        return None;
    }
    migrate_pi_header_value(&mut header_value);

    let header: PiSessionHeader = match serde_json::from_value(header_value) {
        Ok(header) => header,
        Err(error) => {
            log::warn!(
                "Failed to parse Pi session header '{}': {error}",
                path.display()
            );
            return None;
        }
    };
    let mut entry_values = Vec::new();
    let stats = crate::provider_utils::for_each_jsonl_record_from(
        std::io::Cursor::new(entry_lines),
        path,
        2,
        |_, value: Value| {
            entry_values.push(value);
            std::ops::ControlFlow::Continue(())
        },
    );
    let mut parse_warning_count = stats
        .read_error_count
        .saturating_add(stats.parse_error_count);
    migrate_pi_entry_values(&mut entry_values, original_version);

    let mut entries: Vec<PiEntry> = Vec::new();
    for (i, value) in entry_values.into_iter().enumerate() {
        match serde_json::from_value::<PiEntry>(value) {
            Ok(entry) => entries.push(entry),
            Err(error) => {
                parse_warning_count = parse_warning_count.saturating_add(1);
                log::warn!(
                    "Failed to parse Pi session entry at line {} in '{}': {error}",
                    i + 2,
                    path.display()
                );
            }
        }
    }

    Some((header, entries, parse_warning_count))
}

fn migrate_pi_header_value(header: &mut Value) {
    let Some(obj) = header.as_object_mut() else {
        return;
    };
    obj.insert("version".to_string(), Value::from(3));
    if !obj.contains_key("parentSession")
        && let Some(branched_from) = obj.remove("branchedFrom")
    {
        obj.insert("parentSession".to_string(), branched_from);
    }
}

fn migrate_pi_entry_values(entries: &mut [Value], original_version: u32) {
    if original_version < 2 {
        let mut file_index_to_id: HashMap<usize, String> = HashMap::new();
        let mut previous_id: Option<String> = None;

        for (entry_index, value) in entries.iter_mut().enumerate() {
            let id = format!("legacy-{entry_index:08}");
            file_index_to_id.insert(entry_index + 1, id.clone());
            if let Some(obj) = value.as_object_mut() {
                obj.insert("id".to_string(), Value::String(id.clone()));
                obj.insert(
                    "parentId".to_string(),
                    previous_id
                        .as_ref()
                        .map(|parent_id| Value::String(parent_id.clone()))
                        .unwrap_or(Value::Null),
                );
            }
            previous_id = Some(id);
        }

        for value in entries.iter_mut() {
            let Some(obj) = value.as_object_mut() else {
                continue;
            };
            let Some(first_kept_index) = obj
                .get("firstKeptEntryIndex")
                .and_then(Value::as_u64)
                .and_then(|idx| usize::try_from(idx).ok())
            else {
                continue;
            };
            if let Some(first_kept_id) = file_index_to_id.get(&first_kept_index) {
                obj.insert(
                    "firstKeptEntryId".to_string(),
                    Value::String(first_kept_id.clone()),
                );
            }
            obj.remove("firstKeptEntryIndex");
        }
    }

    if original_version < 3 {
        for value in entries.iter_mut() {
            let Some(message) = value.get_mut("message").and_then(Value::as_object_mut) else {
                continue;
            };
            if message.get("role").and_then(Value::as_str) == Some("hookMessage") {
                message.insert("role".to_string(), Value::String("custom".to_string()));
            }
        }
    }
}

/// Build the active branch by walking from the last entry to root
fn build_active_branch(entries: &[PiEntry]) -> Vec<String> {
    if entries.is_empty() {
        return Vec::new();
    }

    // Find the last entry (leaf)
    let last_entry = match entries.last() {
        Some(e) => e,
        None => return Vec::new(),
    };
    let leaf_id = get_entry_id(last_entry);

    // Build parent map
    let mut parent_map: HashMap<String, String> = HashMap::new();
    for entry in entries {
        if let (Some(id), Some(parent_id)) = (get_entry_id(entry), get_entry_parent_id(entry)) {
            parent_map.insert(id, parent_id);
        }
    }

    // Walk from leaf to root
    let mut branch = Vec::new();
    let mut current = leaf_id;
    while let Some(id) = current {
        branch.push(id.clone());
        current = parent_map.get(&id).cloned();
    }

    branch.reverse();
    branch
}

/// Build the message context branch using Pi's compaction semantics.
///
/// Pi's active tree path still contains all entries from root to leaf, but the
/// visible/LLM context after compaction is:
///   compaction summary, kept pre-compaction entries, post-compaction entries.
fn build_context_branch(entries: &[PiEntry], active_branch: &[String], path: &Path) -> Vec<String> {
    let entry_by_id: HashMap<String, &PiEntry> = entries
        .iter()
        .filter_map(|entry| get_entry_id(entry).map(|id| (id, entry)))
        .collect();
    let latest_compaction = active_branch
        .iter()
        .enumerate()
        .filter_map(|(idx, id)| match entry_by_id.get(id).copied() {
            Some(PiEntry::Compaction(compaction)) => Some((idx, compaction)),
            _ => None,
        })
        .next_back();

    let Some((compaction_idx, compaction)) = latest_compaction else {
        return active_branch.to_vec();
    };

    let mut context_branch = vec![compaction.base.id.clone()];
    if let Some(first_kept_id) = compaction_first_kept_id(compaction, entries) {
        if let Some(first_kept_idx) = active_branch[..compaction_idx]
            .iter()
            .position(|id| id == &first_kept_id)
        {
            context_branch.extend_from_slice(&active_branch[first_kept_idx..compaction_idx]);
        } else {
            log::warn!(
                "Pi compaction firstKeptEntryId '{}' was not on the active branch: {}",
                first_kept_id,
                path.display()
            );
        }
    }
    context_branch.extend_from_slice(&active_branch[compaction_idx + 1..]);
    context_branch
}

fn compaction_first_kept_id(compaction: &PiCompactionEntry, entries: &[PiEntry]) -> Option<String> {
    if let Some(first_kept_id) = compaction.first_kept_entry_id.as_ref() {
        return Some(first_kept_id.clone());
    }
    let legacy_index = compaction.first_kept_entry_index?;
    if legacy_index == 0 {
        return None;
    }
    entries.get(legacy_index - 1).and_then(get_entry_id)
}

/// Extract messages from entries on the active branch
/// Extract title from session
fn extract_title(entries: &[PiEntry], branch: &[String], header: &PiSessionHeader) -> String {
    let branch_set: HashSet<&str> = branch.iter().map(String::as_str).collect();

    // Look for the latest session_info entry. Empty/missing names clear the
    // custom title, so older custom names must not win after a clear.
    if let Some(info) = entries.iter().rev().find_map(|entry| match entry {
        PiEntry::SessionInfo(info) if branch_set.contains(info.base.id.as_str()) => Some(info),
        _ => None,
    }) && let Some(name) = info
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return name.to_string();
    }

    // Fall back to first user message
    for entry in entries {
        if let PiEntry::Message(msg_entry) = entry
            && branch_set.contains(msg_entry.base.id.as_str())
            && let PiAgentMessage::User(user) = &msg_entry.message
        {
            let text = extract_content_text(&user.content);
            if !text.trim().is_empty() {
                return crate::provider_utils::session_title(Some(&text));
            }
        }
    }

    format!("Session {}", header.id)
}

/// Extract model from entries
fn extract_model(entries: &[PiEntry], branch: &[String]) -> Option<String> {
    let branch_set: HashSet<&str> = branch.iter().map(String::as_str).collect();

    // Look for model_change entry
    for entry in entries.iter().rev() {
        if let PiEntry::ModelChange(model_change) = entry
            && branch_set.contains(model_change.base.id.as_str())
        {
            return Some(format!(
                "{}/{}",
                model_change.provider, model_change.model_id
            ));
        }
    }

    // Fall back to assistant message model
    for entry in entries.iter().rev() {
        if let PiEntry::Message(msg_entry) = entry
            && branch_set.contains(msg_entry.base.id.as_str())
            && let PiAgentMessage::Assistant(assistant) = &msg_entry.message
            && let Some(model) = &assistant.model
        {
            let provider = assistant.provider.as_deref().unwrap_or("unknown");
            return Some(format!("{}/{}", provider, model));
        }
    }

    None
}

fn extract_usage_events(
    entries: &[PiEntry],
    path: &Path,
    parse_warning_count: &mut u32,
) -> Vec<UsageEvent> {
    entries
        .iter()
        .filter_map(|entry| {
            let PiEntry::Message(message) = entry else {
                return None;
            };
            let PiAgentMessage::Assistant(assistant) = &message.message else {
                return None;
            };
            let usage = assistant.usage.as_ref()?;
            let Some(model) = assistant.model.as_deref().filter(|model| !model.is_empty()) else {
                log::warn!(
                    "skipping Pi usage without a model in '{}' at entry {}",
                    path.display(),
                    message.base.id
                );
                *parse_warning_count = parse_warning_count.saturating_add(1);
                return None;
            };
            Some(UsageEvent {
                timestamp: message.base.timestamp.clone(),
                model: model.to_string(),
                input_tokens: usage.input,
                output_tokens: usage.output,
                cache_read_input_tokens: usage.cache_read,
                cache_creation_input_tokens: usage.cache_write,
                usage_hash: None,
            })
        })
        .collect()
}

/// Extract the Pi session-list `modified` timestamp as epoch seconds.
///
/// Pi derives this from the latest user/assistant message timestamp, not from
/// the last JSONL entry. Tool results, labels, and model changes do not update
/// the session-list activity time.
fn extract_modified_at(entries: &[PiEntry], path: &Path) -> Option<i64> {
    let mut modified_at = None;

    for entry in entries {
        let PiEntry::Message(message_entry) = entry else {
            continue;
        };
        let timestamp = match &message_entry.message {
            PiAgentMessage::User(user) => Some(user.timestamp),
            PiAgentMessage::Assistant(assistant) => Some(assistant.timestamp),
            _ => None,
        };
        let Some(timestamp) = timestamp else {
            continue;
        };

        match parse_millis_epoch_seconds(timestamp) {
            Some(timestamp) => {
                modified_at =
                    Some(modified_at.map_or(timestamp, |current: i64| current.max(timestamp)));
            }
            None => log::warn!(
                "Skipping invalid Pi activity timestamp '{}': {}",
                timestamp,
                path.display()
            ),
        }
    }

    modified_at
}

fn resolve_parent_session_id(parent_session: Option<&str>, child_path: &Path) -> Option<String> {
    let parent_session = parent_session?.trim();
    if parent_session.is_empty() {
        return None;
    }

    let parent_path = resolve_parent_session_path(parent_session, child_path);
    if parent_path == child_path {
        log::warn!(
            "Skipping Pi parentSession that points to itself: {}",
            child_path.display()
        );
        return None;
    }

    let file = match std::fs::File::open(&parent_path) {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "Skipping unresolved Pi parentSession '{}': {error}",
                parent_path.display()
            );
            return None;
        }
    };
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    if let Err(error) = reader.read_line(&mut line) {
        log::warn!(
            "Failed to read Pi parentSession header '{}': {error}",
            parent_path.display()
        );
        return None;
    }
    if line.trim().is_empty() {
        log::warn!(
            "Skipping Pi parentSession with empty header: {}",
            parent_path.display()
        );
        return None;
    }

    match serde_json::from_str::<PiSessionHeader>(&line) {
        Ok(header) => Some(header.id),
        Err(error) => {
            log::warn!(
                "Failed to parse Pi parentSession header '{}': {error}",
                parent_path.display()
            );
            None
        }
    }
}

fn resolve_parent_session_path(parent_session: &str, child_path: &Path) -> PathBuf {
    let raw_path = Path::new(parent_session);
    if raw_path.is_absolute() {
        return raw_path.to_path_buf();
    }

    child_path
        .parent()
        .map(|dir| dir.join(raw_path))
        .unwrap_or_else(|| raw_path.to_path_buf())
}

/// Get entry ID
pub(super) fn get_entry_id(entry: &PiEntry) -> Option<String> {
    match entry {
        PiEntry::Session(_) => None,
        PiEntry::Message(e) => Some(e.base.id.clone()),
        PiEntry::ModelChange(e) => Some(e.base.id.clone()),
        PiEntry::ThinkingLevelChange(e) => Some(e.base.id.clone()),
        PiEntry::Compaction(e) => Some(e.base.id.clone()),
        PiEntry::BranchSummary(e) => Some(e.base.id.clone()),
        PiEntry::Custom(e) => Some(e.base.id.clone()),
        PiEntry::CustomMessage(e) => Some(e.base.id.clone()),
        PiEntry::Label(e) => Some(e.base.id.clone()),
        PiEntry::SessionInfo(e) => Some(e.base.id.clone()),
    }
}

/// Get entry parent ID
fn get_entry_parent_id(entry: &PiEntry) -> Option<String> {
    match entry {
        PiEntry::Session(_) => None,
        PiEntry::Message(e) => e.base.parent_id.clone(),
        PiEntry::ModelChange(e) => e.base.parent_id.clone(),
        PiEntry::ThinkingLevelChange(e) => e.base.parent_id.clone(),
        PiEntry::Compaction(e) => e.base.parent_id.clone(),
        PiEntry::BranchSummary(e) => e.base.parent_id.clone(),
        PiEntry::Custom(e) => e.base.parent_id.clone(),
        PiEntry::CustomMessage(e) => e.base.parent_id.clone(),
        PiEntry::Label(e) => e.base.parent_id.clone(),
        PiEntry::SessionInfo(e) => e.base.parent_id.clone(),
    }
}

fn parse_millis_datetime(timestamp_millis: u64) -> Option<DateTime<Utc>> {
    let timestamp_millis = i64::try_from(timestamp_millis).ok()?;
    crate::provider_utils::epoch_ms_to_datetime(timestamp_millis)
}

fn parse_millis_epoch_seconds(timestamp_millis: u64) -> Option<i64> {
    parse_millis_datetime(timestamp_millis).map(|dt| dt.timestamp())
}

/// Normalize an entry-level RFC3339 timestamp to a UTC ISO string for messages.
pub(super) fn format_rfc3339_timestamp(ts: &str) -> Option<String> {
    match DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => Some(dt.with_timezone(&Utc).to_rfc3339()),
        Err(error) => {
            log::warn!("Skipping malformed Pi message timestamp '{ts}': {error}");
            None
        }
    }
}

/// Format a Pi message timestamp. Pi stores message timestamps as Unix milliseconds.
pub(super) fn format_millis_timestamp(timestamp_millis: u64) -> Option<String> {
    match parse_millis_datetime(timestamp_millis) {
        Some(timestamp) => Some(timestamp.to_rfc3339()),
        None => {
            log::warn!("Skipping invalid Pi message timestamp '{timestamp_millis}'");
            None
        }
    }
}

/// Extract project name from path
fn extract_project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

mod messages;

#[cfg(test)]
mod tests;

use messages::{extract_content_text, extract_messages};
