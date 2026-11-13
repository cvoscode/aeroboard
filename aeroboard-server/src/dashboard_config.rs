//! Server-wide dashboard defaults (starred charts, target lines, chart order).
//!
//! Read from the `[dashboard]` table of a TOML file:
//!
//! ```toml
//! [dashboard]
//! starred = ["loss", "val/accuracy"]   # charts expanded and pinned to the top
//! order   = ["loss", "accuracy", "lr"] # initial chart order (rest follow alphabetically)
//!
//! [dashboard.targets]                  # dashed "optimal" reference lines
//! "val/accuracy" = 0.95
//! loss = 0.05
//! ```
//!
//! Metric names may be either the full name (`train/loss`) or the chart name
//! (`loss`); the dashboard matches both. Per-run settings sent by the training
//! script override these, and anything the user changes in the UI wins over both.
//!
//! The file is re-read on every request so edits apply on the next page load.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardDefaults {
    pub starred: Vec<String>,
    pub order: Vec<String>,
    pub targets: BTreeMap<String, f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
    dashboard: DashboardDefaults,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfigResponse {
    pub dashboard: DashboardDefaults,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub const DEFAULT_CONFIG_FILE: &str = "aeroboard.toml";

/// Resolve which file to read: an explicit path must exist; the default is optional.
pub fn resolve_path(explicit: Option<&Path>) -> Option<PathBuf> {
    match explicit {
        Some(path) => Some(path.to_path_buf()),
        None => {
            let default = PathBuf::from(DEFAULT_CONFIG_FILE);
            default.is_file().then_some(default)
        }
    }
}

pub fn parse(contents: &str) -> Result<DashboardDefaults, String> {
    let file: ConfigFile = toml::from_str(contents).map_err(|error| error.to_string())?;
    let mut dashboard = file.dashboard;
    if let Some((name, _)) = dashboard
        .targets
        .iter()
        .find(|(_, value)| !value.is_finite())
    {
        return Err(format!("dashboard.targets.{name} must be a finite number"));
    }
    dashboard.starred.retain(|name| !name.trim().is_empty());
    dashboard.order.retain(|name| !name.trim().is_empty());
    Ok(dashboard)
}

pub fn load(path: Option<&Path>) -> Result<DashboardDefaults, String> {
    let Some(path) = resolve_path(path) else {
        return Ok(DashboardDefaults::default());
    };
    let contents = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    parse(&contents).map_err(|error| format!("invalid {}: {error}", path.display()))
}

/// GET /api/config
pub async fn config_handler(State(state): State<AppState>) -> Json<ConfigResponse> {
    let path = state.config.dashboard_config_path.clone();
    let result = tokio::task::spawn_blocking(move || {
        type Cache = std::collections::HashMap<
            Option<PathBuf>,
            (
                Option<(std::time::SystemTime, u64)>,
                Result<DashboardDefaults, String>,
            ),
        >;
        static CACHE: std::sync::OnceLock<std::sync::Mutex<Cache>> = std::sync::OnceLock::new();
        let resolved = resolve_path(path.as_deref());
        let stamp = resolved
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((cached_stamp, result)) = cache.get(&resolved)
            && cached_stamp == &stamp
        {
            return result.clone();
        }
        let result = load(path.as_deref());
        cache.insert(resolved, (stamp, result.clone()));
        result
    })
    .await
    .unwrap_or_else(|error| Err(error.to_string()));

    match result {
        Ok(dashboard) => Json(ConfigResponse {
            dashboard,
            error: None,
        }),
        Err(error) => {
            tracing::warn!("Dashboard config: {error}");
            Json(ConfigResponse {
                dashboard: DashboardDefaults::default(),
                error: Some(error),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dashboard_table() {
        let parsed = parse(
            r#"
            [dashboard]
            starred = ["loss", " "]
            order = ["loss", "accuracy"]
            [dashboard.targets]
            "val/accuracy" = 0.95
            loss = 5
            "#,
        )
        .unwrap();
        assert_eq!(parsed.starred, vec!["loss"]);
        assert_eq!(parsed.order, vec!["loss", "accuracy"]);
        assert_eq!(parsed.targets.get("val/accuracy"), Some(&0.95));
        assert_eq!(parsed.targets.get("loss"), Some(&5.0));
    }

    #[test]
    fn missing_table_yields_defaults() {
        assert_eq!(parse("").unwrap(), DashboardDefaults::default());
    }

    #[test]
    fn rejects_unknown_keys_and_bad_values() {
        assert!(
            parse("[dashboard]\nstared = [\"loss\"]")
                .unwrap_err()
                .contains("unknown field")
        );
        assert!(parse("[dashboard.targets]\nloss = \"low\"").is_err());
        assert!(
            parse("[dashboard.targets]\nloss = nan")
                .unwrap_err()
                .contains("finite")
        );
    }
}
