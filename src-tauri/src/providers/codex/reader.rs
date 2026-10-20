//! Rollout storage uses plain JSONL or a Zstandard stream ending in `.jsonl.zst`.
//! Physical sizes bound the input snapshot; history byte offsets address the
//! decoded JSONL. Decoding streams in memory and leaves the source untouched.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub(super) fn is_compressed(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "zst")
        && path.file_stem().is_some_and(|stem| {
            Path::new(stem)
                .extension()
                .is_some_and(|ext| ext == "jsonl")
        })
}

pub(super) fn plain_path(path: &Path) -> PathBuf {
    if is_compressed(path) {
        path.with_extension("")
    } else {
        path.to_path_buf()
    }
}

pub(super) fn open(path: &Path) -> io::Result<Box<dyn BufRead>> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    from_file(path, file, size)
}

pub(super) fn from_file(path: &Path, file: File, size: u64) -> io::Result<Box<dyn BufRead>> {
    let snapshot = file.take(size);
    if is_compressed(path) {
        Ok(Box::new(BufReader::new(zstd::stream::read::Decoder::new(
            snapshot,
        )?)))
    } else {
        Ok(Box::new(BufReader::new(snapshot)))
    }
}
