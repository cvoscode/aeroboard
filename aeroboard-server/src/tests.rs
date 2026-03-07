#[cfg(test)]
mod tests {
    use arrow::array::{Float64Array, StringArray, TimestampMillisecondArray};
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_mock_insert() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
            Field::new("name", DataType::Utf8, false),
            Field::new("value", DataType::Float64, false),
        ]));

        let timestamps = TimestampMillisecondArray::from(vec![1678123456000]);
        let names = StringArray::from(vec!["loss"]);
        let values = Float64Array::from(vec![0.42]);

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(timestamps), Arc::new(names), Arc::new(values)],
        )
        .unwrap();

        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 3);
        
        // This validates we can craft batches locally matching the schema design
        println!("Successfully crafted Arrow Batch: {:?}", batch);
    }
}
