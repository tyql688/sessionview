//! Embedded frontend assets. Release builds embed `../dist` into the binary
//! (single-file distribution); debug builds read from disk so `vite build`
//! output is picked up without recompiling.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../dist"]
struct Assets;

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = Assets::get(path) {
        return asset_response(path, file);
    }

    // SPA fallback: unknown paths serve the app shell so client routing works.
    match Assets::get("index.html") {
        Some(index) => asset_response("index.html", index),
        None => (
            StatusCode::NOT_FOUND,
            "frontend assets missing — run `npm run build` before building the headless server",
        )
            .into_response(),
    }
}

fn asset_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    // Vite emits content-hashed filenames under assets/ — cache those hard;
    // everything else (index.html) must revalidate so deploys take effect.
    let cache_control = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    // Release builds embed assets as &'static [u8]; serve them zero-copy
    // instead of cloning multi-hundred-KB chunks per request.
    let body = match file.data {
        std::borrow::Cow::Borrowed(bytes) => axum::body::Bytes::from_static(bytes),
        std::borrow::Cow::Owned(bytes) => axum::body::Bytes::from(bytes),
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::CACHE_CONTROL, cache_control.to_string()),
        ],
        body,
    )
        .into_response()
}
