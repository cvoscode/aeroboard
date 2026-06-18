#[cfg(test)]
mod tests_integration {
    use axum::{body::Body, http::{Request, StatusCode}};
    use arrow::array::{BinaryArray, Float64Array, StringArray, TimestampMillisecondArray};
    use arrow::record_batch::RecordBatch;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tower::util::ServiceExt;

    use crate::history::{HyperparameterEntry, HistoryPoint};
    use crate::app_state::AppState;
    use crate::config::{AppConfig, StorageMode};
    use crate::build_app;
    use crate::schema::{hyperparameter_schema, image_schema, scalar_schema};
    use crate::storage::{
        record_batch_to_line_protocol, FilesystemStore, HistoryQueryOptions, ImageFrameQuery,
        ImageQueryOptions, StorageEngine,
    };
    use crate::t_junction::TJunction;
    use crate::telemetry::{TelemetryBatch, TelemetryKind};

    #[tokio::test]
    async fn test_mock_insert() {
        let timestamps = TimestampMillisecondArray::from(vec![1678123456000]);
        let run_ids = StringArray::from(vec!["run-a"]);
        let names = StringArray::from(vec!["loss"]);
        let values = Float64Array::from(vec![0.42]);

        let batch = RecordBatch::try_new(
            scalar_schema(),
            vec![Arc::new(timestamps), Arc::new(run_ids), Arc::new(names), Arc::new(values)],
        )
        .unwrap();

        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 4);
        
        // This validates we can craft batches locally matching the schema design
        println!("Successfully crafted Arrow Batch: {:?}", batch);
    }

    #[test]
    fn test_scalar_and_hparams_line_protocol() {
        use crate::telemetry::TelemetryKind;

        let scalar_batch = RecordBatch::try_new(
            scalar_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1_700_000_000_000])),
                Arc::new(StringArray::from(vec!["run-a"])),
                Arc::new(StringArray::from(vec!["train/loss"])),
                Arc::new(Float64Array::from(vec![0.123])),
            ],
        ).unwrap();

        let scalar_lp = record_batch_to_line_protocol(&scalar_batch, &TelemetryKind::Scalars).unwrap();
        assert!(scalar_lp.contains("scalars,name=train/loss"));
        assert!(scalar_lp.contains("run_id=\"run-a\""));

        let hparam_batch = RecordBatch::try_new(
            hyperparameter_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1_700_000_000_000])),
                Arc::new(StringArray::from(vec!["run-a"])),
                Arc::new(StringArray::from(vec!["learning_rate"])),
                Arc::new(StringArray::from(vec!["0.01"])),
                Arc::new(StringArray::from(vec!["float"])),
            ],
        ).unwrap();

        let hparam_lp = record_batch_to_line_protocol(&hparam_batch, &TelemetryKind::Hyperparameters).unwrap();
        assert!(hparam_lp.contains("hyperparameters,key=learning_rate"));
        assert!(hparam_lp.contains("value=\"0.01\""));
    }

    #[test]
    fn test_image_line_protocol() {
        use crate::telemetry::TelemetryKind;

        let batch = RecordBatch::try_new(
            image_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1_700_000_000_000])),
                Arc::new(StringArray::from(vec!["run-image"])),
                Arc::new(StringArray::from(vec!["samples/generated"])),
                Arc::new(BinaryArray::from(vec![b"img".as_slice()])),
                Arc::new(StringArray::from(vec!["png"])),
            ],
        ).unwrap();

        let lp = record_batch_to_line_protocol(&batch, &TelemetryKind::Images).unwrap();
        assert!(lp.contains("images,name=samples/generated"));
        assert!(lp.contains("image_hex=\"696d67\""));
    }

    #[test]
    fn test_build_run_summaries() {
        let points = vec![
            HistoryPoint { ts_ms: 10, run_id: "run-a".into(), name: "train/loss".into(), value: 0.9 },
            HistoryPoint { ts_ms: 20, run_id: "run-a".into(), name: "train/loss".into(), value: 0.5 },
            HistoryPoint { ts_ms: 30, run_id: "run-a".into(), name: "val/acc".into(), value: 0.8 },
        ];
        let hparams = vec![
            ("run-a".into(), HyperparameterEntry { key: "learning_rate".into(), value: "0.1".into(), value_type: "float".into() }),
        ];

        let summaries = super::super::history::build_run_summaries(&points, hparams);
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
    fn test_server_side_run_resolution() {
        let existing = BTreeSet::from([
            "experiment".to_string(),
            "experiment-v2".to_string(),
            "experiment-v4".to_string(),
        ]);

        let resolved = super::super::history::resolve_run_id("experiment", &existing);
        assert_eq!(resolved, "experiment-v5");
    }

    #[tokio::test]
    async fn test_filesystem_store_persists_images_as_files() {
        let temp_dir = std::env::temp_dir().join(format!("aeroboard-test-{}", uuid::Uuid::new_v4()));
        let store = FilesystemStore::new(temp_dir.clone()).unwrap();

        let batch = RecordBatch::try_new(
            image_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1_700_000_000_000])),
                Arc::new(StringArray::from(vec!["run-image"])),
                Arc::new(StringArray::from(vec!["samples/generated"])),
                Arc::new(BinaryArray::from(vec![b"img".as_slice()])),
                Arc::new(StringArray::from(vec!["png"])),
            ],
        ).unwrap();

        store.append_batch(Arc::new(TelemetryBatch::new(TelemetryKind::Images, Arc::new(batch)))).await.unwrap();

        let page = store.query_image_history(&ImageQueryOptions::default()).await.unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.next_cursor, None);
        assert_eq!(page.rows[0].run_id, "run-image");
        assert_eq!(page.rows[0].name, "samples/generated");
        assert_eq!(page.rows[0].format, "png");
        assert!(page.rows[0].image_base64.is_none());

        let payload = store
            .load_image_frame(&ImageFrameQuery {
                run_id: "run-image".into(),
                name: "samples/generated".into(),
                ts_ms: 1_700_000_000_000,
            })
            .await
            .unwrap()
            .expect("expected image payload");
        assert_eq!(payload.image_base64, "aW1n");

        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[tokio::test]
    async fn test_filesystem_store_filters_history_by_run_and_pages() {
        let temp_dir = std::env::temp_dir().join(format!("aeroboard-test-{}", uuid::Uuid::new_v4()));
        let store = FilesystemStore::new(temp_dir.clone()).unwrap();

        let batch = RecordBatch::try_new(
            scalar_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec!["run-a", "run-b", "run-a"])),
                Arc::new(StringArray::from(vec!["train/loss", "train/loss", "train/acc"])),
                Arc::new(Float64Array::from(vec![0.5, 0.7, 0.9])),
            ],
        )
        .unwrap();

        store
            .append_batch(Arc::new(TelemetryBatch::new(TelemetryKind::Scalars, Arc::new(batch))))
            .await
            .unwrap();

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

        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[tokio::test]
    async fn test_filesystem_store_queries_indexes_without_arrow_files() {
        let temp_dir = std::env::temp_dir().join(format!("aeroboard-index-test-{}", uuid::Uuid::new_v4()));
        let store = FilesystemStore::new(temp_dir.clone()).unwrap();

        let scalar_batch = RecordBatch::try_new(
            scalar_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1, 2])),
                Arc::new(StringArray::from(vec!["run-a", "run-a"])),
                Arc::new(StringArray::from(vec!["train/loss", "train/acc"])),
                Arc::new(Float64Array::from(vec![0.5, 0.9])),
            ],
        )
        .unwrap();
        store
            .append_batch(Arc::new(TelemetryBatch::new(TelemetryKind::Scalars, Arc::new(scalar_batch))))
            .await
            .unwrap();

        let hparam_batch = RecordBatch::try_new(
            hyperparameter_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1])),
                Arc::new(StringArray::from(vec!["run-a"])),
                Arc::new(StringArray::from(vec!["learning_rate"])),
                Arc::new(StringArray::from(vec!["0.01"])),
                Arc::new(StringArray::from(vec!["float"])),
            ],
        )
        .unwrap();
        store
            .append_batch(Arc::new(TelemetryBatch::new(TelemetryKind::Hyperparameters, Arc::new(hparam_batch))))
            .await
            .unwrap();

        let scalar_dir = temp_dir.join("runs").join("run-a").join("scalars");
        let hparam_dir = temp_dir.join("runs").join("run-a").join("hyperparameters");
        for dir in [&scalar_dir, &hparam_dir] {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                if entry.path().extension().and_then(|ext| ext.to_str()) == Some("arrow") {
                    std::fs::remove_file(entry.path()).unwrap();
                }
            }
        }

        let history = store
            .query_history(&HistoryQueryOptions {
                run_ids: BTreeSet::from(["run-a".to_string()]),
                ..HistoryQueryOptions::default()
            })
            .await
            .unwrap();
        assert_eq!(history.rows.len(), 2);

        let hparams = store
            .query_hyperparameters(&BTreeSet::from(["run-a".to_string()]))
            .await
            .unwrap();
        assert_eq!(hparams.len(), 1);
        assert_eq!(hparams[0].1.key, "learning_rate");

        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[tokio::test]
    async fn test_app_routes_expose_run_summaries_endpoint() {
        let temp_dir = std::env::temp_dir().join(format!("aeroboard-route-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("index.html"), "<!doctype html><title>aeroboard</title>").unwrap();

        let state = AppState {
            config: AppConfig {
                http_port: 3000,
                flight_port: 50051,
                storage_mode: StorageMode::Filesystem,
                data_dir: temp_dir.join("data"),
                greptime_url: "http://localhost:4000".to_string(),
                web_dist_dir: temp_dir.clone(),
            },
            t_junction: Arc::new(TJunction::new(16)),
            storage: Arc::new(FilesystemStore::new(temp_dir.join("data")).unwrap()),
        };

        let response = build_app(state)
            .oneshot(
                Request::builder()
                    .uri("/api/runs/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(content_type.starts_with("application/json"));

        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[tokio::test]
    async fn test_app_routes_expose_image_frame_endpoint() {
        let temp_dir = std::env::temp_dir().join(format!("aeroboard-frame-route-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("index.html"), "<!doctype html><title>aeroboard</title>").unwrap();

        let state = AppState {
            config: AppConfig {
                http_port: 3000,
                flight_port: 50051,
                storage_mode: StorageMode::Filesystem,
                data_dir: temp_dir.join("data"),
                greptime_url: "http://localhost:4000".to_string(),
                web_dist_dir: temp_dir.clone(),
            },
            t_junction: Arc::new(TJunction::new(16)),
            storage: Arc::new(FilesystemStore::new(temp_dir.join("data")).unwrap()),
        };

        let response = build_app(state)
            .oneshot(
                Request::builder()
                    .uri("/api/history/images/frame?run_id=missing&name=samples&ts_ms=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(content_type.starts_with("application/json"));

        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[tokio::test]
    async fn test_filesystem_image_history_omits_payloads_but_frame_endpoint_returns_them() {
        let temp_dir = std::env::temp_dir().join(format!("aeroboard-image-contract-test-{}", uuid::Uuid::new_v4()));
        let store = FilesystemStore::new(temp_dir.clone()).unwrap();

        let batch = RecordBatch::try_new(
            image_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1_700_000_000_000])),
                Arc::new(StringArray::from(vec!["run-image"])),
                Arc::new(StringArray::from(vec!["samples/generated"])),
                Arc::new(BinaryArray::from(vec![b"img".as_slice()])),
                Arc::new(StringArray::from(vec!["png"])),
            ],
        ).unwrap();

        store
            .append_batch(Arc::new(TelemetryBatch::new(TelemetryKind::Images, Arc::new(batch))))
            .await
            .unwrap();

        let page = store
            .query_image_history(&ImageQueryOptions {
                include_data: false,
                ..ImageQueryOptions::default()
            })
            .await
            .unwrap();
        assert_eq!(page.rows.len(), 1);
        assert!(page.rows[0].image_base64.is_none());

        let payload = store
            .load_image_frame(&ImageFrameQuery {
                run_id: "run-image".into(),
                name: "samples/generated".into(),
                ts_ms: 1_700_000_000_000,
            })
            .await
            .unwrap()
            .expect("expected image payload");
        assert_eq!(payload.image_base64, "aW1n");

        std::fs::remove_dir_all(temp_dir).unwrap();
    }
}
