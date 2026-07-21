use std::io::BufRead;
use std::ops::ControlFlow;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::models::TokenUsage;

mod content_parts;
mod tool_pairing;
pub(crate) use content_parts::{ContentPartsRender, RenderedToolOutput, render_content_parts};
pub(crate) use tool_pairing::ToolCallPairer;

pub const NO_PROJECT: &str = "(No Project)";

/// Outcome of a [`for_each_jsonl_record`] scan: how many lines failed to
/// read or deserialize (each already logged with file/line context), and
/// whether the callback stopped the scan early.
pub(crate) struct JsonlScanStats {
    pub read_error_count: u32,
    pub parse_error_count: u32,
    pub stopped_early: bool,
}

/// Iterate JSONL records from `reader`, deserializing each non-empty line as
/// `T`. Unreadable and malformed lines are logged (with path + line number)
/// and skipped; the returned stats let callers fold the skip counts into
/// their parse-warning totals. The callback receives the 1-based line number
/// and may return `ControlFlow::Break(())` to stop the scan early.
///
/// Line numbers are relative to the reader's first line — for seeked tail
/// readers they count from the seek point, not the start of the file.
pub(crate) fn for_each_jsonl_record<R, T>(
    reader: R,
    path: &Path,
    f: impl FnMut(usize, T) -> ControlFlow<()>,
) -> JsonlScanStats
where
    R: BufRead,
    T: DeserializeOwned,
{
    for_each_jsonl_record_from(reader, path, 1, f)
}

/// [`for_each_jsonl_record`] with an explicit first line number, for callers
/// that consumed leading lines (e.g. a header) before handing over the rest.
pub(crate) fn for_each_jsonl_record_from<R, T>(
    reader: R,
    path: &Path,
    first_line_no: usize,
    mut f: impl FnMut(usize, T) -> ControlFlow<()>,
) -> JsonlScanStats
where
    R: BufRead,
    T: DeserializeOwned,
{
    let mut stats = JsonlScanStats {
        read_error_count: 0,
        parse_error_count: 0,
        stopped_early: false,
    };
    for (index, line) in reader.lines().enumerate() {
        let line_no = first_line_no.saturating_add(index);
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                log::warn!(
                    "failed to read JSONL line {line_no} from '{}': {error}",
                    path.display()
                );
                stats.read_error_count = stats.read_error_count.saturating_add(1);
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let record: T = match serde_json::from_str(&line) {
            Ok(record) => record,
            Err(error) => {
                log::warn!(
                    "skipping malformed JSONL at line {line_no} in '{}': {error}",
                    path.display()
                );
                stats.parse_error_count = stats.parse_error_count.saturating_add(1);
                continue;
            }
        };
        if f(line_no, record).is_break() {
            stats.stopped_early = true;
            break;
        }
    }
    stats
}

/// Nearest ancestor directory named `subagents`, if any. Identifies a file
/// as a subagent transcript regardless of how deeply the agent is nested
/// (plain Task subagents sit directly in `subagents/`, Workflow agents
/// under `subagents/workflows/wf_*/`).
pub fn subagents_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors()
        .skip(1)
        .find(|dir| dir.file_name().is_some_and(|name| name == "subagents"))
}

/// Collect every `.jsonl` file under a session's `subagents/` directory,
/// recursively. Plain Task subagents sit directly in `subagents/`, while
/// Workflow runs nest theirs under `subagents/workflows/wf_*/`, so a
/// single-level read misses them.
pub fn collect_subagent_jsonl_files(subagents_dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![subagents_dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!("failed to read subagent dir '{}': {error}", dir.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
    files
}

pub fn is_system_content(trimmed: &str) -> bool {
    trimmed.starts_with("<environment_context")
        || trimmed.starts_with("<permissions")
        || trimmed.starts_with("<INSTRUCTIONS>")
        || trimmed.starts_with("<system")
        || trimmed.starts_with("<local-command-stdout>")
        || trimmed.starts_with("<local-command-caveat>")
        || trimmed.contains("<observation>")
        || trimmed.contains("</observation>")
        || trimmed.contains("<command-message>")
        || trimmed.contains("</command-message>")
        || trimmed.contains("</facts>")
        || trimmed.contains("</narrative>")
        || trimmed.contains("<INSTRUCTIONS>")
        || trimmed.contains("<environment_context>")
        || trimmed.contains("<permissions instructions>")
        || trimmed.contains("sandbox_mode")
        || (trimmed.starts_with('<') && trimmed.len() > 200 && !trimmed.contains("```"))
}

pub fn project_name_from_path(project_path: &str) -> String {
    if project_path.is_empty() || project_path == NO_PROJECT {
        NO_PROJECT.to_string()
    } else {
        Path::new(project_path).file_name().map_or_else(
            || project_path.to_string(),
            |name| name.to_string_lossy().to_string(),
        )
    }
}

fn replace_user_home_patterns(normalized: &str) -> String {
    let mut value = normalized.to_string();

    while let Some(colon_start) = value.find(":/Users/") {
        let Some(drive_start) = colon_start.checked_sub(1) else {
            break;
        };
        let Some(drive) = value[drive_start..colon_start].chars().next() else {
            break;
        };
        if !drive.is_ascii_alphabetic() {
            break;
        }
        let rest_start = colon_start + ":/Users/".len();
        let rest = &value[rest_start..];
        let user_len = rest
            .find(|c: char| c == '/' || c.is_whitespace())
            .unwrap_or(rest.len());
        value.replace_range(drive_start..rest_start + user_len, "~");
    }

    for prefix in ["/Users/", "/home/"] {
        while let Some(start) = value.find(prefix) {
            let rest_start = start + prefix.len();
            let rest = &value[rest_start..];
            let user_len = rest
                .find(|c: char| c == '/' || c.is_whitespace())
                .unwrap_or(rest.len());
            value.replace_range(start..rest_start + user_len, "~");
        }
    }

    value
}

/// Replace a local home directory prefix with `~` for display/privacy.
///
/// This is intentionally display-only: callers must keep the original path for
/// filesystem operations.
pub fn shorten_home_path(path: &str) -> String {
    if path.is_empty() || path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        return path.to_string();
    }

    let normalized = path.replace('\\', "/");
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy().replace('\\', "/");
        if normalized == home {
            return "~".to_string();
        }
        if let Some(rest) = normalized.strip_prefix(&(home + "/")) {
            return format!("~/{rest}");
        }
    }

    replace_user_home_patterns(&normalized)
}

pub fn parse_rfc3339_timestamp(timestamp: Option<&str>) -> i64 {
    timestamp
        .and_then(|ts| {
            DateTime::parse_from_rfc3339(ts)
                .map_err(|e| log::warn!("failed to parse timestamp '{ts}': {e}"))
                .ok()
        })
        .map_or(0, |dt| dt.timestamp())
}

/// Parse an RFC3339 timestamp into epoch seconds. Returns `None` on parse
/// failure so callers can decide whether the field is fatal or optional.
pub(crate) fn parse_rfc3339_epoch_seconds(ts: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp())
}

/// Convert epoch milliseconds to a UTC `DateTime`. Returns `None` for
/// out-of-range values.
pub(crate) fn epoch_ms_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp_millis(ms)
}

/// Convert epoch milliseconds to an RFC3339 timestamp string.
pub(crate) fn epoch_ms_to_rfc3339(ms: i64) -> Option<String> {
    epoch_ms_to_datetime(ms).map(|dt| dt.to_rfc3339())
}

/// Candidate field paths for [`token_usage_from`]. Each slot lists
/// dot-separated JSON paths tried in order (e.g. `"cache.read"`); the first
/// path resolving to a u64 wins.
pub(crate) struct UsageKeys<'a> {
    pub input: &'a [&'a str],
    pub output: &'a [&'a str],
    pub cache_read: &'a [&'a str],
    pub cache_write: &'a [&'a str],
}

fn usage_field(value: &Value, paths: &[&str]) -> Option<u64> {
    paths.iter().find_map(|path| {
        let mut current = value;
        for segment in path.split('.') {
            current = current.get(segment)?;
        }
        current.as_u64()
    })
}

/// Build a `TokenUsage` from a JSON usage payload using the declared field
/// paths. Missing fields default to 0, but if *none* of the mapped fields is
/// present the payload carries no usage at all and `None` is returned, so
/// callers can distinguish "no usage" from "all-zero usage". Provider-specific
/// semantics (zero filtering, cache folding, cumulative deltas) stay at the
/// call sites.
pub(crate) fn token_usage_from(value: &Value, keys: &UsageKeys) -> Option<TokenUsage> {
    let input = usage_field(value, keys.input);
    let output = usage_field(value, keys.output);
    let cache_read = usage_field(value, keys.cache_read);
    let cache_write = usage_field(value, keys.cache_write);
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
        return None;
    }
    Some(TokenUsage {
        input_tokens: input.unwrap_or(0) as u32,
        output_tokens: output.unwrap_or(0) as u32,
        cache_read_input_tokens: cache_read.unwrap_or(0) as u32,
        cache_creation_input_tokens: cache_write.unwrap_or(0) as u32,
    })
}

pub fn session_title(first_user_message: Option<&str>) -> String {
    first_user_message
        .map(|message| {
            // Strip [Image: source: ...] markers so titles show real text
            let cleaned = strip_image_markers(message);
            let text = cleaned.trim();
            if text.is_empty() {
                "Untitled".to_string()
            } else {
                text.to_string()
            }
        })
        .unwrap_or_else(|| "Untitled".to_string())
}

fn strip_image_markers(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("[Image") {
        result.push_str(&remaining[..start]);
        if let Some(end) = remaining[start..].find(']') {
            remaining = &remaining[start + end + 1..];
        } else {
            remaining = &remaining[start..];
            break;
        }
    }
    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- project_name_from_path ---

    #[test]
    fn project_name_regular_path() {
        assert_eq!(
            project_name_from_path("/home/user/my-project"),
            "my-project"
        );
    }

    #[test]
    fn project_name_empty_string() {
        assert_eq!(project_name_from_path(""), NO_PROJECT);
    }

    #[test]
    fn project_name_no_project_sentinel() {
        assert_eq!(project_name_from_path(NO_PROJECT), NO_PROJECT);
    }

    #[test]
    fn project_name_root_path() {
        // Path::new("/").file_name() returns None, so falls through to project_path.to_string()
        let result = project_name_from_path("/");
        assert_eq!(result, "/");
    }

    #[test]
    fn shorten_home_path_replaces_unix_home_prefixes() {
        assert_eq!(
            shorten_home_path("/Users/alice/project/src/main.rs"),
            "~/project/src/main.rs"
        );
        assert_eq!(
            shorten_home_path("/home/bob/project/src/main.rs"),
            "~/project/src/main.rs"
        );
    }

    #[test]
    fn shorten_home_path_replaces_windows_home_prefix() {
        assert_eq!(
            shorten_home_path("C:\\Users\\Alice\\project\\src\\main.rs"),
            "~/project/src/main.rs"
        );
    }

    #[test]
    fn shorten_home_path_replaces_embedded_home_path() {
        assert_eq!(
            shorten_home_path("*** Update File: /Users/alice/project/src/main.rs"),
            "*** Update File: ~/project/src/main.rs"
        );
    }

    #[test]
    fn shorten_home_path_preserves_existing_tilde_and_other_paths() {
        assert_eq!(shorten_home_path("~/project"), "~/project");
        assert_eq!(shorten_home_path("/opt/project"), "/opt/project");
    }

    // --- session_title ---

    #[test]
    fn session_title_normal_message() {
        assert_eq!(session_title(Some("Fix the bug")), "Fix the bug");
    }

    #[test]
    fn session_title_none() {
        assert_eq!(session_title(None), "Untitled");
    }

    #[test]
    fn session_title_strips_image_markers() {
        let msg = "Hello [Image: source: /path/to/img.png] world";
        assert_eq!(session_title(Some(msg)), "Hello  world");
    }

    #[test]
    fn session_title_only_image_marker() {
        let msg = "[Image: source: data:image/png;base64,abc]";
        assert_eq!(session_title(Some(msg)), "Untitled");
    }

    #[test]
    fn session_title_long_message_preserved() {
        let msg = "a".repeat(200);
        let result = session_title(Some(&msg));
        assert_eq!(result, msg);
    }

    // --- is_system_content ---

    #[test]
    fn is_system_environment_context() {
        assert!(is_system_content(
            "<environment_context>some stuff</environment_context>"
        ));
    }

    #[test]
    fn is_system_permissions() {
        assert!(is_system_content(
            "<permissions instructions>do things</permissions>"
        ));
    }

    #[test]
    fn is_system_normal_text_false() {
        assert!(!is_system_content("Just a normal message"));
    }

    #[test]
    fn is_system_short_unknown_tag_false() {
        assert!(!is_system_content("<short>"));
    }

    #[test]
    fn is_system_long_unknown_tag_true() {
        let long_tag = format!("<unknown_tag>{}</unknown_tag>", "x".repeat(250));
        assert!(is_system_content(&long_tag));
    }

    #[test]
    fn is_system_long_tag_with_code_block_false() {
        let content = format!("<unknown_tag>{}```code```</unknown_tag>", "x".repeat(250));
        assert!(!is_system_content(&content));
    }

    // --- parse_rfc3339_timestamp ---

    #[test]
    fn parse_valid_rfc3339() {
        let result = parse_rfc3339_timestamp(Some("2024-01-15T10:30:00Z"));
        assert!(result > 0);
    }

    #[test]
    fn parse_none_timestamp() {
        assert_eq!(parse_rfc3339_timestamp(None), 0);
    }

    #[test]
    fn parse_invalid_timestamp() {
        assert_eq!(parse_rfc3339_timestamp(Some("not-a-date")), 0);
    }

    // --- epoch/rfc3339 helpers ---

    #[test]
    fn parse_rfc3339_epoch_seconds_valid_and_invalid() {
        assert_eq!(
            parse_rfc3339_epoch_seconds("1970-01-01T00:01:00Z"),
            Some(60)
        );
        assert_eq!(parse_rfc3339_epoch_seconds("nope"), None);
    }

    #[test]
    fn epoch_ms_to_rfc3339_converts_millis() {
        assert_eq!(
            epoch_ms_to_rfc3339(1500).as_deref(),
            Some("1970-01-01T00:00:01.500+00:00")
        );
    }

    // --- for_each_jsonl_record ---

    #[test]
    fn jsonl_scan_skips_malformed_and_empty_lines() {
        let data = "{\"a\":1}\n\nnot json\n{\"a\":2}\n";
        let mut seen: Vec<(usize, Value)> = Vec::new();
        let stats = for_each_jsonl_record(
            std::io::Cursor::new(data),
            Path::new("/synthetic/test.jsonl"),
            |line_no, value: Value| {
                seen.push((line_no, value));
                ControlFlow::Continue(())
            },
        );
        assert_eq!(stats.parse_error_count, 1);
        assert_eq!(stats.read_error_count, 0);
        assert!(!stats.stopped_early);
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, 1);
        assert_eq!(seen[1].0, 4);
    }

    #[test]
    fn jsonl_scan_break_stops_early_and_offsets_line_numbers() {
        let data = "{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n";
        let mut seen = Vec::new();
        let stats = for_each_jsonl_record_from(
            std::io::Cursor::new(data),
            Path::new("/synthetic/test.jsonl"),
            10,
            |line_no, _: Value| {
                seen.push(line_no);
                if seen.len() == 2 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        );
        assert!(stats.stopped_early);
        assert_eq!(seen, vec![10, 11]);
    }

    // --- token_usage_from ---

    fn keys_flat() -> UsageKeys<'static> {
        UsageKeys {
            input: &["input_tokens"],
            output: &["output_tokens"],
            cache_read: &["cache_read_input_tokens"],
            cache_write: &["cache_creation_input_tokens"],
        }
    }

    #[test]
    fn token_usage_from_maps_declared_fields() {
        let value: Value = serde_json::json!({
            "input_tokens": 5,
            "output_tokens": 7,
            "cache_read_input_tokens": 11,
            "cache_creation_input_tokens": 13,
        });
        let usage = token_usage_from(&value, &keys_flat()).unwrap();
        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.cache_read_input_tokens, 11);
        assert_eq!(usage.cache_creation_input_tokens, 13);
    }

    #[test]
    fn token_usage_from_none_when_no_field_present() {
        let value: Value = serde_json::json!({ "unrelated": 1 });
        assert!(token_usage_from(&value, &keys_flat()).is_none());
    }

    #[test]
    fn token_usage_from_resolves_nested_paths_and_alternates() {
        let value: Value = serde_json::json!({
            "input": 3,
            "output": 4,
            "cache": { "read": 9, "write": 2 },
        });
        let keys = UsageKeys {
            input: &["missing_first", "input"],
            output: &["output"],
            cache_read: &["cache.read"],
            cache_write: &["cache.write"],
        };
        let usage = token_usage_from(&value, &keys).unwrap();
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 4);
        assert_eq!(usage.cache_read_input_tokens, 9);
        assert_eq!(usage.cache_creation_input_tokens, 2);
    }
}
