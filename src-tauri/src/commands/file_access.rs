use std::path::Path;

use anyhow::{anyhow, Context};
use tauri::State;

use crate::error::CommandResult;

use super::AppState;

/// Session images must live under the user home or system temp (same policy as HTML export).
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

#[tauri::command]
pub async fn read_image_base64(path: String) -> CommandResult<String> {
    tokio::task::spawn_blocking(move || read_image_base64_sync(&path))
        .await
        .context("task join error")?
}

fn read_image_base64_sync(path: &str) -> CommandResult<String> {
    use crate::services::image_cache::{image_cache_data_dir, ImageCacheService};
    use base64::{engine::general_purpose::STANDARD, Engine};

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

    if let Ok(canonical) = resolved.canonicalize() {
        if !read_image_canonical_allowed(&canonical) {
            log::warn!(
                "read_image_base64 denied (not under home/temp): {}",
                canonical.display()
            );
            return Err(anyhow!("image path not allowed: {path}").into());
        }
    }

    let ext = resolved
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        _ => "image/png",
    };

    let data = std::fs::read(&resolved)
        .with_context(|| format!("failed to read image {}", resolved.display()))?;
    let b64 = STANDARD.encode(&data);
    Ok(format!("data:{mime};base64,{b64}"))
}

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

#[tauri::command]
pub async fn read_tool_result_text(path: String) -> CommandResult<String> {
    tokio::task::spawn_blocking(move || read_tool_result_text_sync(&path))
        .await
        .context("task join error")?
}

/// Resolve a `<persisted-output>` referenced file lazily on demand.
/// The frontend calls this when rendering a Claude tool result that
/// contains a "Full output saved to: <path>" payload, so parse-time
/// session loads no longer pay the synchronous fs cost per message.
#[tauri::command]
pub async fn resolve_persisted_output(
    path: String,
    state: State<'_, AppState>,
) -> CommandResult<String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || resolve_persisted_output_sync(&path, &state))
        .await
        .context("task join error")?
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

#[tauri::command]
pub async fn open_in_folder(path: String) -> CommandResult<()> {
    tokio::task::spawn_blocking(move || open_in_folder_sync(&path))
        .await
        .context("task join error")?
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
