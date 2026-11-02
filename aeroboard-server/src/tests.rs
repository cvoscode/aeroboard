#[cfg(test)]
mod tests_integration {
    use axum::{body::Body, http::{Request, StatusCode}};
    use arrow::array::{Array, AsArray, BinaryArray, Float64Array, Int64Array, StringArray, TimestampMicrosecondArray, TimestampMillisecondArray};
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
    use arrow::record_batch::RecordBatch;
    use async_trait::async_trait;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::Duration;
    use tower::util::ServiceExt;

    use crate::history::{HyperparameterEntry, HistoryPoint, ImageHistoryPoint};
    use crate::app_state::AppState;
    use crate::config::{AppConfig, CorsPolicy};
    use crate::build_app;
    use crate::schema::{hyperparameter_schema, image_schema, legacy_scalar_schema, normalize_batch};
    use crate::storage::{
        boxed_error, record_batch_to_line_protocol, BoxError, FilesystemStore, HistoryQueryOptions,
        ImageFrameQuery, ImageQueryOptions, MediaKind, MediaPayload, MediaSource, QueryPage, StorageEngine,
    };
    use crate::t_junction::TJunction;
    use crate::telemetry::{TelemetryBatch, TelemetryKind};

    // ---------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------

    fn temp_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("aeroboard-{label}-{}", uuid::Uuid::new_v4()))
    }

    /// A scalar batch in the canonical layout (as ingestion would produce it).
    fn scalar_batch(ts: Vec<i64>, runs: Vec<&str>, names: Vec<&str>, values: Vec<f64>, steps: Option<Vec<Option<f64>>>) -> RecordBatch {
        let legacy = RecordBatch::try_new(
            legacy_scalar_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(ts)),
                Arc::new(StringArray::from(runs)),
                Arc::new(StringArray::from(names)),
                Arc::new(Float64Array::from(values)),
            ],
        )
        .unwrap();
        let mut batch = normalize_batch(&TelemetryKind::Scalars, &legacy).unwrap();
        if let Some(steps) = steps {
            let mut columns = batch.columns().to_vec();
            columns[4] = Arc::new(Float64Array::from(steps));
            batch = RecordBatch::try_new(batch.schema(), columns).unwrap();
        }
        batch
    }

    fn hparam_batch(run_id: &str, key: &str, value: &str) -> RecordBatch {
        RecordBatch::try_new(
            hyperparameter_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1])),
                Arc::new(StringArray::from(vec![run_id])),
                Arc::new(StringArray::from(vec![key])),
                Arc::new(StringArray::from(vec![value])),
                Arc::new(StringArray::from(vec!["float"])),
            ],
        )
        .unwrap()
    }

    fn telemetry(kind: TelemetryKind, batch: RecordBatch) -> Arc<TelemetryBatch> {
        Arc::new(TelemetryBatch::new(kind, Arc::new(batch)))
    }

    fn media_batch(data_column: &str, run_id: &str, name: &str, ts: Vec<i64>, payloads: Vec<&[u8]>, format: &str) -> RecordBatch {
        let n = ts.len();
        RecordBatch::try_new(
            crate::schema::media_schema(data_column),
            vec![
                Arc::new(TimestampMillisecondArray::from(ts)),
                Arc::new(StringArray::from(vec![run_id; n])),
                Arc::new(StringArray::from(vec![name; n])),
                Arc::new(BinaryArray::from(payloads)),
                Arc::new(StringArray::from(vec![format; n])),
            ],
        )
        .unwrap()
    }

    fn state_with_storage(temp_dir: &std::path::Path, storage: Arc<dyn StorageEngine>) -> AppState {
        std::fs::create_dir_all(temp_dir).unwrap();
        std::fs::write(temp_dir.join("index.html"), "<!doctype html><title>aeroboard</title>").unwrap();
        let config = AppConfig::for_tests(temp_dir.join("data"), temp_dir.to_path_buf());
        AppState::new(config, storage).0
    }

    fn filesystem_state(temp_dir: &std::path::Path) -> AppState {
        let storage = Arc::new(FilesystemStore::new(temp_dir.join("data")).unwrap());
        state_with_storage(temp_dir, storage)
    }

    async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
        axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    async fn get(app: &axum::Router, uri: &str) -> axum::response::Response {
        app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap()
    }

    async fn read_media(payload: MediaPayload) -> Vec<u8> {
        match payload.source {
            MediaSource::File(path) => std::fs::read(path).unwrap(),
            MediaSource::Bytes(bytes) => bytes,
        }
    }

    /// Storage whose every operation fails, for error-path tests.
    struct FailingStore;

    #[async_trait]
    impl StorageEngine for FailingStore {
        async fn append_batch(&self, _batch: Arc<TelemetryBatch>) -> Result<(), BoxError> { Err(boxed_error("disk on fire")) }
        async fn query_history(&self, _query: &HistoryQueryOptions) -> Result<QueryPage<HistoryPoint>, BoxError> { Err(boxed_error("disk on fire")) }
        async fn query_hyperparameters(&self, _run_ids: &BTreeSet<String>) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> { Err(boxed_error("disk on fire")) }
        async fn query_media_history(&self, _kind: MediaKind, _query: &ImageQueryOptions) -> Result<QueryPage<ImageHistoryPoint>, BoxError> { Err(boxed_error("disk on fire")) }
        async fn load_media(&self, _kind: MediaKind, _query: &ImageFrameQuery) -> Result<Option<MediaPayload>, BoxError> { Err(boxed_error("disk on fire")) }
        async fn list_runs(&self) -> Result<Vec<String>, BoxError> { Err(boxed_error("disk on fire")) }
        async fn clear_history(&self) -> Result<(), BoxError> { Err(boxed_error("disk on fire")) }
    }

    // ---------------------------------------------------------------------
    // Schema validation
    // ---------------------------------------------------------------------

    #[test]
    fn normalize_adds_step_and_casts_compatible_types() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("timestamp", DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())), false),
            Field::new("run_id", DataType::LargeUtf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("value", DataType::Int64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(TimestampMicrosecondArray::from(vec![5_000]).with_timezone("UTC")),
                Arc::new(arrow::array::LargeStringArray::from(vec!["run"])),
                Arc::new(StringArray::from(vec!["loss"])),
                Arc::new(Int64Array::from(vec![3])),
            ],
        )
        .unwrap();

        let normalized = normalize_batch(&TelemetryKind::Scalars, &batch).unwrap();
        assert_eq!(normalized.schema(), crate::schema::scalar_schema());
        assert_eq!(normalized.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>().value(0), 5);
        assert_eq!(normalized.column(3).as_primitive::<arrow::datatypes::Float64Type>().value(0), 3.0);
        assert!(normalized.column(4).is_null(0));
    }

    #[test]
    fn normalize_rejects_malformed_batches() {
        let wrong_type = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
                Field::new("run_id", DataType::Utf8, false),
                Field::new("name", DataType::Utf8, false),
                Field::new("value", DataType::Binary, false),
            ])),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1])),
                Arc::new(StringArray::from(vec!["run"])),
                Arc::new(StringArray::from(vec!["loss"])),
                Arc::new(BinaryArray::from(vec![b"x".as_slice()])),
            ],
        )
        .unwrap();
        let error = normalize_batch(&TelemetryKind::Scalars, &wrong_type).unwrap_err();
        assert!(error.contains("value"), "{error}");

        let null_run = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
                Field::new("run_id", DataType::Utf8, true),
                Field::new("name", DataType::Utf8, false),
                Field::new("value", DataType::Float64, false),
            ])),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1])),
                Arc::new(StringArray::from(vec![None::<&str>])),
                Arc::new(StringArray::from(vec!["loss"])),
                Arc::new(Float64Array::from(vec![1.0])),
            ],
        )
        .unwrap();
        assert!(normalize_batch(&TelemetryKind::Scalars, &null_run).unwrap_err().contains("null"));

        // Image columns sent with a scalars descriptor.
        let image = media_batch("image_data", "r", "i", vec![1], vec![b"x"], "png");
        assert!(normalize_batch(&TelemetryKind::Scalars, &image).is_err());
        assert!(normalize_batch(&TelemetryKind::Unknown, &image).is_err());
    }

    #[test]
    fn kinds_are_inferred_from_column_names() {
        let audio = media_batch("audio_data", "r", "a", vec![1], vec![b"x"], "wav");
        let video = media_batch("video_data", "r", "v", vec![1], vec![b"x"], "mp4");
        assert_eq!(TelemetryKind::from_batch(&audio), TelemetryKind::Audio);
        assert_eq!(TelemetryKind::from_batch(&video), TelemetryKind::Video);
        assert_eq!(TelemetryKind::from_batch(&hparam_batch("r", "k", "v")), TelemetryKind::Hyperparameters);
        assert_eq!(TelemetryKind::from_batch(&scalar_batch(vec![1], vec!["r"], vec!["m"], vec![1.0], None)), TelemetryKind::Scalars);
        assert_eq!(TelemetryKind::from_descriptor_path(&["audio".to_string()]), TelemetryKind::Audio);
        assert_eq!(TelemetryKind::from_descriptor_path(&["video".to_string()]), TelemetryKind::Video);
        assert_eq!(TelemetryKind::Audio.as_dir_name(), Some("audio"));
        assert_eq!(TelemetryKind::Video.as_dir_name(), Some("video"));
    }

    // ---------------------------------------------------------------------
    // Flight ingestion (real gRPC server)
    // ---------------------------------------------------------------------

    async fn start_flight_server(t_junction: Arc<TJunction>) -> String {
        use arrow_flight::flight_service_server::FlightServiceServer;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let service = FlightServiceServer::new(crate::ingestion::AeroboardFlightServer { t_junction });
        tokio::spawn(tonic::transport::Server::builder().add_service(service).serve(addr));
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        format!("http://{addr}")
    }

    async fn flight_put(url: &str, path: &str, batch: RecordBatch) -> Result<(), tonic::Status> {
        use arrow_flight::{encode::FlightDataEncoderBuilder, FlightClient, FlightDescriptor};
        use futures::TryStreamExt;
        let channel = tonic::transport::Channel::from_shared(url.to_string()).unwrap().connect().await.unwrap();
        let mut client = FlightClient::new(channel);
        let stream = FlightDataEncoderBuilder::new()
            .with_flight_descriptor(Some(FlightDescriptor::new_path(vec![path.to_string()])))
            .build(futures::stream::iter(vec![Ok(batch)]));
        let results = client.do_put(stream).await.map_err(tonic::Status::from)?;
        results.try_collect::<Vec<_>>().await.map_err(tonic::Status::from)?;
        Ok(())
    }

    #[tokio::test]
    async fn flight_rejects_malformed_batches_and_keeps_serving() {
        let (hub, mut persist_rx) = TJunction::new(16, 16);
        let url = start_flight_server(Arc::new(hub)).await;
        // Stand-in persistence loop: record what was published and acknowledge it.
        let (published_tx, mut published) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(job) = persist_rx.recv().await {
                let _ = published_tx.send(job.batch.clone());
                let _ = job.done.send(Ok(()));
            }
        });

        // `value` as a string: previously this panicked inside the persistence task.
        let bad = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
                Field::new("run_id", DataType::Utf8, false),
                Field::new("name", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1])),
                Arc::new(StringArray::from(vec!["run"])),
                Arc::new(StringArray::from(vec!["loss"])),
                Arc::new(StringArray::from(vec!["oops"])),
            ],
        )
        .unwrap();
        let error = flight_put(&url, "scalars", bad).await.unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument, "{error}");

        // A legacy 4-column scalar batch is accepted and normalised.
        let legacy = RecordBatch::try_new(
            legacy_scalar_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1, 2])),
                Arc::new(StringArray::from(vec!["run", "run"])),
                Arc::new(StringArray::from(vec!["loss", "loss"])),
                Arc::new(Float64Array::from(vec![0.5, 0.4])),
            ],
        )
        .unwrap();
        flight_put(&url, "scalars", legacy).await.unwrap();

        let first = tokio::time::timeout(Duration::from_secs(2), published.recv()).await.unwrap().unwrap();
        assert_eq!(first.kind, TelemetryKind::Scalars);
        assert_eq!(first.batch.num_columns(), 5);
        assert_eq!(first.batch.num_rows(), 2);
        assert!(published.try_recv().is_err(), "the rejected batch must not be published");
    }

    #[tokio::test]
    async fn flight_acknowledges_only_after_persistence_and_reports_failures() {
        let (hub, mut persist_rx) = TJunction::new(16, 16);
        let url = start_flight_server(Arc::new(hub)).await;
        let batch = || scalar_batch(vec![1], vec!["r"], vec!["m"], vec![1.0], None);

        let upload = { let url = url.clone(); tokio::spawn(async move { flight_put(&url, "scalars", batch()).await }) };
        let job = tokio::time::timeout(Duration::from_secs(2), persist_rx.recv()).await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!upload.is_finished(), "the upload must wait for the write");
        job.done.send(Ok(())).unwrap();
        upload.await.unwrap().unwrap();

        let upload = tokio::spawn(async move { flight_put(&url, "scalars", batch()).await });
        let job = persist_rx.recv().await.unwrap();
        job.done.send(Err("disk full".to_string())).unwrap();
        let error = upload.await.unwrap().unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unavailable);
        assert!(error.message().contains("disk full"), "{error}");
    }

    // ---------------------------------------------------------------------
    // T-junction
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn persistence_queue_applies_backpressure_instead_of_dropping() {
        let (hub, mut persist_rx) = TJunction::new(1, 1);
        let hub = Arc::new(hub);
        let batch = || telemetry(TelemetryKind::Scalars, scalar_batch(vec![1], vec!["r"], vec!["m"], vec![1.0], None));

        hub.publish(batch()).await.unwrap();
        // The queue is full: the next publish waits rather than dropping a batch.
        let pending = { let hub = hub.clone(); tokio::spawn(async move { hub.publish(batch()).await }) };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!pending.is_finished());

        persist_rx.recv().await.unwrap();
        pending.await.unwrap().unwrap();
        persist_rx.recv().await.unwrap();

        assert_eq!(hub.current_runs(), vec!["r".to_string()]);

        hub.close_persistence();
        assert!(hub.publish(batch()).await.is_err());
        assert!(persist_rx.recv().await.is_none(), "closing ends the persistence loop");
    }

    // ---------------------------------------------------------------------
    // Line protocol (GreptimeDB)
    // ---------------------------------------------------------------------

    #[test]
    fn line_protocol_uses_run_id_as_tag_and_escapes_values() {
        let scalars = scalar_batch(
            vec![1_700_000_000_000, 1_700_000_000_001],
            vec!["my run,1", "run-a"],
            vec!["train/loss", "x"],
            vec![0.123, f64::NAN],
            Some(vec![Some(7.0), None]),
        );
        let lp = record_batch_to_line_protocol(&scalars, &TelemetryKind::Scalars).unwrap();
        assert_eq!(lp, "scalars,run_id=my\\ run\\,1,name=train/loss value=0.123,step=7 1700000000000000000\n");

        let lp = record_batch_to_line_protocol(&hparam_batch("run-a", "learning_rate", "0.\"01"), &TelemetryKind::Hyperparameters).unwrap();
        assert!(lp.starts_with("hyperparameters,run_id=run-a,key=learning_rate value=\"0.\\\"01\""), "{lp}");

        let image = media_batch("image_data", "run-image", "samples/generated", vec![1], vec![b"img"], "png");
        let lp = record_batch_to_line_protocol(&image, &TelemetryKind::Images).unwrap();
        assert!(lp.starts_with("images,run_id=run-image,name=samples/generated format=\"png\",image_hex=\"696d67\""), "{lp}");

        let audio = media_batch("audio_data", "run-a", "speech", vec![1], vec![&[0xde, 0xad]], "wav");
        let lp = record_batch_to_line_protocol(&audio, &TelemetryKind::Audio).unwrap();
        assert!(lp.starts_with("audio,run_id=run-a,name=speech format=\"wav\",audio_hex=\"dead\""), "{lp}");
    }

    #[test]
    fn test_hex_codec_roundtrip() {
        let bytes: Vec<u8> = (0..=255).collect();
        let batch = media_batch("video_data", "r", "v", vec![1], vec![&bytes], "mp4");
        let lp = record_batch_to_line_protocol(&batch, &TelemetryKind::Video).unwrap();
        let hex = lp.split("video_hex=\"").nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(crate::storage::decode_hex_for_tests(hex).unwrap(), bytes);
        assert!(crate::storage::decode_hex_for_tests("0g").is_err());
        assert!(crate::storage::decode_hex_for_tests("abc").is_err());
    }

    // ---------------------------------------------------------------------
    // Run IDs and summaries
    // ---------------------------------------------------------------------

    #[test]
    fn test_build_run_summaries() {
        let point = |ts_ms, name: &str, value| HistoryPoint { ts_ms, run_id: "run-a".into(), name: name.into(), value, step: None };
        let points = vec![point(10, "train/loss", 0.9), point(20, "train/loss", 0.5), point(30, "val/acc", 0.8)];
        let hparams = vec![
            ("run-a".into(), HyperparameterEntry { key: "learning_rate".into(), value: "0.1".into(), value_type: "float".into() }),
        ];

        let summaries = crate::history::build_run_summaries(&points, hparams);
        assert_eq!(summaries.len(), 1);
        let summary = &summaries[0];
        assert_eq!(summary.run_id, "run-a");
        assert_eq!(summary.hyperparameters.len(), 1);
        assert_eq!(summary.metric_summaries.len(), 2);
        let loss_summary = summary.metric_summaries.iter().find(|m| m.name == "train/loss").unwrap();
        assert_eq!(loss_summary.last, 0.5);
        assert_eq!(loss_summary.min, 0.5);
        assert_eq!(loss_summary.max, 0.9);
    }

    #[test]
    fn summary_last_breaks_timestamp_ties_by_step() {
        // Three steps logged within one millisecond, returned out of order.
        let point = |step, value| HistoryPoint { ts_ms: 5, run_id: "r".into(), name: "acc".into(), value, step: Some(step) };
        let summaries = crate::history::build_run_summaries(&[point(2.0, 0.9), point(0.0, 0.1), point(1.0, 0.5)], vec![]);
        assert_eq!(summaries[0].metric_summaries[0].last, 0.9);
    }

    #[test]
    fn test_server_side_run_resolution() {
        use crate::history::resolve_run_id;
        let existing = BTreeSet::from(["experiment".to_string(), "experiment-v2".to_string(), "experiment-v4".to_string()]);
        assert_eq!(resolve_run_id("experiment", &existing), "experiment-v5");

        let lightning = BTreeSet::from(["exp-pl2.5.1".to_string(), "exp-v2-pl2.5.1".to_string()]);
        assert_eq!(resolve_run_id("exp-pl2.5.1", &lightning), "exp-v3-pl2.5.1");
        assert_eq!(resolve_run_id("exp-pl2.4.0", &lightning), "exp-pl2.4.0");

        // `-pl` inside an ordinary name is not a framework suffix.
        let placement = BTreeSet::from(["sample-placement".to_string()]);
        assert_eq!(resolve_run_id("sample-placement", &placement), "sample-placement-v2");
    }

    #[tokio::test]
    async fn concurrent_resolves_of_one_name_get_distinct_ids() {
        let dir = temp_dir("resolve");
        let app = build_app(filesystem_state(&dir));
        let resolve = || {
            app.clone().oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/runs/resolve")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"base_run_id":"exp"}"#))
                    .unwrap(),
            )
        };
        let (first, second) = tokio::join!(resolve(), resolve());
        let first: serde_json::Value = serde_json::from_slice(&body_bytes(first.unwrap()).await).unwrap();
        let second: serde_json::Value = serde_json::from_slice(&body_bytes(second.unwrap()).await).unwrap();
        let ids = BTreeSet::from([first["run_id"].as_str().unwrap().to_string(), second["run_id"].as_str().unwrap().to_string()]);
        assert_eq!(ids, BTreeSet::from(["exp".to_string(), "exp-v2".to_string()]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn summary_cache_loads_once_then_folds_new_batches() {
        let dir = temp_dir("summaries");
        let state = filesystem_state(&dir);
        let first = telemetry(TelemetryKind::Scalars, scalar_batch(vec![1, 2], vec!["run-a", "run-a"], vec!["loss", "loss"], vec![0.9, 0.5], None));
        state.storage.append_batch(first).await.unwrap();
        state.storage.append_batch(telemetry(TelemetryKind::Hyperparameters, hparam_batch("run-a", "lr", "0.1"))).await.unwrap();

        let runs = BTreeSet::from(["run-a".to_string()]);
        let summaries = state.summaries.summaries(state.storage.as_ref(), &runs).await.unwrap();
        assert_eq!(summaries[0].metric_summaries[0].last, 0.5);

        // Persisted later: folded in by the persistence loop's `record`.
        let later = telemetry(TelemetryKind::Scalars, scalar_batch(vec![3], vec!["run-a"], vec!["loss"], vec![0.1], None));
        state.storage.append_batch(later.clone()).await.unwrap();
        state.summaries.record(&later).await;
        state.summaries.record(&telemetry(TelemetryKind::Hyperparameters, hparam_batch("run-a", "lr", "0.2"))).await;

        let summaries = state.summaries.summaries(state.storage.as_ref(), &runs).await.unwrap();
        let loss = &summaries[0].metric_summaries[0];
        assert_eq!((loss.last, loss.min, loss.max), (0.1, 0.1, 0.9));
        assert_eq!(summaries[0].hyperparameters[0].value, "0.2");

        // Runs with no data are omitted rather than returned empty.
        let missing = BTreeSet::from(["nope".to_string()]);
        assert!(state.summaries.summaries(state.storage.as_ref(), &missing).await.unwrap().is_empty());

        state.summaries.clear().await;
        state.storage.clear_history().await.unwrap();
        assert!(state.summaries.summaries(state.storage.as_ref(), &runs).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------------------------------------------------------------------
    // Filesystem store
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn test_filesystem_store_filters_history_by_run_and_pages() {
        let dir = temp_dir("fs-page");
        let store = FilesystemStore::new(dir.clone()).unwrap();
        let batch = scalar_batch(vec![1, 2, 3], vec!["run-a", "run-b", "run-a"], vec!["train/loss", "train/loss", "train/acc"], vec![0.5, 0.7, 0.9], None);
        store.append_batch(telemetry(TelemetryKind::Scalars, batch)).await.unwrap();

        let page = store
            .query_history(&HistoryQueryOptions {
                run_ids: BTreeSet::from(["run-a".to_string()]),
                limit: Some(1),
                cursor: Some(1),
                ..HistoryQueryOptions::default()
            })
            .await
            .unwrap();

        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].run_id, "run-a");
        assert_eq!(page.rows[0].name, "train/acc");
        assert_eq!(page.next_cursor, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn filesystem_store_appends_jsonl_and_round_trips_steps_and_nan() {
        let dir = temp_dir("fs-jsonl");
        let store = FilesystemStore::new(dir.clone()).unwrap();
        for (ts, value, step) in [(1, 0.5, Some(10.0)), (2, f64::NAN, Some(11.0)), (3, f64::INFINITY, None)] {
            let batch = scalar_batch(vec![ts], vec!["run-a"], vec!["loss"], vec![value], Some(vec![step]));
            store.append_batch(telemetry(TelemetryKind::Scalars, batch)).await.unwrap();
        }

        let scalar_dir = dir.join("runs").join("run-a").join("scalars");
        let files: Vec<String> = std::fs::read_dir(&scalar_dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(files, vec!["index.jsonl".to_string()], "no per-batch .arrow files, no rewritten index.json");
        assert_eq!(std::fs::read_to_string(scalar_dir.join("index.jsonl")).unwrap().lines().count(), 3);

        let rows = store.query_history(&HistoryQueryOptions::default()).await.unwrap().rows;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].step, Some(10.0));
        assert!(rows[1].value.is_nan());
        assert_eq!(rows[2].value, f64::INFINITY);
        assert_eq!(rows[2].step, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn filesystem_store_survives_torn_lines_and_reads_legacy_indexes() {
        let dir = temp_dir("fs-legacy");
        let store = FilesystemStore::new(dir.clone()).unwrap();
        store.append_batch(telemetry(TelemetryKind::Scalars, scalar_batch(vec![1], vec!["run-a"], vec!["loss"], vec![0.5], None))).await.unwrap();

        // Simulate a crash mid-write, then keep logging.
        let scalar_dir = dir.join("runs").join("run-a").join("scalars");
        let mut file = std::fs::OpenOptions::new().append(true).open(scalar_dir.join("index.jsonl")).unwrap();
        std::io::Write::write_all(&mut file, br#"{"t":2,"n":"lo"#).unwrap();
        drop(file);
        store.append_batch(telemetry(TelemetryKind::Scalars, scalar_batch(vec![3], vec!["run-a"], vec!["loss"], vec![0.3], None))).await.unwrap();

        // Pre-0.2 index.json array (NaN was written as null).
        std::fs::write(
            scalar_dir.join("index.json"),
            r#"[{"ts_ms":0,"run_id":"run-a","name":"loss","value":0.9},{"ts_ms":0,"run_id":"run-a","name":"bad","value":null}]"#,
        )
        .unwrap();

        let rows = store.query_history(&HistoryQueryOptions::default()).await.unwrap().rows;
        assert_eq!(rows.iter().map(|row| row.ts_ms).collect::<Vec<_>>(), vec![0, 1, 3]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn filesystem_store_queries_hyperparameters() {
        let dir = temp_dir("fs-hparams");
        let store = FilesystemStore::new(dir.clone()).unwrap();
        store.append_batch(telemetry(TelemetryKind::Hyperparameters, hparam_batch("run-a", "learning_rate", "0.01"))).await.unwrap();

        let hparams = store.query_hyperparameters(&BTreeSet::from(["run-a".to_string()])).await.unwrap();
        assert_eq!(hparams.len(), 1);
        assert_eq!(hparams[0].1.key, "learning_rate");
        assert_eq!(store.list_runs().await.unwrap(), vec!["run-a".to_string()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn filesystem_store_persists_images_as_files() {
        let dir = temp_dir("fs-images");
        let store = FilesystemStore::new(dir.clone()).unwrap();
        let batch = media_batch("image_data", "run-image", "samples/generated", vec![1_700_000_000_000], vec![b"img"], "png");
        store.append_batch(telemetry(TelemetryKind::Images, batch)).await.unwrap();

        let page = store.query_image_history(&ImageQueryOptions::default()).await.unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.next_cursor, None);
        assert_eq!(page.rows[0].run_id, "run-image");
        assert_eq!(page.rows[0].format, "png");
        assert!(page.rows[0].image_base64.is_none());

        let with_data = store.query_image_history(&ImageQueryOptions { include_data: true, ..ImageQueryOptions::default() }).await.unwrap();
        assert_eq!(with_data.rows[0].image_base64.as_deref(), Some("aW1n"));

        let payload = store
            .load_media(MediaKind::Image, &ImageFrameQuery { run_id: "run-image".into(), name: "samples/generated".into(), ts_ms: 1_700_000_000_000 })
            .await
            .unwrap()
            .expect("expected image payload");
        assert_eq!(read_media(payload).await, b"img");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn test_filesystem_store_persists_audio_and_video_separately() {
        let dir = temp_dir("fs-media");
        let store = FilesystemStore::new(dir.clone()).unwrap();

        let audio = media_batch("audio_data", "run-m", "speech", vec![20, 10], vec![b"a2", b"a1"], "wav");
        let video = media_batch("video_data", "run-m", "rollout", vec![30], vec![b"v1"], "mp4");
        store.append_batch(telemetry(TelemetryKind::Audio, audio)).await.unwrap();
        store.append_batch(telemetry(TelemetryKind::Video, video)).await.unwrap();

        let audio_page = store.query_media_history(MediaKind::Audio, &ImageQueryOptions::default()).await.unwrap();
        assert_eq!(audio_page.rows.iter().map(|row| row.ts_ms).collect::<Vec<_>>(), vec![10, 20]);
        assert!(audio_page.rows.iter().all(|row| row.name == "speech" && row.format == "wav"));

        let video_page = store.query_media_history(MediaKind::Video, &ImageQueryOptions::default()).await.unwrap();
        assert_eq!(video_page.rows.len(), 1);
        assert_eq!(video_page.rows[0].name, "rollout");

        // Kinds are isolated from each other and from images.
        assert!(store.query_image_history(&ImageQueryOptions::default()).await.unwrap().rows.is_empty());

        let payload = store
            .load_media(MediaKind::Audio, &ImageFrameQuery { run_id: "run-m".into(), name: "speech".into(), ts_ms: 20 })
            .await
            .unwrap()
            .expect("audio payload");
        let MediaSource::File(path) = payload.source else { panic!("filesystem payloads should stream from disk") };
        assert_eq!(std::fs::read(path).unwrap(), b"a2");
        assert!(store
            .load_media(MediaKind::Video, &ImageFrameQuery { run_id: "run-m".into(), name: "speech".into(), ts_ms: 20 })
            .await
            .unwrap()
            .is_none());
        assert_eq!(store.list_runs().await.unwrap(), vec!["run-m".to_string()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ---------------------------------------------------------------------
    // HTTP API
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn history_arrow_format_round_trips_with_steps() {
        let dir = temp_dir("arrow-route");
        let state = filesystem_state(&dir);
        let batch = scalar_batch(vec![1, 2], vec!["run-a", "run-a"], vec!["loss", "loss"], vec![0.5, 0.4], Some(vec![Some(1.0), None]));
        state.storage.append_batch(telemetry(TelemetryKind::Scalars, batch)).await.unwrap();
        let app = build_app(state);

        let response = get(&app, "/api/history?run_id=run-a&format=arrow").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[axum::http::header::CONTENT_TYPE], crate::history::ARROW_STREAM_CONTENT_TYPE);
        let bytes = body_bytes(response).await;
        let reader = arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None).unwrap();
        let batches: Vec<RecordBatch> = reader.map(Result::unwrap).collect();
        assert_eq!(batches[0].num_rows(), 2);
        let steps = batches[0].column_by_name("step").unwrap().as_primitive::<arrow::datatypes::Float64Type>();
        assert_eq!(steps.value(0), 1.0);
        assert!(steps.is_null(1));
        assert!(matches!(batches[0].column_by_name("run_id").unwrap().data_type(), DataType::Dictionary(_, _)));

        let json: serde_json::Value = serde_json::from_slice(&body_bytes(get(&app, "/api/history?run_id=run-a").await).await).unwrap();
        assert_eq!(json["rows"][0]["step"], 1.0);
        assert!(json["rows"][1].get("step").is_none());
        assert_eq!(json["run_summaries"][0]["metric_summaries"][0]["last"], 0.4);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn storage_failures_return_error_statuses() {
        let dir = temp_dir("failing");
        let app = build_app(state_with_storage(&dir, Arc::new(FailingStore)));
        for uri in ["/api/history", "/api/runs/summary", "/api/history/images", "/api/history/media?kind=audio"] {
            let response = get(&app, uri).await;
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR, "{uri}");
            let json: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
            assert!(json["error"].as_str().unwrap().contains("disk on fire"), "{uri}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn cors_is_same_origin_unless_configured() {
        let dir = temp_dir("cors");
        let request = || Request::builder().uri("/api/runs").header("origin", "https://evil.example").body(Body::empty()).unwrap();

        let app = build_app(filesystem_state(&dir));
        let response = app.oneshot(request()).await.unwrap();
        assert!(response.headers().get("access-control-allow-origin").is_none());

        let mut state = filesystem_state(&dir);
        state.config.cors = CorsPolicy::Origins(vec!["https://evil.example".to_string()]);
        let response = build_app(state).oneshot(request()).await.unwrap();
        assert_eq!(response.headers()["access-control-allow-origin"], "https://evil.example");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_app_routes_expose_run_summaries_endpoint() {
        let dir = temp_dir("route-summary");
        let app = build_app(filesystem_state(&dir));
        let response = get(&app, "/api/runs/summary").await;
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response.headers()[axum::http::header::CONTENT_TYPE].to_str().unwrap().to_string();
        assert!(content_type.starts_with("application/json"));

        let missing = get(&app, "/api/history/images/frame?run_id=r&name=n&ts_ms=1").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND, "base64 frame endpoint was replaced by /history/media/content");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn test_config_route_serves_dashboard_defaults_and_reports_errors() {
        let dir = temp_dir("config");
        let mut state = filesystem_state(&dir);
        let config_path = dir.join("aeroboard.toml");
        state.config.dashboard_config_path = Some(config_path.clone());
        let app = build_app(state);

        std::fs::write(&config_path, "[dashboard]\nstarred = [\"loss\"]\n[dashboard.targets]\nloss = 0.05\n").unwrap();
        let response = get(&app, "/api/config").await;
        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
        assert_eq!(json["dashboard"]["starred"], serde_json::json!(["loss"]));
        assert_eq!(json["dashboard"]["targets"]["loss"], serde_json::json!(0.05));
        assert!(json.get("error").is_none());

        // Edits apply without a restart; a broken file degrades to empty defaults + an error.
        std::fs::write(&config_path, "[dashboard\n").unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes(get(&app, "/api/config").await).await).unwrap();
        assert_eq!(json["dashboard"]["starred"], serde_json::json!([]));
        assert!(json["error"].as_str().unwrap().contains("aeroboard.toml"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_byte_range() {
        use crate::history::parse_byte_range;
        assert_eq!(parse_byte_range("bytes=0-3", 10), Some((0, 3)));
        assert_eq!(parse_byte_range("bytes=4-", 10), Some((4, 9)));
        assert_eq!(parse_byte_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_byte_range("bytes=5-100", 10), Some((5, 9)));
        assert_eq!(parse_byte_range("bytes=0-1, 4-5", 10), Some((0, 1)));
        assert_eq!(parse_byte_range("bytes=10-", 10), None);
        assert_eq!(parse_byte_range("bytes=6-2", 10), None);
        assert_eq!(parse_byte_range("bytes=-0", 10), None);
        assert_eq!(parse_byte_range("items=0-1", 10), None);
    }

    #[tokio::test]
    async fn test_media_routes_list_metadata_and_stream_content_with_ranges() {
        let dir = temp_dir("media-route");
        let state = filesystem_state(&dir);
        let video = media_batch("video_data", "run v", "eval/rollout", vec![1_700_000_000_000], vec![b"0123456789"], "mp4");
        state.storage.append_batch(telemetry(TelemetryKind::Video, video)).await.unwrap();
        let image = media_batch("image_data", "run v", "samples", vec![5], vec![b"png-bytes"], "png");
        state.storage.append_batch(telemetry(TelemetryKind::Images, image)).await.unwrap();
        let app = build_app(state);

        let list = get(&app, "/api/history/media?kind=video&run_id=run%20v").await;
        assert_eq!(list.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&body_bytes(list).await).unwrap();
        let rows = json["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["format"], "mp4");
        assert!(rows[0].get("image_base64").is_none(), "media listing must not inline payloads");

        let content_uri = "/api/history/media/content?kind=video&run_id=run%20v&name=eval%2Frollout&ts_ms=1700000000000";
        let full = get(&app, content_uri).await;
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[axum::http::header::CONTENT_TYPE], "video/mp4");
        assert_eq!(full.headers()[axum::http::header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(body_bytes(full).await, b"0123456789");

        let partial = app.clone()
            .oneshot(Request::builder().uri(content_uri).header(axum::http::header::RANGE, "bytes=2-5").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.headers()[axum::http::header::CONTENT_RANGE], "bytes 2-5/10");
        assert_eq!(body_bytes(partial).await, b"2345");

        // Images are served raw by the same endpoint (the dashboard uses it as <img src>).
        let image = get(&app, "/api/history/media/content?kind=image&run_id=run%20v&name=samples&ts_ms=5").await;
        assert_eq!(image.status(), StatusCode::OK);
        assert_eq!(image.headers()[axum::http::header::CONTENT_TYPE], "image/png");
        assert_eq!(body_bytes(image).await, b"png-bytes");

        let missing = get(&app, "/api/history/media/content?kind=audio&run_id=run%20v&name=eval%2Frollout&ts_ms=1700000000000").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let bad_kind = get(&app, "/api/history/media?kind=hologram").await;
        assert_eq!(bad_kind.status(), StatusCode::BAD_REQUEST);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ---------------------------------------------------------------------
    // Live streaming
    // ---------------------------------------------------------------------

    #[test]
    fn test_live_batch_strips_av_payloads_only_when_persisted() {
        let video = Arc::new(media_batch("video_data", "r", "v", vec![1, 2], vec![b"abc", b"defg"], "mp4"));
        let video_telemetry = TelemetryBatch::new(TelemetryKind::Video, video.clone());

        let stripped = crate::streaming::live_batch(&video_telemetry, true);
        assert_eq!(stripped.schema(), video.schema());
        assert_eq!(stripped.num_rows(), 2);
        let data = stripped.column(3).as_any().downcast_ref::<BinaryArray>().unwrap();
        assert!(data.iter().all(|value| value == Some(&[][..])));

        let live_only = crate::streaming::live_batch(&video_telemetry, false);
        let data = live_only.column(3).as_any().downcast_ref::<BinaryArray>().unwrap();
        assert_eq!(data.value(1), b"defg");

        let image = Arc::new(image_batch_for_live());
        let image_telemetry = TelemetryBatch::new(TelemetryKind::Images, image);
        let untouched = crate::streaming::live_batch(&image_telemetry, true);
        let data = untouched.column(3).as_any().downcast_ref::<BinaryArray>().unwrap();
        assert_eq!(data.value(0), b"img");
    }

    fn image_batch_for_live() -> RecordBatch {
        RecordBatch::try_new(
            image_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1])),
                Arc::new(StringArray::from(vec!["r"])),
                Arc::new(StringArray::from(vec!["i"])),
                Arc::new(BinaryArray::from(vec![b"img".as_slice()])),
                Arc::new(StringArray::from(vec!["png"])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn live_payload_is_encoded_once_per_batch() {
        let telemetry = TelemetryBatch::new(TelemetryKind::Scalars, Arc::new(scalar_batch(vec![1], vec!["r"], vec!["m"], vec![1.0], None)));
        let first = crate::streaming::live_payload(&telemetry, true).unwrap();
        let second = crate::streaming::live_payload(&telemetry, true).unwrap();
        assert_eq!(first.as_ptr(), second.as_ptr(), "subscribers share one encoding");

        let message: serde_json::Value = serde_json::from_str(&crate::streaming::lagged_message(3)).unwrap();
        assert_eq!(message, serde_json::json!({ "type": "lagged", "skipped": 3 }));
    }
}
