use std::{net::SocketAddr, time::Duration};

use arrow_flight::flight_service_server::FlightServiceServer;
use axum::{
    Router,
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
    serve,
};
use tokio::sync::watch;
use tonic::transport::Server;
use tower_http::{
    compression::predicate::{DefaultPredicate, NotForContentType, Predicate},
    cors::{Any, CorsLayer},
};

mod analytics;
mod app_error;
mod app_state;
mod catalog;
mod config;
mod dashboard_config;
mod history;
mod ingestion;
mod models;
mod persistence;
mod retry;
mod schema;
mod storage;
mod streaming;
mod summaries;
mod t_junction;
mod telemetry;
#[cfg(test)]
#[path = "../tests/http/mod.rs"]
mod tests;
mod workspace;
#[cfg(test)]
#[path = "../tests/workspace/mod.rs"]
mod workspace_tests;

use crate::app_state::AppState;
use crate::config::{AppConfig, CorsPolicy};
use crate::ingestion::AeroboardFlightServer;
use crate::storage::build_storage;

/// How long shutdown waits for queued batches to reach storage.
const PERSISTENCE_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Return a 404 JSON response for unmatched `/api/*` requests so the SPA
/// fallback never gets cached as the response of an API call.
async fn api_not_found_fallback() -> Response {
    let mut response = (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": "not_found",
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn cors_layer(policy: &CorsPolicy) -> Option<CorsLayer> {
    let methods = [
        Method::GET,
        Method::POST,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ];
    match policy {
        CorsPolicy::SameOrigin => None,
        CorsPolicy::Any => {
            tracing::warn!(
                "CORS: allowing every origin; any website you visit can read and delete your runs"
            );
            Some(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(methods)
                    .allow_headers(Any),
            )
        }
        CorsPolicy::Origins(origins) => {
            let parsed: Vec<HeaderValue> = origins
                .iter()
                .filter_map(|origin| match origin.parse::<HeaderValue>() {
                    Ok(value) => Some(value),
                    Err(_) => {
                        tracing::warn!("CORS: ignoring invalid origin {origin:?}");
                        None
                    }
                })
                .collect();
            tracing::info!("CORS: allowing origins {}", origins.join(", "));
            Some(
                CorsLayer::new()
                    .allow_origin(parsed)
                    .allow_methods(methods)
                    .allow_headers(Any),
            )
        }
    }
}

pub fn build_app(app_state: AppState) -> Router {
    let config = app_state.config.clone();
    let api_routes = Router::new()
        .route(
            "/history",
            get(history::history_handler).delete(history::delete_history_handler),
        )
        .route("/history/images", get(history::image_history_handler))
        .route("/history/media", get(history::media_history_handler))
        .route(
            "/history/media/content",
            get(history::media_content_handler),
        )
        .route("/runs", get(history::runs_handler))
        .route("/runs/summary", get(history::run_summaries_handler))
        .route("/runs/resolve", post(history::resolve_run_id_handler))
        .route("/config", get(dashboard_config::config_handler))
        .merge(workspace::routes())
        .merge(analytics::routes())
        .fallback(api_not_found_fallback);

    let router = Router::new()
        .route("/ws/live", get(streaming::ws_handler))
        .nest("/api", api_routes)
        .fallback_service(
            tower_http::services::ServeDir::new(config.web_dist_dir.clone())
                .precompressed_gzip()
                .precompressed_br()
                .fallback(tower_http::services::ServeFile::new(
                    config.web_dist_dir.join("index.html"),
                )),
        );

    let router = match cors_layer(&config.cors) {
        Some(layer) => router.layer(layer),
        None => router,
    };
    router
        .layer(
            tower_http::compression::CompressionLayer::new().compress_when(
                DefaultPredicate::new().and(NotForContentType::const_new(
                    history::ARROW_STREAM_CONTENT_TYPE,
                )),
            ),
        )
        .with_state(app_state)
}

fn server_outcome<E: std::fmt::Display>(
    name: &str,
    result: Result<Result<(), E>, tokio::task::JoinError>,
) -> Result<(), crate::storage::BoxError> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("{name} server failed: {error}").into()),
        Err(error) => Err(format!("{name} server task failed: {error}").into()),
    }
}

async fn wait_for_shutdown(mut signal: watch::Receiver<bool>) {
    let _ = signal.wait_for(|stop| *stop).await;
}

#[tokio::main]
async fn main() -> Result<(), crate::storage::BoxError> {
    tracing_subscriber::fmt::init();
    let config = AppConfig::from_env();
    tracing::info!(
        "Starting Aeroboard Server with storage={} host={} http_port={} flight_port={}",
        config.storage_mode.as_str(),
        config.host,
        config.http_port,
        config.flight_port
    );
    if !config.host.is_loopback() {
        tracing::warn!(
            "Listening on {}: anyone who can reach this machine can send telemetry and delete runs",
            config.host
        );
    }

    let storage = build_storage(&config)?;
    let (app_state, persist_rx) = AppState::new(config.clone(), storage)?;
    app_state.storage.initialize().await?;
    let t_junction = app_state.t_junction.clone();

    // Path A: persistence. Awaited during shutdown so queued batches are written.
    let persistence_loop = persistence::start_persistence_loop(app_state.clone(), persist_rx);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        #[cfg(unix)]
        let shutdown = async {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            tokio::select! { result=tokio::signal::ctrl_c()=>result, _=term.recv()=>Ok(()) }
        };
        #[cfg(not(unix))]
        let shutdown = tokio::signal::ctrl_c();
        if shutdown.await.is_ok() {
            tracing::info!("Shutdown requested; finishing in-flight uploads");
            let _ = shutdown_tx.send(true);
        }
    });

    // HTTP + WebSocket server (dashboard, history API, live stream).
    let http_addr = SocketAddr::new(config.host, config.http_port);
    let http_listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .map_err(|e| format!("Failed to bind HTTP server to {}: {}", http_addr, e))?;
    let app = build_app(app_state.clone());
    let http_shutdown = wait_for_shutdown(shutdown_rx.clone());
    let mut http_server = tokio::spawn(async move {
        tracing::info!("HTTP+WebSocket Server listening on {}", http_addr);
        serve(http_listener, app)
            .with_graceful_shutdown(http_shutdown)
            .await
    });

    // gRPC Flight server (ingestion). tonic defaults to 4 MiB per message, which is
    // too small for audio/video clips.
    let flight_addr = SocketAddr::new(config.host, config.flight_port);
    let flight_service = FlightServiceServer::new(AeroboardFlightServer {
        t_junction: t_junction.clone(),
        catalog: Some(app_state.catalog.clone()),
    })
    .max_decoding_message_size(config.max_message_bytes);
    let grpc_shutdown = wait_for_shutdown(shutdown_rx.clone());
    let mut grpc_server = tokio::spawn(async move {
        tracing::info!("gRPC Flight Server listening on {}", flight_addr);
        Server::builder()
            .add_service(flight_service)
            .serve_with_shutdown(flight_addr, grpc_shutdown)
            .await
    });

    // Run until both servers stop (normally via Ctrl-C). If one fails, stop the other.
    let first = tokio::select! {
        result = &mut http_server => server_outcome("HTTP", result),
        result = &mut grpc_server => server_outcome("gRPC", result),
    };
    let first_error = match first {
        Ok(()) => {
            // Graceful shutdown: let the other server finish its in-flight requests too.
            let (http, grpc) = (http_server.is_finished(), grpc_server.is_finished());
            let other = match (http, grpc) {
                (true, false) => server_outcome("gRPC", grpc_server.await),
                (false, true) => server_outcome("HTTP", http_server.await),
                _ => Ok(()),
            };
            other.err()
        }
        Err(error) => {
            http_server.abort();
            grpc_server.abort();
            Some(error)
        }
    };

    // Everything accepted so far is in the persistence queue; write it out.
    t_junction.close_persistence();
    if tokio::time::timeout(PERSISTENCE_DRAIN_TIMEOUT, persistence_loop)
        .await
        .is_err()
    {
        tracing::error!(
            "Timed out after {:?} writing queued telemetry; some batches were not persisted",
            PERSISTENCE_DRAIN_TIMEOUT
        );
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
