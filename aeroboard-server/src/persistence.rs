use std::sync::Arc;
use crate::t_junction::TJunction;
use arrow::array::{Array, AsArray};
use arrow::record_batch::RecordBatch;

const GREPTIME_WRITE_URL: &str = "http://localhost:4000/v1/influxdb/write?db=public";

pub async fn start_persistence_loop(t_junction: Arc<TJunction>) {
    let mut rx = t_junction.subscribe();

    tokio::spawn(async move {
        let client = reqwest::Client::new();

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

            match client.post(GREPTIME_WRITE_URL)
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

pub fn record_batch_to_line_protocol(batch: &RecordBatch) -> Result<String, Box<dyn std::error::Error>> {
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

    // Check if it's the hyperparameter schema: timestamp, run_id, key, value, value_type (5 columns)
    let is_hparam = schema.fields().len() == 5 && schema.field(2).name() == "key" && schema.field(3).name() == "value";
    
    // Check if it's the distribution schema: timestamp, run_id, name, values (4 columns)
    let _is_dist = schema.fields().len() == 4 && schema.field(3).name() == "values";

    let timestamp_col = batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
    let run_id_col = batch.column(1).as_string::<i32>();
    let text_col = batch.column(2).as_string::<i32>();

    if is_scalar {
        let value_col = batch.column(3).as_primitive::<arrow::datatypes::Float64Type>();
        
        for i in 0..num_rows {
            if timestamp_col.is_null(i) || run_id_col.is_null(i) || text_col.is_null(i) || value_col.is_null(i) {
                continue;
            }
            
            let ts    = timestamp_col.value(i);
            let run_id = run_id_col.value(i);
            let name  = text_col.value(i);
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
        let image_col = batch.column(3).as_binary::<i32>();
        let format_col = batch.column(4).as_string::<i32>();

        for i in 0..num_rows {
            if timestamp_col.is_null(i) || run_id_col.is_null(i) || text_col.is_null(i) || image_col.is_null(i) || format_col.is_null(i) {
                continue;
            }

            let ts = timestamp_col.value(i);
            let run_id = run_id_col.value(i);
            let name = text_col.value(i);
            let format = format_col.value(i);
            let encoded = hex_encode(image_col.value(i));

            let safe_name = escape_tag_value(name);
            let safe_run_id = escape_field_string(run_id);
            let safe_format = escape_field_string(format);
            let safe_image = escape_field_string(&encoded);

            lines.push_str(&format!(
                "images,name={safe_name} run_id=\"{safe_run_id}\",format=\"{safe_format}\",image_hex=\"{safe_image}\" {ts_ns}\n",
                ts_ns = ts * 1_000_000
            ));
        }
    } else if is_hparam {
        let value_col = batch.column(3).as_string::<i32>();
        let value_type_col = batch.column(4).as_string::<i32>();

        for i in 0..num_rows {
            if timestamp_col.is_null(i) || run_id_col.is_null(i) || text_col.is_null(i) || value_col.is_null(i) || value_type_col.is_null(i) {
                continue;
            }

            let ts = timestamp_col.value(i);
            let run_id = run_id_col.value(i);
            let key = text_col.value(i);
            let value = value_col.value(i);
            let value_type = value_type_col.value(i);

            let safe_key = escape_tag_value(key);
            let safe_run_id = escape_field_string(run_id);
            let safe_value = escape_field_string(value);
            let safe_type = escape_field_string(value_type);

            lines.push_str(&format!(
                "hyperparameters,key={safe_key} run_id=\"{safe_run_id}\",value=\"{safe_value}\",value_type=\"{safe_type}\" {ts_ns}\n",
                ts_ns = ts * 1_000_000
            ));
        }
    } else {
        // Assume distribution
        tracing::debug!("Skipping distribution persistence via InfluxDB line protocol; requires JSON API or flattened schema.");
    }

    Ok(lines)
}

fn escape_tag_value(value: &str) -> String {
    value.replace(' ', "\\ ").replace(',', "\\,").replace('=', "\\=")
}

fn escape_field_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}
