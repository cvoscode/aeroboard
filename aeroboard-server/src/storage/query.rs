use std::{collections::BTreeSet, path::PathBuf};

use serde::Deserialize;

use crate::{
    history::{HistoryPoint, ImageHistoryPoint},
    telemetry::TelemetryKind,
};

/// Binary media kinds that share the "encoded blob + format" storage layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Audio,
    Video,
}

impl MediaKind {
    pub fn from_telemetry(kind: &TelemetryKind) -> Option<Self> {
        match kind {
            TelemetryKind::Images => Some(Self::Image),
            TelemetryKind::Audio => Some(Self::Audio),
            TelemetryKind::Video => Some(Self::Video),
            _ => None,
        }
    }

    /// Filesystem directory and GreptimeDB table name.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Image => "images",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    /// GreptimeDB field holding the hex-encoded payload.
    pub fn hex_column(self) -> &'static str {
        match self {
            Self::Image => "image_hex",
            Self::Audio => "audio_hex",
            Self::Video => "video_hex",
        }
    }

    /// Arrow column holding the binary payload.
    pub fn data_column(self) -> &'static str {
        match self {
            Self::Image => "image_data",
            Self::Audio => "audio_data",
            Self::Video => "video_data",
        }
    }
}

/// Where the bytes of a stored media item live. Filesystem-backed items are
/// streamed straight from disk (with Range support) instead of being buffered.
#[derive(Clone, Debug)]
pub enum MediaSource {
    File(PathBuf),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct MediaPayload {
    pub format: String,
    pub source: MediaSource,
}

#[derive(Clone, Debug, Default)]
pub struct HistoryQueryOptions {
    pub run_ids: BTreeSet<String>,
    pub metrics: BTreeSet<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct ImageQueryOptions {
    pub run_ids: BTreeSet<String>,
    pub metrics: BTreeSet<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
    pub include_data: bool,
}

#[derive(Clone, Debug)]
pub struct ImageFrameQuery {
    pub run_id: String,
    pub name: String,
    pub ts_ms: i64,
}

#[derive(Clone, Debug)]
pub struct QueryPage<T> {
    pub rows: Vec<T>,
    pub next_cursor: Option<usize>,
}

impl<T> Default for QueryPage<T> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            next_cursor: None,
        }
    }
}

fn paginate_vec<T>(rows: Vec<T>, cursor: Option<usize>, limit: Option<usize>) -> QueryPage<T> {
    let start = cursor.unwrap_or(0);
    let page_limit = limit.unwrap_or(rows.len().max(1));
    let total = rows.len();
    let mut iter = rows.into_iter().skip(start);
    let page_rows: Vec<T> = iter.by_ref().take(page_limit).collect();
    let consumed = start + page_rows.len();
    let next_cursor = if consumed < total { Some(consumed) } else { None };
    QueryPage { rows: page_rows, next_cursor }
}

pub(crate) fn apply_history_query(rows: Vec<HistoryPoint>, query: &HistoryQueryOptions) -> QueryPage<HistoryPoint> {
    let mut filtered: Vec<HistoryPoint> = rows
        .into_iter()
        .filter(|row| {
            (query.run_ids.is_empty() || query.run_ids.contains(&row.run_id))
                && (query.metrics.is_empty() || query.metrics.contains(&row.name))
                && query.start_ms.is_none_or(|start| row.ts_ms >= start)
                && query.end_ms.is_none_or(|end| row.ts_ms <= end)
        })
        .collect();
    filtered.sort_by_key(|row| row.ts_ms);
    paginate_vec(filtered, query.cursor, query.limit)
}

pub(crate) fn apply_image_query(rows: Vec<ImageHistoryPoint>, query: &ImageQueryOptions) -> QueryPage<ImageHistoryPoint> {
    let mut filtered: Vec<ImageHistoryPoint> = rows
        .into_iter()
        .filter(|row| {
            (query.run_ids.is_empty() || query.run_ids.contains(&row.run_id))
                && (query.metrics.is_empty() || query.metrics.contains(&row.name))
                && query.start_ms.is_none_or(|start| row.ts_ms >= start)
                && query.end_ms.is_none_or(|end| row.ts_ms <= end)
        })
        .collect();
    filtered.sort_by_key(|row| row.ts_ms);
    paginate_vec(filtered, query.cursor, query.limit)
}

pub(crate) fn finalize_page<T>(mut rows: Vec<T>, cursor: Option<usize>, limit: Option<usize>) -> QueryPage<T> {
    if let Some(limit) = limit {
        let next_cursor = if rows.len() > limit {
            rows.truncate(limit);
            Some(cursor.unwrap_or(0) + limit)
        } else {
            None
        };
        QueryPage { rows, next_cursor }
    } else {
        QueryPage { rows, next_cursor: None }
    }
}
