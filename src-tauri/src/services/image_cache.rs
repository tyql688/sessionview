use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::models::Message;
use crate::services::image_markers::{extract_image_source_segments, extract_path_from_segment};

// ---------------------------------------------------------------------------
// Path extraction
// ---------------------------------------------------------------------------

/// Walk `messages` and return every `[Image: source: ...]` payload as
/// an owned `String`, in document order. Provider-agnostic — works for
/// any parser that emits the universal marker format.
///
/// Data URIs and remote URLs are returned alongside file paths; the
/// caller (e.g. `ImageCacheService::cache_images`) filters out anything
/// that isn't a readable local file.
pub(crate) fn extract_image_paths(messages: &[Message]) -> Vec<String> {
    let mut paths = Vec::new();
    for msg in messages {
        for segment in extract_image_source_segments(&msg.content) {
            if let Some(path) = extract_path_from_segment(&segment) {
                paths.push(path.to_string());
            }
        }
    }
    paths
}

// ---------------------------------------------------------------------------
// Data directory helper
// ---------------------------------------------------------------------------

/// Resolve the app data directory for image caching.
pub(crate) fn image_cache_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("cc-session"))
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

pub(crate) struct ImageCacheService {
    cache_dir: PathBuf,
}

impl ImageCacheService {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            cache_dir: data_dir.join("images"),
        }
    }

    pub(crate) fn cache_name(original_path: &str) -> String {
        let hash = Sha256::digest(original_path.as_bytes());
        let hex = format!("{hash:x}");
        let ext = Path::new(original_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png");
        format!("{hex}.{ext}")
    }

    /// Copy every readable local image referenced by `messages` into
    /// the cache directory (idempotent — existing cache entries are
    /// left untouched). Data URIs and remote URLs naturally fall
    /// through because `Path::exists()` returns false for them.
    pub(crate) fn cache_images(&self, messages: &[Message]) {
        let paths = extract_image_paths(messages);
        if paths.is_empty() {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&self.cache_dir) {
            log::warn!("failed to create image cache dir: {e}");
            return;
        }
        for path in &paths {
            let cache_name = Self::cache_name(path);
            let cache_path = self.cache_dir.join(&cache_name);
            if cache_path.exists() {
                continue;
            }
            let original = Path::new(path);
            if !original.exists() {
                continue;
            }
            if let Err(e) = std::fs::copy(original, &cache_path) {
                log::warn!("failed to cache image {path}: {e}");
            }
        }
    }

    pub(crate) fn resolve_cached_path(&self, original_path: &str) -> Option<PathBuf> {
        let cache_path = self.cache_dir.join(Self::cache_name(original_path));
        cache_path.exists().then_some(cache_path)
    }

    /// Remove every cache entry referenced by `messages`. Used when a
    /// session is permanently deleted so we don't leak disk space.
    pub(crate) fn cleanup_images(&self, messages: &[Message]) {
        let paths = extract_image_paths(messages);
        for path in &paths {
            let cache_name = Self::cache_name(path);
            let cache_path = self.cache_dir.join(&cache_name);
            if cache_path.exists() {
                if let Err(e) = std::fs::remove_file(&cache_path) {
                    log::warn!(
                        "failed to remove cached image {}: {e}",
                        cache_path.display()
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MessageRole;
    use std::io::Write;
    use tempfile::TempDir;

    fn msg(content: &str) -> Message {
        Message {
            role: MessageRole::Assistant,
            message_kind: None,
            content: content.to_string(),
            timestamp: None,
            tool_name: None,
            tool_input: None,
            tool_metadata: None,
            token_usage: None,
            model: None,
            usage_hash: None,
        }
    }

    #[test]
    fn extract_image_paths_returns_inner_paths_in_order() {
        let messages = vec![
            msg("Here is the result [Image: source: /tmp/test-image-cache/sess/1.png] done"),
            msg("Another [Image: source: /tmp/screenshot.jpg] and [Image: source: /tmp/test-image-cache/sess/2.png]"),
            msg("No images here"),
        ];
        let paths = extract_image_paths(&messages);
        assert_eq!(
            paths,
            vec![
                "/tmp/test-image-cache/sess/1.png",
                "/tmp/screenshot.jpg",
                "/tmp/test-image-cache/sess/2.png",
            ]
        );
    }

    #[test]
    fn extract_image_paths_returns_empty_for_no_images() {
        let messages = vec![msg("just text"), msg("more text")];
        assert!(extract_image_paths(&messages).is_empty());
    }

    #[test]
    fn cache_name_is_deterministic() {
        let name = ImageCacheService::cache_name("/tmp/test-image-cache/sess/1.png");
        assert_eq!(
            name,
            ImageCacheService::cache_name("/tmp/test-image-cache/sess/1.png")
        );
        assert!(name.ends_with(".png"));
        assert_eq!(name.len(), 64 + 4); // 64 hex + ".png"
    }

    #[test]
    fn cache_name_defaults_to_png_for_no_extension() {
        let name = ImageCacheService::cache_name("/some/path/noext");
        assert!(name.ends_with(".png"));
    }

    #[test]
    fn cache_and_resolve_round_trip() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("data");
        let service = ImageCacheService::new(&data_dir);

        let img_dir = tmp.path().join("images");
        std::fs::create_dir_all(&img_dir).unwrap();
        let img_path = img_dir.join("test.png");
        let mut f = std::fs::File::create(&img_path).unwrap();
        f.write_all(b"fake png data").unwrap();
        let img_path_str = img_path.to_str().unwrap();

        assert!(service.resolve_cached_path(img_path_str).is_none());

        let messages = vec![msg(&format!("[Image: source: {img_path_str}]"))];
        service.cache_images(&messages);

        let cached = service.resolve_cached_path(img_path_str);
        assert!(cached.is_some());
        assert_eq!(std::fs::read(cached.unwrap()).unwrap(), b"fake png data");

        service.cleanup_images(&messages);
        assert!(service.resolve_cached_path(img_path_str).is_none());
    }

    #[test]
    fn cache_skips_missing_original() {
        let tmp = TempDir::new().unwrap();
        let service = ImageCacheService::new(tmp.path());
        let messages = vec![msg("[Image: source: /nonexistent/path/img.png]")];
        service.cache_images(&messages);
        assert!(service
            .resolve_cached_path("/nonexistent/path/img.png")
            .is_none());
    }

    #[test]
    fn cache_skips_data_uri_source() {
        // Codex emits inline base64 markers for tiny screenshots. The
        // path test fails (data URIs aren't files), so cache_images
        // must silently skip without panicking.
        let tmp = TempDir::new().unwrap();
        let service = ImageCacheService::new(tmp.path());
        let messages = vec![msg("[Image: source: data:image/png;base64,iVBOR...]")];
        service.cache_images(&messages);
        assert!(service
            .resolve_cached_path("data:image/png;base64,iVBOR...")
            .is_none());
    }
}
