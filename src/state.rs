use crate::config::{ConfigDocument, InstanceConfig};
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::broadcast;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub struct AppState {
    pub attention_lock: Mutex<()>,
    pub maintenance_activity: Mutex<BTreeMap<String, (usize, bool)>>,
    pub home: PathBuf,
    pub profile_lock: Mutex<()>,
    pub config: RwLock<ConfigDocument>,
    pub db: Mutex<Connection>,
    pub events: broadcast::Sender<Value>,
    pub runtime_stop: broadcast::Sender<()>,
    pub runtime_enabled: AtomicBool,
    pub connections: Mutex<BTreeMap<String, Value>>,
    pub started_at: i64,
    pub update_commands: Mutex<()>,
    pub service_commands: Mutex<()>,
    pub update_admission: Mutex<(i64, usize)>,
    pub addresses: RwLock<Value>,
    pub context_leases: Mutex<BTreeMap<String, crate::context::Lease>>,
    pub login_gate: std::sync::Arc<tokio::sync::Semaphore>,
    pub native_login: Mutex<Option<std::sync::Arc<crate::native_account::Login>>>,
}
impl AppState {
    pub fn open(home: PathBuf) -> Result<Self> {
        let config: ConfigDocument =
            serde_json::from_slice(&std::fs::read(home.join("config.json"))?)?;
        config.config.validate()?;
        let db = Connection::open(home.join("state.sqlite3"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS records(collection TEXT NOT NULL,id TEXT NOT NULL,value TEXT NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(collection,id)); CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT,at INTEGER NOT NULL,kind TEXT NOT NULL,data TEXT NOT NULL); CREATE TABLE IF NOT EXISTS tokens(hash TEXT PRIMARY KEY,id TEXT NOT NULL,name TEXT NOT NULL,scopes TEXT NOT NULL,expires_at INTEGER NOT NULL,created_at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS browser_sessions(hash TEXT PRIMARY KEY,owner TEXT NOT NULL,csrf TEXT NOT NULL,expires_at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS login_attempts(key TEXT PRIMARY KEY,failures INTEGER NOT NULL,locked_until INTEGER NOT NULL);")?;
        crate::activity::migrate(&db, config.config.limits.event_retention)?;
        let enabled = config.config.enabled;
        let update_until = db
            .query_row(
                "SELECT value FROM records WHERE collection='updates' AND id='helper'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["drain_until"].as_i64())
            .unwrap_or(0);
        Ok(Self {
            attention_lock: Mutex::new(()),
            maintenance_activity: Mutex::new(BTreeMap::new()),
            home,
            profile_lock: Mutex::new(()),
            config: RwLock::new(config),
            db: Mutex::new(db),
            events: broadcast::channel(256).0,
            runtime_stop: broadcast::channel(16).0,
            runtime_enabled: AtomicBool::new(enabled),
            connections: Mutex::new(BTreeMap::new()),
            started_at: now_ms(),
            update_commands: Mutex::new(()),
            service_commands: Mutex::new(()),
            update_admission: Mutex::new((update_until, 0)),
            addresses: RwLock::new(json!({})),
            context_leases: Mutex::new(BTreeMap::new()),
            login_gate: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
            native_login: Mutex::new(None),
        })
    }
    pub fn instance_config(&self) -> InstanceConfig {
        self.config.read().unwrap().config.clone()
    }
    pub fn event(&self, kind: &str, data: Value) -> Result<Value> {
        let at = now_ms();
        let meta = crate::activity::metadata(kind, &data);
        let limit = if meta.diagnostic {
            crate::activity::DIAGNOSTIC_RETENTION
        } else {
            self.config.read().unwrap().config.limits.event_retention
        };
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO events(at,kind,data,activity_diagnostic,activity_category,activity_severity,activity_summary,activity_data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![at, kind, data.to_string(),meta.diagnostic,meta.category,meta.severity,meta.summary,meta.data.to_string()],
        )?;
        let seq = db.last_insert_rowid();
        crate::activity::retain(&db, meta.diagnostic, limit)?;
        drop(db);
        let event = json!({"seq":seq,"at":at,"kind":kind,"data":data});
        let _ = self.events.send(event.clone());
        Ok(event)
    }
    pub fn set_record(&self, collection: &str, id: &str, value: Value) -> Result<()> {
        self.db.lock().unwrap().execute("INSERT INTO records(collection,id,value,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(collection,id) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",params![collection,id,value.to_string(),now_ms()])?;
        Ok(())
    }
    pub fn get_record(&self, collection: &str, id: &str) -> Result<Option<Value>> {
        use rusqlite::OptionalExtension;
        let value: Option<String> = self
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM records WHERE collection=?1 AND id=?2",
                params![collection, id],
                |row| row.get(0),
            )
            .optional()?;
        value.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    }
    pub fn list_records(&self, collection: &str) -> Result<Vec<Value>> {
        let db = self.db.lock().unwrap();
        let mut s = db.prepare(
            "SELECT value FROM records WHERE collection=?1 ORDER BY updated_at DESC LIMIT 1000",
        )?;
        let rows = s
            .query_map([collection], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn read_secret(&self, name: &str) -> Result<String> {
        anyhow::ensure!(crate::config::valid_name(name), "Invalid secret name");
        Ok(std::fs::read_to_string(
            self.home.join("secrets").join(name),
        )?)
    }
    pub fn enabled(&self) -> bool {
        self.runtime_enabled.load(Ordering::SeqCst)
    }
    pub fn profile_text(&self) -> Result<String> {
        Ok(crate::profiles::context(self)?.0)
    }
}
