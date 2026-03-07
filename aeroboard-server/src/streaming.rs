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

use crate::t_junction::TJunction;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(t_junction): State<Arc<TJunction>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, t_junction))
}

async fn handle_socket(mut socket: WebSocket, t_junction: Arc<TJunction>) {
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
        while let Ok(batch) = rx.recv().await {
            // Serialize Arrow RecordBatch to IPC Bytes
            let mut buf: Vec<u8> = Vec::new();
            {
                let mut writer = StreamWriter::try_new(&mut buf, &batch.schema()).unwrap();
                writer.write(&batch).unwrap();
                writer.finish().unwrap();
            }

            // Send binary IPC data over WebSocket
            tracing::debug!("Pushing {} bytes to WebSocket", buf.len());
            if sender.send(Message::Binary(buf.into())).await.is_err() {
                break; // Connection lost
            }
        }
    });

    // If either task exits, abort the other.
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}
