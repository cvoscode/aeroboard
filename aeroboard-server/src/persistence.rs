use std::sync::Arc;
use crate::t_junction::TJunction;
use arrow::array::{Array, AsArray};
use arrow::record_batch::RecordBatch;

pub async fn start_persistence_loop(t_junction: Arc<TJunction>) {
    let mut rx = t_junction.subscribe();

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        // GreptimeDB respects the InfluxDB line protocol on this endpoint
        let db_url = "http://localhost:4000/v1/influxdb/write?db=public"; 

        while let Ok(batch) = rx.recv().await {
            let line_protocol_data = match record_batch_to_line_protocol(&batch) {
                Ok(data) => data,
                Err(e) => {
                    tracing::error!("Failed to convert batch to line protocol: {}", e);
                    continue;
                }
            };

            if line_protocol_data.is_empty() {
                continue;
            }

            match client.post(db_url)
                .body(line_protocol_data)
                .send()
                .await 
            {
                Ok(response) => {
                    if !response.status().is_success() {
                        let status = response.status();
                        let text = response.text().await.unwrap_or_default();
                        tracing::error!("Failed to insert to GreptimeDB: {} - {}", status, text);
                    } else {
                        tracing::debug!("Successfully inserted batch of {} rows", batch.num_rows());
                    }
                }
                Err(e) => {
                    tracing::error!("HTTP error sending to GreptimeDB: {}", e);
                }
            }
        }
    });
}

fn record_batch_to_line_protocol(batch: &RecordBatch) -> Result<String, Box<dyn std::error::Error>> {
    let mut lines = String::new();
    let num_rows = batch.num_rows();

    if num_rows == 0 {
        return Ok(lines);
    }

    // Determine which schema we are dealing with based on column names/types
    let schema = batch.schema();
    
    // Check if it's the scalar schema: timestamp, run_id, name, value (4 columns)
    let is_scalar = schema.fields().len() == 4 && schema.field(3).name() == "value";
    
    // Check if it's the image schema: timestamp, run_id, name, image_data, format (5 columns)
    let is_image = schema.fields().len() == 5 && schema.field(3).name() == "image_data";
    
    // Check if it's the distribution schema: timestamp, run_id, name, values (4 columns)
    let _is_dist = schema.fields().len() == 4 && schema.field(3).name() == "values";

    let timestamp_col = batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
    let run_id_col = batch.column(1).as_string::<i32>();
    let name_col = batch.column(2).as_string::<i32>();

    if is_scalar {
        let value_col = batch.column(3).as_primitive::<arrow::datatypes::Float64Type>();
        
        for i in 0..num_rows {
            if timestamp_col.is_null(i) || run_id_col.is_null(i) || name_col.is_null(i) || value_col.is_null(i) {
                continue;
            }
            
            let ts    = timestamp_col.value(i);
            let run_id = run_id_col.value(i);
            let name  = name_col.value(i);
            let val   = value_col.value(i);
            
            // InfluxDB Line Protocol:
            //   measurement[,tag=val...] field_key=field_val[,...] timestamp_ns
            //
            // Use "scalars" as measurement, name as a tag (time index key in GreptimeDB),
            // run_id and value as fields so both are queryable.
            // Escape spaces in run_id/name (InfluxDB spec).
            let safe_name   = name.replace(' ', "\\ ").replace(',', "\\,");
            let safe_run_id = run_id.replace(' ', "\\ ").replace(',', "\\,").replace('"', "\\\"");
            
            lines.push_str(&format!(
                "scalars,name={safe_name} run_id=\"{safe_run_id}\",value={val} {ts_ns}\n",
                safe_name = safe_name,
                safe_run_id = safe_run_id,
                val = val,
                ts_ns = ts * 1_000_000
            ));
        }
    } else if is_image {
        // Image logging: we just record metadata, binary data is too big to index easily in tags
        // GreptimeDB can store binary in a string/binary column, but for line protocol we hex-encode
        // or base64 encode. Alternatively we skip persistence of images in the TSDB and just keep them in path B,
        // but GreptimeDB supports string types. To avoid OOM during line protocol, we skip image persistence via HTTP for now,
        // or just store metadata.
        tracing::debug!("Skipping image persistence in TSDB for now to save space, rely on live stream.");
    } else {
        // Assume distribution
        tracing::debug!("Skipping distribution persistence via InfluxDB line protocol; requires JSON API or flattened schema.");
    }

    Ok(lines)
}
