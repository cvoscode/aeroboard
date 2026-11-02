use std::sync::Arc;

use arrow::array::{ArrayRef, BinaryArray};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{sink::SinkExt, stream::StreamExt};
use tokio::sync::broadcast;

use crate::{
    app_state::AppState,
    config::StorageMode,
    t_junction::TJunction,
    telemetry::{TelemetryBatch, TelemetryKind},
};

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // When clips are persisted, browsers fetch them on demand from
    // /api/history/media/content, so live pushes only carry metadata.
    let strip_media_payloads = state.config.storage_mode != StorageMode::LiveOnly;
    ws.on_upgrade(move |socket| handle_socket(socket, state.t_junction.clone(), strip_media_payloads))
}

/// Replace the binary payload column of audio/video batches with empty values.
/// The schema is unchanged; an empty payload tells the dashboard to fetch lazily.
pub(crate) fn live_batch(telemetry: &TelemetryBatch, strip_media_payloads: bool) -> Arc<RecordBatch> {
    let is_av = matches!(telemetry.kind, TelemetryKind::Audio | TelemetryKind::Video);
    let batch = &telemetry.batch;
    let data_index = telemetry
        .kind
        .media_data_column()
        .and_then(|column| batch.schema().index_of(column).ok());
    let (true, true, Some(data_index)) = (strip_media_payloads, is_av, data_index) else {
        return batch.clone();
    };

    let empty: ArrayRef = Arc::new(BinaryArray::from_iter_values(
        std::iter::repeat_n(&[] as &[u8], batch.num_rows()),
    ));
    let mut columns = batch.columns().to_vec();
    columns[data_index] = empty;
    RecordBatch::try_new(batch.schema(), columns)
        .map(Arc::new)
        .unwrap_or_else(|_| batch.clone())
}

fn encode_ipc(batch: &RecordBatch) -> Result<Bytes, arrow::error::ArrowError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut writer = StreamWriter::try_new(&mut buf, &batch.schema())?;
    writer.write(batch)?;
    writer.finish()?;
    drop(writer);
    Ok(Bytes::from(buf))
}

/// Arrow IPC bytes for a live push, encoded once and shared by every subscriber.
pub(crate) fn live_payload(telemetry: &TelemetryBatch, strip_media_payloads: bool) -> Result<Bytes, arrow::error::ArrowError> {
    telemetry.cached_live_ipc(strip_media_payloads, || encode_ipc(&live_batch(telemetry, strip_media_payloads)))
}

/// JSON control message telling the dashboard it missed `skipped` batches and
/// should re-fetch history for the runs it shows.
pub(crate) fn lagged_message(skipped: u64) -> String {
    serde_json::json!({ "type": "lagged", "skipped": skipped }).to_string()
}

async fn handle_socket(socket: WebSocket, t_junction: Arc<TJunction>, strip_media_payloads: bool) {
    let mut rx = t_junction.subscribe();
    drop(t_junction);
    let (mut sender, mut receiver) = socket.split();

    // Read from the socket only to notice when the browser goes away.
    let mut recv_task = tokio::spawn(async move {
        while let Some(msg_result) = receiver.next().await {
            if matches!(msg_result, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    });

    let mut send_task = tokio::spawn(async move {
        loop {
            let message = match rx.recv().await {
                Ok(telemetry) => match live_payload(&telemetry, strip_media_payloads) {
                    Ok(bytes) => Message::Binary(bytes),
                    Err(error) => {
                        tracing::error!("Failed to serialize batch for WebSocket: {}", error);
                        continue;
                    }
                },
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!("WebSocket subscriber lagged and skipped {} batches", skipped);
                    Message::Text(lagged_message(skipped).into())
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });

    // If either task exits, abort the other.
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };
}
