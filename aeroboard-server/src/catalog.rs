//! Transactional control-plane metadata. Telemetry remains in StorageEngine.
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use uuid::Uuid;

pub const STALE_AFTER_MS: i64 = 60_000;
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Debug)]
pub struct CatalogError(pub StatusCode, pub String);
impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}
impl std::error::Error for CatalogError {}
impl CatalogError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self(StatusCode::CONFLICT, message.into())
    }
    pub fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "record not found".into())
    }
}
impl From<rusqlite::Error> for CatalogError {
    fn from(error: rusqlite::Error) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}
impl IntoResponse for CatalogError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
pub type Result<T> = std::result::Result<T, CatalogError>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub parent: String,
    pub revision: i64,
    pub data: Value,
}

#[derive(Clone)]
pub struct Catalog {
    db: Arc<Mutex<Connection>>,
    pub changes: broadcast::Sender<String>,
    pub store_id: String,
    pub persistent: bool,
    pub stale_after_ms: i64,
}

impl Catalog {
    pub fn open(
        root: &Path,
        persistent: bool,
    ) -> std::result::Result<Self, crate::storage::BoxError> {
        let db = if persistent {
            std::fs::create_dir_all(root)?;
            Connection::open(root.join("catalog.sqlite"))?
        } else {
            Connection::open_in_memory()?
        };
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 1 {
            return Err(
                format!("catalog schema {version} is newer than this server supports").into(),
            );
        }
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS entities (
                kind TEXT NOT NULL, id TEXT NOT NULL, parent TEXT NOT NULL,
                revision INTEGER NOT NULL, body TEXT NOT NULL, PRIMARY KEY(kind,id));
            CREATE INDEX IF NOT EXISTS entity_parent ON entities(kind,parent);
            CREATE TABLE IF NOT EXISTS receipts (
                id TEXT PRIMARY KEY, checksum TEXT NOT NULL, run_uid TEXT NOT NULL,
                sequence INTEGER NOT NULL, body BLOB, committed INTEGER NOT NULL DEFAULT 0);
            PRAGMA user_version=1;",
        )?;
        db.execute(
            "INSERT OR IGNORE INTO settings VALUES ('store_id',?1)",
            [Uuid::new_v4().to_string()],
        )?;
        let stale_after_ms = std::env::var("AEROBOARD_STALE_AFTER_MS")
            .ok()
            .map(|s| s.parse::<i64>())
            .transpose()?
            .unwrap_or(STALE_AFTER_MS);
        if !(1000..=86_400_000).contains(&stale_after_ms) {
            return Err("AEROBOARD_STALE_AFTER_MS must be between 1000 and 86400000".into());
        }
        db.execute("INSERT INTO settings VALUES ('stale_after_ms',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[stale_after_ms.to_string()])?;
        let store_id = db.query_row(
            "SELECT value FROM settings WHERE key='store_id'",
            [],
            |row| row.get(0),
        )?;
        if get(&db, "project", "default")?.is_none() {
            save(
                &db,
                "project",
                "default",
                "",
                &json!({"name":"Default","default_view":null}),
                None,
            )?;
        }
        let (changes, _) = broadcast::channel(128);
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            changes,
            store_id,
            persistent,
            stale_after_ms,
        })
    }

    pub async fn read<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let connection = db.lock().map_err(|_| {
                CatalogError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "catalog lock poisoned".into(),
                )
            })?;
            work(&connection)
        })
        .await
        .map_err(|e| CatalogError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    }

    pub async fn write<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        self.transaction(work, true).await
    }
    pub async fn quiet<T, F>(&self, work: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        self.transaction(work, false).await
    }
    async fn transaction<T, F>(&self, work: F, notify: bool) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let db = self.db.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut connection = db.lock().map_err(|_| {
                CatalogError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "catalog lock poisoned".into(),
                )
            })?;
            let transaction = connection.transaction()?;
            let result = work(&transaction)?;
            transaction.commit()?;
            Ok(result)
        })
        .await
        .map_err(|e| CatalogError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if notify && result.is_ok() {
            let _ = self
                .changes
                .send(json!({"type":"catalog_changed","at_ms":now_ms()}).to_string());
        }
        result
    }

    pub async fn import_runs(&self, runs: BTreeSet<String>) -> Result<()> {
        let existing: BTreeSet<String> = self
            .read(|db| {
                Ok(list(db, "run", None)?
                    .iter()
                    .filter_map(|r| r.data["run_id"].as_str().map(str::to_owned))
                    .collect())
            })
            .await?;
        if runs.is_subset(&existing) {
            return Ok(());
        }
        self.write(move |db| {
            let existing: BTreeSet<String> = list(db,"run",None)?.iter()
                .filter_map(|r| r.data["run_id"].as_str().map(str::to_owned)).collect();
            for name in runs.difference(&existing) {
                let uid = Uuid::new_v4().to_string();
                save(db,"run",&uid,"default",&json!({
                    "run_id":name,"display_name":name,"project_id":"default", "status":"unknown",
                    "tags":[],"notes":"","provenance":{},"archived":false,"deleted":false,"metadata_revision":1,
                    "created_at_ms":null,"started_at_ms":null,"ended_at_ms":null,
                    "last_heartbeat_ms":null,"active_session":null,"attempt":0
                }),None)?;
            }
            Ok(())
        }).await
    }
}

pub fn get(db: &Connection, kind: &str, id: &str) -> Result<Option<Entity>> {
    let row: Option<(String, i64, String)> = db
        .query_row(
            "SELECT parent,revision,body FROM entities WHERE kind=?1 AND id=?2",
            params![kind, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    row.map(|(parent, revision, body)| {
        let data = serde_json::from_str(&body)
            .map_err(|e| CatalogError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Entity {
            id: id.into(),
            parent,
            revision,
            data,
        })
    })
    .transpose()
}

pub fn list(db: &Connection, kind: &str, parent: Option<&str>) -> Result<Vec<Entity>> {
    let mut statement = db.prepare("SELECT id,parent,revision,body FROM entities WHERE kind=?1 AND (?2 IS NULL OR parent=?2) ORDER BY id")?;
    let rows = statement.query_map(params![kind, parent], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (id, parent, revision, body) = row?;
        Ok(Entity {
            id,
            parent,
            revision,
            data: serde_json::from_str(&body)
                .map_err(|e| CatalogError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        })
    })
    .collect()
}

pub fn save(
    db: &Connection,
    kind: &str,
    id: &str,
    parent: &str,
    data: &Value,
    expected: Option<i64>,
) -> Result<Entity> {
    let current = get(db, kind, id)?;
    let revision = current.as_ref().map_or(0, |e| e.revision);
    if expected.is_some_and(|value| value != revision) {
        return Err(CatalogError::conflict(
            "record changed; reload or save a copy",
        ));
    }
    if !data.is_object() {
        return Err(CatalogError::invalid("data must be an object"));
    }
    db.execute("INSERT INTO entities VALUES (?1,?2,?3,?4,?5) ON CONFLICT(kind,id) DO UPDATE SET parent=excluded.parent,revision=excluded.revision,body=excluded.body",
        params![kind,id,parent,revision+1,data.to_string()])?;
    Ok(Entity {
        id: id.into(),
        parent: parent.into(),
        revision: revision + 1,
        data: data.clone(),
    })
}

pub fn remove(db: &Connection, kind: &str, id: &str, revision: i64) -> Result<()> {
    if get(db, kind, id)?
        .ok_or_else(CatalogError::missing)?
        .revision
        != revision
    {
        return Err(CatalogError::conflict("record changed"));
    }
    db.execute(
        "DELETE FROM entities WHERE kind=?1 AND id=?2",
        params![kind, id],
    )?;
    Ok(())
}

pub fn run_by_id(db: &Connection, id: &str) -> Result<Entity> {
    let run = if let Some(run) = get(db, "run", id)? {
        run
    } else {
        list(db, "run", None)?
            .into_iter()
            .find(|r| r.data["run_id"] == id)
            .ok_or_else(CatalogError::missing)?
    };
    if run.data["deleted"] == true {
        return Err(CatalogError(
            StatusCode::GONE,
            "run has been deleted".into(),
        ));
    }
    Ok(run)
}

pub fn decorate(mut entity: Entity, stale_after_ms: i64) -> Entity {
    let status = entity.data["status"].as_str().unwrap_or("unknown");
    entity.data["display_status"] = json!(if status == "running"
        && entity.data["last_heartbeat_ms"]
            .as_i64()
            .is_none_or(|t| now_ms() - t > stale_after_ms)
    {
        "stale"
    } else {
        status
    });
    entity
}

pub fn with_delivery(db: &Connection, mut run: Entity) -> Result<Entity> {
    let session = run.data["active_session"]
        .as_str()
        .map(|id| get(db, "session", id))
        .transpose()?
        .flatten();
    if let Some(session) = session {
        let committed:i64=db.query_row("SELECT COUNT(*) FROM receipts WHERE run_uid=?1 AND committed=1 AND json_extract(CAST(body AS TEXT),'$.envelope.session_id')=?2",params![run.id,session.id],|r|r.get(0))?;
        let final_sequence = session.data["final_sequence"].as_i64();
        let dropped = session.data["dropped_rows"].as_i64().unwrap_or(0);
        run.data["delivery"] = json!({"committed_batches":committed,"final_sequence":final_sequence,"dropped_rows":dropped,
            "status":if dropped>0{"partial"}else if final_sequence.is_some_and(|n|n==committed){"delivered"}else if final_sequence.is_some(){"pending"}else{"open"}});
    }
    Ok(decorate(run, stale_interval(db)?))
}
fn stale_interval(db: &Connection) -> Result<i64> {
    Ok(db
        .query_row(
            "SELECT value FROM settings WHERE key='stale_after_ms'",
            [],
            |r| r.get::<_, String>(0),
        )?
        .parse()
        .unwrap_or(STALE_AFTER_MS))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterRun {
    pub run_uid: String,
    pub name: String,
    #[serde(default = "default_project")]
    pub project_id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub provenance: Value,
}
fn default_project() -> String {
    "default".into()
}

pub fn register(db: &Connection, input: RegisterRun, known: BTreeSet<String>) -> Result<Entity> {
    Uuid::parse_str(&input.run_uid).map_err(|_| CatalogError::invalid("run_uid must be a UUID"))?;
    if input.tags.len() > 50
        || input.tags.iter().any(|t| t.len() > 128)
        || (!input.provenance.is_object() && !input.provenance.is_null())
        || input.provenance.to_string().len() > 65536
    {
        return Err(CatalogError::invalid(
            "invalid tags or provenance (maximum 64 KiB)",
        ));
    }
    if input.name.trim().is_empty()
        || input.name.len() > 256
        || [".", ".."].contains(&input.name.trim())
    {
        return Err(CatalogError::invalid(
            "name must have 1–256 characters and cannot be . or ..",
        ));
    }
    if get(db, "project", &input.project_id)?.is_none() {
        return Err(CatalogError::invalid("unknown project"));
    }
    if let Some(existing) = get(db, "run", &input.run_uid)? {
        if existing.data["deleted"] == true {
            return Err(CatalogError(
                StatusCode::GONE,
                "run has been deleted".into(),
            ));
        }
        if existing.parent != input.project_id
            || existing.data["requested_name"] != input.name.trim()
        {
            return Err(CatalogError::conflict(
                "run UID was already registered with different parameters",
            ));
        }
        return Ok(existing);
    }
    let mut all = known;
    all.extend(
        list(db, "run", None)?
            .iter()
            .filter_map(|e| e.data["run_id"].as_str().map(str::to_owned)),
    );
    let name = crate::history::resolve_run_id(input.name.trim(), &all);
    save(
        db,
        "run",
        &input.run_uid,
        &input.project_id,
        &json!({
            "run_id":name,"display_name":name,"requested_name":input.name.trim(),"project_id":input.project_id,
            "status":"unknown","tags":input.tags,"provenance":input.provenance,
            "notes":"","archived":false,"deleted":false,"metadata_revision":1,"created_at_ms":now_ms(),
            "started_at_ms":null,"ended_at_ms":null,"last_heartbeat_ms":null,"active_session":null,"attempt":0
        }),
        Some(0),
    )
}

pub fn start_session(db: &Connection, uid: &str, input: &Value) -> Result<Entity> {
    let mut run = run_by_id(db, uid)?;
    let sid = required_text(input, "session_id")?;
    if !input["segment"].is_null()
        && !input["segment"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 128)
    {
        return Err(CatalogError::invalid(
            "segment must be a nonempty string up to 128 bytes",
        ));
    }
    Uuid::parse_str(sid).map_err(|_| CatalogError::invalid("session_id must be a UUID"))?;
    if let Some(existing) = get(db, "session", sid)? {
        if existing.parent != run.id {
            return Err(CatalogError::conflict("session belongs to another run"));
        }
        if existing.data["segment"] != input["segment"] {
            return Err(CatalogError::conflict("session segment cannot change"));
        }
        return Ok(existing);
    }
    let previous = run.data["active_session"].as_str();
    if previous.is_some() && input["resume"] != true {
        return Err(CatalogError::conflict(
            "use resume to continue an existing run",
        ));
    }
    if run.data["status"] == "running"
        && run.data["last_heartbeat_ms"]
            .as_i64()
            .is_some_and(|t| now_ms() - t < stale_interval(db).unwrap_or(STALE_AFTER_MS))
    {
        return Err(CatalogError::conflict("run already has an active writer"));
    }
    let attempt = run.data["attempt"].as_i64().unwrap_or(0) + 1;
    let time = input["captured_at_ms"]
        .as_i64()
        .filter(|t| *t >= 0 && *t <= now_ms() + 60_000)
        .unwrap_or_else(now_ms);
    run.data["status"] = json!("running");
    run.data["active_session"] = json!(sid);
    run.data["attempt"] = json!(attempt);
    run.data["last_heartbeat_ms"] = if input["historical"] == true {
        Value::Null
    } else {
        json!(now_ms())
    };
    run.data["ended_at_ms"] = Value::Null;
    if run.data["started_at_ms"].is_null() {
        run.data["started_at_ms"] = json!(time);
    }
    save(
        db,
        "run",
        &run.id,
        &run.parent,
        &run.data,
        Some(run.revision),
    )?;
    save(
        db,
        "session",
        sid,
        &run.id,
        &json!({"attempt":attempt,"started_at_ms":time,"ended_at_ms":null,"sequence":-1,"status":"running","segment":input["segment"]}),
        Some(0),
    )
}

pub fn update_session(
    db: &Connection,
    uid: &str,
    sid: &str,
    input: &Value,
    finish: bool,
) -> Result<Entity> {
    let mut run = run_by_id(db, uid)?;
    let active = run.data["active_session"] == sid;
    if !active && !finish {
        return Err(CatalogError::conflict("session has been superseded"));
    }
    let mut session = get(db, "session", sid)?.ok_or_else(CatalogError::missing)?;
    if session.parent != run.id {
        return Err(CatalogError::invalid("session belongs to another run"));
    }
    let sequence = input["sequence"]
        .as_i64()
        .filter(|s| *s >= 0)
        .ok_or_else(|| CatalogError::invalid("sequence must be nonnegative"))?;
    if sequence <= session.data["sequence"].as_i64().unwrap_or(-1) {
        return Ok(session);
    }
    if session.data["status"] != "running" {
        return Err(CatalogError::conflict("session is finished"));
    }
    if finish {
        let status = required_text(input, "status")?;
        if !["completed", "failed", "cancelled"].contains(&status) {
            return Err(CatalogError::invalid("invalid terminal status"));
        }
        session.data["status"] = json!(status);
        let ended = input["captured_at_ms"]
            .as_i64()
            .filter(|t| {
                *t >= session.data["started_at_ms"].as_i64().unwrap_or(0) && *t <= now_ms() + 60_000
            })
            .unwrap_or_else(now_ms);
        session.data["ended_at_ms"] = json!(ended);
        if input.get("final_sequence").is_some()
            && input["final_sequence"].as_i64().is_none_or(|n| n < 0)
        {
            return Err(CatalogError::invalid("final_sequence must be nonnegative"));
        }
        session.data["final_sequence"] = input["final_sequence"].clone();
        session.data["dropped_rows"] = input["dropped_rows"].clone();
        session.data["error"] = input["error"].clone();
        run.data["status"] = json!(status);
        run.data["ended_at_ms"] = session.data["ended_at_ms"].clone();
        run.data["error"] = input["error"].clone();
    } else {
        run.data["last_heartbeat_ms"] = json!(now_ms());
    }
    session.data["sequence"] = json!(sequence);
    if active {
        save(
            db,
            "run",
            &run.id,
            &run.parent,
            &run.data,
            Some(run.revision),
        )?;
    }
    save(
        db,
        "session",
        sid,
        &run.id,
        &session.data,
        Some(session.revision),
    )
}

pub fn required_text<'a>(input: &'a Value, name: &str) -> Result<&'a str> {
    input[name]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
        .ok_or_else(|| {
            CatalogError::invalid(format!("{name} must be a nonempty string (max 4096 bytes)"))
        })
}
