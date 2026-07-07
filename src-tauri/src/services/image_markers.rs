//! Provider-agnostic helpers for the `[Image: source: ...]` marker
//! format that every provider's parser emits.
//!
//! Centralising these here means the image cache pipeline doesn't need
//! per-provider extraction logic — any text written by any parser is
//! grepped the same way.
//!
//! Claude-specific transforms (placeholder/marker merging used to
//! reunite `tool_use` and `tool_result` content blocks) stay in
//! `providers::claude::images` because they encode quirks of Claude's
//! wire format, not the marker format itself.

/// Walk `text` and return each well-formed `[Image: source: ...]`
/// segment in document order. Used by the image cache to find paths
/// that need backing up and by parsers to count / validate markers.
pub(crate) fn extract_image_source_segments(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("[Image") {
        let image_slice = &remaining[start..];
        let Some(end_offset) = image_slice.find(']') else {
            break;
        };

        let candidate = &image_slice[..=end_offset];
        if let Some(source) = parse_image_source_segment(candidate) {
            segments.push(source);
        }

        remaining = &image_slice[end_offset + 1..];
    }

    segments
}

/// True iff `segment` is an `[Image ...]` marker without a
/// `source:` payload — a placeholder that wants to be merged with a
/// real source later.
pub(crate) fn is_image_placeholder(segment: &str) -> bool {
    segment.starts_with("[Image") && parse_image_source_segment(segment).is_none()
}

/// Pull the path/URL/data-URI out of an `[Image: source: ...]`
/// segment. Returns `None` for placeholders or malformed segments so
/// callers can skip silently.
pub(crate) fn extract_path_from_segment(segment: &str) -> Option<&str> {
    let trimmed = segment.strip_prefix("[Image: source: ")?;
    let path = trimmed.strip_suffix(']')?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    Some(path)
}

/// Parse a single `[Image: source: ...]` (or the older
/// `[Image source: ...]`) segment into the canonical
/// `[Image: source: <src>]` form. Returns `None` when the segment
/// doesn't carry a non-empty source.
pub(crate) fn parse_image_source_segment(segment: &str) -> Option<String> {
    if !segment.starts_with("[Image") || !segment.ends_with(']') {
        return None;
    }

    let inner = &segment["[Image".len()..segment.len() - 1];
    let source = inner
        .strip_prefix(": source:")
        .or_else(|| inner.strip_prefix(" source:"))?
        .trim();

    if source.is_empty() {
        return None;
    }

    Some(format!("[Image: source: {source}]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_canonical_format() {
        assert_eq!(
            extract_image_source_segments("a [Image: source: /tmp/x.png] b"),
            vec!["[Image: source: /tmp/x.png]"]
        );
    }

    #[test]
    fn extracts_legacy_space_format() {
        // Old Claude format without the colon after [Image still round-trips
        // through `parse_image_source_segment`'s canonicalisation.
        assert_eq!(
            extract_image_source_segments("[Image source: /tmp/x.png]"),
            vec!["[Image: source: /tmp/x.png]"]
        );
    }

    #[test]
    fn skips_placeholder_without_source() {
        assert!(extract_image_source_segments("[Image #1]").is_empty());
    }

    #[test]
    fn is_image_placeholder_detects_bare_marker() {
        assert!(is_image_placeholder("[Image #1]"));
        assert!(!is_image_placeholder("[Image: source: /tmp/x.png]"));
        assert!(!is_image_placeholder("plain text"));
    }

    #[test]
    fn extract_path_from_segment_returns_inner() {
        assert_eq!(
            extract_path_from_segment("[Image: source: /tmp/x.png]"),
            Some("/tmp/x.png")
        );
        assert_eq!(extract_path_from_segment("[Image #1]"), None);
        assert_eq!(extract_path_from_segment("[Image: source: ]"), None);
    }
}
