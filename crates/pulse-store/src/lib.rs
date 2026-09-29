//! SQLite store foundation for PULSE.

use anyhow::{Context, Result, anyhow};
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

include!(concat!(env!("OUT_DIR"), "/ddl_generated.rs"));

pub const SCHEMA_VERSION: i64 = 3;
pub const STORE_NAME: &str = "pulse.sqlite";
pub const HUMAN_AGENT: &str = "human";
pub const EVOLUTION_STOP_SUMMARY: &str = "Evolution stopped; owner review needed";
pub const COMMONS_SLUG: &str = "commons";
pub const BUSY_TIMEOUT_MS: u64 = 10_000;
pub const JOURNAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;
pub const MAX_POOL: usize = 8;

#[derive(Debug, thiserror::Error)]
#[error("No store at {path}. Run `pulse init` in a project first.")]
pub struct NotFoundError {
    pub path: String,
}

#[derive(Debug, thiserror::Error)]
#[error("{msg}")]
pub struct InputError {
    pub msg: String,
}

fn input_err(msg: impl Into<String>) -> anyhow::Error {
    InputError { msg: msg.into() }.into()
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub home: String,
    pub create: bool,
    pub read_only: bool,
    pub skip_records: bool,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub table: &'static str,
    pub name: &'static str,
    pub ddl: &'static str,
}

#[derive(Debug)]
struct Inner {
    path: String,
    read_only: bool,
    now: String,
    pool: Mutex<Vec<Connection>>,
}

#[derive(Clone, Debug)]
pub struct Store {
    inner: Arc<Inner>,
    pub home: String,
    pub path: String,
}

unsafe impl Send for Store {}
unsafe impl Sync for Store {}

pub struct PooledConn {
    store: Store,
    conn: Option<Connection>,
}

impl Deref for PooledConn {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        self.conn.as_ref().expect("pooled connection present")
    }
}

impl DerefMut for PooledConn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.conn.as_mut().expect("pooled connection present")
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let mut pool = self.store.inner.pool.lock().expect("store pool poisoned");
            if pool.len() < MAX_POOL {
                pool.push(conn);
            }
        }
    }
}

impl Store {
    pub fn open(o: Options) -> Result<Self> {
        let home = resolve_home(&o.home)?;
        let path = Path::new(&home).join(STORE_NAME);
        let exists = path.exists();
        let guard = worker_guard(&home);
        if !exists {
            if !o.create || guard || o.read_only {
                return Err(NotFoundError {
                    path: path.display().to_string(),
                }
                .into());
            }
            refuse_inside_checkout(&home)?;
            fs::create_dir_all(&home)?;
            let _ = fs::set_permissions(&home, std::os::unix::fs::PermissionsExt::from_mode(0o700));
        }
        let s = Store {
            inner: Arc::new(Inner {
                path: path.display().to_string(),
                read_only: o.read_only,
                now: o.now.clone(),
                pool: Mutex::new(Vec::new()),
            }),
            home,
            path: path.display().to_string(),
        };
        if !exists {
            let conn = s.conn()?;
            init_schema(&conn, &s.now()).with_context(|| format!("creating {}", s.path))?;
        } else {
            s.check_version()?;
        }
        if !o.read_only && !o.skip_records && !guard {
            s.ensure_records()?;
        }
        Ok(s)
    }

    pub fn create(path: impl AsRef<Path>) -> Result<()> {
        create(path)
    }

    pub fn conn(&self) -> Result<PooledConn> {
        if let Some(conn) = self.inner.pool.lock().expect("store pool poisoned").pop() {
            return Ok(PooledConn {
                store: self.clone(),
                conn: Some(conn),
            });
        }
        let flags = if self.inner.read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
        };
        let conn = Connection::open_with_flags(&self.inner.path, flags)?;
        configure_conn(&conn, self.inner.read_only)?;
        Ok(PooledConn {
            store: self.clone(),
            conn: Some(conn),
        })
    }

    pub fn db_path(&self) -> &str {
        &self.path
    }

    pub fn now(&self) -> String {
        if !self.inner.now.is_empty() {
            return self.inner.now.clone();
        }
        now()
    }

    pub fn close(&self) -> Result<()> {
        self.inner.pool.lock().expect("store pool poisoned").clear();
        Ok(())
    }

    pub fn begin(&self) -> Result<Tx> {
        let conn = self.conn()?;
        conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Tx {
            store: self.clone(),
            conn,
            done: false,
        })
    }

    pub fn event<D: Into<Detail>>(
        &self,
        actor: &str,
        action: &str,
        subject: &str,
        detail: D,
    ) -> Result<()> {
        let tx = self.begin()?;
        tx.event(actor, action, subject, detail)?;
        tx.commit()
    }

    fn check_version(&self) -> Result<()> {
        let conn = self.conn()?;
        let have: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match have.as_deref() {
            Some("3") => Ok(()),
            Some(v) => Err(anyhow!(
                "unsupported store schema {v}; this pulse speaks {SCHEMA_VERSION}"
            )),
            None => Err(anyhow!("store {} has no schema_version", self.path)),
        }
    }

    pub fn ensure_records(&self) -> Result<()> {
        if self.additive_complete() {
            return Ok(());
        }
        let tx = self.begin()?;
        for list in additive_ddl() {
            for stmt in list {
                tx.execute(stmt, [])?;
            }
        }
        for c in additive_columns() {
            if !has_column(&tx, c.table, c.name)? {
                tx.execute(
                    &format!("ALTER TABLE {} ADD COLUMN {} {}", c.table, c.name, c.ddl),
                    [],
                )?;
            }
        }
        let changed = tx.execute(
            "INSERT INTO meta (key, value) VALUES ('records_version', ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value WHERE value <> excluded.value",
            [RECORDS_VERSION],
        )?;
        if changed > 0 {
            let subject = format!("records_version={RECORDS_VERSION}");
            tx.event("pulse", "schema.records", &subject, "")?;
        }
        tx.commit()
    }

    fn additive_complete(&self) -> bool {
        let Ok((have, ok)) = self.meta("records_version") else {
            return false;
        };
        if !ok || have != RECORDS_VERSION {
            return false;
        }
        additive_columns().iter().all(|c| {
            self.conn()
                .and_then(|conn| has_column(&conn, c.table, c.name))
                .unwrap_or(false)
        })
    }

    pub fn checkpoint(&self) -> Result<CheckpointResult> {
        let before = wal_size(&self.path);
        let conn = self.conn()?;
        let (busy, log, checkpointed): (i64, i64, i64) =
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        Ok(CheckpointResult {
            busy: busy as i32,
            log: log as i32,
            checkpointed: checkpointed as i32,
            wal_bytes_before: before,
            wal_bytes_after: wal_size(&self.path),
        })
    }

    pub fn meta(&self, key: &str) -> Result<(String, bool)> {
        let conn = self.conn()?;
        let v = conn
            .query_row("SELECT value FROM meta WHERE key = ?", [key], |r| r.get(0))
            .optional()?;
        Ok(match v {
            Some(v) => (v, true),
            None => (String::new(), false),
        })
    }
}

pub struct Tx {
    store: Store,
    conn: PooledConn,
    done: bool,
}

impl Deref for Tx {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl DerefMut for Tx {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

impl Tx {
    pub fn event<D: Into<Detail>>(
        &self,
        actor: &str,
        action: &str,
        subject: &str,
        detail: D,
    ) -> Result<()> {
        let detail = detail.into().text()?;
        event_insert(
            &self.conn,
            &self.store.now(),
            actor,
            action,
            subject,
            &detail,
        )?;
        Ok(())
    }

    pub fn commit(mut self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, at, actor, action, subject, detail FROM events ORDER BY seq DESC LIMIT 128",
        )?;
        let rows = stmt.query_map([], scan_event)?;
        let mut latest: Vec<EventRecord> = rows.collect::<rusqlite::Result<_>>()?;
        latest.reverse();
        self.conn.execute_batch("COMMIT")?;
        self.done = true;
        publish(&latest);
        Ok(())
    }

    pub fn rollback(mut self) -> Result<()> {
        if !self.done {
            self.conn.execute_batch("ROLLBACK")?;
            self.done = true;
        }
        Ok(())
    }
}

impl Drop for Tx {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.conn.execute_batch("ROLLBACK");
            self.done = true;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckpointResult {
    pub busy: i32,
    pub log: i32,
    pub checkpointed: i32,
    pub wal_bytes_before: i64,
    pub wal_bytes_after: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventRecord {
    pub seq: i64,
    pub at: String,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub detail: String,
}

type Observer = Box<dyn Fn(EventRecord) + Send + Sync + 'static>;
static OBSERVERS: OnceLock<Mutex<Vec<Observer>>> = OnceLock::new();

pub fn on_event<F>(f: F)
where
    F: Fn(EventRecord) + Send + Sync + 'static,
{
    OBSERVERS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("event observers poisoned")
        .push(Box::new(f));
}

fn publish(events: &[EventRecord]) {
    if let Some(obs) = OBSERVERS.get() {
        for e in events {
            for f in obs.lock().expect("event observers poisoned").iter() {
                f(e.clone());
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Detail {
    Empty,
    Text(String),
    Json(Value),
}

impl Detail {
    pub fn text(self) -> Result<String> {
        match self {
            Detail::Empty => Ok(String::new()),
            Detail::Text(s) => Ok(s),
            Detail::Json(v) => py_json_value(&v),
        }
    }

    pub fn json<T: Serialize>(value: T) -> Result<Self> {
        Ok(Self::Json(serde_json::to_value(value)?))
    }
}

impl From<&str> for Detail {
    fn from(value: &str) -> Self {
        Detail::Text(value.to_string())
    }
}
impl From<String> for Detail {
    fn from(value: String) -> Self {
        Detail::Text(value)
    }
}
impl From<Vec<u8>> for Detail {
    fn from(value: Vec<u8>) -> Self {
        Detail::Text(String::from_utf8_lossy(&value).to_string())
    }
}
impl From<&[u8]> for Detail {
    fn from(value: &[u8]) -> Self {
        Detail::Text(String::from_utf8_lossy(value).to_string())
    }
}
impl From<Value> for Detail {
    fn from(value: Value) -> Self {
        Detail::Json(value)
    }
}
impl<T> From<Option<T>> for Detail
where
    T: Into<Detail>,
{
    fn from(value: Option<T>) -> Self {
        value.map(Into::into).unwrap_or(Detail::Empty)
    }
}

pub fn now() -> String {
    if let Ok(s) = std::env::var("PULSE_NOW")
        && !s.is_empty()
    {
        return s;
    }
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn resolve_home(explicit: &str) -> Result<String> {
    let mut home = if explicit.is_empty() {
        std::env::var("PULSE_HOME").unwrap_or_default()
    } else {
        explicit.to_string()
    };
    if home.is_empty() {
        home = home::home_dir()
            .ok_or_else(|| anyhow!("cannot find the home directory"))?
            .join(".pulse")
            .display()
            .to_string();
    }
    if home.starts_with('~') {
        let user = home::home_dir().ok_or_else(|| anyhow!("cannot find the home directory"))?;
        let rest = home
            .trim_start_matches('~')
            .trim_start_matches(std::path::MAIN_SEPARATOR);
        home = user.join(rest).display().to_string();
    }
    let abs = PathBuf::from(home).canonicalize().or_else(|_| {
        let p = PathBuf::from(explicit);
        if p.is_absolute() {
            Ok(p)
        } else {
            std::env::current_dir().map(|cwd| cwd.join(p))
        }
    })?;
    Ok(abs.display().to_string())
}

pub fn store_path(explicit: &str) -> Result<String> {
    Ok(Path::new(&resolve_home(explicit)?)
        .join(STORE_NAME)
        .display()
        .to_string())
}

pub fn worker_guard(home: &str) -> bool {
    if std::env::var("PULSE_JOB").is_ok_and(|v| !v.is_empty()) {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.canonicalize().unwrap_or(exe);
    let home = Path::new(home);
    exe.starts_with(home) && !exe.starts_with(home.join("bin"))
}

pub fn create(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    if path.exists() {
        return Err(anyhow!("{} already exists", path.display()));
    }
    fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))?;
    let conn = Connection::open(path)?;
    configure_conn(&conn, false)?;
    init_schema(&conn, &now())?;
    let s = Store {
        inner: Arc::new(Inner {
            path: path.display().to_string(),
            read_only: false,
            now: String::new(),
            pool: Mutex::new(vec![conn]),
        }),
        home: path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .display()
            .to_string(),
        path: path.display().to_string(),
    };
    s.ensure_records()
}

pub fn additive_ddl() -> Vec<&'static [&'static str]> {
    vec![
        RECORDS_DDL,
        TASKS_DDL,
        HOOKS_DDL,
        RUNNER_DDL,
        EVOLVE_DDL,
        HARNESS_DDL,
    ]
}

pub fn additive_columns() -> Vec<Column> {
    ADDITIVE_COLUMNS_DATA
        .iter()
        .map(|(table, name, ddl)| Column { table, name, ddl })
        .collect()
}

fn init_schema(conn: &Connection, at: &str) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        conn.execute_batch(DDL)?;
        let have: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if have.is_some() {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?)",
            [SCHEMA_VERSION.to_string()],
        )?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('created_at', ?)",
            [at],
        )?;
        for (name, value) in DEFAULT_SETTINGS_DATA {
            conn.execute(
                "INSERT OR IGNORE INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, 'init')",
                params![name, value, at],
            )?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO agents (name, display, kind, status, created_at, notes) VALUES (?, ?, ?, ?, ?, ?)",
            params![HUMAN_AGENT, "the human", "human", "active", at, "the maintainer; records yes or no with `pulse decide`"],
        )?;
        Ok::<_, anyhow::Error>(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

fn configure_conn(conn: &Connection, read_only: bool) -> Result<()> {
    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    if !read_only {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT_BYTES)?;
        conn.pragma_update(None, "wal_autocheckpoint", 1000)?;
    }
    Ok(())
}

fn refuse_inside_checkout(home: &str) -> Result<()> {
    let mut dir = PathBuf::from(home);
    while !dir.exists() {
        if !dir.pop() {
            break;
        }
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["rev-parse", "--show-toplevel"])
        .env_clear()
        .envs(pulse_gitenv::env().into_iter().filter_map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        }))
        .output();
    if let Ok(out) = out
        && out.status.success()
    {
        let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !top.is_empty() {
            return Err(input_err(format!(
                "The store {}/{} must live outside the project {}; set PULSE_HOME elsewhere",
                home, STORE_NAME, top
            )));
        }
    }
    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?)")?;
    let mut rows = stmt.query([table])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn event_insert(
    conn: &Connection,
    at: &str,
    actor: &str,
    action: &str,
    subject: &str,
    detail: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
        params![at, actor, action, subject, detail],
    )?;
    Ok(conn.last_insert_rowid())
}

fn scan_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    Ok(EventRecord {
        seq: row.get(0)?,
        at: row.get(1)?,
        actor: row.get(2)?,
        action: row.get(3)?,
        subject: row.get(4)?,
        detail: row.get(5)?,
    })
}

fn wal_size(path: &str) -> i64 {
    fs::metadata(format!("{path}-wal"))
        .map(|m| i64::try_from(m.len()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

pub fn py_json<T: Serialize>(value: T) -> Result<String> {
    py_json_value(&serde_json::to_value(value)?)
}

fn py_json_value(value: &Value) -> Result<String> {
    fn write(out: &mut String, v: &Value) -> Result<()> {
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => out.push_str(&n.to_string()),
            Value::String(s) => out.push_str(&serde_json::to_string(s)?),
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write(out, x)?;
                }
                out.push(']');
            }
            Value::Object(m) => {
                out.push('{');
                let ordered: BTreeMap<_, _> = m.iter().collect();
                for (i, (k, v)) in ordered.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&serde_json::to_string(k)?);
                    out.push_str(": ");
                    write(out, v)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }
    let mut out = String::new();
    write(&mut out, value)?;
    Ok(out)
}

pub fn default_settings() -> BTreeMap<String, String> {
    DEFAULT_SETTINGS_DATA
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

pub fn setting_names() -> Vec<String> {
    DEFAULT_SETTINGS_DATA
        .iter()
        .map(|(k, _)| (*k).to_string())
        .collect()
}

pub fn validate_setting(name: &str, value: &str) -> Result<String> {
    if !DEFAULT_SETTINGS_DATA.iter().any(|(k, _)| *k == name) {
        return Err(input_err(format!("unknown setting {name}")));
    }
    Ok(value.trim().to_string())
}

impl Store {
    pub fn setting(&self, key: &str, def: &str) -> String {
        let Ok(conn) = self.conn() else {
            return def.to_string();
        };
        conn.query_row("SELECT value FROM settings WHERE name = ?", [key], |r| {
            r.get(0)
        })
        .optional()
        .ok()
        .flatten()
        .or_else(|| default_settings().get(key).cloned())
        .unwrap_or_else(|| def.to_string())
    }

    pub fn setting_int(&self, key: &str) -> i64 {
        self.setting(key, "0").parse().unwrap_or(0)
    }

    pub fn setting_float(&self, key: &str) -> f64 {
        self.setting(key, "0").parse().unwrap_or(0.0)
    }

    pub fn settings(&self) -> Result<BTreeMap<String, String>> {
        let mut out = default_settings();
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT name, value FROM settings ORDER BY name")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (k, v) = row?;
            out.insert(k, v);
        }
        Ok(out)
    }

    pub fn set_setting(&self, actor: &str, key: &str, value: &str) -> Result<()> {
        let value = validate_setting(key, value)?;
        let tx = self.begin()?;
        tx.execute(
            "INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, ?) \
             ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![key, value, self.now(), actor],
        )?;
        tx.event(actor, "setting.set", key, value.as_str())?;
        tx.commit()
    }

    pub fn run_model(&self, kind: &str) -> String {
        if ["task", "improve", "experiment"].contains(&kind) {
            self.setting("worker_model", "").trim().to_string()
        } else {
            self.light_model()
        }
    }

    pub fn light_model(&self) -> String {
        let light = self.setting("light_model", "").trim().to_string();
        if light.is_empty() {
            self.setting("worker_model", "").trim().to_string()
        } else {
            light
        }
    }

    pub fn harness_native(&self) -> bool {
        self.setting("harness", "") == "native"
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Project {
    pub slug: String,
    pub name: String,
    pub path: String,
    pub remote: String,
    pub brain: bool,
    pub verify_cmd: String,
    pub default_branch: String,
    pub status: String,
    pub added_at: String,
    pub added_by: String,
    pub last_seen: String,
    pub notes: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Agent {
    pub name: String,
    pub display: String,
    pub runner: String,
    pub model: String,
    pub kind: String,
    pub status: String,
    pub created_at: String,
    pub last_active_at: String,
    pub last_useful_at: String,
    pub sessions: String,
    pub jobs_done: i64,
    pub jobs_failed: i64,
    pub posts: i64,
    pub pruned_at: String,
    pub pruned_reason: String,
    pub legacy_id: String,
    pub notes: String,
}

pub fn post_for_owner(extra: &str) -> Result<bool> {
    let v: Value = serde_json::from_str(extra).unwrap_or(Value::Null);
    Ok(v.get("for_owner").and_then(Value::as_bool).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn py_json_sorts_and_spaces() {
        let v = serde_json::json!({"b": [1, true], "a": "é"});
        assert_eq!(py_json(v).unwrap(), r#"{"a": "é", "b": [1, true]}"#);
    }

    #[test]
    fn creates_store_with_rows() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(s.setting("jobs_per_day", ""), "2");
        assert_eq!(s.meta("schema_version").unwrap(), ("3".into(), true));
        assert!(s.meta("records_version").unwrap().1);
    }
}
