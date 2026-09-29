use crate::ddl::DDL;
use crate::evolve_ddl::EVOLVE_DDL;
use crate::harness_ddl::{HARNESS_COLUMNS, HARNESS_DDL};
use crate::hooks_ddl::HOOKS_DDL;
use crate::pyjson::py_json_value;
use crate::records_ddl::{RECORDS_DDL, RECORDS_VERSION};
use crate::runner_ddl::{RUNNER_COLUMNS, RUNNER_DDL};
use crate::settings::{default_settings, setting_names};
use crate::tasks_ddl::TASKS_DDL;
use anyhow::{Context, Result, anyhow};
use chrono::{SecondsFormat, Utc};
use regex::Regex;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

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

pub(crate) fn input_err(msg: impl Into<String>) -> anyhow::Error {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("pooled connection present")
    }
}
impl DerefMut for PooledConn {
    fn deref_mut(&mut self) -> &mut Connection {
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
        let guard = worker_guard(&home);
        let exists = path.exists();
        if !exists {
            if !o.create || guard || o.read_only {
                return Err(NotFoundError {
                    path: path.display().to_string(),
                }
                .into());
            }
            refuse_inside_checkout(&home)?;
            fs::create_dir_all(&home)?;
            #[cfg(unix)]
            {
                let _ =
                    fs::set_permissions(&home, std::os::unix::fs::PermissionsExt::from_mode(0o700));
            }
        }
        let s = Store {
            inner: Arc::new(Inner {
                path: path.display().to_string(),
                read_only: o.read_only,
                now: o.now,
                pool: Mutex::new(Vec::new()),
            }),
            home,
            path: path.display().to_string(),
        };
        if !exists {
            let conn = s.conn()?;
            if let Err(e) = init_schema(&conn, &s.now()) {
                drop(conn);
                let _ = fs::remove_file(&s.path);
                return Err(e).with_context(|| format!("creating {}", s.path));
            }
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
            self.inner.now.clone()
        } else {
            now()
        }
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
            pending: Mutex::new(Vec::new()),
        })
    }

    pub fn event<D: Into<Detail>>(
        &self,
        actor: &str,
        action: &str,
        subject: &str,
        detail: D,
    ) -> Result<()> {
        let text = detail.into().text()?;
        let conn = self.conn()?;
        let rec = event_insert(&conn, &self.now(), actor, action, subject, &text)?;
        publish(&[rec]);
        Ok(())
    }

    fn check_version(&self) -> Result<()> {
        let conn = self.conn()?;
        let have: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .with_context(|| format!("{} has no schema_version (not a PULSE store?)", self.path))?;
        let n: i64 = have.parse().map_err(|_| {
            anyhow!(
                "{} has a schema_version that is not a number: {:?}",
                self.path,
                have
            )
        })?;
        if n > SCHEMA_VERSION {
            anyhow::bail!(
                "Store schema {n} is newer than this tool ({SCHEMA_VERSION}); update pulse"
            );
        }
        if n < SCHEMA_VERSION {
            anyhow::bail!(
                "Store schema {n} is older than this tool ({SCHEMA_VERSION}); run `python3 legacy/pulse.py status` once to migrate it"
            );
        }
        Ok(())
    }

    pub fn ensure_records(&self) -> Result<()> {
        if self.additive_complete() {
            return Ok(());
        }
        let tx = self.begin()?;
        for list in additive_ddl() {
            for stmt in list {
                tx.execute(stmt, [])
                    .map_err(|e| anyhow!("records schema: {e}"))?;
            }
        }
        for c in additive_columns() {
            if !has_column(&tx, c.table, c.name).map_err(|e| anyhow!("records schema: {e}"))? {
                tx.execute(
                    &format!("ALTER TABLE {} ADD COLUMN {} {}", c.table, c.name, c.ddl),
                    [],
                )
                .map_err(|e| anyhow!("records schema: {e}"))?;
            }
        }
        let changed = tx.execute("INSERT INTO meta (key, value) VALUES ('records_version', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value WHERE value <> excluded.value", [RECORDS_VERSION])?;
        if changed > 0 {
            tx.event(
                "pulse",
                "schema.records",
                &format!("records_version={RECORDS_VERSION}"),
                "",
            )?;
        }
        tx.commit()
    }

    pub(crate) fn additive_complete(&self) -> bool {
        let Ok((have, ok)) = self.meta("records_version") else {
            return false;
        };
        if !ok || have != RECORDS_VERSION {
            return false;
        }
        let mut names = std::collections::BTreeSet::new();
        let re = Regex::new(r"(?i)CREATE\s+(?:TABLE|INDEX|TRIGGER)\s+IF\s+NOT\s+EXISTS\s+(\w+)")
            .unwrap();
        for list in additive_ddl() {
            for stmt in list {
                if let Some(c) = re.captures(stmt) {
                    names.insert(c[1].to_string());
                }
            }
        }
        let Ok(conn) = self.conn() else {
            return false;
        };
        let mut stmt = match conn
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table', 'index', 'trigger')")
        {
            Ok(s) => s,
            Err(_) => return false,
        };
        let rows = match stmt.query_map([], |r| r.get::<_, String>(0)) {
            Ok(r) => r,
            Err(_) => return false,
        };
        for row in rows {
            let Ok(name) = row else {
                return false;
            };
            names.remove(&name);
        }
        if !names.is_empty() {
            return false;
        }
        additive_columns()
            .iter()
            .all(|c| has_column(&conn, c.table, c.name).unwrap_or(false))
    }

    pub fn checkpoint(&self) -> Result<CheckpointResult> {
        let before = wal_size(&self.path);
        let conn = self.conn()?;
        let (busy, log, checkpointed): (i64, i64, i64) =
            conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        let mut out = CheckpointResult {
            busy: busy as i32,
            log: log as i32,
            checkpointed: checkpointed as i32,
            truncated: false,
            wal_bytes_before: before,
            wal_bytes_after: wal_size(&self.path),
        };
        if out.busy != 0 {
            return Ok(out);
        }
        let (busy, _, _): (i64, i64, i64) =
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        out.busy = busy as i32;
        out.wal_bytes_after = wal_size(&self.path);
        out.truncated = out.busy == 0 && before > out.wal_bytes_after;
        Ok(out)
    }

    pub fn meta(&self, key: &str) -> Result<(String, bool)> {
        let conn = self.conn()?;
        let v = conn
            .query_row("SELECT value FROM meta WHERE key = ?", [key], |r| r.get(0))
            .optional()?;
        Ok(v.map_or((String::new(), false), |v| (v, true)))
    }
}

pub struct Tx {
    store: Store,
    conn: PooledConn,
    done: bool,
    pending: Mutex<Vec<EventRecord>>,
}
impl Deref for Tx {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}
impl DerefMut for Tx {
    fn deref_mut(&mut self) -> &mut Connection {
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
        let text = detail.into().text()?;
        let rec = event_insert(&self.conn, &self.store.now(), actor, action, subject, &text)?;
        self.pending
            .lock()
            .expect("tx pending events poisoned")
            .push(rec);
        Ok(())
    }
    pub fn commit(mut self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        self.done = true;
        let pending = self
            .pending
            .lock()
            .expect("tx pending events poisoned")
            .clone();
        publish(&pending);
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
    pub truncated: bool,
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
        let obs = obs.lock().expect("event observers poisoned");
        for e in events {
            for f in obs.iter() {
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
    let abs = PathBuf::from(&home).canonicalize().or_else(|_| {
        let p = PathBuf::from(&home);
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
    is_inside(&exe, Path::new(home)) && !is_inside(&exe, &Path::new(home).join("bin"))
}
fn is_inside(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).is_ok()
}

pub fn create(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    if path.exists() {
        anyhow::bail!("{} already exists", path.display());
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
    RUNNER_COLUMNS
        .iter()
        .chain(HARNESS_COLUMNS.iter())
        .copied()
        .collect()
}

fn init_schema(conn: &Connection, at: &str) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<()> {
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
        let defaults = default_settings();
        for name in setting_names() {
            let value = defaults.get(&name).cloned().unwrap_or_default();
            conn.execute("INSERT OR IGNORE INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, 'init')", params![name, value, at])?;
        }
        conn.execute("INSERT OR IGNORE INTO agents (name, display, kind, status, created_at, notes) VALUES (?, ?, ?, ?, ?, ?)", params![HUMAN_AGENT, "the human", "human", "active", at, "the maintainer; records yes or no with `pulse decide`"])?;
        Ok(())
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
    while !dir.exists() && dir.pop() {}
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

pub(crate) fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
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

pub(crate) fn event_insert(
    conn: &Connection,
    at: &str,
    actor: &str,
    action: &str,
    subject: &str,
    detail: &str,
) -> Result<EventRecord> {
    conn.execute(
        "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
        params![at, actor, action, subject, detail],
    )?;
    Ok(EventRecord {
        seq: conn.last_insert_rowid(),
        at: at.to_string(),
        actor: actor.to_string(),
        action: action.to_string(),
        subject: subject.to_string(),
        detail: detail.to_string(),
    })
}

fn wal_size(path: &str) -> i64 {
    fs::metadata(format!("{path}-wal"))
        .map(|m| i64::try_from(m.len()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
