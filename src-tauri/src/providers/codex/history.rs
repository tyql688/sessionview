//! Paginated Codex rollouts can continue the same thread in another file.
//! `history_base` retains a prefix ending at a recorded byte offset and ordinal;
//! the continuation starts at that ordinal. The database must index the logical
//! history once, with its leaf path as the source, rather than letting physical
//! files with the same session id overwrite one another on alternate scans.
//! Prefixes are immutable: appends beyond a retained boundary are not inherited.
//! Codex's wire field `history_base.thread_id` identifies the physical rollout
//! (the filename's final UUID). After a revert it differs from `session_meta.id`,
//! which remains the stable logical thread ID across every retained segment.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use memchr::memrchr;
use memmap2::Mmap;
use serde::Deserialize;
use serde_json::Value;

use super::CodexProvider;

#[derive(Clone, Deserialize)]
struct HistoryBase {
    /// Physical rollout ID, despite the protocol's historical field name.
    thread_id: String,
    end_ordinal_exclusive: u64,
    end_byte_offset: u64,
}

pub(super) struct Header {
    pub(super) id: String,
    ordinal: u64,
    base: Option<HistoryBase>,
}

impl Header {
    pub(super) fn is_continuation(&self) -> bool {
        self.base.is_some()
    }
}

pub(super) fn read_header(path: &Path) -> anyhow::Result<Option<Header>> {
    let mut first = String::new();
    BufReader::new(File::open(path)?).read_line(&mut first)?;
    let Ok(row) = serde_json::from_str::<Value>(&first) else {
        return Ok(None); // The ordinary parser owns malformed-record warnings.
    };
    if row.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Ok(None);
    }
    let Some(id) = row.pointer("/payload/id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let base = row
        .pointer("/payload/history_base")
        .filter(|value| !value.is_null())
        .map(|value| serde_json::from_value::<HistoryBase>(value.clone()))
        .transpose()
        .context("malformed Codex history_base")?;
    let ordinal = match (row.get("ordinal").and_then(Value::as_u64), &base) {
        (Some(ordinal), _) => ordinal,
        (None, None) => 0,
        (None, Some(_)) => bail!("Codex history continuation is missing its ordinal"),
    };
    Ok(Some(Header {
        id: id.to_string(),
        ordinal,
        base,
    }))
}

fn headers(files: &[PathBuf]) -> anyhow::Result<HashMap<PathBuf, Header>> {
    let mut result = HashMap::new();
    for path in files {
        if let Some(header) =
            read_header(path).with_context(|| format!("reading '{}'", path.display()))?
        {
            result.insert(path.clone(), header);
        }
    }
    Ok(result)
}

fn boundary_matches(path: &Path, base: &HistoryBase) -> anyhow::Result<bool> {
    let file = File::open(path)?;
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    // SAFETY: The read-only file stays open for the mapping's lifetime. Codex
    // appends to rollouts; the retained prefix is never modified by this reader.
    let map = unsafe { Mmap::map(&file) }?;
    let Ok(end) = usize::try_from(base.end_byte_offset) else {
        return Ok(false);
    };
    if end == 0 || end > map.len() || map[end - 1] != b'\n' {
        return Ok(false);
    }
    let start = memrchr(b'\n', &map[..end - 1]).map_or(0, |index| index + 1);
    let row: Value = serde_json::from_slice(&map[start..end - 1])?;
    Ok(row
        .get("ordinal")
        .and_then(Value::as_u64)
        .and_then(|n| n.checked_add(1))
        == Some(base.end_ordinal_exclusive))
}

fn history_parts(
    path: &Path,
    limit: u64,
    catalog: &HashMap<PathBuf, Header>,
    parts: &mut Vec<(PathBuf, u64)>,
) -> anyhow::Result<()> {
    if parts.len() >= 64 {
        bail!("Codex history chain exceeds 64 segments");
    }
    if let Some(header) = catalog.get(path)
        && let Some(base) = &header.base
    {
        if base.end_ordinal_exclusive != header.ordinal {
            bail!("Codex history_base ordinal does not match its continuation");
        }
        let mut candidates = Vec::new();
        for (candidate, previous) in catalog {
            if previous.id == header.id
                && previous.ordinal < header.ordinal
                && super::session_uuid_from_filename(&candidate.to_string_lossy()).as_deref()
                    == Some(base.thread_id.as_str())
                && boundary_matches(candidate, base)?
            {
                candidates.push(candidate);
            }
        }
        let [parent] = candidates.as_slice() else {
            bail!(
                "Codex history base resolves to {} files; expected exactly one",
                candidates.len()
            );
        };
        // Every link strictly decreases its starting ordinal, so cycles cannot
        // occur. Reserve a slot before descending to bound recursive depth.
        parts.push((path.to_path_buf(), limit));
        history_parts(parent, base.end_byte_offset, catalog, parts)?;
    } else {
        parts.push((path.to_path_buf(), limit));
    }
    Ok(())
}

pub(super) fn leaf_paths(files: Vec<PathBuf>) -> anyhow::Result<Vec<PathBuf>> {
    let catalog = headers(&files)?;
    let mut inherited = HashSet::new();
    for (path, header) in &catalog {
        if header.base.is_none() {
            continue;
        }
        let mut parts = Vec::new();
        history_parts(path, std::fs::metadata(path)?.len(), &catalog, &mut parts)?;
        inherited.extend(parts.into_iter().skip(1).map(|(path, _)| path));
    }
    let leaves: Vec<_> = files
        .into_iter()
        .filter(|path| !inherited.contains(path))
        .collect();
    let mut ids = HashSet::new();
    for path in &leaves {
        if let Some(header) = catalog.get(path)
            && !ids.insert(&header.id)
        {
            bail!("multiple unrelated Codex rollouts claim the same thread id");
        }
    }
    Ok(leaves)
}

pub(super) fn open_reader(
    provider: &CodexProvider,
    path: &Path,
    file: File,
    file_size: u64,
) -> anyhow::Result<Box<dyn BufRead>> {
    if read_header(path)?.is_none_or(|header| !header.is_continuation()) {
        return Ok(Box::new(BufReader::new(file.take(file_size))));
    }
    let catalog = headers(&provider.collect_jsonl_files())?;
    let mut parts = Vec::new();
    history_parts(path, file_size, &catalog, &mut parts)?;
    let mut reader: Box<dyn Read> = Box::new(std::io::empty());
    for (part, limit) in parts.into_iter().rev() {
        reader = Box::new(reader.chain(File::open(&part)?.take(limit)));
    }
    Ok(Box::new(BufReader::new(reader)))
}

#[cfg(test)]
mod tests;
