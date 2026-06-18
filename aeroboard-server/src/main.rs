use std::{sync::Arc, net::SocketAddr, env};
use axum::{
    http::{header, StatusCode, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post},
    serve, Router,
};
use tonic::transport::Server;
use arrow_flight::flight_service_server::FlightServiceServer;
use tower_http::cors::{CorsLayer, Any};

mod app_state;
mod config;
mod telemetry;
mod t_junction;
mod ingestion;
mod streaming;
mod persistence;
mod schema;
mod history;
mod storage;
mod retry;
mod tests;

use crate::app_state::AppState;
use crate::config::AppConfig;
use crate::t_junction::TJunction;
use crate::ingestion::AeroboardFlightServer;
use crate::storage::build_storage;

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
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

pub fn build_app(app_state: AppState) -> Router {
    let config = app_state.config.clone();
    let api_routes = Router::new()
        .route("/history", get(history::history_handler).delete(history::delete_history_handler))
        .route("/history/images", get(history::image_history_handler))
        .route("/history/images/frame", get(history::image_frame_content_handler))
        .route("/runs", get(history::runs_handler))
        .route("/runs/summary", get(history::run_summaries_handler))
        .route("/runs/resolve", post(history::resolve_run_id_handler))
        .fallback(api_not_found_fallback);

    Router::new()
        .route("/ws/live", get(streaming::ws_handler))
        .nest("/api", api_routes)
        .fallback_service(
            tower_http::services::ServeDir::new(config.web_dist_dir.clone())
                .fallback(tower_http::services::ServeFile::new(config.web_dist_dir.join("index.html"))),
        )
        .layer({
            let cors = CorsLayer::new();
            match env::var("CORS_ORIGIN") {
                Ok(origins) if !origins.is_empty() => {
                    tracing::info!("CORS: allowing origins: {}", origins);
                    cors.allow_origin(
                        origins.split(',')
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.parse::<axum::http::HeaderValue>().unwrap())
                            .collect::<Vec<_>>(),
                    )
                },
                _ => {
                    tracing::warn!("CORS: allowing all origins (set CORS_ORIGIN env var to restrict)");
                    cors.allow_origin(Any)
                }
            }.allow_methods(Any).allow_headers(Any)
        })
        .with_state(app_state)
}

#[tokio::main]
async fn main() -> Result<(), crate::storage::BoxError> {
    tracing_subscriber::fmt::init();
    let config = AppConfig::from_env();
    tracing::info!(
        "Starting Aeroboard Server with storage={} http_port={} flight_port={}",
        config.storage_mode.as_str(),
        config.http_port,
        config.flight_port
    );

    // 1. Initialize T-Junction Hub
    let t_junction = Arc::new(TJunction::new(100));
    let storage = build_storage(&config)?;
    let app_state = AppState {
        config: config.clone(),
        t_junction: t_junction.clone(),
        storage: storage.clone(),
    };

    // 2. Start Persistence Loop (Path A) — must be awaited so shutdown waits for it
    let persistence_loop = persistence::start_persistence_loop(app_state.clone());

    // 3. Setup HTTP + WebSocket Server
    let app = build_app(app_state.clone());

    let ws_addr = SocketAddr::from(([0, 0, 0, 0], config.http_port));
    let ws_listener = tokio::net::TcpListener::bind(ws_addr).await.map_err(|e| {
        format!("Failed to bind HTTP server to {}: {}", ws_addr, e)
    })?;

    let ws_server = tokio::spawn(async move {
        tracing::info!("HTTP+WebSocket Server listening on {}", ws_addr);
        serve(ws_listener, app).await.unwrap();
    });

    // 4. Setup gRPC Flight Server (Ingestion)
    let flight_addr = format!("[::]:{}", config.flight_port).parse()?;
    let flight_service = FlightServiceServer::new(AeroboardFlightServer {
        t_junction: t_junction.clone(),
    });

    let grpc_server = tokio::spawn(async move {
        tracing::info!("gRPC Flight Server listening on {}", flight_addr);
        Server::builder()
            .add_service(flight_service)
            .serve(flight_addr)
            .await
            .unwrap();
    });

    tokio::try_join!(ws_server, grpc_server, persistence_loop)?;

    Ok(())
}
