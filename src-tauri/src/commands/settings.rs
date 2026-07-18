use anyhow::{anyhow, Context};
use serde::Serialize;

use crate::error::{CommandError, CommandResult};
use crate::exporter;
use crate::models::{IndexStats, PricingCatalogStatus, ProviderSnapshot};
use crate::pricing::{
    count_models_dev_models, parse_catalog, parse_models_dev, PRICING_CATALOG_JSON_KEY,
    PRICING_CATALOG_MODEL_COUNT_KEY, PRICING_CATALOG_UPDATED_AT_KEY, PRICING_CATALOG_URL,
};
use crate::services::{EventBus, ProviderSnapshotService};

use super::sessions::load_detail;
use super::AppState;

#[derive(Clone, Serialize)]
struct MaintenanceEventPayload {
    job: &'static str,
    phase: &'static str,
    message: Option<String>,
}

fn emit_maintenance(
    events: &dyn EventBus,
    job: &'static str,
    phase: &'static str,
    message: Option<String>,
) {
    match serde_json::to_value(MaintenanceEventPayload {
        job,
        phase,
        message,
    }) {
        Ok(payload) => events.emit("maintenance-status", payload),
        Err(e) => log::warn!("failed to serialize maintenance event: {e:#}"),
    }
}

pub async fn get_index_stats(state: AppState) -> CommandResult<IndexStats> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<IndexStats> {
        let session_count = state
            .db
            .session_count()
            .context("failed to get session count")?;

        let db_size_bytes = state.db.db_size_bytes();

        let last_index_time = state
            .db
            .get_meta("last_index_time")
            .context("failed to read last_index_time")?
            .unwrap_or_default();
        let usage_last_refreshed_at = state
            .db
            .get_meta("usage_last_refreshed_at")
            .context("failed to read usage_last_refreshed_at")?
            .unwrap_or_default();

        Ok(IndexStats {
            session_count,
            db_size_bytes,
            last_index_time,
            usage_last_refreshed_at,
        })
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn get_pricing_catalog_status(state: AppState) -> CommandResult<PricingCatalogStatus> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<PricingCatalogStatus> {
        let updated_at = state
            .db
            .get_meta(PRICING_CATALOG_UPDATED_AT_KEY)
            .context("failed to read pricing updated_at")?;
        let model_count = if let Some(raw_count) = state
            .db
            .get_meta(PRICING_CATALOG_MODEL_COUNT_KEY)
            .context("failed to read pricing model count")?
        {
            raw_count
                .parse::<u64>()
                .with_context(|| format!("invalid stored pricing model count '{raw_count}'"))?
        } else if let Some(json) = state
            .db
            .get_meta(PRICING_CATALOG_JSON_KEY)
            .context("failed to read pricing catalog JSON")?
        {
            parse_catalog(&json)
                .context("invalid stored pricing catalog JSON")?
                .len() as u64
        } else {
            0
        };

        Ok(PricingCatalogStatus {
            updated_at,
            model_count,
        })
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn refresh_pricing_catalog(state: AppState) -> CommandResult<PricingCatalogStatus> {
    // Bounded timeout: the first-use bootstrap awaits this before the initial
    // reindex, so a hung connection must not block indexing forever.
    // reqwest's rustls-no-provider feature requires an explicit process-wide
    // provider before Client construction; updater checks may run later.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("failed to build pricing catalog client")?;
    let response = client
        .get(PRICING_CATALOG_URL)
        .send()
        .await
        .context("failed to fetch pricing catalog")?;
    let response = response
        .error_for_status()
        .context("pricing catalog request failed")?;
    let body = response
        .text()
        .await
        .context("failed to read pricing catalog body")?;
    let model_count = count_models_dev_models(&body).context("invalid models.dev JSON")?;
    let catalog = parse_models_dev(&body).context("invalid models.dev JSON")?;
    let body = serde_json::to_string(&catalog).context("failed to serialize pricing catalog")?;
    let updated_at = chrono::Utc::now().to_rfc3339();

    // DB writes can wait on the busy timeout when another instance holds the
    // write lock — keep them off the async runtime like every other command.
    let stored_updated_at = updated_at.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        state
            .db
            .set_meta(PRICING_CATALOG_JSON_KEY, &body)
            .context("failed to store pricing catalog")?;
        state
            .db
            .set_meta(PRICING_CATALOG_UPDATED_AT_KEY, &stored_updated_at)
            .context("failed to store pricing timestamp")?;
        state
            .db
            .set_meta(PRICING_CATALOG_MODEL_COUNT_KEY, &model_count.to_string())
            .context("failed to store pricing model count")?;
        Ok(())
    })
    .await
    .context("task join error")??;

    Ok(PricingCatalogStatus {
        updated_at: Some(updated_at),
        model_count,
    })
}

pub async fn start_rebuild_index(state: AppState) -> CommandResult<bool> {
    use std::sync::atomic::Ordering;

    if state.maintenance_running.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }

    tokio::spawn(async move {
        emit_maintenance(&*state.events, "rebuild_index", "started", None);
        let result = tokio::task::spawn_blocking({
            let state = state.clone();
            move || state.indexer.reindex()
        })
        .await
        .map_err(|e| format!("task join error: {e:#}"))
        .and_then(|result| result.map_err(|e| e.to_string()));

        match result {
            Ok(_) => emit_maintenance(&*state.events, "rebuild_index", "finished", None),
            Err(error) => emit_maintenance(&*state.events, "rebuild_index", "failed", Some(error)),
        }
        state.maintenance_running.store(false, Ordering::SeqCst);
    });

    Ok(true)
}

pub async fn clear_index(state: AppState) -> CommandResult<()> {
    tokio::task::spawn_blocking(move || state.db.clear_all().context("failed to clear index"))
        .await
        .context("task join error")?
        .map_err(CommandError::from)?;
    Ok(())
}

/// Clear cached usage stats and invalidate the incremental-scan snapshot so
/// the next reindex re-parses every file. Used by the first-use bootstrap to
/// re-price stats that were indexed before a pricing catalog existed.
pub async fn clear_usage_stats(state: AppState) -> CommandResult<()> {
    tokio::task::spawn_blocking(move || {
        state
            .db
            .clear_usage_stats()
            .context("failed to clear usage stats")
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)?;
    Ok(())
}

pub async fn start_refresh_usage(state: AppState) -> CommandResult<bool> {
    use std::sync::atomic::Ordering;

    if state.maintenance_running.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }

    tokio::spawn(async move {
        emit_maintenance(&*state.events, "refresh_usage", "started", None);
        // Full forced reparse; token stats are swapped per-session inside the
        // provider commits. No destructive global clear up front — a failure
        // part-way leaves the previous stats intact instead of an empty panel.
        let result = tokio::task::spawn_blocking({
            let state = state.clone();
            move || {
                state
                    .indexer
                    .refresh_usage()
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
        })
        .await
        .map_err(|e| format!("task join error: {e:#}"))
        .and_then(|result| result);

        match result {
            Ok(_) => emit_maintenance(&*state.events, "refresh_usage", "finished", None),
            Err(error) => emit_maintenance(&*state.events, "refresh_usage", "failed", Some(error)),
        }
        state.maintenance_running.store(false, Ordering::SeqCst);
    });

    Ok(true)
}

pub async fn get_provider_snapshots(state: AppState) -> CommandResult<Vec<ProviderSnapshot>> {
    tokio::task::spawn_blocking(move || ProviderSnapshotService::new(&state.db).list())
        .await
        .context("task join error")?
        .map_err(CommandError::from)
}

pub async fn export_session(
    session_id: String,
    format: String,
    output_path: String,
    state: AppState,
) -> CommandResult<()> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let detail = load_detail(&session_id, &state.db)?;
        exporter::export(&detail, &format, &output_path)?;
        Ok(())
    })
    .await
    .context("task join error")?
    .map_err(CommandError::from)
}

pub async fn export_sessions_batch(
    items: Vec<String>,
    format: String,
    output_path: String,
    state: AppState,
) -> CommandResult<()> {
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = std::fs::File::create(&output_path).context("failed to create zip file")?;
        write_sessions_zip(&state, &items, &format, std::io::BufWriter::new(file))?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow!("task join error: {e}"))??;
    Ok(())
}

/// Render `items` into a zip archive on `writer`, emitting `export-progress`
/// events per entry. Shared by the file-path batch export (GUI save dialog)
/// and the headless streaming download.
pub(crate) fn write_sessions_zip<W: std::io::Write + std::io::Seek>(
    state: &AppState,
    items: &[String],
    format: &str,
    writer: W,
) -> anyhow::Result<()> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(writer);
    let options = zip::write::SimpleFileOptions::default();
    let total = items.len();

    for (idx, session_id) in items.iter().enumerate() {
        state.events.emit(
            "export-progress",
            serde_json::json!({ "current": idx + 1, "total": total }),
        );
        let detail = load_detail(session_id, &state.db)?;
        let ext = export_extension(format)?;
        // Append short session ID suffix to prevent filename collisions
        let id_suffix = if session_id.len() > 8 {
            &session_id[..8]
        } else {
            session_id.as_str()
        };
        let filename = format!(
            "{}_{}.{}",
            sanitize_filename(&detail.meta.title),
            id_suffix,
            ext
        );
        let content = render_session_export(&detail, format)?;
        zip.start_file(&filename, options)
            .context("failed to write zip entry")?;
        zip.write_all(content.as_bytes())
            .context("failed to write zip content")?;
    }
    zip.finish().context("failed to finish zip")?;
    Ok(())
}

/// File extension for a supported export format.
pub(crate) fn export_extension(format: &str) -> anyhow::Result<&'static str> {
    match format {
        "json" => Ok("json"),
        "markdown" | "md" => Ok("md"),
        _ => anyhow::bail!("unsupported export format: {format}"),
    }
}

/// Render one session to its export representation.
pub(crate) fn render_session_export(
    detail: &crate::models::SessionDetail,
    format: &str,
) -> anyhow::Result<String> {
    match format {
        "json" => serde_json::to_string_pretty(detail).context("failed to serialize session"),
        "markdown" | "md" => Ok(crate::exporter::markdown::render(detail)),
        _ => anyhow::bail!("unsupported export format: {format}"),
    }
}

pub(crate) fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .take(100)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    #[test]
    fn configured_rustls_provider_builds_reqwest_client() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder().build().unwrap();
    }

    #[test]
    fn default_zip_options_create_readable_deflated_entries() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("session.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"session export").unwrap();

        let cursor = writer.finish().unwrap();
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let mut entry = archive.by_name("session.md").unwrap();
        let mut content = String::new();
        entry.read_to_string(&mut content).unwrap();

        assert_eq!(entry.compression(), zip::CompressionMethod::Deflated);
        assert_eq!(content, "session export");
    }
}
