use std::io::Read;
use std::path::Path;

use anyhow::{Context, anyhow};

use crate::error::CommandResult;

use super::AppState;

/// Session images must live under the user home or system temp.
fn read_image_canonical_allowed(canonical: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return tmp_dir_allows_image(canonical);
    };
    if canonical_under_home(canonical, &home) {
        return true;
    }
    tmp_dir_allows_image(canonical)
}

/// Whether `canonical` lies under the user's profile directory.
#[cfg(windows)]
fn canonical_under_home(canonical: &Path, home: &Path) -> bool {
    if canonical.starts_with(home) {
        return true;
    }
    if let Ok(home_canon) = home.canonicalize() {
        if canonical.starts_with(&home_canon) {
            return true;
        }
    }
    // Last resort: normalized comparison (strips Windows verbatim `\\?\`,
    // folds case). Covers prefix-form disagreements between paths.
    crate::services::path_norm::norm_starts_with(canonical, home)
}

#[cfg(not(windows))]
fn canonical_under_home(canonical: &Path, home: &Path) -> bool {
    canonical.starts_with(home)
}

#[cfg(not(target_os = "windows"))]
fn tmp_dir_allows_image(canonical: &Path) -> bool {
    let s = canonical.to_string_lossy();
    s.starts_with("/tmp/")
        || s.starts_with("/private/tmp/")
        || s.starts_with("/var/folders/")
        || s.starts_with("/private/var/folders/")
}

#[cfg(target_os = "windows")]
fn tmp_dir_allows_image(canonical: &Path) -> bool {
    use crate::services::path_norm::norm_starts_with;
    ["TEMP", "TMP"].iter().any(|key| {
        std::env::var(key).ok().is_some_and(|raw| {
            let base = Path::new(raw.trim());
            match base.canonicalize() {
                Ok(c) => norm_starts_with(canonical, &c),
                Err(_) => norm_starts_with(canonical, base),
            }
        })
    })
}

pub async fn read_image_base64(path: String) -> CommandResult<String> {
    super::blocking(move || read_image_base64_sync(&path)).await
}

fn read_image_base64_sync(path: &str) -> CommandResult<String> {
    use crate::services::image_cache::{ImageCacheService, image_cache_data_dir};

    let path = path.trim().trim_start_matches('\u{feff}').to_string();
    let p = Path::new(&path);

    // Determine which file to read: original if it exists, else cached copy
    let resolved = if p.exists() {
        p.to_path_buf()
    } else {
        // Try cache fallback
        let data_dir = image_cache_data_dir().ok_or_else(|| anyhow!("image not found: {path}"))?;
        let service = ImageCacheService::new(&data_dir);
        service
            .resolve_cached_path(&path)
            .ok_or_else(|| anyhow!("image not found: {path}"))?
    };

    // Reachable over HTTP: fail closed, no canonical path means no read.
    let canonical = resolved
        .canonicalize()
        .with_context(|| format!("failed to resolve image path {}", resolved.display()))?;
    if !read_image_canonical_allowed(&canonical) {
        log::warn!(
            "read_image_base64 denied (not under home/temp): {}",
            canonical.display()
        );
        return Err(anyhow!("image path not allowed: {path}").into());
    }
    read_image_within(&canonical, MAX_IMAGE_BYTES)
}

/// Read and encode an already-authorized image path, refusing anything over
/// `max_bytes` or whose bytes are not an image.
fn read_image_within(canonical: &Path, max_bytes: u64) -> CommandResult<String> {
    use base64::{Engine, engine::general_purpose::STANDARD};

    // A FIFO under the home dir would block this thread forever on open.
    if !std::fs::metadata(canonical)
        .with_context(|| format!("failed to stat {}", canonical.display()))?
        .is_file()
    {
        return Err(anyhow!("not a regular file: {}", canonical.display()).into());
    }
    // Cap the read itself: the file may grow after the stat.
    let mut data = Vec::new();
    std::io::copy(
        &mut std::fs::File::open(canonical)
            .with_context(|| format!("failed to open {}", canonical.display()))?
            .take(max_bytes + 1),
        &mut data,
    )
    .with_context(|| format!("failed to read {}", canonical.display()))?;
    if data.len() as u64 > max_bytes {
        return Err(anyhow!("image exceeds {max_bytes} bytes").into());
    }

    // Sniff first, or naming a secret `x.png` turns this into a file reader.
    // SVG is text with no magic bytes, so it stays extension-gated.
    let extension = canonical
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_lowercase);
    let mime = match infer::get(&data).map(|kind| kind.mime_type()) {
        Some(mime) if mime.starts_with("image/") => mime,
        _ if extension.as_deref() == Some("svg") => "image/svg+xml",
        _ => return Err(anyhow!("not a recognized image").into()),
    };
    let b64 = STANDARD.encode(&data);
    Ok(format!("data:{mime};base64,{b64}"))
}

const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

fn read_tool_result_canonical_allowed(canonical: &Path) -> bool {
    if !canonical
        .components()
        .any(|component| component.as_os_str() == "tool-results")
    {
        return false;
    }

    let Some(home) = dirs::home_dir() else {
        return false;
    };
    [home.join(".claude"), home.join(".cc-mirror")]
        .iter()
        .any(|base| match base.canonicalize() {
            Ok(base) => canonical.starts_with(base),
            Err(_) => canonical.starts_with(base),
        })
}

pub async fn read_tool_result_text(path: String) -> CommandResult<String> {
    super::blocking(move || read_tool_result_text_sync(&path)).await
}

/// Resolve a `<persisted-output>` referenced file lazily on demand.
/// The frontend calls this when rendering a Claude tool result that
/// contains a "Full output saved to: <path>" payload, so parse-time
/// session loads no longer pay the synchronous fs cost per message.
pub async fn resolve_persisted_output(path: String, state: AppState) -> CommandResult<String> {
    super::blocking(move || resolve_persisted_output_sync(&path, &state)).await
}

fn persisted_output_canonical_allowed(canonical: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    [home.join(".claude"), home.join(".cc-mirror")]
        .iter()
        .any(|base| match base.canonicalize() {
            Ok(b) => canonical.starts_with(&b),
            Err(_) => canonical.starts_with(base),
        })
}

fn resolve_persisted_output_sync(path: &str, state: &AppState) -> CommandResult<String> {
    let path = path.trim().trim_start_matches('\u{feff}').to_string();
    let p = Path::new(&path);
    if !p.exists() {
        return Err(anyhow!("persisted output not found: {path}").into());
    }

    let canonical = p
        .canonicalize()
        .with_context(|| format!("failed to resolve persisted output '{path}'"))?;

    if !persisted_output_canonical_allowed(&canonical) {
        log::warn!(
            "resolve_persisted_output denied (outside ~/.claude or ~/.cc-mirror): {}",
            canonical.display()
        );
        return Err(anyhow!("persisted output path not allowed: {path}").into());
    }

    let content = state
        .persisted_output_cache
        .get_or_load(&canonical)
        .with_context(|| format!("failed to read persisted output {}", canonical.display()))?;
    Ok(content)
}

fn read_tool_result_text_sync(path: &str) -> CommandResult<String> {
    const MAX_TOOL_RESULT_BYTES: u64 = 1_000_000;

    let path = path.trim().trim_start_matches('\u{feff}').to_string();
    let p = Path::new(&path);
    if !p.exists() {
        return Err(anyhow!("tool result not found: {path}").into());
    }

    let canonical = p
        .canonicalize()
        .with_context(|| format!("failed to resolve tool result '{path}'"))?;
    if !read_tool_result_canonical_allowed(&canonical) {
        log::warn!(
            "read_tool_result_text denied (outside tool-results): {}",
            canonical.display()
        );
        return Err(anyhow!("tool result path not allowed: {path}").into());
    }

    let metadata = std::fs::metadata(&canonical)
        .with_context(|| format!("failed to inspect tool result {path}"))?;
    if metadata.len() > MAX_TOOL_RESULT_BYTES {
        return Err(anyhow!(
            "tool result is too large to preview ({} bytes)",
            metadata.len()
        )
        .into());
    }

    let text = std::fs::read_to_string(&canonical)
        .with_context(|| format!("failed to read tool result {path}"))?;
    Ok(text)
}

pub async fn open_in_folder(path: String) -> CommandResult<()> {
    super::blocking(move || open_in_folder_sync(&path)).await
}

fn open_in_folder_sync(path: &str) -> CommandResult<()> {
    // Session text often references files as ~/... — expand before checks.
    let expanded: std::path::PathBuf = if let Some(rest) = path.strip_prefix("~/") {
        match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => return Err(anyhow!("cannot resolve home directory").into()),
        }
    } else {
        std::path::PathBuf::from(path)
    };
    let path = expanded.to_string_lossy().as_ref().to_string();
    let path = path.as_str();
    let p = Path::new(path);
    if !p.exists() {
        return Err(anyhow!("path not found: {path}").into());
    }
    // Validate path is under HOME to prevent opening arbitrary system directories
    let canonical = p
        .canonicalize()
        .with_context(|| format!("failed to resolve path '{path}'"))?;
    let home_ok = dirs::home_dir().is_some_and(|h| canonical.starts_with(&h));
    if !home_ok {
        return Err(anyhow!("path not allowed: {path}").into());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .context("failed to open")?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .context("failed to open")?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .context("failed to open")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_IMAGE_BYTES, read_image_within};

    fn png_bytes() -> Vec<u8> {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&[0u8; 64]);
        png
    }

    #[test]
    fn serves_images_and_refuses_everything_else() {
        let dir = tempfile::tempdir().unwrap();

        let sniffed = dir.path().join("screenshot");
        std::fs::write(&sniffed, png_bytes()).unwrap();
        assert!(
            read_image_within(&sniffed, MAX_IMAGE_BYTES)
                .unwrap()
                .starts_with("data:image/png;base64,")
        );

        // Named like an image, but not one.
        let disguised = dir.path().join("id_rsa.png");
        std::fs::write(&disguised, b"-----BEGIN OPENSSH PRIVATE KEY-----").unwrap();
        assert!(read_image_within(&disguised, MAX_IMAGE_BYTES).is_err());

        // Text, so extension-gated rather than sniffed.
        let svg = dir.path().join("diagram.svg");
        std::fs::write(&svg, b"<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        assert!(
            read_image_within(&svg, MAX_IMAGE_BYTES)
                .unwrap()
                .starts_with("data:image/svg+xml;base64,")
        );

        // The cap is enforced on the read, at exactly the limit.
        let png = png_bytes();
        assert!(read_image_within(&sniffed, png.len() as u64).is_ok());
        assert!(read_image_within(&sniffed, png.len() as u64 - 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_non_regular_files_instead_of_blocking() {
        use std::os::unix::fs::FileTypeExt;

        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.png");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must run");
        assert!(status.success());
        assert!(std::fs::metadata(&fifo).unwrap().file_type().is_fifo());

        // Returns instead of hanging: the guard rejects it before the open.
        assert!(read_image_within(&fifo, MAX_IMAGE_BYTES).is_err());
    }
}
