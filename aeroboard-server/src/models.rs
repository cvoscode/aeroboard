//! Required control-plane shapes. Extension metadata remains losslessly round-trippable.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub display_name: String,
    pub project_id: String,
    pub status: String,
    pub deleted: bool,
    pub archived: bool,
    pub active_session: Option<String>,
    pub metadata_revision: i64,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub attempt: i64,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub sequence: i64,
    pub status: String,
    pub segment: Option<String>,
    #[serde(default)]
    pub final_sequence: Option<i64>,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub batch_id: String,
    pub row: usize,
    pub kind: String,
    pub run_uid: String,
    pub run_id: String,
    pub name: String,
    pub format: String,
    pub ts_ms: i64,
    pub step: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub label: String,
    pub kind: String,
    pub step: Option<f64>,
    pub timestamp_ms: i64,
    pub run_uid: String,
    pub details: Value,
    pub media_ids: Vec<String>,
}
impl crate::catalog::Entity {
    pub fn typed<T: serde::de::DeserializeOwned>(
        self,
    ) -> crate::catalog::Result<crate::catalog::Entity<T>> {
        Ok(crate::catalog::Entity {
            id: self.id,
            parent: self.parent,
            revision: self.revision,
            data: serde_json::from_value(self.data)
                .map_err(|e| crate::catalog::CatalogError::invalid(e.to_string()))?,
        })
    }
}
