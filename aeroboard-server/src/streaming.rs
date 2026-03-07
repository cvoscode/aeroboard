use std::sync::Arc;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use arrow::ipc::writer::StreamWriter;
use tokio::sync::broadcast;

use crate::{app_state::AppState, t_junction::TJunction};

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state.t_junction.clone()))
}

async fn handle_socket(socket: WebSocket, t_junction: Arc<TJunction>) {
    let mut rx = t_junction.subscribe();
    
    // Set up a channel to send messages to the WebSocket
    let (mut sender, mut receiver) = socket.split();

    // Spawn a task to read from the WebSocket connection to keep it alive
    let mut recv_task = tokio::spawn(async move {
        while let Some(msg_result) = receiver.next().await {
            if let Ok(msg) = msg_result {
            match msg {
                Message::Close(_) => break,
                _ => {} // Ignore other messages from client
            }
            }
        }
    });

    let mut send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(telemetry) => {
                    let batch = telemetry.batch.clone();
                    let mut buf: Vec<u8> = Vec::new();
                    let write_result = (|| -> Result<(), arrow::error::ArrowError> {
                        let mut writer = StreamWriter::try_new(&mut buf, &batch.schema())?;
                        writer.write(&batch)?;
                        writer.finish()?;
                        Ok(())
                    })();

                    if let Err(error) = write_result {
                        tracing::error!("Failed to serialize batch for WebSocket: {}", error);
                        continue;
                    }

                    tracing::debug!("Pushing {} bytes to WebSocket", buf.len());
                    if sender.send(Message::Binary(buf.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!("WebSocket subscriber lagged and skipped {} batches", skipped);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // If either task exits, abort the other.
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}
