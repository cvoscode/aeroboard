use std::sync::{Arc, OnceLock};

use arrow::record_batch::RecordBatch;
use axum::body::Bytes;
use serde::{Deserialize, Serialize};

pub const ENVELOPE_KEY: &str = "aeroboard.envelope";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchEnvelope {
    pub version: u32,
    pub batch_id: String,
    pub run_uid: String,
    pub session_id: String,
    pub server_id: String,
    pub sequence: i64,
    pub checksum: String,
    pub captured_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<String>,
}

/// Cross-language checksum: names, type tags and little-endian canonical values.
pub fn batch_checksum(batch: &RecordBatch) -> Result<String, String> {
    use arrow::{
        array::{Array, AsArray},
        datatypes::{DataType, Float64Type, TimestampMillisecondType},
    };
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update((batch.num_rows() as u64).to_le_bytes());
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        hash.update((field.name().len() as u32).to_le_bytes());
        hash.update(field.name().as_bytes());
        let tag = match field.data_type() {
            DataType::Utf8 => b's',
            DataType::Binary => b'b',
            DataType::Float64 => b'f',
            DataType::Timestamp(_, _) => b't',
            _ => return Err("unsupported checksum type".into()),
        };
        hash.update([tag]);
        for index in 0..batch.num_rows() {
            if column.is_null(index) {
                hash.update([0]);
                continue;
            }
            hash.update([1]);
            match tag {
                b's' => {
                    let value = column.as_string::<i32>().value(index).as_bytes();
                    hash.update((value.len() as u64).to_le_bytes());
                    hash.update(value);
                }
                b'b' => {
                    let value = column.as_binary::<i32>().value(index);
                    hash.update((value.len() as u64).to_le_bytes());
                    hash.update(value);
                }
                b'f' => {
                    let value = column.as_primitive::<Float64Type>().value(index);
                    hash.update(
                        if value.is_nan() {
                            f64::NAN.to_bits()
                        } else {
                            value.to_bits()
                        }
                        .to_le_bytes(),
                    );
                }
                _ => hash.update(
                    column
                        .as_primitive::<TimestampMillisecondType>()
                        .value(index)
                        .to_le_bytes(),
                ),
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// v2 hashes canonical column buffers in bulk, independent of Arrow slice offsets.
pub fn batch_checksum_v2(batch: &RecordBatch) -> Result<String, String> {
    use arrow::{
        array::{Array, AsArray},
        datatypes::{DataType, Float64Type, TimestampMillisecondType},
    };
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"aeroboard-columnar-v2\0");
    hash.update((batch.num_rows() as u64).to_le_bytes());
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        hash.update((field.name().len() as u32).to_le_bytes());
        hash.update(field.name().as_bytes());
        let tag = match field.data_type() {
            DataType::Utf8 => b's',
            DataType::Binary => b'b',
            DataType::Float64 => b'f',
            DataType::Timestamp(_, _) => b't',
            _ => return Err("unsupported checksum type".into()),
        };
        hash.update([tag]);
        hash.update(
            (0..batch.num_rows())
                .map(|i| u8::from(!column.is_null(i)))
                .collect::<Vec<_>>(),
        );
        let mut bytes = Vec::new();
        if tag == b's' || tag == b'b' {
            let mut offsets = Vec::with_capacity((batch.num_rows() + 1) * 4);
            let mut offset = 0u32;
            offsets.extend_from_slice(&offset.to_le_bytes());
            for i in 0..batch.num_rows() {
                if !column.is_null(i) {
                    let value = if tag == b's' {
                        column.as_string::<i32>().value(i).as_bytes()
                    } else {
                        column.as_binary::<i32>().value(i)
                    };
                    bytes.extend_from_slice(value);
                    offset = offset
                        .checked_add(value.len() as u32)
                        .ok_or("column exceeds checksum offset range")?;
                }
                offsets.extend_from_slice(&offset.to_le_bytes());
            }
            hash.update(offsets);
        } else {
            bytes.reserve(batch.num_rows() * 8);
            for i in 0..batch.num_rows() {
                let bits = if column.is_null(i) {
                    0
                } else if tag == b'f' {
                    let v = column.as_primitive::<Float64Type>().value(i);
                    if v.is_nan() {
                        0x7ff8000000000000
                    } else {
                        v.to_bits()
                    }
                } else {
                    column.as_primitive::<TimestampMillisecondType>().value(i) as u64
                };
                bytes.extend_from_slice(&bits.to_le_bytes());
            }
        }
        hash.update(bytes);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetryKind {
    Scalars,
    Images,
    Audio,
    Video,
    Hyperparameters,
    Distributions,
    Unknown,
}

impl TelemetryKind {
    pub fn from_descriptor_path(path: &[String]) -> Self {
        let Some(first) = path.first() else {
            return Self::Unknown;
        };

        match first.to_ascii_lowercase().as_str() {
            "scalars" => Self::Scalars,
            "images" => Self::Images,
            "audio" => Self::Audio,
            "video" => Self::Video,
            "hyperparameters" => Self::Hyperparameters,
            "distributions" => Self::Distributions,
            _ => Self::Unknown,
        }
    }

    /// Infer the kind from column names when the client sent no descriptor.
    /// The result is only a guess; `schema::normalize_batch` does the real validation.
    pub fn from_batch(batch: &RecordBatch) -> Self {
        let schema = batch.schema();
        let has = |name: &str| schema.index_of(name).is_ok();

        if has("image_data") {
            Self::Images
        } else if has("audio_data") {
            Self::Audio
        } else if has("video_data") {
            Self::Video
        } else if has("key") && has("value_type") {
            Self::Hyperparameters
        } else if has("values") {
            Self::Distributions
        } else if has("name") && has("value") {
            Self::Scalars
        } else {
            Self::Unknown
        }
    }

    pub fn as_dir_name(&self) -> Option<&'static str> {
        match self {
            Self::Scalars => Some("scalars"),
            Self::Images => Some("images"),
            Self::Audio => Some("audio"),
            Self::Video => Some("video"),
            Self::Hyperparameters => Some("hyperparameters"),
            Self::Distributions | Self::Unknown => None,
        }
    }

    /// Name of the binary payload column for media kinds.
    pub fn media_data_column(&self) -> Option<&'static str> {
        match self {
            Self::Images => Some("image_data"),
            Self::Audio => Some("audio_data"),
            Self::Video => Some("video_data"),
            _ => None,
        }
    }
}

/// A validated batch in the canonical layout for its kind (see `schema::normalize_batch`).
#[derive(Debug)]
pub struct TelemetryBatch {
    pub envelope: Option<BatchEnvelope>,
    pub kind: TelemetryKind,
    pub batch: Arc<RecordBatch>,
    /// Arrow IPC encoding pushed to WebSocket subscribers, computed once per batch
    /// instead of once per connected browser. Index 0: payloads kept, 1: AV payloads stripped.
    live_ipc: [OnceLock<Bytes>; 2],
    run_ids: OnceLock<std::collections::BTreeSet<String>>,
}

impl TelemetryBatch {
    pub fn new(kind: TelemetryKind, batch: Arc<RecordBatch>) -> Self {
        let envelope = batch
            .schema()
            .metadata()
            .get(ENVELOPE_KEY)
            .and_then(|raw| serde_json::from_str(raw).ok());
        Self {
            kind,
            batch,
            envelope,
            live_ipc: [OnceLock::new(), OnceLock::new()],
            run_ids: OnceLock::new(),
        }
    }

    /// Resolve identities once per batch, before taking shared registry locks.
    pub fn run_ids(&self) -> &std::collections::BTreeSet<String> {
        use arrow::array::{Array, AsArray};
        self.run_ids.get_or_init(|| {
            let ids = self.batch.column(1).as_string::<i32>();
            if self.envelope.is_some() && !ids.is_empty() {
                return std::collections::BTreeSet::from([ids.value(0).to_owned()]);
            }
            ids.iter()
                .flatten()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(str::to_owned)
                .collect()
        })
    }

    pub(crate) fn cached_live_ipc(
        &self,
        strip_media_payloads: bool,
        encode: impl FnOnce() -> Result<Bytes, arrow::error::ArrowError>,
    ) -> Result<Bytes, arrow::error::ArrowError> {
        let slot = &self.live_ipc[usize::from(strip_media_payloads)];
        if let Some(bytes) = slot.get() {
            return Ok(bytes.clone());
        }
        let bytes = encode()?;
        Ok(slot.get_or_init(|| bytes).clone())
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use arrow::{
        array::{ArrayRef, BinaryArray, Float64Array, StringArray, TimestampMillisecondArray},
        datatypes::{DataType, Field, Schema, TimeUnit},
    };
    #[test]
    fn shared_checksum_vectors_and_slices() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../protocol/checksum-vectors.json")).unwrap();
        for vector in vectors.as_array().unwrap() {
            let mut fields = vec![];
            let mut arrays: Vec<ArrayRef> = vec![];
            for column in vector["columns"].as_array().unwrap() {
                let values = column["values"].as_array().unwrap();
                let (kind, array): (DataType, ArrayRef) = match column["type"].as_str().unwrap() {
                    "s" => (
                        DataType::Utf8,
                        Arc::new(StringArray::from_iter(
                            std::iter::once(None).chain(values.iter().map(|v| v.as_str())),
                        )),
                    ),
                    "b" => {
                        let bytes: Vec<_> = values
                            .iter()
                            .map(|v| {
                                v.as_str()
                                    .map(|s| crate::storage::codec::decode_hex(s).unwrap())
                            })
                            .collect();
                        (
                            DataType::Binary,
                            Arc::new(BinaryArray::from_iter(
                                std::iter::once(None).chain(bytes.iter().map(|v| v.as_deref())),
                            )),
                        )
                    }
                    "t" => (
                        DataType::Timestamp(TimeUnit::Millisecond, None),
                        Arc::new(TimestampMillisecondArray::from_iter(
                            std::iter::once(None).chain(values.iter().map(|v| v.as_i64())),
                        )),
                    ),
                    _ => (
                        DataType::Float64,
                        Arc::new(Float64Array::from_iter(std::iter::once(None).chain(
                            values.iter().map(|v| {
                                if let Some(s) = v.as_str() {
                                    Some(s.parse::<f64>().unwrap())
                                } else {
                                    v.as_f64()
                                }
                            }),
                        ))),
                    ),
                };
                fields.push(Field::new(column["name"].as_str().unwrap(), kind, true));
                arrays.push(array.slice(1, values.len()));
            }
            let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap();
            assert_eq!(
                batch_checksum(&batch).unwrap(),
                vector["v1"].as_str().unwrap()
            );
            assert_eq!(
                batch_checksum_v2(&batch).unwrap(),
                vector["v2"].as_str().unwrap()
            );
        }
    }
}
