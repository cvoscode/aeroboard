use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use arrow_flight::{
    Action, ActionType, Criteria, Empty, FlightData, FlightDescriptor, FlightInfo,
    HandshakeRequest, HandshakeResponse, PutResult, SchemaResult, Ticket, PollInfo,
};
use arrow_flight::flight_service_server::FlightService;

use crate::telemetry::{TelemetryBatch, TelemetryKind};
use crate::t_junction::TJunction;

#[derive(Clone)]
pub struct AeroboardFlightServer {
    pub t_junction: Arc<TJunction>,
}

#[tonic::async_trait]
impl FlightService for AeroboardFlightServer {
    type HandshakeStream = ReceiverStream<Result<HandshakeResponse, Status>>;
    type ListFlightsStream = ReceiverStream<Result<FlightInfo, Status>>;
    type DoGetStream = ReceiverStream<Result<FlightData, Status>>;
    type DoPutStream = ReceiverStream<Result<PutResult, Status>>;
    type DoActionStream = ReceiverStream<Result<arrow_flight::Result, Status>>;
    type ListActionsStream = ReceiverStream<Result<ActionType, Status>>;
    type DoExchangeStream = ReceiverStream<Result<FlightData, Status>>;

    async fn handshake(
        &self,
        _request: Request<Streaming<HandshakeRequest>>,
    ) -> Result<Response<Self::HandshakeStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn list_flights(
        &self,
        _request: Request<Criteria>,
    ) -> Result<Response<Self::ListFlightsStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn get_flight_info(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<FlightInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn poll_flight_info(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<PollInfo>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn get_schema(
        &self,
        _request: Request<FlightDescriptor>,
    ) -> Result<Response<SchemaResult>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn do_get(
        &self,
        _request: Request<Ticket>,
    ) -> Result<Response<Self::DoGetStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn do_put(
        &self,
        request: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoPutStream>, Status> {
        let mut stream = request.into_inner();
        let t_junction = self.t_junction.clone();

        // Channel for returning PutResult to the client.
        let (tx, rx) = mpsc::channel(2);

        // Track the spawned task so we can detect if it dies unexpectedly (panic or error
        // before sending PutResult). If the task panics, joining returns an Err and we
        // return an error to the client rather than hanging.
        let worker_handle = tokio::spawn(async move {
            use arrow_flight::utils::flight_data_to_arrow_batch;
            use arrow::datatypes::Schema;

            let dictionaries_by_id = std::collections::HashMap::new();
            let mut schema: Option<Arc<Schema>> = None;
            let mut declared_kind = TelemetryKind::Unknown;

            while let Some(msg) = stream.message().await.unwrap_or(None) {
                if let Some(descriptor) = msg.flight_descriptor.as_ref() {
                    declared_kind = TelemetryKind::from_descriptor_path(&descriptor.path);
                }

                if msg.data_header.is_empty() {
                    continue;
                }

                let message: arrow::ipc::Message<'_> = arrow::ipc::root_as_message(&msg.data_header).unwrap();

                match message.header_type() {
                    arrow::ipc::MessageHeader::Schema => {
                        let ipc_schema = message.header_as_schema().unwrap();
                        let parsed_schema = arrow::ipc::convert::fb_to_schema(ipc_schema);
                        schema = Some(Arc::new(parsed_schema));
                    }
                    arrow::ipc::MessageHeader::DictionaryBatch => {
                        let _batch = message.header_as_dictionary_batch().unwrap();
                        if let Some(ref schem) = schema {
                            let _ = arrow_flight::utils::flight_data_to_arrow_batch(
                                &msg,
                                schem.clone(),
                                &dictionaries_by_id,
                            );
                        }
                    }
                    arrow::ipc::MessageHeader::RecordBatch => {
                        if let Some(ref schem) = schema {
                            match flight_data_to_arrow_batch(&msg, schem.clone(), &dictionaries_by_id) {
                                Ok(batch) => {
                                    tracing::debug!("Received RecordBatch with {} rows", batch.num_rows());
                                    let batch = Arc::new(batch);
                                    let kind = if declared_kind == TelemetryKind::Unknown {
                                        TelemetryKind::from_batch(&batch)
                                    } else {
                                        declared_kind.clone()
                                    };
                                    let _ = t_junction.publish(Arc::new(TelemetryBatch::new(kind, batch)));
                                }
                                Err(e) => {
                                    tracing::error!("Error decoding batch: {}", e);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Signal completion so the client receives PutResult.
            let _ = tx.send(Ok(PutResult { app_metadata: vec![].into() })).await;
        });

        // Ensure the worker task completes (or panics) before we return the response stream.
        // If the handle returns Err (panic), propagate an error to the client instead of
        // letting the stream hang.
        match worker_handle.await {
            Ok(()) => {} // task completed normally; PutResult was or will be sent
            Err(_) => {
                // Join error — the worker task panicked or was cancelled before sending PutResult.
                // Return an error to the client rather than letting the response stream hang.
                return Err(Status::internal("Worker task panicked during batch ingestion"));
            }
        }

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn do_action(
        &self,
        _request: Request<Action>,
    ) -> Result<Response<Self::DoActionStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn list_actions(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::ListActionsStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }

    async fn do_exchange(
        &self,
        _request: Request<Streaming<FlightData>>,
    ) -> Result<Response<Self::DoExchangeStream>, Status> {
        Err(Status::unimplemented("Not yet implemented"))
    }
}
