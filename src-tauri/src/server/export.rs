//! Streaming export downloads for the headless shell. Browsers cannot write
//! to arbitrary local paths, so instead of the GUI's native save dialog the
//! frontend requests these endpoints and lets the browser download the file.

use anyhow::Context;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::commands::{
    export_extension, load_detail, render_session_export, sanitize_filename, write_sessions_zip,
};

use super::ServerCtx;

#[derive(Deserialize)]
pub struct ExportQuery {
    format: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchExportRequest {
    items: Vec<String>,
    format: String,
}

pub async fn export_session_download(
    State(ctx): State<ServerCtx>,
    Path(session_id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Response {
    let state = ctx.state.clone();
    let result =
        tokio::task::spawn_blocking(move || -> anyhow::Result<(String, &'static str, String)> {
            let detail = load_detail(&session_id, &state.db)?;
            let ext = export_extension(&query.format)?;
            let content = render_session_export(&detail, &query.format)?;
            let filename = format!("{}.{}", sanitize_filename(&detail.meta.title), ext);
            let content_type = match ext {
                "json" => "application/json; charset=utf-8",
                _ => "text/markdown; charset=utf-8",
            };
            Ok((filename, content_type, content))
        })
        .await
        .context("task join error")
        .and_then(|r| r);

    match result {
        Ok((filename, content_type, content)) => {
            download_response(&filename, content_type, content.into_bytes())
        }
        Err(e) => error_response(e),
    }
}

pub async fn export_batch_download(
    State(ctx): State<ServerCtx>,
    Json(request): Json<BatchExportRequest>,
) -> Response {
    if request.items.is_empty() {
        return (StatusCode::BAD_REQUEST, "no sessions selected").into_response();
    }
    let state = ctx.state.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        write_sessions_zip(&state, &request.items, &request.format, &mut cursor)?;
        Ok(cursor.into_inner())
    })
    .await
    .context("task join error")
    .and_then(|r| r);

    match result {
        Ok(bytes) => download_response("sessions-export.zip", "application/zip", bytes),
        Err(e) => error_response(e),
    }
}

fn error_response(e: anyhow::Error) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response()
}

fn download_response(filename: &str, content_type: &str, bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"{}\"; filename*=UTF-8''{}",
                    ascii_fallback(filename),
                    percent_encode(filename)
                ),
            ),
        ],
        bytes,
    )
        .into_response()
}

/// ASCII-only fallback for the plain `filename=` parameter; non-ASCII and
/// quote-breaking characters become `_`.
fn ascii_fallback(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// RFC 5987 percent-encoding for the UTF-8 `filename*` parameter.
fn percent_encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len() * 3);
    for byte in name.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_escapes_non_ascii() {
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("会"), "%E4%BC%9A");
        assert_eq!(percent_encode("safe-name_1.md"), "safe-name_1.md");
    }

    #[test]
    fn ascii_fallback_replaces_unsafe_chars() {
        assert_eq!(ascii_fallback("a\"b会.md"), "a_b_.md");
    }
}
