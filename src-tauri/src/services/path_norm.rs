//! Path prefix comparison that tolerates Windows verbatim prefixes and
//! filesystem case differences.
//!
//! `std::fs::canonicalize` on Windows returns verbatim paths (`\\?\C:\...`),
//! which never `starts_with` a non-verbatim base like `C:\Users\me` — so any
//! naive prefix allowlist silently rejects everything. These helpers compare
//! paths as normalized strings: verbatim prefix stripped, ASCII case folded.
//! Pure string logic, testable on every platform.

use std::path::Path;

/// Normalize a path into a comparable string: strips the Windows verbatim
/// prefix (`\\?\C:\...` → `C:\...`, `\\?\UNC\server\share` → `\\server\share`)
/// and lowercases ASCII (Windows and default macOS filesystems are
/// case-insensitive).
pub(crate) fn lossy_norm(path: &Path) -> String {
    let s = path.to_string_lossy();
    let stripped = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    };
    stripped.to_ascii_lowercase()
}

/// Whether `path` equals `base` or lies strictly under it, comparing the
/// normalized forms from [`lossy_norm`]. Both `\` and `/` count as separators
/// so the check works for Windows and POSIX canonical paths alike.
pub(crate) fn norm_starts_with(path: &Path, base: &Path) -> bool {
    let p = lossy_norm(path);
    let b_full = lossy_norm(base);
    let b = b_full.trim_end_matches(['\\', '/']);
    if b.is_empty() {
        return false;
    }
    p == b
        || p.strip_prefix(b)
            .is_some_and(|rest| rest.starts_with(['\\', '/']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn lossy_norm_strips_verbatim_prefix_and_folds_case() {
        assert_eq!(
            lossy_norm(&PathBuf::from(r"\\?\C:\Users\Someone\Pictures")),
            r"c:\users\someone\pictures"
        );
    }

    #[test]
    fn lossy_norm_rewrites_verbatim_unc_to_plain_unc() {
        assert_eq!(
            lossy_norm(&PathBuf::from(r"\\?\UNC\Server\Share\file.png")),
            r"\\server\share\file.png"
        );
    }

    #[test]
    fn lossy_norm_leaves_posix_paths_untouched_except_case() {
        assert_eq!(
            lossy_norm(&PathBuf::from("/tmp/Images/a.PNG")),
            "/tmp/images/a.png"
        );
    }

    #[test]
    fn norm_starts_with_matches_verbatim_child_under_plain_base() {
        assert!(norm_starts_with(
            &PathBuf::from(r"\\?\C:\Users\Someone\Pictures\a.png"),
            &PathBuf::from(r"C:\Users\Someone"),
        ));
    }

    #[test]
    fn norm_starts_with_matches_despite_case_difference() {
        assert!(norm_starts_with(
            &PathBuf::from(r"C:\USERS\SOMEONE\a.png"),
            &PathBuf::from(r"c:\users\someone"),
        ));
        assert!(norm_starts_with(
            &PathBuf::from("/private/TMP/a.png"),
            &PathBuf::from("/private/tmp"),
        ));
    }

    #[test]
    fn norm_starts_with_accepts_exact_base_match() {
        assert!(norm_starts_with(
            &PathBuf::from(r"\\?\C:\Users\Someone"),
            &PathBuf::from(r"C:\Users\Someone\"),
        ));
    }

    #[test]
    fn norm_starts_with_rejects_sibling_prefix_without_separator() {
        // "C:\Users\Someone-evil" must not match base "C:\Users\Someone".
        assert!(!norm_starts_with(
            &PathBuf::from(r"C:\Users\Someone-evil\a.png"),
            &PathBuf::from(r"C:\Users\Someone"),
        ));
        assert!(!norm_starts_with(
            &PathBuf::from("/tmpfoo/a.png"),
            &PathBuf::from("/tmp"),
        ));
    }

    #[test]
    fn norm_starts_with_rejects_unrelated_path() {
        assert!(!norm_starts_with(
            &PathBuf::from(r"D:\Other\a.png"),
            &PathBuf::from(r"C:\Users\Someone"),
        ));
    }

    #[test]
    fn norm_starts_with_rejects_empty_base() {
        assert!(!norm_starts_with(
            &PathBuf::from("/tmp/a.png"),
            &PathBuf::from("")
        ));
    }

    #[test]
    fn norm_starts_with_posix_child_under_base() {
        assert!(norm_starts_with(
            &PathBuf::from("/var/folders/ab/xyz/T/img.png"),
            &PathBuf::from("/var/folders"),
        ));
    }
}
