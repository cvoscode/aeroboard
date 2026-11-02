use std::sync::Arc;

use arrow_flight::decode::{DecodedPayload, FlightDataDecoder};
use arrow_flight::error::FlightError;
use arrow_flight::flight_service_server::FlightService;
use arrow_flight::{
    Action, ActionType, Criteria, Empty, FlightData, FlightDescriptor, FlightInfo,
    HandshakeRequest, HandshakeResponse, PollInfo, PutResult, SchemaResult, Ticket,
};
use futures::{stream, StreamExt, TryStreamExt};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use crate::schema::normalize_batch;
use crate::t_junction::TJunction;
use crate::telemetry::{TelemetryBatch, TelemetryKind};

#[derive(Clone)]
pub struct AeroboardFlightServer {
    pub t_junction: Arc<TJunction>,
}

impl AeroboardFlightServer {
    /// Decode, validate and publish every batch of one `do_put` stream.
    ///
    /// Returns the number of rows accepted, once every batch has been persisted.
    /// Any malformed message, schema mismatch, transport or storage error fails the
    /// whole call so the client knows to retry (batches before a validation failure
    /// have already been published).
    async fn ingest(&self, mut input: Streaming<FlightData>) -> Result<usize, Status> {
        // The descriptor travels on the first message; peek at it, then hand the
        // full stream (first message included) to the Arrow Flight decoder, which
        // handles schema and dictionary messages correctly.
        let Some(first) = input.message().await? else {
            return Ok(0);
        };
        let declared_kind = first
            .flight_descriptor
            .as_ref()
            .map(|descriptor| TelemetryKind::from_descriptor_path(&descriptor.path))
            .unwrap_or(TelemetryKind::Unknown);

        let messages = stream::once(async move { Ok(first) })
            .chain(input.map_err(FlightError::from))
            .boxed();
        let mut decoder = FlightDataDecoder::new(messages);

        let mut rows = 0;
        let mut pending_writes = Vec::new();
        while let Some(decoded) = decoder.next().await {
            let decoded = decoded.map_err(|error| match error {
                FlightError::Tonic(status) => *status,
                other => Status::invalid_argument(format!("invalid Arrow Flight data: {other}")),
            })?;
            let DecodedPayload::RecordBatch(batch) = decoded.payload else {
                continue;
            };

            let kind = if declared_kind == TelemetryKind::Unknown {
                TelemetryKind::from_batch(&batch)
            } else {
                declared_kind.clone()
            };
            let batch = normalize_batch(&kind, &batch).map_err(|reason| {
                tracing::warn!("Rejected telemetry batch: {reason}");
                Status::invalid_argument(reason)
            })?;
            if batch.num_rows() == 0 {
                continue;
            }

            tracing::debug!("Received {:?} batch with {} rows", kind, batch.num_rows());
            rows += batch.num_rows();
            let persisted = self
                .t_junction
                .publish(Arc::new(TelemetryBatch::new(kind, Arc::new(batch))))
                .await
                .map_err(|error| Status::unavailable(error.to_string()))?;
            pending_writes.push(persisted);
        }

        // Acknowledge only what is stored: `flush()` on the client then means "durable".
        for persisted in pending_writes {
            match persisted.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(Status::unavailable(format!("failed to persist telemetry: {error}"))),
                Err(_) => return Err(Status::unavailable("server shut down before telemetry was persisted")),
            }
        }

        Ok(rows)
    }
}

#[tonic::async_trait]
impl FlightService for AeroboardFlightServer {
    type HandshakeStream = ReceiverStream<Result<HandshakeResponse, Status>>;
    type ListFlightsStream = ReceiverStream<Result<FlightInfo, Status>>;
    type DoGetStream = ReceiverStream<Result<FlightData, Status>>;
    type DoPutStream = futures::stream::Iter<std::vec::IntoIter<Result<PutResult, Status>>>;
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
        let rows = self.ingest(request.into_inner()).await?;
        let ack = PutResult { app_metadata: rows.to_string().into_bytes().into() };
        Ok(Response::new(stream::iter(vec![Ok(ack)])))
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
