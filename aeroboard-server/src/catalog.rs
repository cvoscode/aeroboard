//! Transactional control-plane metadata. Telemetry remains in StorageEngine.
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::http::StatusCode;
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

pub use crate::app_error::AppError as CatalogError;
pub type Result<T> = std::result::Result<T, CatalogError>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity<T = Value> {
    pub id: String,
    pub parent: String,
    pub revision: i64,
    pub data: T,
}

#[derive(Clone)]
pub struct Catalog {
    db: Arc<Mutex<Connection>>,
    readers: Arc<Vec<Mutex<Connection>>>,
    next_reader: Arc<std::sync::atomic::AtomicUsize>,
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
        if version > 2 {
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
            ",
        )?;
        if version < 2 {
            db.execute_batch("BEGIN IMMEDIATE;
                ALTER TABLE receipts ADD COLUMN session_id TEXT NOT NULL DEFAULT '';
                ALTER TABLE receipts ADD COLUMN kind TEXT NOT NULL DEFAULT '';
                UPDATE receipts SET session_id=COALESCE(json_extract(CAST(body AS TEXT),'$.envelope.session_id'),''),
                    kind=COALESCE(json_extract(CAST(body AS TEXT),'$.kind'),'');
                PRAGMA user_version=2; COMMIT;")?;
        }
        db.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS receipts_sequence ON receipts(run_uid,session_id,sequence);
            CREATE INDEX IF NOT EXISTS receipts_run ON receipts(run_uid,kind,committed);
            CREATE INDEX IF NOT EXISTS receipts_session ON receipts(run_uid,session_id,committed);
            CREATE TABLE IF NOT EXISTS run_lookup (uid TEXT PRIMARY KEY, run_id TEXT NOT NULL UNIQUE, deleted INTEGER NOT NULL, project TEXT NOT NULL);
            INSERT OR REPLACE INTO run_lookup SELECT id,json_extract(body,'$.run_id'),COALESCE(json_extract(body,'$.deleted'),0),parent FROM entities WHERE kind='run';
            CREATE INDEX IF NOT EXISTS run_deleted ON run_lookup(deleted,run_id);
            CREATE TRIGGER IF NOT EXISTS run_insert AFTER INSERT ON entities WHEN NEW.kind='run' BEGIN
                INSERT INTO run_lookup VALUES(NEW.id,json_extract(NEW.body,'$.run_id'),COALESCE(json_extract(NEW.body,'$.deleted'),0),NEW.parent)
                ON CONFLICT(uid) DO UPDATE SET run_id=excluded.run_id,deleted=excluded.deleted,project=excluded.project; END;
            CREATE TRIGGER IF NOT EXISTS run_update AFTER UPDATE ON entities WHEN NEW.kind='run' BEGIN
                UPDATE run_lookup SET run_id=json_extract(NEW.body,'$.run_id'),deleted=COALESCE(json_extract(NEW.body,'$.deleted'),0),project=NEW.parent WHERE uid=NEW.id; END;
            CREATE TRIGGER IF NOT EXISTS run_remove AFTER DELETE ON entities WHEN OLD.kind='run' BEGIN DELETE FROM run_lookup WHERE uid=OLD.id; END;
            CREATE TABLE IF NOT EXISTS payload_locations (id TEXT PRIMARY KEY, body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS run_summaries (run_id TEXT PRIMARY KEY, body BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS summary_batches (id TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS run_scores (run_id TEXT PRIMARY KEY, body BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS data_revisions (run_id TEXT PRIMARY KEY, revision INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS catalog_changes (kind TEXT, id TEXT, revision INTEGER);
            CREATE TRIGGER IF NOT EXISTS entity_insert AFTER INSERT ON entities BEGIN INSERT INTO catalog_changes VALUES(NEW.kind,NEW.id,NEW.revision); END;
            CREATE TRIGGER IF NOT EXISTS entity_update AFTER UPDATE ON entities BEGIN INSERT INTO catalog_changes VALUES(NEW.kind,NEW.id,NEW.revision); END;
            CREATE TRIGGER IF NOT EXISTS entity_remove AFTER DELETE ON entities BEGIN INSERT INTO catalog_changes VALUES(OLD.kind,OLD.id,OLD.revision+1); END;")?;
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
        db.execute("DELETE FROM catalog_changes", [])?;
        let mut readers = Vec::new();
        if persistent {
            for _ in 0..4 {
                let reader = Connection::open(root.join("catalog.sqlite"))?;
                reader.busy_timeout(std::time::Duration::from_secs(5))?;
                reader.execute_batch("PRAGMA query_only=ON;")?;
                readers.push(Mutex::new(reader));
            }
        }
        let (changes, _) = broadcast::channel(128);
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            readers: Arc::new(readers),
            next_reader: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        let readers = self.readers.clone();
        let index = self
            .next_reader
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tokio::task::spawn_blocking(move || {
            let source = if readers.is_empty() {
                &*db
            } else {
                &readers[index % readers.len()]
            };
            let connection = source.lock().map_err(|_| {
                CatalogError::http(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "catalog lock poisoned".into(),
                )
            })?;
            let transaction = connection.unchecked_transaction()?;
            let result = work(&transaction)?;
            transaction.commit()?;
            Ok(result)
        })
        .await
        .map_err(CatalogError::from)?
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
                CatalogError::http(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "catalog lock poisoned".into(),
                )
            })?;
            let transaction = connection.transaction()?;
            let result = work(&transaction)?;
            let events = {
                let mut statement = transaction.prepare(
                    "SELECT kind,id,MAX(revision) FROM catalog_changes GROUP BY kind,id",
                )?;
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            transaction.execute("DELETE FROM catalog_changes", [])?;
            transaction.commit()?;
            Ok::<_, CatalogError>((result, events))
        })
        .await
        .map_err(CatalogError::from)?;
        let (value, events) = result?;
        if notify {
            for (kind, id, revision) in events {
                let _ = self.changes.send(json!({"type":format!("{kind}_changed"),"kind":kind,"id":id,"revision":revision}).to_string());
            }
        }
        Ok(value)
    }

    pub async fn import_runs(&self, runs: BTreeSet<String>) -> Result<()> {
        let existing: BTreeSet<String> = self.read(|db| run_names(db, None)).await?;
        if runs.is_subset(&existing) {
            return Ok(());
        }
        self.write(move |db| {
            let existing = run_names(db, None)?;
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
        let data = serde_json::from_str(&body).map_err(CatalogError::from)?;
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
            data: serde_json::from_str(&body).map_err(CatalogError::from)?,
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
    if kind == "run" {
        serde_json::from_value::<crate::models::RunRecord>(data.clone())
            .map_err(|error| CatalogError::invalid(format!("invalid run: {error}")))?;
    }
    if kind == "session" {
        serde_json::from_value::<crate::models::Session>(data.clone())
            .map_err(|error| CatalogError::invalid(format!("invalid session: {error}")))?;
    }
    if kind == "event" {
        serde_json::from_value::<crate::models::Event>(data.clone())
            .map_err(|e| CatalogError::invalid(format!("invalid event: {e}")))?;
    }
    if kind == "asset" {
        serde_json::from_value::<crate::models::Asset>(data.clone())
            .map_err(|e| CatalogError::invalid(format!("invalid asset: {e}")))?;
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
        let uid: Option<String> = db
            .query_row("SELECT uid FROM run_lookup WHERE run_id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        uid.map(|uid| get(db, "run", &uid))
            .transpose()?
            .flatten()
            .ok_or_else(CatalogError::missing)?
    };
    if run.data["deleted"] == true {
        return Err(CatalogError::http(
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

pub fn with_delivery(db: &Connection, mut run: Entity, stale_after_ms: i64) -> Result<Entity> {
    let session = run.data["active_session"]
        .as_str()
        .map(|id| get(db, "session", id))
        .transpose()?
        .flatten();
    if let Some(session) = session {
        let committed: i64 = db.query_row(
            "SELECT COUNT(*) FROM receipts WHERE run_uid=?1 AND committed=1 AND session_id=?2",
            params![run.id, session.id],
            |r| r.get(0),
        )?;
        let final_sequence = session.data["final_sequence"].as_i64();
        let dropped = session.data["dropped_rows"].as_i64().unwrap_or(0);
        run.data["delivery"] = json!({"committed_batches":committed,"final_sequence":final_sequence,"dropped_rows":dropped,
            "status":if dropped>0{"partial"}else if final_sequence.is_some_and(|n|n==committed){"delivered"}else if final_sequence.is_some(){"pending"}else{"open"}});
    }
    Ok(decorate(run, stale_after_ms))
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
            return Err(CatalogError::http(
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
    all.extend(run_names(db, None)?);
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

/// Indexed run identities; avoids deserializing metadata for storage lookups.
pub fn run_names(db: &Connection, deleted: Option<bool>) -> Result<BTreeSet<String>> {
    if let Some(deleted) = deleted {
        let mut statement = db.prepare("SELECT run_id FROM run_lookup WHERE deleted=?1")?;
        Ok(statement
            .query_map([deleted], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?)
    } else {
        let mut statement = db.prepare("SELECT run_id FROM run_lookup")?;
        Ok(statement
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?)
    }
}

/// Entity-parent indexes constrain media lookups to the selected runs.
pub fn assets(
    db: &Connection,
    runs: &BTreeSet<String>,
    kind: &str,
    name: Option<&str>,
    timestamp: Option<i64>,
) -> Result<Vec<Entity<crate::models::Asset>>> {
    let selection = if runs.is_empty() {
        "1"
    } else {
        "run_id IN (SELECT value FROM json_each(?1))"
    };
    let sql = format!(
        "SELECT id,parent,revision,body FROM entities WHERE kind='asset' AND parent IN (SELECT uid FROM run_lookup WHERE deleted=0 AND {selection}) AND json_extract(body,'$.kind')=?2 AND (?3 IS NULL OR json_extract(body,'$.name')=?3) AND (?4 IS NULL OR json_extract(body,'$.ts_ms')=?4) ORDER BY id"
    );
    let mut statement = db.prepare(&sql)?;
    let rows = statement.query_map(
        params![json!(runs).to_string(), kind, name, timestamp],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
    rows.map(|row| {
        let (id, parent, revision, body) = row?;
        Ok(Entity {
            id,
            parent,
            revision,
            data: serde_json::from_str::<crate::models::Asset>(&body)?,
        })
    })
    .collect()
}

/// Validate lifecycle and immutable identity at ingestion and again at commit.
pub fn check_batch(
    db: &Connection,
    envelope: &crate::telemetry::BatchEnvelope,
    assets: &[String],
) -> Result<(String, bool)> {
    let run = run_by_id(db, &envelope.run_uid)?.typed::<crate::models::RunRecord>()?;
    let session = get(db, "session", &envelope.session_id)?
        .ok_or_else(CatalogError::missing)?
        .typed::<crate::models::Session>()?;
    if session.parent != run.id {
        return Err(CatalogError::invalid("session belongs to another run"));
    }
    if session.data.segment.as_deref() != envelope.segment.as_deref() {
        return Err(CatalogError::invalid(
            "batch segment does not match its session",
        ));
    }
    if session
        .data
        .final_sequence
        .is_some_and(|last| envelope.sequence > last)
    {
        return Err(CatalogError::invalid(
            "batch exceeds finished session sequence",
        ));
    }
    let mut unique = BTreeSet::new();
    for id in assets {
        if Uuid::parse_str(id).is_err() || !unique.insert(id) {
            return Err(CatalogError::invalid("asset IDs must be distinct UUIDs"));
        }
        if let Some(asset) = get(db, "asset", id)?
            && asset.data["batch_id"] != envelope.batch_id
        {
            return Err(CatalogError::conflict("asset ID was reused"));
        }
    }
    let receipt: Option<(String, bool, Vec<u8>)> = db
        .query_row(
            "SELECT checksum,committed,body FROM receipts WHERE id=?1",
            [&envelope.batch_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let ready = if let Some((checksum, ready, body)) = receipt {
        let previous: Value = serde_json::from_slice(&body)
            .map_err(|_| CatalogError::invalid("invalid batch receipt"))?;
        if checksum != envelope.checksum || previous["envelope"] != json!(envelope) {
            return Err(CatalogError::conflict(
                "batch ID was reused with different contents or identity",
            ));
        }
        ready
    } else {
        false
    };
    Ok((run.data.run_id, ready))
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    #[tokio::test]
    async fn migrates_v1_receipts_and_enforces_indexed_session_sequences() {
        let root =
            std::env::temp_dir().join(format!("aeroboard-catalog-migration-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Connection::open(root.join("catalog.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE receipts(id TEXT PRIMARY KEY,checksum TEXT NOT NULL,run_uid TEXT NOT NULL,sequence INTEGER NOT NULL,body BLOB,committed INTEGER NOT NULL DEFAULT 0); PRAGMA user_version=1;").unwrap();
        let body = json!({"kind":"scalars","envelope":{"session_id":"session"}}).to_string();
        db.execute(
            "INSERT INTO receipts VALUES('first','hash','run',1,?1,1)",
            [body.as_bytes()],
        )
        .unwrap();
        drop(db);
        let catalog = Catalog::open(&root, true).unwrap();
        catalog.read(|db| {
            assert_eq!(db.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0))?,2);
            let (session,kind)=db.query_row("SELECT session_id,kind FROM receipts WHERE id='first'",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?;
            assert_eq!((session.as_str(),kind.as_str()),("session","scalars"));
            let plan:String=db.query_row("EXPLAIN QUERY PLAN SELECT body FROM receipts WHERE run_uid='run' AND kind='scalars' AND committed=1",[],|r|r.get(3))?;
            assert!(plan.contains("receipts_run"),"{plan}");Ok(())
        }).await.unwrap();
        assert!(catalog.quiet(|db| {db.execute("INSERT INTO receipts(id,checksum,run_uid,session_id,kind,sequence) VALUES('second','hash','run','session','scalars',1)",[])?;Ok(())}).await.is_err());
        let mut events = catalog.changes.subscribe();
        catalog
            .quiet(|db| {
                db.execute(
                    "INSERT OR REPLACE INTO settings VALUES('heartbeat','quiet')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(events.try_recv().is_err());
        drop(catalog);
        std::fs::remove_dir_all(root).unwrap();
    }
}
