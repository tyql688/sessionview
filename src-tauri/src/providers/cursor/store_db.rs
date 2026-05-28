//! Pull metadata + inline images out of Cursor's session `store.db`.
//!
//! `~/.cursor/chats/<md5>/<sessionId>/store.db` is a SQLite file with
//! two tables: `meta` (single-row JSON envelope) and `blobs`
//! (content-addressed by sha256). The meta envelope points at a root
//! protobuf blob whose payload is just a list of length-32 byte
//! arrays — each is the sha256 id of a child blob the conversation
//! references. Child blobs are either JSON envelopes wrapping a
//! chat message or raw binary (JPEG/PNG, occasional shell snapshot,
//! etc.).
//!
//! We only need three things:
//!
//! * **Workspace path** — the `<user_info>` blob has a `Workspace
//!   Path:` line we can grep.
//! * **Last used model** — top-level `lastUsedModel` field in meta.
//! * **Inline images** — `content[]` parts of user-role JSON blobs
//!   carry `{ type: "image", image: { __type: "Uint8Array", hex } }`.
//!   The hex unpacks to the raw JPEG/PNG bytes. We dump those into
//!   the shared image cache so the frontend renderer picks them up
//!   the same way it handles every other provider's images.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::services::image_cache::image_cache_data_dir;

use super::tools::extract_workspace_path;

/// Everything we extract from a single session's `store.db`.
pub(crate) struct StoreDbInfo {
    pub workspace_path: Option<String>,
    pub model: Option<String>,
    /// File paths in the shared image cache directory, in the order
    /// Cursor labels `[Image #N]` (1-indexed, so `image_paths[0]`
    /// corresponds to `[Image #1]`). Empty when no inline images
    /// were attached.
    pub image_paths: Vec<PathBuf>,
    /// Session creation time in seconds since epoch, parsed from the
    /// meta envelope's `createdAt` (stored in ms). ACP sessions need
    /// this because the file mtime is polluted by long-lived WAL
    /// connections — cursor keeps the db open and bumps mtime even
    /// when no new turn was written.
    pub created_at_secs: Option<i64>,
}

impl StoreDbInfo {
    fn empty() -> Self {
        Self {
            workspace_path: None,
            model: None,
            image_paths: Vec::new(),
            created_at_secs: None,
        }
    }
}

/// Open `store.db` and return everything we can pull out in one pass.
/// Failures degrade gracefully — partial info is better than none.
pub(crate) fn read_store_db(store_db: &Path, session_id: &str) -> StoreDbInfo {
    let conn = match Connection::open(store_db) {
        Ok(c) => c,
        Err(error) => {
            log::warn!(
                "failed to open Cursor store.db '{}': {error}",
                store_db.display()
            );
            return StoreDbInfo::empty();
        }
    };

    // ---- meta envelope ----
    let meta_value = read_meta_value(&conn, store_db);
    // `lastUsedModel` is only written when the user explicitly pinned
    // a model in a turn. Sessions that ran on the CLI's default model
    // routing leave the field missing; `agent --print` reports those
    // as "Auto", so we surface the same label rather than leave the
    // badge empty.
    let model = match meta_value
        .as_ref()
        .and_then(|v| v.get("lastUsedModel"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        Some("default") | None => meta_value.as_ref().map(|_| "Auto".to_string()),
        Some(other) => Some(other.to_string()),
    };
    let root_blob_id = meta_value
        .as_ref()
        .and_then(|v| v.get("latestRootBlobId"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let created_at_secs = meta_value
        .as_ref()
        .and_then(|v| v.get("createdAt"))
        .and_then(|v| v.as_i64())
        .map(|ms| ms / 1000);

    // ---- blob enumeration in root-order ----
    let ordered_ids = root_blob_id
        .as_deref()
        .and_then(|id| read_root_blob_children(&conn, id))
        .unwrap_or_default();

    // ---- image cache prep ----
    let cache_dir = image_cache_data_dir().map(|d| d.join("images"));
    let mut workspace_path: Option<String> = None;
    let mut image_paths: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let walk_ids = if ordered_ids.is_empty() {
        // No root blob (or unparseable) — fall back to walking every
        // blob in id order. We lose [Image #N] ordering but still
        // surface the data.
        all_blob_ids(&conn)
    } else {
        ordered_ids
    };

    for blob_id in &walk_ids {
        if !seen.insert(blob_id.clone()) {
            continue;
        }
        let Some(bytes) = read_blob(&conn, blob_id) else {
            continue;
        };
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            if workspace_path.is_none() {
                workspace_path = extract_workspace_from_blob(&value);
            }
            collect_images_from_blob(&value, session_id, cache_dir.as_deref(), &mut image_paths);
        } else if workspace_path.is_none() {
            // Binary blobs occasionally still embed `<user_info>` text;
            // sniff for it without paying the cost of a UTF-8 alloc
            // unless we have to.
            if let Some(p) = scan_workspace_in_bytes(&bytes) {
                workspace_path = Some(p);
            }
        }
    }

    StoreDbInfo {
        workspace_path,
        model,
        image_paths,
        created_at_secs,
    }
}

// ---------------------------------------------------------------------------
// Blob fetch helpers
// ---------------------------------------------------------------------------

pub(super) fn read_meta_value(conn: &Connection, store_db: &Path) -> Option<Value> {
    let mut stmt = match conn.prepare("SELECT value FROM meta LIMIT 1") {
        Ok(s) => s,
        Err(error) => {
            log::warn!(
                "failed to prepare Cursor store.db meta query '{}': {error}",
                store_db.display()
            );
            return None;
        }
    };
    let result = stmt.query_row([], |row| row.get::<_, String>(0));
    let raw = match result {
        Ok(text) => text,
        Err(rusqlite::Error::QueryReturnedNoRows) => return None,
        Err(error) => {
            log::warn!(
                "failed to read Cursor store.db meta '{}': {error}",
                store_db.display()
            );
            return None;
        }
    };
    // Cursor stores the meta envelope as a hex-encoded TEXT column —
    // raw bytes round-tripped through a hex string. Decode first,
    // then JSON-parse the resulting UTF-8.
    let json_text = match decode_hex(&raw) {
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(error) => {
                log::warn!(
                    "Cursor store.db meta hex did not decode to UTF-8 '{}': {error}",
                    store_db.display()
                );
                return None;
            }
        },
        None => {
            // Older sessions may have stored the JSON directly without
            // hex wrapping — try the raw value as a fallback.
            raw
        }
    };
    serde_json::from_str::<Value>(&json_text)
        .map_err(|error| {
            log::warn!(
                "failed to parse Cursor store.db meta JSON '{}': {error}",
                store_db.display()
            );
            error
        })
        .ok()
}

pub(super) fn read_blob(conn: &Connection, blob_id: &str) -> Option<Vec<u8>> {
    let mut stmt = conn.prepare("SELECT data FROM blobs WHERE id = ?1").ok()?;
    stmt.query_row([blob_id], |row| row.get::<_, Vec<u8>>(0))
        .ok()
}

/// Scan a protobuf blob's bytes for length-32 child references —
/// every `0A 20 <32 bytes>` run is a sha256 id pointing at another
/// blob in the same table. Used by the root-blob walker (chats/store)
/// and the recursive ACP walker.
pub(super) fn scan_pb_hash_refs(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 + 32 <= bytes.len() {
        if bytes[i] == 0x0A && bytes[i + 1] == 0x20 {
            let hex: String = bytes[i + 2..i + 2 + 32]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            out.push(hex);
            i += 2 + 32;
        } else {
            i += 1;
        }
    }
    out
}

/// Write raw image bytes to `cache_dir`, naming the file
/// `cursor-<session>-<sha256>.<ext>` so `cleanup_on_permanent_delete`
/// can clear them surgically. Returns the cached path on success.
/// Callers pass the already-resolved cache dir so tests can inject a
/// tempdir without touching the user's real app data dir.
pub(super) fn write_image_to_cache(
    cache_dir: &Path,
    session_id: &str,
    bytes: &[u8],
    mime: &str,
) -> Option<PathBuf> {
    if let Err(error) = std::fs::create_dir_all(cache_dir) {
        log::warn!(
            "failed to create Cursor image cache dir '{}': {error}",
            cache_dir.display()
        );
        return None;
    }
    let ext = ext_for_mime_or_magic(mime, bytes);
    let hash = Sha256::digest(bytes);
    let cache_path = cache_dir.join(format!("cursor-{session_id}-{:x}.{ext}", hash));
    if !cache_path.exists() {
        if let Err(error) = std::fs::write(&cache_path, bytes) {
            log::warn!(
                "failed to write Cursor image cache '{}': {error}",
                cache_path.display()
            );
            return None;
        }
    }
    Some(cache_path)
}

/// Resolve the default Cursor image cache directory. Wraps
/// `image_cache_data_dir().join("images")` so callers can build it
/// once and pass into `write_image_to_cache`.
pub(super) fn default_cursor_image_cache_dir() -> Option<PathBuf> {
    image_cache_data_dir().map(|d| d.join("images"))
}

fn all_blob_ids(conn: &Connection) -> Vec<String> {
    let mut stmt = match conn.prepare("SELECT id FROM blobs ORDER BY id") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map([], |row| row.get::<_, String>(0)) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(Result::ok).collect()
}

/// Fetch the root blob and decode its protobuf hash list. Returns
/// `None` when the blob is missing or empty.
fn read_root_blob_children(conn: &Connection, root_id: &str) -> Option<Vec<String>> {
    Some(scan_pb_hash_refs(&read_blob(conn, root_id)?))
}

// ---------------------------------------------------------------------------
// Per-blob extraction
// ---------------------------------------------------------------------------

fn extract_workspace_from_blob(value: &Value) -> Option<String> {
    if let Some(content) = value.get("content").and_then(Value::as_str) {
        if let Some(p) = extract_workspace_path(content) {
            return Some(p);
        }
    }
    None
}

fn scan_workspace_in_bytes(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    extract_workspace_path(text.as_ref())
}

/// Walk a chat-message JSON blob looking for image parts. Each hex
/// payload is written to the image cache directory under a name that
/// includes the session id + a stable hash of the bytes, so repeat
/// scans don't duplicate work. The cached path is appended to
/// `image_paths` in encounter order.
fn collect_images_from_blob(
    value: &Value,
    session_id: &str,
    cache_dir: Option<&Path>,
    image_paths: &mut Vec<PathBuf>,
) {
    let Some(content) = value.get("content").and_then(|v| v.as_array()) else {
        return;
    };
    for part in content {
        if part.get("type").and_then(|v| v.as_str()) != Some("image") {
            continue;
        }
        let hex = part
            .get("image")
            .and_then(|i| i.get("hex"))
            .and_then(|v| v.as_str());
        let Some(hex) = hex else { continue };
        let Some(bytes) = decode_hex(hex) else {
            log::warn!("Cursor store.db image blob has malformed hex (session {session_id})");
            continue;
        };
        let mime = part.get("mimeType").and_then(|v| v.as_str()).unwrap_or("");
        let ext = ext_for_mime_or_magic(mime, &bytes);

        let Some(cache_dir) = cache_dir else {
            continue;
        };
        if let Err(error) = std::fs::create_dir_all(cache_dir) {
            log::warn!(
                "failed to create Cursor image cache dir '{}': {error}",
                cache_dir.display()
            );
            return;
        }
        let hash = Sha256::digest(&bytes);
        let cache_name = format!("cursor-{session_id}-{:x}.{ext}", hash);
        let cache_path = cache_dir.join(&cache_name);
        if !cache_path.exists() {
            if let Err(error) = std::fs::write(&cache_path, &bytes) {
                log::warn!(
                    "failed to write Cursor image cache '{}': {error}",
                    cache_path.display()
                );
                continue;
            }
        }
        image_paths.push(cache_path);
    }
}

pub(super) fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn ext_for_mime_or_magic(mime: &str, bytes: &[u8]) -> &'static str {
    if mime.contains("png") || bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "png"
    } else if mime.contains("gif") || bytes.starts_with(b"GIF8") {
        "gif"
    } else if mime.contains("webp") {
        "webp"
    } else {
        // Cursor's default for pasted screenshots is JPEG.
        "jpg"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_hex_round_trip() {
        assert_eq!(decode_hex("00ff10").unwrap(), vec![0, 255, 16]);
        assert!(decode_hex("0").is_none());
        assert!(decode_hex("zz").is_none());
    }

    #[test]
    fn ext_from_mime_or_magic_recognises_png() {
        assert_eq!(ext_for_mime_or_magic("image/png", &[]), "png");
        assert_eq!(
            ext_for_mime_or_magic("application/octet-stream", &[0x89, 0x50, 0x4E, 0x47]),
            "png"
        );
        assert_eq!(ext_for_mime_or_magic("", &[0xFF, 0xD8]), "jpg");
    }
}
