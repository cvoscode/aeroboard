use std::sync::Arc;
use std::net::SocketAddr;
use axum::{routing::get, serve, Router};
use tonic::transport::Server;
use arrow_flight::flight_service_server::FlightServiceServer;
use tower_http::cors::{CorsLayer, Any};

mod t_junction;
mod ingestion;
mod streaming;
mod persistence;
mod schema;
mod history;
mod tests;

use crate::t_junction::TJunction;
use crate::ingestion::AeroboardFlightServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting Aeroboard Server...");

    // 1. Initialize T-Junction Hub
    let t_junction = Arc::new(TJunction::new(100));

    // 2. Start Persistence Loop (Path A)
    persistence::start_persistence_loop(t_junction.clone()).await;

    // 3. Setup HTTP + WebSocket Server
    let ws_state = t_junction.clone();
    let app = Router::new()
        .route("/ws/live", get(streaming::ws_handler))
        .route("/api/history", get(history::history_handler).delete(history::delete_history_handler))
        .fallback_service(tower_http::services::ServeDir::new("../aeroboard-web/dist").fallback(tower_http::services::ServeFile::new("../aeroboard-web/dist/index.html")))
        // Allow the Vite dev server to call /api/history
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .with_state(ws_state);

    let ws_addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    let ws_listener = tokio::net::TcpListener::bind(ws_addr).await?;

    let ws_server = tokio::spawn(async move {
        tracing::info!("HTTP+WebSocket Server listening on {}", ws_addr);
        serve(ws_listener, app).await.unwrap();
    });

    // 4. Setup gRPC Flight Server (Ingestion)
    let flight_addr = "[::]:50051".parse()?;
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
