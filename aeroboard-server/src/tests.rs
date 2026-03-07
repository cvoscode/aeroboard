#[cfg(test)]
mod tests {
    use arrow::array::{BinaryArray, Float64Array, StringArray, TimestampMillisecondArray};
    use arrow::record_batch::RecordBatch;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use crate::history::{HyperparameterEntry, HistoryPoint};
    use crate::persistence::record_batch_to_line_protocol;
    use crate::schema::{hyperparameter_schema, image_schema, scalar_schema};
    use crate::storage::{FilesystemStore, StorageEngine};
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
        let scalar_batch = RecordBatch::try_new(
            scalar_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from(vec![1_700_000_000_000])),
                Arc::new(StringArray::from(vec!["run-a"])),
                Arc::new(StringArray::from(vec!["train/loss"])),
                Arc::new(Float64Array::from(vec![0.123])),
            ],
        ).unwrap();

        let scalar_lp = record_batch_to_line_protocol(&scalar_batch).unwrap();
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

        let hparam_lp = record_batch_to_line_protocol(&hparam_batch).unwrap();
        assert!(hparam_lp.contains("hyperparameters,key=learning_rate"));
        assert!(hparam_lp.contains("value=\"0.01\""));
    }

    #[test]
    fn test_image_line_protocol() {
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

        let lp = record_batch_to_line_protocol(&batch).unwrap();
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

        let rows = store.query_image_history().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].run_id, "run-image");
        assert_eq!(rows[0].name, "samples/generated");
        assert_eq!(rows[0].format, "png");
        assert_eq!(rows[0].image_base64, "aW1n");

        std::fs::remove_dir_all(temp_dir).unwrap();
    }
}
