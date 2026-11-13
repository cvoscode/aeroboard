use std::sync::Arc;

use arrow_flight::decode::{DecodedPayload, FlightDataDecoder};
use arrow_flight::error::FlightError;
use arrow_flight::flight_service_server::FlightService;
use arrow_flight::{
    Action, ActionType, Criteria, Empty, FlightData, FlightDescriptor, FlightInfo,
    HandshakeRequest, HandshakeResponse, PollInfo, PutResult, SchemaResult, Ticket,
};
use futures::{StreamExt, TryStreamExt, stream};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use crate::schema::normalize_batch;
use crate::t_junction::TJunction;
use crate::telemetry::{BatchEnvelope, TelemetryBatch, TelemetryKind};
use arrow::record_batch::RecordBatch;

#[derive(Clone)]
pub struct AeroboardFlightServer {
    pub t_junction: Arc<TJunction>,
    pub catalog: Option<crate::catalog::Catalog>,
}

impl AeroboardFlightServer {
    async fn check_identity(
        &self,
        envelope: &BatchEnvelope,
        batch: &RecordBatch,
    ) -> Result<(String, bool), Status> {
        use arrow::array::AsArray;
        let catalog = self
            .catalog
            .as_ref()
            .ok_or_else(|| Status::unimplemented("versioned ingestion unavailable"))?;
        if envelope.server_id != catalog.store_id {
            return Err(Status::invalid_argument(
                "batch belongs to another server store",
            ));
        }
        let check = envelope.clone();
        let asset_ids = batch
            .schema()
            .index_of("asset_id")
            .ok()
            .map(|index| {
                let ids = batch.column(index).as_string::<i32>();
                (0..batch.num_rows())
                    .map(|row| ids.value(row).to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        catalog
            .read(move |db| crate::catalog::check_batch(db, &check, &asset_ids))
            .await
            .map_err(|error| Status::invalid_argument(error.to_string()))
    }

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
            let app_metadata = decoded.app_metadata().clone();
            let DecodedPayload::RecordBatch(batch) = decoded.payload else {
                continue;
            };

            let kind = if declared_kind == TelemetryKind::Unknown {
                TelemetryKind::from_batch(&batch)
            } else {
                declared_kind.clone()
            };
            let batch = if !app_metadata.is_empty() {
                let mut metadata = batch.schema().metadata().clone();
                metadata.insert(
                    crate::telemetry::ENVELOPE_KEY.into(),
                    String::from_utf8(app_metadata.to_vec())
                        .map_err(|_| Status::invalid_argument("invalid envelope UTF-8"))?,
                );
                {
                    let schema = Arc::new(batch.schema().as_ref().clone().with_metadata(metadata));
                    arrow::record_batch::RecordBatch::try_new(schema, batch.columns().to_vec())
                }
                .map_err(|e| Status::invalid_argument(e.to_string()))?
            } else {
                batch
            };
            let envelope = batch
                .schema()
                .metadata()
                .get(crate::telemetry::ENVELOPE_KEY)
                .map(|raw| parse_envelope(raw))
                .transpose()?;
            let mut batch = normalize_batch(&kind, &batch).map_err(|reason| {
                tracing::warn!("Rejected telemetry batch: {reason}");
                Status::invalid_argument(reason)
            })?;
            if batch.num_rows() == 0 {
                continue;
            }
            use arrow::array::AsArray;
            for index in 0..batch.num_rows() {
                let id = batch.column(1).as_string::<i32>().value(index);
                if id.is_empty() || id == "." || id == ".." {
                    return Err(Status::invalid_argument("invalid run ID"));
                }
            }
            if let Some(envelope) = envelope {
                validate_envelope(&envelope, &batch)?;
                let (canonical, ready) = self.check_identity(&envelope, &batch).await?;
                if ready {
                    rows += batch.num_rows();
                    continue;
                }
                let mut columns = batch.columns().to_vec();
                columns[1] = Arc::new(arrow::array::StringArray::from(vec![
                    canonical;
                    batch.num_rows()
                ]));
                let metadata = std::collections::HashMap::from([(
                    crate::telemetry::ENVELOPE_KEY.to_owned(),
                    serde_json::to_string(&envelope)
                        .map_err(|e| Status::internal(e.to_string()))?,
                )]);
                let schema = Arc::new(batch.schema().as_ref().clone().with_metadata(metadata));
                batch = arrow::record_batch::RecordBatch::try_new(schema, columns)
                    .map_err(|e| Status::invalid_argument(e.to_string()))?;
            } else if let Some(catalog) = &self.catalog {
                let names: std::collections::BTreeSet<_> = (0..batch.num_rows())
                    .map(|i| batch.column(1).as_string::<i32>().value(i).to_owned())
                    .collect();
                let deleted = catalog
                    .read(move |db| {
                        let deleted = crate::catalog::run_names(db, Some(true))?;
                        Ok(!names.is_disjoint(&deleted))
                    })
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?;
                if deleted {
                    return Err(Status::invalid_argument("run has been deleted"));
                }
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
                Ok(Err(error)) => {
                    return Err(Status::unavailable(format!(
                        "failed to persist telemetry: {error}"
                    )));
                }
                Err(_) => {
                    return Err(Status::unavailable(
                        "server shut down before telemetry was persisted",
                    ));
                }
            }
        }

        Ok(rows)
    }
}

fn parse_envelope(raw: &str) -> Result<BatchEnvelope, Status> {
    if raw.len() > 16384 {
        return Err(Status::invalid_argument("batch envelope is too large"));
    }
    serde_json::from_str(raw)
        .map_err(|error| Status::invalid_argument(format!("invalid batch envelope: {error}")))
}
fn validate_envelope(envelope: &BatchEnvelope, batch: &RecordBatch) -> Result<(), Status> {
    if ![1, 2].contains(&envelope.version)
        || envelope.sequence < 1
        || envelope.captured_at_ms < 0
        || [&envelope.batch_id, &envelope.run_uid, &envelope.session_id]
            .iter()
            .any(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(Status::invalid_argument(
            "invalid batch identity or version",
        ));
    }
    let checksum = if envelope.version == 2 {
        crate::telemetry::batch_checksum_v2(batch)
    } else {
        crate::telemetry::batch_checksum(batch)
    }
    .map_err(Status::invalid_argument)?;
    if checksum != envelope.checksum {
        return Err(Status::invalid_argument(
            "batch checksum does not match its envelope",
        ));
    }
    Ok(())
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
        let ack = PutResult {
            app_metadata: rows.to_string().into_bytes().into(),
        };
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
