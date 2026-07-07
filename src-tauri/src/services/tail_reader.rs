//! Locate the byte offset of the *N*-th line from the end of a JSONL
//! file without parsing the file content.
//!
//! Backed by `memmap2` so the OS can page in only the trailing region of
//! the file; for an 87 MB Codex transcript we touch ~few hundred KB of
//! disk at most when targeting the last 300 messages. `memchr::memrchr`
//! does the actual newline scan, which on Apple Silicon is ~10 GB/s.
//!
//! The returned offset points to the first byte *after* the `(target +
//! 1)`-th newline counted from the end — i.e. the start of the line we
//! want the parser to begin at. Callers should consume from this offset
//! to EOF and feed each whole line into their existing line-by-line
//! parser; no partial line will be returned because we always land on a
//! `\n` boundary.

use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::Path;

use memchr::memrchr;
use memmap2::Mmap;

/// Result of locating a tail-byte window in a JSONL file.
#[derive(Debug, Clone, Copy)]
pub struct TailWindow {
    /// Byte offset where the tail window starts. The caller should
    /// `seek(SeekFrom::Start(start_offset))` and read line-by-line
    /// from there.
    pub start_offset: u64,
    /// Total file size in bytes (handy for sanity checks).
    pub file_size: u64,
    /// True iff `start_offset == 0`, meaning the requested tail covered
    /// the entire file. Useful so callers can mark the result as "full
    /// parse, not partial" without an extra stat.
    pub covers_whole_file: bool,
}

/// Find the byte offset where the last `target_lines` lines of `path`
/// start. Returns `Ok(window)` with `start_offset == 0` when the file
/// has fewer than `target_lines` complete lines.
///
/// `Err` is returned only for IO failures (open / metadata / mmap);
/// empty files and "fewer lines than requested" are normal results.
pub(crate) fn tail_byte_offset(path: &Path, target_lines: usize) -> std::io::Result<TailWindow> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    let file_size = metadata.len();
    if file_size == 0 {
        return Ok(TailWindow {
            start_offset: 0,
            file_size: 0,
            covers_whole_file: true,
        });
    }

    // SAFETY: We hold the file handle open for the lifetime of the mmap
    // and treat the bytes as read-only. The mmap is dropped at the end
    // of this function, before the file handle, so the OS-level mapping
    // is torn down while the fd is still valid.
    let mmap = unsafe { Mmap::map(&file)? };
    let bytes: &[u8] = mmap.as_ref();

    // We want the (target_lines + 1)-th `\n` counted from the end —
    // anything strictly after it is the tail window the caller wants.
    let mut count: usize = 0;
    let mut end = bytes.len();
    while end > 0 {
        let Some(idx) = memrchr(b'\n', &bytes[..end]) else {
            // No more newlines before `end` — the file is shorter than
            // requested. Parse from the beginning.
            return Ok(TailWindow {
                start_offset: 0,
                file_size,
                covers_whole_file: true,
            });
        };
        count += 1;
        if count > target_lines {
            let start = (idx + 1) as u64;
            return Ok(TailWindow {
                start_offset: start,
                file_size,
                covers_whole_file: start == 0,
            });
        }
        end = idx;
    }

    Ok(TailWindow {
        start_offset: 0,
        file_size,
        covers_whole_file: true,
    })
}

/// Open `path`'s tail window for line-by-line parsing: locate the byte
/// offset of the last `scan_lines` lines, open the file, wrap it in a
/// `BufReader`, and seek past the leading partial region. Returns the
/// reader positioned at the start of the tail window, paired with the
/// `TailWindow` (so callers that need `start_offset`/`covers_whole_file`
/// — e.g. to decide whether to prime fallback state from the file head —
/// don't have to recompute it).
///
/// `label` is the human-readable provider name spliced into the warn
/// logs on IO failure ("failed to locate <label> session tail", etc.).
/// Returns `None` on any locate/open/seek failure (after logging), so
/// the provider's tail parser falls back to the full-file parse — this
/// matches the per-parser scaffold each provider previously inlined.
pub(crate) fn open_tail_reader(
    path: &Path,
    scan_lines: usize,
    label: &str,
) -> Option<(BufReader<File>, TailWindow)> {
    let window = match tail_byte_offset(path, scan_lines) {
        Ok(w) => w,
        Err(error) => {
            log::warn!(
                "failed to locate {label} session tail in '{}': {error}",
                path.display()
            );
            return None;
        }
    };

    let file = match File::open(path) {
        Ok(f) => f,
        Err(error) => {
            log::warn!(
                "failed to open {label} session for tail parse '{}': {error}",
                path.display()
            );
            return None;
        }
    };

    let mut reader = BufReader::new(file);
    if window.start_offset > 0 {
        if let Err(error) = reader.seek(SeekFrom::Start(window.start_offset)) {
            log::warn!(
                "failed to seek {label} session for tail parse '{}': {error}",
                path.display()
            );
            return None;
        }
    }

    Some((reader, window))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_lines(lines: &[&str]) -> NamedTempFile {
        let mut tmp = NamedTempFile::new().expect("temp file");
        for line in lines {
            writeln!(tmp, "{line}").expect("write");
        }
        tmp.flush().expect("flush");
        tmp
    }

    #[test]
    fn returns_start_offset_for_last_two_lines() {
        // File:  "A\nB\nC\n"  → offsets 0,2,4
        // Last 2 lines = "B\nC\n", starting at offset 2.
        let tmp = write_lines(&["A", "B", "C"]);
        let window = tail_byte_offset(tmp.path(), 2).expect("tail offset");
        assert_eq!(window.start_offset, 2);
        assert!(!window.covers_whole_file);
        assert_eq!(window.file_size, 6);
    }

    #[test]
    fn returns_zero_when_target_exceeds_total_lines() {
        let tmp = write_lines(&["only"]);
        let window = tail_byte_offset(tmp.path(), 10).expect("tail offset");
        assert_eq!(window.start_offset, 0);
        assert!(window.covers_whole_file);
    }

    #[test]
    fn empty_file_returns_zero_with_covers_whole_file() {
        let tmp = NamedTempFile::new().expect("temp file");
        let window = tail_byte_offset(tmp.path(), 5).expect("tail offset");
        assert_eq!(window.file_size, 0);
        assert!(window.covers_whole_file);
        assert_eq!(window.start_offset, 0);
    }

    #[test]
    fn target_exactly_matching_total_lines_starts_at_file_top() {
        let tmp = write_lines(&["A", "B", "C"]);
        let window = tail_byte_offset(tmp.path(), 3).expect("tail offset");
        // 3 lines requested, 3 lines exist → start at the very beginning.
        assert_eq!(window.start_offset, 0);
        assert!(window.covers_whole_file);
    }

    #[test]
    fn handles_long_lines_inside_tail_range() {
        let big = "x".repeat(20_000);
        let tmp = write_lines(&[big.as_str(), "small1", "small2"]);
        let window = tail_byte_offset(tmp.path(), 2).expect("tail offset");
        // "small1\nsmall2\n" → 14 bytes, starting after the big line.
        let bytes = std::fs::read(tmp.path()).expect("read");
        assert_eq!(window.start_offset, (bytes.len() - 14) as u64);
        let tail = &bytes[window.start_offset as usize..];
        assert_eq!(tail, b"small1\nsmall2\n");
    }
}
