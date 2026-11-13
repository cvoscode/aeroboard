use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, BinaryArray, BooleanArray};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use axum::{
    body::Bytes,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
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

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    // When clips are persisted, browsers fetch them on demand from
    // /api/history/media/content, so live pushes only carry metadata.
    let strip_media_payloads = state.config.storage_mode != StorageMode::LiveOnly;
    ws.on_upgrade(move |socket| {
        handle_socket(
            socket,
            state.t_junction.clone(),
            state.catalog.clone(),
            strip_media_payloads,
        )
    })
}

/// Replace the binary payload column of audio/video batches with empty values.
/// The schema is unchanged; an empty payload tells the dashboard to fetch lazily.
pub(crate) fn live_batch(
    telemetry: &TelemetryBatch,
    strip_media_payloads: bool,
) -> Arc<RecordBatch> {
    let is_av = matches!(
        telemetry.kind,
        TelemetryKind::Images | TelemetryKind::Audio | TelemetryKind::Video
    );
    let batch = &telemetry.batch;
    let data_index = telemetry
        .kind
        .media_data_column()
        .and_then(|column| batch.schema().index_of(column).ok());
    let (true, true, Some(data_index)) = (strip_media_payloads, is_av, data_index) else {
        return batch.clone();
    };

    let empty: ArrayRef = Arc::new(BinaryArray::from_iter_values(std::iter::repeat_n(
        &[] as &[u8],
        batch.num_rows(),
    )));
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
pub(crate) fn live_payload(
    telemetry: &TelemetryBatch,
    strip_media_payloads: bool,
) -> Result<Bytes, arrow::error::ArrowError> {
    telemetry.cached_live_ipc(strip_media_payloads, || {
        encode_ipc(&live_batch(telemetry, strip_media_payloads))
    })
}

/// JSON control message telling the dashboard it missed `skipped` batches and
/// should re-fetch history for the runs it shows.
pub(crate) fn lagged_message(skipped: u64) -> String {
    serde_json::json!({ "type": "lagged", "skipped": skipped }).to_string()
}

async fn handle_socket(
    socket: WebSocket,
    t_junction: Arc<TJunction>,
    catalog: crate::catalog::Catalog,
    strip_media_payloads: bool,
) {
    let mut rx = t_junction.subscribe();
    let mut metadata = catalog.changes.subscribe();
    drop(t_junction);
    let (mut sender, mut receiver) = socket.split();

    // None preserves compatibility for clients that have not sent subscriptions.
    let mut subscriptions: Option<std::collections::BTreeSet<String>> = None;
    loop {
        let message = tokio::select! {
            input=receiver.next()=>{
                match input {
                    Some(Ok(Message::Text(text)))=>{
                        if let Ok(value)=serde_json::from_str::<serde_json::Value>(&text) && value["type"]=="subscribe"
                            && let Some(runs)=value["runs"].as_array() && runs.len()<=1000 && runs.iter().all(|r|r.as_str().is_some_and(|s|s.len()<=256)) {
                                subscriptions=Some(runs.iter().filter_map(|r|r.as_str().map(str::to_owned)).collect());
                        }
                        continue;
                    },
                    Some(Ok(Message::Ping(bytes)))=>Message::Pong(bytes),
                    Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,
                    _=>continue
                }
            },
            update=metadata.recv()=>{
                match update {
                    Ok(message)=>Message::Text(message.into()),
                    Err(broadcast::error::RecvError::Lagged(_))=>Message::Text(serde_json::json!({"type":"catalog_changed"}).to_string().into()),
                    Err(broadcast::error::RecvError::Closed)=>break
                }
            },
            update=rx.recv()=>{
                match update {
                    Ok(telemetry)=>{
                        let ids=telemetry.batch.column(1).as_string::<i32>();
                        let runs=telemetry.run_ids();
                        let activity=serde_json::json!({"type":"data_changed","runs":runs,"batch_id":telemetry.envelope.as_ref().map(|e|&e.batch_id)}).to_string();
                        if sender.send(Message::Text(activity.into())).await.is_err(){break;}
                        let filtered = if let Some(selected)=&subscriptions {
                            let mask=BooleanArray::from((0..ids.len()).map(|i|selected.contains(ids.value(i))).collect::<Vec<_>>());
                            let count=mask.values().count_set_bits();
                            if count==0 {continue;}
                            if count==ids.len(){None}else{
                                match arrow::compute::filter_record_batch(&telemetry.batch,&mask){
                                    Ok(batch)=>Some(TelemetryBatch::new(telemetry.kind.clone(),Arc::new(batch))),
                                    Err(error)=>{tracing::warn!("Cannot filter live batch: {error}");continue;}
                                }
                            }
                        }else{None};
                        match live_payload(filtered.as_ref().unwrap_or(&telemetry),strip_media_payloads){
                            Ok(bytes)=>Message::Binary(bytes),
                            Err(error)=>{tracing::warn!("Cannot encode live batch: {error}");continue;}
                        }
                    },
                    Err(broadcast::error::RecvError::Lagged(skipped))=>Message::Text(lagged_message(skipped).into()),
                    Err(broadcast::error::RecvError::Closed)=>break
                }
            }
        };
        if sender.send(message).await.is_err() {
            break;
        }
    }
}
