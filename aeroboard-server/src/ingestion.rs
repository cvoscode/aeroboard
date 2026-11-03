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
use crate::telemetry::{TelemetryBatch, TelemetryKind};

#[derive(Clone)]
pub struct AeroboardFlightServer {
    pub t_junction: Arc<TJunction>,
    pub catalog: Option<crate::catalog::Catalog>,
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
        let mut identified = false;
        let mut identified_batch = None;
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
            if identified {
                return Err(Status::invalid_argument(
                    "versioned uploads require one batch per RPC",
                ));
            }
            let envelope = batch
                .schema()
                .metadata()
                .get(crate::telemetry::ENVELOPE_KEY)
                .map(|raw| {
                    if raw.len() > 16384 {
                        return Err(Status::invalid_argument("batch envelope is too large"));
                    }
                    serde_json::from_str::<crate::telemetry::BatchEnvelope>(raw).map_err(|e| {
                        Status::invalid_argument(format!("invalid batch envelope: {e}"))
                    })
                })
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
                identified = true;
                if rows > 0 {
                    return Err(Status::invalid_argument(
                        "versioned uploads require one batch per RPC",
                    ));
                }
                if envelope.version != 1
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
                if crate::telemetry::batch_checksum(&batch).map_err(Status::invalid_argument)?
                    != envelope.checksum
                {
                    return Err(Status::invalid_argument(
                        "batch checksum does not match its envelope",
                    ));
                }
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
                let asset_ids = batch.schema().index_of("asset_id").ok().map(|index| {
                    let ids = batch.column(index).as_string::<i32>();
                    (0..batch.num_rows())
                        .map(|row| ids.value(row).to_owned())
                        .collect::<Vec<_>>()
                });
                let (canonical,ready)=catalog.read(move|db|{
                    use rusqlite::OptionalExtension;
                    let run=crate::catalog::run_by_id(db,&check.run_uid)?;
                    let session=crate::catalog::get(db,"session",&check.session_id)?.ok_or_else(crate::catalog::CatalogError::missing)?;
                    if session.parent!=run.id{return Err(crate::catalog::CatalogError::invalid("session belongs to another run"));}
                    if session.data["segment"].as_str()!=check.segment.as_deref(){return Err(crate::catalog::CatalogError::invalid("batch segment does not match its session"));}
                    if session.data["final_sequence"].as_i64().is_some_and(|last|check.sequence>last){return Err(crate::catalog::CatalogError::invalid("batch exceeds finished session sequence"));}
                    let occupied:Option<String>=db.query_row("SELECT id FROM receipts WHERE run_uid=?1 AND sequence=?2 AND json_extract(CAST(body AS TEXT),'$.envelope.session_id')=?3",rusqlite::params![check.run_uid,check.sequence,check.session_id],|r|r.get(0)).optional()?;
                    if occupied.is_some_and(|id|id!=check.batch_id){return Err(crate::catalog::CatalogError::conflict("session sequence was reused"));}
                    if let Some(ids)=asset_ids{
                        let mut unique=std::collections::BTreeSet::new();
                        for id in &ids{
                            if uuid::Uuid::parse_str(id).is_err()||!unique.insert(id.to_owned()){return Err(crate::catalog::CatalogError::invalid("asset IDs must be distinct UUIDs"));}
                            if let Some(asset)=crate::catalog::get(db,"asset",id)?&& asset.data["batch_id"]!=check.batch_id{return Err(crate::catalog::CatalogError::conflict("asset ID was reused"));}
                        }
                    }
                    let receipt:Option<(String,bool,Vec<u8>)>=db.query_row("SELECT checksum,committed,body FROM receipts WHERE id=?1",[&check.batch_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
                    let ready=if let Some((checksum,ready,body))=receipt{
                        let previous:serde_json::Value=serde_json::from_slice(&body).map_err(|_|crate::catalog::CatalogError::invalid("invalid batch receipt"))?;
                        let identity=serde_json::from_value::<crate::telemetry::BatchEnvelope>(previous["envelope"].clone()).map_err(|_|crate::catalog::CatalogError::invalid("invalid stored batch identity"))?;
                        if checksum!=check.checksum||identity!=check{return Err(crate::catalog::CatalogError::conflict("batch ID was reused with different contents or identity"));}ready
                    }else{false};
                    Ok((run.data["run_id"].as_str().unwrap_or("").to_owned(),ready))
                }).await.map_err(|e|Status::invalid_argument(e.to_string()))?;
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
                        Ok(crate::catalog::list(db, "run", None)?.iter().any(|r| {
                            r.data["deleted"] == true
                                && r.data["run_id"]
                                    .as_str()
                                    .is_some_and(|id| names.contains(id))
                        }))
                    })
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?;
                if deleted {
                    return Err(Status::invalid_argument("run has been deleted"));
                }
            }

            tracing::debug!("Received {:?} batch with {} rows", kind, batch.num_rows());
            rows += batch.num_rows();
            if identified {
                identified_batch = Some((kind, batch));
                continue;
            }
            let persisted = self
                .t_junction
                .publish(Arc::new(TelemetryBatch::new(kind, Arc::new(batch))))
                .await
                .map_err(|error| Status::unavailable(error.to_string()))?;
            pending_writes.push(persisted);
        }

        if let Some((kind, batch)) = identified_batch {
            pending_writes.push(
                self.t_junction
                    .publish(Arc::new(TelemetryBatch::new(kind, Arc::new(batch))))
                    .await
                    .map_err(|e| Status::unavailable(e.to_string()))?,
            );
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
