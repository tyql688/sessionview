pub(crate) fn contains_image_source(text: &str) -> bool {
    text.contains("[Image: source:") || text.contains("[Image source:")
}

pub(crate) fn contains_image_placeholder_without_source(text: &str) -> bool {
    text.contains("[Image") && !contains_image_source(text)
}

pub(crate) fn count_image_markers(text: &str) -> usize {
    let mut count = 0;
    let mut remaining = text;

    while let Some(start) = remaining.find("[Image") {
        let image_slice = &remaining[start..];
        let Some(end_offset) = image_slice.find(']') else {
            break;
        };

        count += 1;
        remaining = &image_slice[end_offset + 1..];
    }

    count
}

pub(crate) fn normalize_image_source_segments(text: &str) -> String {
    let mut normalized = String::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("[Image") {
        normalized.push_str(&remaining[..start]);
        let image_slice = &remaining[start..];
        let Some(end_offset) = image_slice.find(']') else {
            normalized.push_str(image_slice);
            return normalized;
        };

        let candidate = &image_slice[..=end_offset];
        if let Some(source) = parse_image_source_segment(candidate) {
            normalized.push_str(&source);
        } else {
            normalized.push_str(candidate);
        }

        remaining = &image_slice[end_offset + 1..];
    }

    normalized.push_str(remaining);
    normalized
}

pub(crate) fn merge_image_placeholders_with_sources(
    placeholder_text: &str,
    meta_text: &str,
) -> String {
    let sources = extract_image_source_segments(meta_text);
    if sources.is_empty() {
        return placeholder_text.to_string();
    }

    let mut merged = String::new();
    let mut remaining = placeholder_text;
    let mut source_index = 0usize;

    while let Some(start) = remaining.find("[Image") {
        merged.push_str(&remaining[..start]);
        let image_slice = &remaining[start..];
        let Some(end_offset) = image_slice.find(']') else {
            merged.push_str(image_slice);
            remaining = "";
            break;
        };

        let candidate = &image_slice[..=end_offset];
        if source_index < sources.len() && is_image_placeholder(candidate) {
            merged.push_str(&sources[source_index]);
            source_index += 1;
        } else {
            merged.push_str(candidate);
        }

        remaining = &image_slice[end_offset + 1..];
    }

    merged.push_str(remaining);

    if source_index < sources.len() {
        if !merged.is_empty() && !merged.ends_with('\n') {
            merged.push('\n');
        }
        merged.push_str(&sources[source_index..].join("\n"));
    }

    merged
}

// `extract_image_source_segments`, `is_image_placeholder`, and the
// shared `parse_image_source_segment` validator live in
// `services::image_markers` because every provider's parser emits the
// same `[Image: source: ...]` marker — re-export the ones this module
// already exposed so existing callers compile unchanged.
pub(crate) use crate::services::image_markers::{
    extract_image_source_segments, is_image_placeholder, parse_image_source_segment,
};

#[cfg(test)]
mod tests {
    use super::{
        count_image_markers, extract_image_source_segments, merge_image_placeholders_with_sources,
        normalize_image_source_segments,
    };

    #[test]
    fn normalizes_new_claude_image_source_marker_format() {
        let text = "[Image source: /tmp/demo.png]";
        assert_eq!(
            normalize_image_source_segments(text),
            "[Image: source: /tmp/demo.png]"
        );
    }

    #[test]
    fn extracts_new_claude_image_source_marker_format() {
        let text = "[Image source: /tmp/demo.png]";
        assert_eq!(
            extract_image_source_segments(text),
            vec!["[Image: source: /tmp/demo.png]".to_string()]
        );
    }

    #[test]
    fn counts_image_markers() {
        assert_eq!(count_image_markers("before [Image #1] after [Image]"), 2);
        assert_eq!(count_image_markers("no image"), 0);
    }

    #[test]
    fn merges_placeholder_with_new_claude_image_source_marker_format() {
        assert_eq!(
            merge_image_placeholders_with_sources(
                "before [Image #1] after",
                "[Image source: /tmp/demo.png]"
            ),
            "before [Image: source: /tmp/demo.png] after"
        );
    }
}
