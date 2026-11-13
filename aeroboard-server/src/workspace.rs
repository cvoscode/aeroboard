//! HTTP control-plane operations. All mutations run in catalog transactions.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    catalog::{self, CatalogError, Entity, RegisterRun, Result},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/capabilities", get(capabilities))
        .route("/run-records", get(runs))
        .route(
            "/run-records/{id}",
            get(run).patch(patch_run).delete(delete_run),
        )
        .route("/runs", post(register))
        .route("/runs/{id}/sessions", post(start_session))
        .route("/runs/{id}/sessions/{session}/heartbeat", post(heartbeat))
        .route("/runs/{id}/sessions/{session}/finish", post(finish))
        .route("/projects", get(projects).post(create_project))
        .route(
            "/projects/{id}",
            get(project).patch(patch_project).delete(delete_project),
        )
        .route("/projects/{parent}/views", get(views).post(save_view))
        .route(
            "/views/{id}",
            get(view).patch(patch_view).delete(delete_view),
        )
        .route("/metric-definitions", get(definitions).post(define_metric))
        .route("/events", get(events).post(save_event))
        .route(
            "/events/{id}",
            axum::routing::patch(patch_event).delete(delete_event),
        )
        .route("/assets/{id}", get(asset))
        .route("/assets/{id}/content", get(asset_content))
}

async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    Json(
        json!({"protocol_version":2,"catalog_version":2,"server_id":state.catalog.store_id,
        "persistent":state.catalog.persistent,"storage":state.config.storage_mode.as_str(),
        "features":["projects","lifecycle","resume","metric_definitions","views","events","batch_envelopes","batch_streams","assets","scorecards","aggregation","run_deletion"],
        "durable_replay":state.catalog.persistent,"max_message_bytes":state.config.max_message_bytes,"stale_after_ms":state.catalog.stale_after_ms}),
    )
}

#[derive(Default, Deserialize)]
pub struct ListQuery {
    pub project: Option<String>,
    pub parent: Option<String>,
    pub run_id: Option<String>,
    pub status: Option<String>,
    pub tag: Option<String>,
    pub search: Option<String>,
    pub archived: Option<bool>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub filters: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunFilter {
    field: String,
    op: String,
    value: Value,
}
fn filter_matches(actual: &Value, op: &str, expected: &Value) -> bool {
    if actual.is_null() {
        return op == "missing";
    }
    match op {
        "eq" => actual.as_f64().zip(expected.as_f64()).map_or_else(
            || actual == expected,
            |(a, b)| a.is_finite() && b.is_finite() && a == b,
        ),
        "ne" => actual.as_f64().zip(expected.as_f64()).map_or_else(
            || actual != expected,
            |(a, b)| a.is_finite() && b.is_finite() && a != b,
        ),
        "contains" => {
            actual
                .as_str()
                .zip(expected.as_str())
                .is_some_and(|(a, b)| a.contains(b))
                || actual.as_array().is_some_and(|a| a.contains(expected))
        }
        "lt" | "lte" | "gt" | "gte" => {
            actual
                .as_f64()
                .zip(expected.as_f64())
                .is_some_and(|(a, b)| {
                    a.is_finite()
                        && b.is_finite()
                        && match op {
                            "lt" => a < b,
                            "lte" => a <= b,
                            "gt" => a > b,
                            _ => a >= b,
                        }
                })
        }
        "exists" => true,
        _ => false,
    }
}

async fn runs(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>> {
    let filters: Vec<RunFilter> = query
        .filters
        .as_ref()
        .map(|s| serde_json::from_str(s))
        .transpose()
        .map_err(|e| CatalogError::invalid(format!("invalid filters: {e}")))?
        .unwrap_or_default();
    if filters.len() > 20
        || filters.iter().any(|f| {
            !matches!(
                f.op.as_str(),
                "eq" | "ne" | "lt" | "lte" | "gt" | "gte" | "contains" | "exists" | "missing"
            ) || !(f.field.starts_with("params.")
                || f.field.starts_with("metrics.")
                || f.field.starts_with("provenance.")
                || ["tags", "notes", "display_name"].contains(&f.field.as_str()))
        })
    {
        return Err(CatalogError::invalid(
            "invalid filter field/operator (maximum 20)",
        ));
    }
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let stale_after_ms = state.catalog.stale_after_ms;
    let mut rows = state
        .catalog
        .read(move |db| {
            let search = query.search.as_ref().map(|s| s.to_lowercase());
            let mut rows: Vec<Entity> = catalog::list(db, "run", query.project.as_deref())?
                .into_iter()
                .filter(|e| e.data["deleted"] != true)
                .filter(|e| query.archived.unwrap_or(false) || e.data["archived"] != true)
                .map(|r| catalog::with_delivery(db, r, stale_after_ms))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|e| {
                    query
                        .status
                        .as_ref()
                        .is_none_or(|s| e.data["display_status"] == *s)
                })
                .filter(|e| {
                    query.tag.as_ref().is_none_or(|t| {
                        e.data["tags"]
                            .as_array()
                            .is_some_and(|tags| tags.contains(&json!(t)))
                    })
                })
                .filter(|e| {
                    search
                        .as_ref()
                        .is_none_or(|s| e.data.to_string().to_lowercase().contains(s))
                })
                .collect();
            rows.sort_by(|a, b| {
                b.data["created_at_ms"]
                    .as_i64()
                    .cmp(&a.data["created_at_ms"].as_i64())
                    .then(a.id.cmp(&b.id))
            });
            Ok(rows)
        })
        .await?;
    if !filters.is_empty() {
        let ids = rows
            .iter()
            .filter_map(|r| r.data["run_id"].as_str().map(str::to_owned))
            .collect();
        let summaries = if filters
            .iter()
            .any(|f| f.field.starts_with("params.") || f.field.starts_with("metrics."))
        {
            state
                .summaries
                .summaries(state.storage.as_ref(), &ids)
                .await
                .map_err(|e| {
                    CatalogError::http(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                })?
        } else {
            vec![]
        };
        let definitions = state
            .catalog
            .read(|db| catalog::list(db, "metric", None))
            .await?;
        rows.retain(|run| {
            filters.iter().all(|filter| {
                let summary = summaries.iter().find(|s| run.data["run_id"] == s.run_id);
                let value = if let Some(key) = filter.field.strip_prefix("params.") {
                    summary
                        .and_then(|s| s.hyperparameters.iter().find(|h| h.key == key))
                        .map(|h| {
                            if matches!(h.value_type.as_str(), "int" | "float" | "bool" | "json") {
                                serde_json::from_str(&h.value).unwrap_or_else(|_| json!(h.value))
                            } else {
                                json!(h.value)
                            }
                        })
                        .unwrap_or(Value::Null)
                } else if let Some(key) = filter.field.strip_prefix("metrics.") {
                    key.rsplit_once('.')
                        .and_then(|(name, reducer)| {
                            summary
                                .and_then(|s| s.metric_summaries.iter().find(|m| m.name == name))
                                .map(|m| {
                                    let definition =
                                        crate::analytics::definition_for(run, name, &definitions);
                                    match reducer {
                                        "last" => json!(m.last),
                                        "min" => json!(m.min),
                                        "max" => json!(m.max),
                                        "best" => match definition["goal"].as_str() {
                                            Some("minimize") => json!(m.min),
                                            Some("maximize") => json!(m.max),
                                            _ => Value::Null,
                                        },
                                        _ => Value::Null,
                                    }
                                })
                        })
                        .unwrap_or(Value::Null)
                } else if let Some(key) = filter.field.strip_prefix("provenance.") {
                    run.data["provenance"][key].clone()
                } else {
                    run.data[&filter.field].clone()
                };
                filter_matches(&value, &filter.op, &filter.value)
            })
        });
    }
    let total = rows.len();
    Ok(Json(
        json!({"rows":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"total":total,"next_offset":(offset+limit<total).then_some(offset+limit)}),
    ))
}
async fn run(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Entity>> {
    let stale_after_ms = state.catalog.stale_after_ms;
    state
        .catalog
        .read(move |db| {
            Ok(Json(catalog::with_delivery(
                db,
                catalog::run_by_id(db, &id)?,
                stale_after_ms,
            )?))
        })
        .await
}
async fn register(
    State(state): State<AppState>,
    Json(input): Json<RegisterRun>,
) -> Result<Json<Entity>> {
    let known = crate::history::known_runs(&state).await;
    state
        .catalog
        .write(move |db| Ok(Json(catalog::register(db, input, known)?)))
        .await
}
async fn start_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    state
        .catalog
        .write(move |db| Ok(Json(catalog::start_session(db, &id, &input)?)))
        .await
}
async fn heartbeat(
    State(state): State<AppState>,
    Path((id, sid)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    state
        .catalog
        .quiet(move |db| Ok(Json(catalog::update_session(db, &id, &sid, &input, false)?)))
        .await
}
async fn finish(
    State(state): State<AppState>,
    Path((id, sid)): Path<(String, String)>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    state
        .catalog
        .write(move |db| Ok(Json(catalog::update_session(db, &id, &sid, &input, true)?)))
        .await
}

fn expected(input: &Value) -> Result<i64> {
    input["revision"]
        .as_i64()
        .filter(|r| *r >= 0)
        .ok_or_else(|| CatalogError::invalid("expected revision is required"))
}
fn validate_labels(input: &Value) -> Result<()> {
    if let Some(tags) = input.get("tags")
        && !tags.as_array().is_some_and(|a| {
            a.len() <= 50 && a.iter().all(|v| v.as_str().is_some_and(|s| s.len() <= 128))
        })
    {
        return Err(CatalogError::invalid(
            "tags must contain at most 50 strings of 128 bytes",
        ));
    }
    if input
        .get("notes")
        .is_some_and(|v| !v.as_str().is_some_and(|s| s.len() <= 65536))
    {
        return Err(CatalogError::invalid("notes must be a string (max 64 KiB)"));
    }
    Ok(())
}
async fn patch_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    validate_labels(&input)?;
    let revision = expected(&input)?;
    state
        .catalog
        .write(move |db| {
            let mut record = catalog::run_by_id(db, &id)?;
            let metadata_revision = record.data["metadata_revision"].as_i64().unwrap_or(1);
            if revision != metadata_revision {
                return Err(CatalogError::conflict(
                    "metadata changed; reload before saving",
                ));
            }
            for field in ["display_name", "notes", "tags", "archived"] {
                if let Some(value) = input.get(field) {
                    record.data[field] = value.clone();
                }
            }
            if !record.data["display_name"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 256)
            {
                return Err(CatalogError::invalid("invalid display name"));
            }
            if !record.data["archived"].is_boolean() {
                return Err(CatalogError::invalid("archived must be boolean"));
            }
            record.data["metadata_revision"] = json!(metadata_revision + 1);
            Ok(Json(catalog::save(
                db,
                "run",
                &record.id,
                &record.parent,
                &record.data,
                Some(record.revision),
            )?))
        })
        .await
}
async fn delete_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RevisionQuery>,
) -> Result<Json<Value>> {
    if !state.catalog.persistent {
        return Err(CatalogError::conflict(
            "live-only mode has no persisted run data to delete",
        ));
    }
    let (canonical, uid) = state
        .catalog
        .write(move |db| {
            let mut record = catalog::get(db, "run", &id)?.ok_or_else(CatalogError::missing)?;
            let revision = record.data["metadata_revision"].as_i64().unwrap_or(1);
            if record.data["deleted"] != true && query.revision != revision {
                return Err(CatalogError::conflict(
                    "metadata changed; reload before deleting",
                ));
            }
            record.data["deleted"] = json!(true);
            record.data["deletion_pending"] = json!(true);
            catalog::save(
                db,
                "run",
                &id,
                &record.parent,
                &record.data,
                Some(record.revision),
            )?;
            Ok((record.data["run_id"].as_str().unwrap_or("").to_owned(), id))
        })
        .await?;
    state.storage.delete_run(&canonical).await.map_err(|e| {
        CatalogError::http(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("deletion pending; retry this run ID: {e}"),
        )
    })?;
    state.summaries.remove(&canonical).await;
    state.analysis.remove(&canonical).await;
    state.t_junction.remove_run(&canonical);
    state
        .catalog
        .write(move |db| {
            let mut record = catalog::get(db, "run", &uid)?.ok_or_else(CatalogError::missing)?;
            record.data["deletion_pending"] = json!(false);
            catalog::save(
                db,
                "run",
                &uid,
                &record.parent,
                &record.data,
                Some(record.revision),
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({"deleted":true})))
}

async fn projects(State(state): State<AppState>) -> Result<Json<Vec<Entity>>> {
    state
        .catalog
        .read(|db| Ok(Json(catalog::list(db, "project", None)?)))
        .await
}
async fn project(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Entity>> {
    state
        .catalog
        .read(move |db| {
            Ok(Json(
                catalog::get(db, "project", &id)?.ok_or_else(CatalogError::missing)?,
            ))
        })
        .await
}
async fn create_project(
    State(state): State<AppState>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    let name = catalog::required_text(&input, "name")?.to_owned();
    let id = input["id"].as_str().unwrap_or(&name).to_owned();
    if id.is_empty()
        || id.len() > 128
        || !id.chars().all(|c| c.is_alphanumeric() || "-_".contains(c))
    {
        return Err(CatalogError::invalid(
            "project ID must contain letters, numbers, - or _",
        ));
    }
    state
        .catalog
        .write(move |db| {
            Ok(Json(catalog::save(
                db,
                "project",
                &id,
                "",
                &json!({"name":name,"default_view":null}),
                Some(0),
            )?))
        })
        .await
}
async fn patch_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    let revision = expected(&input)?;
    state
        .catalog
        .write(move |db| {
            let mut record = catalog::get(db, "project", &id)?.ok_or_else(CatalogError::missing)?;
            if let Some(name) = input.get("name") {
                catalog::required_text(&input, "name")?;
                record.data["name"] = name.clone();
            }
            if let Some(view) = input.get("default_view") {
                if let Some(view_id) = view.as_str() {
                    let selected =
                        catalog::get(db, "view", view_id)?.ok_or_else(CatalogError::missing)?;
                    if selected.parent != id {
                        return Err(CatalogError::invalid("view belongs to another project"));
                    }
                } else if !view.is_null() {
                    return Err(CatalogError::invalid(
                        "default_view must be a view ID or null",
                    ));
                }
                record.data["default_view"] = view.clone();
            }
            Ok(Json(catalog::save(
                db,
                "project",
                &id,
                "",
                &record.data,
                Some(revision),
            )?))
        })
        .await
}
#[derive(Deserialize)]
struct RevisionQuery {
    revision: i64,
}
async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RevisionQuery>,
) -> Result<Json<Value>> {
    state
        .catalog
        .write(move |db| {
            if id == "default"
                || !catalog::list(db, "run", Some(&id))?.is_empty()
                || !catalog::list(db, "view", Some(&id))?.is_empty()
            {
                return Err(CatalogError::conflict(
                    "project must be empty and cannot be the default project",
                ));
            }
            catalog::remove(db, "project", &id, query.revision)?;
            Ok(Json(json!({"deleted":true})))
        })
        .await
}
async fn views(
    State(state): State<AppState>,
    Path(parent): Path<String>,
) -> Result<Json<Vec<Entity>>> {
    state
        .catalog
        .read(move |db| Ok(Json(catalog::list(db, "view", Some(&parent))?)))
        .await
}
fn validate_view(input: &Value) -> Result<()> {
    catalog::required_text(input, "name")?;
    if !input["settings"].is_object() || input["settings"].to_string().len() > 262144 {
        return Err(CatalogError::invalid(
            "view settings must be an object up to 256 KiB",
        ));
    }
    let settings = &input["settings"];
    let strings = |value: &Value| {
        value.as_array().is_some_and(|a| {
            a.len() <= 5000
                && a.iter()
                    .all(|s| s.as_str().is_some_and(|s| s.len() <= 4096))
        })
    };
    for field in ["selectedRuns", "removedTargets"] {
        if settings.get(field).is_some_and(|v| !strings(v)) {
            return Err(CatalogError::invalid(format!(
                "{field} must be a list of strings"
            )));
        }
    }
    if let Some(dashboard) = settings.get("dashboard") {
        if !dashboard.is_object() {
            return Err(CatalogError::invalid("dashboard must be an object"));
        }
        for field in ["starred", "order"] {
            if dashboard.get(field).is_some_and(|v| !strings(v)) {
                return Err(CatalogError::invalid(format!(
                    "dashboard {field} must be a list of strings"
                )));
            }
        }
        if dashboard.get("targets").is_some_and(|v| {
            !v.as_object()
                .is_some_and(|v| v.values().all(|n| n.as_f64().is_some_and(f64::is_finite)))
        }) {
            return Err(CatalogError::invalid(
                "dashboard targets must be finite numbers",
            ));
        }
    }
    if settings.get("controls").is_some_and(|v| !v.is_object())
        || settings.get("collapsedCharts").is_some_and(|v| {
            !v.as_object()
                .is_some_and(|m| m.values().all(Value::is_boolean))
        })
    {
        return Err(CatalogError::invalid(
            "invalid view controls or collapsedCharts",
        ));
    }
    Ok(())
}
async fn save_view(
    State(state): State<AppState>,
    Path(parent): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    validate_view(&input)?;
    let id = input["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    state
        .catalog
        .write(move |db| {
            catalog::get(db, "project", &parent)?.ok_or_else(CatalogError::missing)?;
            Ok(Json(catalog::save(
                db,
                "view",
                &id,
                &parent,
                &json!({"name":input["name"],"settings":input["settings"],"schema_version":1}),
                Some(0),
            )?))
        })
        .await
}
async fn view(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Entity>> {
    state
        .catalog
        .read(move |db| {
            Ok(Json(
                catalog::get(db, "view", &id)?.ok_or_else(CatalogError::missing)?,
            ))
        })
        .await
}
async fn patch_view(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    validate_view(&input)?;
    let revision = expected(&input)?;
    state
        .catalog
        .write(move |db| {
            let record = catalog::get(db, "view", &id)?.ok_or_else(CatalogError::missing)?;
            Ok(Json(catalog::save(
                db,
                "view",
                &id,
                &record.parent,
                &json!({"name":input["name"],"settings":input["settings"],"schema_version":1}),
                Some(revision),
            )?))
        })
        .await
}
async fn delete_view(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RevisionQuery>,
) -> Result<Json<Value>> {
    state
        .catalog
        .write(move |db| {
            let record = catalog::get(db, "view", &id)?.ok_or_else(CatalogError::missing)?;
            catalog::remove(db, "view", &id, query.revision)?;
            if let Some(mut project) = catalog::get(db, "project", &record.parent)?
                && project.data["default_view"] == id
            {
                project.data["default_view"] = Value::Null;
                catalog::save(
                    db,
                    "project",
                    &project.id,
                    "",
                    &project.data,
                    Some(project.revision),
                )?;
            }
            Ok(Json(json!({"deleted":true})))
        })
        .await
}

async fn definitions(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Entity>>> {
    state
        .catalog
        .read(move |db| Ok(Json(catalog::list(db, "metric", query.parent.as_deref())?)))
        .await
}
async fn define_metric(
    State(state): State<AppState>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    let parent = catalog::required_text(&input, "parent")?.to_owned();
    let name = catalog::required_text(&input, "name")?.to_owned();
    if input
        .get("unit")
        .is_some_and(|u| !u.is_null() && !u.as_str().is_some_and(|s| s.len() <= 128))
    {
        return Err(CatalogError::invalid(
            "unit must be a string up to 128 bytes or null",
        ));
    }
    if input
        .get("goal")
        .is_some_and(|g| !g.is_null() && g != "minimize" && g != "maximize")
    {
        return Err(CatalogError::invalid(
            "goal must be minimize, maximize, or null",
        ));
    }
    if input
        .get("target")
        .is_some_and(|t| !t.is_null() && !t.as_f64().is_some_and(f64::is_finite))
    {
        return Err(CatalogError::invalid(
            "target must be a finite number or null",
        ));
    }
    state.catalog.write(move|db|{
        if catalog::get(db,"project",&parent)?.is_none(){catalog::run_by_id(db,&parent)?;}
        let previous=catalog::list(db,"metric",Some(&parent))?.into_iter().find(|m|m.data["name"]==name);
        let id=previous.as_ref().map(|e|e.id.clone()).unwrap_or_else(||Uuid::new_v4().to_string());
        let data=json!({"name":name,"goal":input["goal"],"target":input["target"],"unit":input["unit"]});
        if let Some(previous)=&previous&& previous.data==data{return Ok(Json(previous.clone()));}
        let revision=input["revision"].as_i64();
        if previous.is_some()&&revision.is_none(){return Err(CatalogError::conflict("expected revision is required to change a metric"));}
        Ok(Json(catalog::save(db,"metric",&id,&parent,&data,Some(revision.unwrap_or(0)))?))
    }).await
}

async fn events(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>> {
    state.catalog.read(move|db|{
        let parent=if let Some(id)=query.run_id {Some(catalog::run_by_id(db,&id)?.id)}else{query.parent};
        let mut rows=catalog::list(db,"event",parent.as_deref())?;
        rows.retain(|e|catalog::run_by_id(db,&e.parent).is_ok());
        rows.sort_by(|a,b|b.data["timestamp_ms"].as_i64().cmp(&a.data["timestamp_ms"].as_i64()).then(a.id.cmp(&b.id)));
        let total=rows.len(); let offset=query.offset.unwrap_or(0);let limit=query.limit.unwrap_or(100).clamp(1,1000);
        Ok(Json(json!({"rows":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"total":total,"next_offset":(offset+limit<total).then_some(offset+limit)})))
    }).await
}
async fn save_event(
    State(state): State<AppState>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    let parent = catalog::required_text(&input, "run_uid")?.to_owned();
    catalog::required_text(&input, "label")?;
    let id = catalog::required_text(&input, "id")?.to_owned();
    if !input["step"].is_null() && input["step"].as_f64().is_none_or(|v| !v.is_finite()) {
        return Err(CatalogError::invalid("invalid step"));
    }
    if input["timestamp_ms"].as_i64().is_none_or(|t| t < 0)
        || !input["details"].is_object()
        || !input["media_ids"].as_array().is_some_and(|ids| {
            ids.len() <= 100
                && ids
                    .iter()
                    .all(|id| id.as_str().is_some_and(|s| s.len() <= 256))
        })
        || input.to_string().len() > 262144
    {
        return Err(CatalogError::invalid(
            "invalid event timestamp, details, or media references",
        ));
    }
    if !["note", "checkpoint", "learning_rate", "warning"]
        .contains(&input["kind"].as_str().unwrap_or(""))
    {
        return Err(CatalogError::invalid(
            "event kind must be note, checkpoint, learning_rate or warning",
        ));
    }
    state
        .catalog
        .write(move |db| {
            let run = catalog::run_by_id(db, &parent)?;
            if let Some(sid) = input["session_id"].as_str()
                && catalog::get(db, "session", sid)?.is_none_or(|s| s.parent != run.id)
            {
                return Err(CatalogError::invalid(
                    "event session belongs to another run",
                ));
            }
            if let Some(source) = catalog::get(db, "event-source", &id)? {
                if source.data["input"] != input {
                    return Err(CatalogError::conflict("event ID has different contents"));
                }
                if source.data["deleted"] == true {
                    return Err(CatalogError::http(
                        axum::http::StatusCode::GONE,
                        "event was deleted".into(),
                    ));
                }
                return Ok(Json(
                    catalog::get(db, "event", &id)?.ok_or_else(CatalogError::missing)?,
                ));
            }
            if let Some(record) = catalog::get(db, "event", &id)? {
                if record.parent != run.id || record.data != input {
                    return Err(CatalogError::conflict("event ID has different contents"));
                }
                return Ok(Json(record));
            }
            catalog::save(
                db,
                "event-source",
                &id,
                &run.id,
                &json!({"input":input,"deleted":false}),
                Some(0),
            )?;
            Ok(Json(catalog::save(
                db,
                "event",
                &id,
                &run.id,
                &input,
                Some(0),
            )?))
        })
        .await
}
async fn patch_event(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Entity>> {
    let revision = expected(&input)?;
    state
        .catalog
        .write(move |db| {
            let mut record = catalog::get(db, "event", &id)?.ok_or_else(CatalogError::missing)?;
            for field in ["label", "details"] {
                if let Some(value) = input.get(field) {
                    record.data[field] = value.clone();
                }
            }
            catalog::required_text(&record.data, "label")?;
            if !record.data["details"].is_object() || record.data.to_string().len() > 262144 {
                return Err(CatalogError::invalid("invalid event details"));
            }
            Ok(Json(catalog::save(
                db,
                "event",
                &id,
                &record.parent,
                &record.data,
                Some(revision),
            )?))
        })
        .await
}
async fn delete_event(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RevisionQuery>,
) -> Result<Json<Value>> {
    state
        .catalog
        .write(move |db| {
            if let Some(mut source) = catalog::get(db, "event-source", &id)? {
                source.data["deleted"] = json!(true);
                catalog::save(
                    db,
                    "event-source",
                    &id,
                    &source.parent,
                    &source.data,
                    Some(source.revision),
                )?;
            }
            catalog::remove(db, "event", &id, query.revision)?;
            Ok(Json(json!({"deleted":true})))
        })
        .await
}

async fn asset(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Entity<crate::models::Asset>>> {
    state
        .catalog
        .read(move |db| {
            let asset = catalog::get(db, "asset", &id)?
                .ok_or_else(CatalogError::missing)?
                .typed::<crate::models::Asset>()?;
            catalog::run_by_id(db, &asset.parent)?;
            Ok(Json(asset))
        })
        .await
}
async fn asset_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match state.storage.load_asset(&id).await {
        Ok(Some(payload)) => crate::history::media_response(payload, request).await,
        Ok(None) => CatalogError::missing().into_response(),
        Err(error) => CatalogError::http(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            error.to_string(),
        )
        .into_response(),
    }
}
