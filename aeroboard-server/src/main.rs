use std::sync::Arc;
use std::net::SocketAddr;
use axum::{routing::{get, post}, serve, Router};
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

    // 2. Start Persistence Loop (Path A)
    persistence::start_persistence_loop(app_state.clone()).await;

    // 3. Setup HTTP + WebSocket Server
    let app = Router::new()
        .route("/ws/live", get(streaming::ws_handler))
        .route("/api/history", get(history::history_handler).delete(history::delete_history_handler))
        .route("/api/history/images", get(history::image_history_handler))
        .route("/api/runs", get(history::runs_handler))
        .route("/api/runs/resolve", post(history::resolve_run_id_handler))
        .fallback_service(
            tower_http::services::ServeDir::new(config.web_dist_dir.clone())
                .fallback(tower_http::services::ServeFile::new(config.web_dist_dir.join("index.html"))),
        )
        // Allow the Vite dev server to call /api/history
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .with_state(app_state.clone());

    let ws_addr = SocketAddr::from(([0, 0, 0, 0], config.http_port));
    let ws_listener = tokio::net::TcpListener::bind(ws_addr).await?;

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

    tokio::try_join!(ws_server, grpc_server)?;

    Ok(())
}