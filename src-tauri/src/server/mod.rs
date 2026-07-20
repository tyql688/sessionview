//! Headless HTTP shell: serves the same React frontend over HTTP and bridges
//! its backend calls onto the shared command cores (see `commands/`). The GUI
//! and headless shells share one data dir / SQLite index, so neither ever
//! re-indexes work the other already did.

mod assets;
mod dispatch;
mod events;
mod export;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use clap::Parser;
use serde::Deserialize;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::commands::AppState;
use dispatch::DispatchError;
use events::BroadcastEventBus;

/// SessionView headless server — browse local AI coding sessions from a browser.
#[derive(Parser, Debug)]
#[command(name = "sessionview-headless", version, about)]
struct Cli {
    /// Port to listen on.
    #[arg(long, default_value_t = 9921)]
    port: u16,

    /// Address to bind. Non-loopback addresses require --token.
    #[arg(long, default_value = "127.0.0.1")]
    host: IpAddr,

    /// Data directory override (defaults to the same directory the GUI uses,
    /// so index and favorites are shared).
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Require this token on every /api request (`X-SessionView-Token` header
    /// or `?token=` query parameter).
    #[arg(long, env = "SESSIONVIEW_TOKEN")]
    token: Option<String>,

    /// Open the browser after the server starts.
    #[arg(long)]
    open: bool,
}

#[derive(Clone)]
pub struct ServerCtx {
    pub state: AppState,
    bus: Arc<BroadcastEventBus>,
    token: Option<Arc<str>>,
}

/// Binary entry point: parse CLI args, build the runtime, serve until Ctrl-C.
pub fn cli_main() {
    let cli = Cli::parse();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to build async runtime: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = runtime.block_on(serve(cli)) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn serve(cli: Cli) -> anyhow::Result<()> {
    if !cli.host.is_loopback() && cli.token.is_none() {
        anyhow::bail!(
            "refusing to bind non-loopback address {} without --token: \
             session logs are sensitive. Pass --token <secret> (or SESSIONVIEW_TOKEN) \
             to expose the server beyond localhost.",
            cli.host
        );
    }

    let data_dir = match &cli.data_dir {
        Some(dir) => dir.clone(),
        None => crate::default_data_dir()?,
    };

    let bus = Arc::new(BroadcastEventBus::new(256));
    let state = crate::build_app_state(&data_dir, bus.clone())?;
    // Informational only — a transient read failure (e.g. the GUI holding
    // the shared DB busy) must not abort startup, but don't report a fake 0.
    let session_count = match state.db.session_count() {
        Ok(count) => count.to_string(),
        Err(error) => {
            log::warn!("failed to read indexed session count: {error}");
            "unknown".to_string()
        }
    };

    let ctx = ServerCtx {
        state,
        bus,
        token: cli.token.as_deref().map(Arc::from),
    };

    let api = Router::new()
        .route("/invoke/{command}", post(invoke_handler))
        .route("/events", get(events_handler))
        .route(
            "/export/{session_id}/download",
            get(export::export_session_download),
        )
        .route(
            "/export/batch/download",
            post(export::export_batch_download),
        )
        .layer(middleware::from_fn_with_state(ctx.clone(), require_token))
        .with_state(ctx);

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .nest("/api", api)
        .fallback(assets::static_handler);

    let addr = SocketAddr::new(cli.host, cli.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    let display_host = if cli.host.is_unspecified() {
        "127.0.0.1".to_string()
    } else {
        cli.host.to_string()
    };
    let url = format!("http://{display_host}:{}", cli.port);
    log::info!("SessionView headless listening on {url}");
    log::info!(
        "data dir: {} ({session_count} indexed sessions)",
        data_dir.display()
    );

    if cli.open {
        open_browser(&url);
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    Ok(())
}

async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        log::warn!("failed to listen for shutdown signal: {e}");
        std::future::pending::<()>().await;
    }
    log::info!("shutting down");
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let launcher = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let launcher = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let launcher = ("xdg-open", vec![url]);

    if let Err(e) = std::process::Command::new(launcher.0)
        .args(&launcher.1)
        .spawn()
    {
        log::warn!("failed to open browser: {e}");
    }
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

/// Token gate for /api. Static assets stay open (they are the public app
/// shell); everything that touches session data requires the token when one
/// is configured.
async fn require_token(
    State(ctx): State<ServerCtx>,
    Query(query): Query<TokenQuery>,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected) = ctx.token.as_deref() else {
        return next.run(request).await;
    };
    let header_token = request
        .headers()
        .get("x-sessionview-token")
        .and_then(|v| v.to_str().ok());
    let authorized = header_token == Some(expected) || query.token.as_deref() == Some(expected);
    if authorized {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, "missing or invalid token").into_response()
    }
}

async fn invoke_handler(
    State(ctx): State<ServerCtx>,
    Path(command): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let raw_args = if body.is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}"))
                    .into_response();
            }
        }
    };

    match dispatch::dispatch(ctx.state.clone(), &command, raw_args).await {
        Ok(value) => axum::Json(value).into_response(),
        Err(DispatchError::UnknownCommand) => {
            (StatusCode::NOT_FOUND, format!("unknown command: {command}")).into_response()
        }
        Err(DispatchError::BadArgs(message)) => (StatusCode::BAD_REQUEST, message).into_response(),
        Err(DispatchError::Command(e)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", e.0)).into_response()
        }
    }
}

async fn events_handler(State(ctx): State<ServerCtx>) -> impl IntoResponse {
    let stream = BroadcastStream::new(ctx.bus.subscribe()).filter_map(|item| match item {
        Ok(event) => Some(Ok::<Event, std::convert::Infallible>(
            Event::default()
                .event(event.name)
                .data(event.payload.to_string()),
        )),
        // A lagged subscriber only misses progress ticks; drop and continue.
        Err(BroadcastStreamRecvError::Lagged(skipped)) => {
            log::warn!("SSE subscriber lagged, skipped {skipped} events");
            None
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
