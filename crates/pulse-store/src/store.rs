use crate::ctx::CancellationToken;
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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

pub const SCHEMA_VERSION: i64 = 3;
pub const STORE_NAME: &str = "pulse.sqlite";
pub const HUMAN_AGENT: &str = "human";
/// Tools a worker or measured agent never gets, whatever the harness.
pub const DENIED_TOOLS: &[&str] = &["shell(git push)", "shell(git remote)", "shell(gh:*)"];
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
    /// Go: database/sql's pool - idle connections plus the accounting behind
    /// SetMaxOpenConns (idle + in-use, and the cap requests wait on; 0 = no
    /// limit). One mutex for both so borrowers and returners never deadlock.
    pool: Mutex<Pool>,
    freed: Condvar,
}

#[derive(Debug, Default)]
struct Pool {
    idle: Vec<Connection>,
    total: usize,
    max_open: usize,
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
            // A borrower's progress handler must not outlive the borrow.
            let _ = conn.progress_handler(0, None::<fn() -> bool>);
            let mut pool = self.store.inner.pool.lock().expect("store pool poisoned");
            if pool.idle.len() < MAX_POOL {
                pool.idle.push(conn);
            } else {
                pool.total = pool.total.saturating_sub(1);
            }
            drop(pool);
            self.store.inner.freed.notify_one();
        }
    }
}

/// Go: what a context-aware query returns once its context is done - the
/// context's error (database/sql and the sqlite driver both report ctx.Err()
/// rather than the interrupted statement). Other errors pass through.
pub fn ctx_error(ctx: &CancellationToken, err: anyhow::Error) -> anyhow::Error {
    match ctx.err() {
        Some(cause) => anyhow::Error::new(cause),
        None => err,
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
                pool: Mutex::new(Pool::default()),
                freed: Condvar::new(),
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
        Ok(PooledConn {
            store: self.clone(),
            conn: Some(self.acquire(&CancellationToken::new())?),
        })
    }

    /// Go: db.Conn(ctx) / QueryContext - a connection for one borrower. With a
    /// SetMaxOpenConns cap in force the caller waits for a free connection
    /// until `ctx` is done (then ctx.Err() comes back), and while it holds the
    /// connection a cancelled `ctx` interrupts the running statement, which
    /// surfaces as ctx.Err() too (see [`ctx_error`]).
    pub fn conn_ctx(&self, ctx: &CancellationToken) -> Result<PooledConn> {
        let conn = self.acquire(ctx)?;
        let watch = ctx.clone();
        conn.progress_handler(1000, Some(move || watch.is_cancelled()))?;
        Ok(PooledConn {
            store: self.clone(),
            conn: Some(conn),
        })
    }

    /// Go: db.SetMaxOpenConns(n) - at most n connections open at once, 0 for
    /// no limit; further borrowers wait (or give up when their context ends).
    pub fn set_max_open_conns(&self, n: usize) {
        self.inner
            .pool
            .lock()
            .expect("store pool poisoned")
            .max_open = n;
        self.inner.freed.notify_all();
    }

    fn acquire(&self, ctx: &CancellationToken) -> Result<Connection> {
        let mut pool = self.inner.pool.lock().expect("store pool poisoned");
        loop {
            if let Some(err) = ctx.err() {
                return Err(anyhow::Error::new(err));
            }
            if let Some(conn) = pool.idle.pop() {
                return Ok(conn);
            }
            if pool.max_open == 0 || pool.total < pool.max_open {
                pool.total += 1;
                drop(pool);
                return match self.open_conn() {
                    Ok(conn) => Ok(conn),
                    Err(e) => {
                        self.inner.pool.lock().expect("store pool poisoned").total -= 1;
                        self.inner.freed.notify_one();
                        Err(e)
                    }
                };
            }
            let wait = ctx
                .remaining()
                .unwrap_or(Duration::from_millis(50))
                .min(Duration::from_millis(50));
            pool = self
                .inner
                .freed
                .wait_timeout(pool, wait)
                .expect("store pool poisoned")
                .0;
        }
    }

    fn open_conn(&self) -> Result<Connection> {
        let flags = if self.inner.read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
        };
        let conn = Connection::open_with_flags(&self.inner.path, flags)?;
        configure_conn(&conn, self.inner.read_only)?;
        Ok(conn)
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
        let mut pool = self.inner.pool.lock().expect("store pool poisoned");
        let closed = pool.idle.len();
        pool.idle.clear();
        pool.total = pool.total.saturating_sub(closed);
        drop(pool);
        self.inner.freed.notify_all();
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
            .map_err(|e| {
                anyhow!(
                    "{} has no schema_version (not a PULSE store?): {e}",
                    self.path
                )
            })?;
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
    /// Go: a struct or map handed to `Event`, rendered by `PyJSON` (struct
    /// fields keep their declaration order; map keys are sorted).
    pub fn json<T: Serialize>(value: T) -> Result<Self> {
        Ok(Self::Text(crate::pyjson::py_json(value)?))
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

/// Go `t.UTC().Format(time.RFC3339Nano)`: the fraction keeps only its
/// significant digits and disappears entirely on whole seconds
/// (`2026-01-02T03:04:05.5Z`, `2026-01-02T03:04:05Z`), unlike chrono's
/// `SecondsFormat::Nanos`, which always prints nine digits.
pub fn rfc3339_nano<Tz: chrono::TimeZone>(t: chrono::DateTime<Tz>) -> String {
    let t = t.with_timezone(&Utc);
    let mut s = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let nanos = t.timestamp_subsec_nanos();
    if nanos != 0 {
        let frac = format!("{nanos:09}");
        s.push('.');
        s.push_str(frac.trim_end_matches('0'));
    }
    s.push('Z');
    s
}

pub fn resolve_home(explicit: &str) -> Result<String> {
    let mut home = if explicit.is_empty() {
        std::env::var("PULSE_HOME").unwrap_or_default()
    } else {
        explicit.to_string()
    };
    if home.is_empty() {
        home = home::home_dir()
            .ok_or_else(|| anyhow!("cannot find the home directory: home directory not found"))?
            .join(".pulse")
            .display()
            .to_string();
    }
    if home.starts_with('~') {
        let user = home::home_dir()
            .ok_or_else(|| anyhow!("cannot find the home directory: home directory not found"))?;
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
    match crate::filepath::rel(&root.display().to_string(), &path.display().to_string()) {
        Ok(rel) => rel == "." || (rel != ".." && !rel.starts_with("../")),
        Err(_) => false,
    }
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
            pool: Mutex::new(Pool {
                idle: vec![conn],
                total: 1,
                max_open: 0,
            }),
            freed: Condvar::new(),
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

pub(crate) fn init_schema(conn: &Connection, at: &str) -> Result<()> {
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

pub(crate) fn configure_conn(conn: &Connection, read_only: bool) -> Result<()> {
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
    let top = git_top_level(&dir);
    if !top.is_empty() {
        return Err(input_err(format!(
            "The store {}/{} must live outside the project {}; set PULSE_HOME elsewhere",
            home, STORE_NAME, top
        )));
    }
    Ok(())
}

fn git_top_level(dir: &Path) -> String {
    let out = std::process::Command::new(pulse_gitenv::git_program())
        .arg("-C")
        .arg(dir)
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
        return String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    String::new()
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

pub(crate) fn wal_size(path: &str) -> i64 {
    fs::metadata(format!("{path}-wal"))
        .map(|m| i64::try_from(m.len()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::ContextError;
    use crate::py_json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;
    use std::thread;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn set_env(k: &str, v: &str) {
        unsafe { std::env::set_var(k, v) }
    }
    fn remove_env(k: &str) {
        unsafe { std::env::remove_var(k) }
    }
    struct TestStore {
        _dir: Option<tempfile::TempDir>,
        store: Store,
    }
    impl std::ops::Deref for TestStore {
        type Target = Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }
    fn open_temp(o: Options) -> TestStore {
        let dir = if o.home.is_empty() {
            Some(tempfile::tempdir().unwrap())
        } else {
            None
        };
        let home = dir
            .as_ref()
            .map(|d| d.path().display().to_string())
            .unwrap_or_else(|| o.home.clone());
        let store = Store::open(Options { home, ..o }).unwrap();
        TestStore { _dir: dir, store }
    }
    fn table_exists(s: &Store, name: &str) -> bool {
        let conn = s.conn().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        n == 1
    }
    fn count_events(s: &Store, action: &str) -> i64 {
        let conn = s.conn().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE action = ?",
            [action],
            |r| r.get(0),
        )
        .unwrap()
    }
    fn write_wal(s: &Store) -> i64 {
        let mut conn = s.conn().unwrap();
        conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        let tx = conn.transaction().unwrap();
        let payload = "x".repeat(8192);
        for _ in 0..1200 {
            tx.execute(
                "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'test', 'checkpoint.write', 'wal', ?)",
                rusqlite::params!["2026-09-06T10:00:00Z", payload],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let size = wal_size(&s.path);
        assert!(size > 0, "test did not create a WAL");
        size
    }

    #[test]
    fn ddl_matches_captured_schema() {
        let _serial = crate::test_helpers::serial();
        let Ok(want) = fs::read_to_string("/tmp/pulse-parity/schema.sql") else {
            return;
        };
        assert_eq!(want, DDL);
    }

    #[test]
    fn create_and_reopen() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let home = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T10:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(s.path, store_path(home.path().to_str().unwrap()).unwrap());
        assert_eq!(s.meta("schema_version").unwrap(), ("3".into(), true));
        assert_eq!(s.meta("created_at").unwrap().0, "2026-09-06T10:00:00Z");
        let settings = s.settings().unwrap();
        assert_eq!(settings.len(), crate::DEFAULT_SETTINGS.len());
        assert_eq!(settings["landing"], "agents");
        assert_eq!(settings["amend_majority"], "2/3");
        let human = s.agent(HUMAN_AGENT).unwrap().unwrap();
        assert_eq!(human.kind, "human");
        assert_eq!(human.display, "the human");
        assert!(table_exists(&s, "records"));
        assert!(table_exists(&s, "record_revisions"));
        assert_eq!(s.meta("records_version").unwrap().0, RECORDS_VERSION);
        assert_eq!(count_events(&s, "schema.records"), 1);
        s.close().unwrap();
        for _ in 0..2 {
            let again = Store::open(Options {
                home: home.path().display().to_string(),
                ..Options::default()
            })
            .unwrap();
            assert_eq!(count_events(&again, "schema.records"), 1);
            again.ensure_records().unwrap();
            assert_eq!(count_events(&again, "schema.records"), 1);
        }
    }

    #[test]
    fn open_sets_journal_size_limit() {
        let _serial = crate::test_helpers::serial();
        let s = open_temp(Options {
            create: true,
            ..Options::default()
        });
        let got: i64 = s
            .conn()
            .unwrap()
            .query_row("PRAGMA journal_size_limit", [], |r| r.get(0))
            .unwrap();
        assert_eq!(got, JOURNAL_SIZE_LIMIT_BYTES);
    }

    #[test]
    fn open_sets_busy_timeout() {
        let _serial = crate::test_helpers::serial();
        let s = open_temp(Options {
            create: true,
            ..Options::default()
        });
        let got: i64 = s
            .conn()
            .unwrap()
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(got, BUSY_TIMEOUT_MS as i64);
    }

    // The workspace .cargo/config.toml compiles SQLite without the unified
    // page cache and without memory statistics; both put every connection in
    // the process behind one mutex, which the hub's parallel snapshots
    // contend on. A build that loses those flags is slower, not wrong, so
    // this is the only place that would notice.
    #[test]
    fn bundled_sqlite_has_no_process_wide_mutexes() {
        let _serial = crate::test_helpers::serial();
        let s = open_temp(Options {
            create: true,
            ..Options::default()
        });
        let conn = s.conn().unwrap();
        let used = |opt: &str| -> i64 {
            conn.query_row("SELECT sqlite_compileoption_used(?)", [opt], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            used("ENABLE_MEMORY_MANAGEMENT"),
            0,
            "SQLite was built with SQLITE_ENABLE_MEMORY_MANAGEMENT: is LIBSQLITE3_FLAGS from .cargo/config.toml reaching the libsqlite3-sys build?"
        );
        assert_eq!(
            used("DEFAULT_MEMSTATUS=0"),
            1,
            "SQLite was built without SQLITE_DEFAULT_MEMSTATUS=0: is LIBSQLITE3_FLAGS from .cargo/config.toml reaching the libsqlite3-sys build?"
        );
    }

    #[test]
    fn max_open_conns_waits_until_the_context_is_done() {
        let _serial = crate::test_helpers::serial();
        let s = open_temp(Options {
            create: true,
            ..Options::default()
        });
        s.set_max_open_conns(1);
        let held = s.conn().unwrap();
        let ctx = CancellationToken::new();
        let waiter = {
            let s = s.store.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                s.conn_ctx(&ctx)
                    .map(|_| ())
                    .map_err(|e| ContextError::of(&e))
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !waiter.is_finished(),
            "a second connection opened past the cap"
        );
        ctx.cancel();
        let started = std::time::Instant::now();
        let got = waiter.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(got, Err(Some(ContextError::Canceled)));
        drop(held);
        let deadline = CancellationToken::new().with_timeout(Duration::from_secs(5));
        let ok = s.conn_ctx(&deadline).unwrap();
        let one: i64 = ok.query_row("SELECT 1", [], |r| r.get(0)).unwrap();
        assert_eq!(one, 1);
    }

    #[test]
    fn cancelled_context_interrupts_a_running_statement() {
        let _serial = crate::test_helpers::serial();
        let s = open_temp(Options {
            create: true,
            ..Options::default()
        });
        let ctx = CancellationToken::new();
        let conn = s.conn_ctx(&ctx).unwrap();
        let canceller = {
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                ctx.cancel();
            })
        };
        let started = std::time::Instant::now();
        let result: rusqlite::Result<i64> = conn.query_row(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200000000) SELECT COUNT(*) FROM n",
            [],
            |r| r.get(0),
        );
        canceller.join().unwrap();
        assert!(result.is_err(), "statement ran to completion");
        assert!(started.elapsed() < Duration::from_secs(5));
        let err = ctx_error(&ctx, anyhow::Error::new(result.unwrap_err()));
        assert!(ContextError::is(&err, ContextError::Canceled), "{err}");
        assert_eq!(err.to_string(), "context canceled");
    }

    #[test]
    fn checkpoint_truncates_wal() {
        let _serial = crate::test_helpers::serial();
        let s = open_temp(Options {
            create: true,
            ..Options::default()
        });
        let before = write_wal(&s);
        let got = s.checkpoint().unwrap();
        assert_eq!(got.busy, 0, "checkpoint reported busy: {got:?}");
        assert!(
            got.truncated,
            "checkpoint did not report truncation after {before}: {got:?}"
        );
        assert!(wal_size(&s.path) <= 4096);
    }

    #[test]
    fn open_missing_store() {
        let _serial = crate::test_helpers::serial();
        let home = tempfile::tempdir().unwrap();
        let err = Store::open(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        })
        .unwrap_err();
        let nf = err.downcast_ref::<NotFoundError>().unwrap();
        assert_eq!(nf.path, store_path(home.path().to_str().unwrap()).unwrap());
        assert!(!Path::new(&nf.path).exists());
    }

    #[test]
    fn worker_guard_skips_records() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let home = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            skip_records: true,
            ..Options::default()
        })
        .unwrap();
        assert!(!table_exists(&s, "records"));
        s.close().unwrap();
        set_env("PULSE_JOB", "job-1");
        assert!(worker_guard(home.path().to_str().unwrap()));
        let w = Store::open(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        })
        .unwrap();
        assert!(!table_exists(&w, "records"));
        assert!(
            Store::open(Options {
                home: tempfile::tempdir().unwrap().path().display().to_string(),
                create: true,
                ..Options::default()
            })
            .unwrap_err()
            .downcast_ref::<NotFoundError>()
            .is_some()
        );
        set_env("PULSE_JOB", "");
        let n = Store::open(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        })
        .unwrap();
        assert!(table_exists(&n, "records"));
    }

    #[test]
    fn worker_guard_executable() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        let dir = exe.parent().unwrap();
        if dir.file_name().is_some_and(|n| n == "bin") {
            return;
        }
        assert!(worker_guard(dir.to_str().unwrap()));
        assert!(!worker_guard(
            tempfile::tempdir().unwrap().path().to_str().unwrap()
        ));
        let bin_home = tempfile::tempdir().unwrap();
        fs::create_dir(bin_home.path().join("bin")).unwrap();
        assert!(is_inside(
            &bin_home.path().join("bin/pulse"),
            bin_home.path()
        ));
    }

    #[test]
    fn schema_version_checks() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        for (version, want) in [
            (
                "4",
                "Store schema 4 is newer than this tool (3); update pulse",
            ),
            (
                "2",
                "Store schema 2 is older than this tool (3); run `python3 legacy/pulse.py status` once to migrate it",
            ),
        ] {
            let home = tempfile::tempdir().unwrap();
            let s = open_temp(Options {
                home: home.path().display().to_string(),
                create: true,
                ..Options::default()
            });
            s.conn()
                .unwrap()
                .execute(
                    "UPDATE meta SET value = ? WHERE key = 'schema_version'",
                    [version],
                )
                .unwrap();
            s.close().unwrap();
            let err = Store::open(Options {
                home: home.path().display().to_string(),
                ..Options::default()
            })
            .unwrap_err();
            assert_eq!(err.to_string(), want);
        }
    }

    #[test]
    fn create_refuses_git_checkout() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let dir = tempfile::tempdir().unwrap();
        let out = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["init", "-q"])
            .output();
        if out.is_err() || !out.unwrap().status.success() {
            return;
        }
        let home = dir.path().join("nested/.pulse");
        let err = Store::open(Options {
            home: home.display().to_string(),
            create: true,
            ..Options::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("must live outside the project"));
        assert!(err.downcast_ref::<InputError>().is_some());
    }

    #[test]
    fn rfc3339_nano_trims_like_go() {
        let _serial = crate::test_helpers::serial();
        let base = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(super::rfc3339_nano(base), "2026-01-02T03:04:05Z");
        assert_eq!(
            super::rfc3339_nano(base + chrono::Duration::milliseconds(500)),
            "2026-01-02T03:04:05.5Z"
        );
        assert_eq!(
            super::rfc3339_nano(base + chrono::Duration::nanoseconds(120_000_000)),
            "2026-01-02T03:04:05.12Z"
        );
        assert_eq!(
            super::rfc3339_nano(base + chrono::Duration::nanoseconds(123_456_789)),
            "2026-01-02T03:04:05.123456789Z"
        );
        assert_eq!(
            super::rfc3339_nano(base + chrono::Duration::nanoseconds(1_000)),
            "2026-01-02T03:04:05.000001Z"
        );
    }

    #[test]
    fn now() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        remove_env("PULSE_NOW");
        let s = open_temp(Options {
            create: true,
            ..Options::default()
        });
        let got = s.now();
        assert_eq!(got.len(), 20);
        assert_eq!(&got[10..11], "T");
        assert_eq!(&got[19..20], "Z");
        set_env("PULSE_NOW", "2030-01-02T03:04:05Z");
        assert_eq!(s.now(), "2030-01-02T03:04:05Z");
        let fixed = open_temp(Options {
            create: true,
            now: "2001-01-01T00:00:00Z".into(),
            ..Options::default()
        });
        assert_eq!(fixed.now(), "2001-01-01T00:00:00Z");
        remove_env("PULSE_NOW");
    }

    #[test]
    fn event_and_py_json() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let s = open_temp(Options {
            create: true,
            now: "2026-09-06T10:00:00Z".into(),
            ..Options::default()
        });
        s.event(
            "tester",
            "unit.test",
            "subject",
            serde_json::json!({"z":1,"a":["x","y"],"m":{"k":true,"n":null}}),
        )
        .unwrap();
        s.event("tester", "unit.text", "", "plain text").unwrap();
        s.event("tester", "unit.nil", "", Option::<&str>::None)
            .unwrap();
        let conn = s.conn().unwrap();
        let mut stmt = conn.prepare("SELECT at, actor, action, subject, detail FROM events WHERE actor = 'tester' ORDER BY seq").unwrap();
        let got: Vec<String> = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{}|{}|{}|{}|{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            got,
            vec![
                r#"2026-09-06T10:00:00Z|tester|unit.test|subject|{"a": ["x", "y"], "m": {"k": true, "n": null}, "z": 1}"#,
                "2026-09-06T10:00:00Z|tester|unit.text||plain text",
                "2026-09-06T10:00:00Z|tester|unit.nil||",
            ]
        );
        assert_eq!(
            py_json("héllo\n\"q\" <b> \\").unwrap(),
            r#""h\u00e9llo\n\"q\" <b> \\""#
        );
        assert_eq!(py_json("😀").unwrap(), r#""\ud83d\ude00""#);
    }

    #[test]
    fn settings() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let s = open_temp(Options {
            create: true,
            now: "2026-09-06T10:00:00Z".into(),
            ..Options::default()
        });
        assert_eq!(s.setting("quorum", ""), "2");
        assert_eq!(s.setting("nope", "fallback"), "fallback");
        s.set_setting("human", "landing", "Human").unwrap();
        assert_eq!(s.setting("landing", ""), "human");
        let row: (String, String) = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT updated_by, updated_at FROM settings WHERE name = 'landing'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("human".into(), "2026-09-06T10:00:00Z".into()));
        let ev: (String, String) = s.conn().unwrap().query_row("SELECT subject, detail FROM events WHERE action = 'setting' ORDER BY seq DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(ev, ("landing".into(), "human".into()));
        s.set_setting("human", "jobs_per_day", " OFF ").unwrap();
        assert_eq!(s.setting("jobs_per_day", ""), "off");
        s.set_setting("human", "amend_majority", "3 / 4").unwrap();
        assert_eq!(s.setting("amend_majority", ""), "3/4");
        for (key, value, want) in [
            (
                "bogus",
                "1",
                "Unknown setting 'bogus'. Known: amend_majority, amend_quorum, claim_hours, expiry_days, jobs_per_day, landing, landing.parallel, max_ai_credits, minutes_per_job, parallel_per_project, prune_after_days, quorum, records.auto_adopt, runner_cmd, stale_days, stop, worker_model, workers",
            ),
            ("quorum", "x", "quorum takes a whole number, not 'x'"),
            (
                "jobs_per_day",
                "x",
                "jobs_per_day takes a whole number, or one of none/off/unlimited, not 'x'",
            ),
            ("quorum", "0", "quorum must be at least 1, not 0"),
            (
                "max_ai_credits",
                "10",
                "max_ai_credits must be at least 30, not 10",
            ),
            (
                "amend_majority",
                "5/3",
                "amend_majority is a share of the votes: the numerator cannot exceed the denominator (5/3)",
            ),
            (
                "amend_majority",
                "abc",
                "A majority must be a fraction such as 2/3, not 'abc'",
            ),
            (
                "landing",
                "nobody",
                "landing takes one of agents, human, not 'nobody'",
            ),
            ("stop", "yes", "stop takes one of 0, 1, not 'yes'"),
            ("runner_cmd", "  ", "runner_cmd cannot be empty"),
        ] {
            let err = s.set_setting("human", key, value).unwrap_err();
            assert_eq!(err.to_string(), want);
            assert!(err.downcast_ref::<InputError>().is_some());
        }
    }

    #[test]
    fn projects_and_agents() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let s = open_temp(Options {
            create: true,
            now: "2026-09-06T10:00:00Z".into(),
            ..Options::default()
        });
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("a/b");
        fs::create_dir_all(&sub).unwrap();
        s.conn().unwrap().execute("INSERT INTO projects (slug, name, path, brain, added_at) VALUES ('brain', 'PULSE', ?, 1, ?), ('other', 'Other', ?, 0, ?)", rusqlite::params![root.path().display().to_string(), s.now(), root.path().join("a").display().to_string(), s.now()]).unwrap();
        let all = s.projects(false).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].slug, "brain");
        assert!(all[0].brain);
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET status = 'inactive' WHERE slug = 'other'",
                [],
            )
            .unwrap();
        assert_eq!(s.projects(true).unwrap().len(), 1);
        assert_eq!(s.brain().unwrap().unwrap().slug, "brain");
        assert!(s.project("nope").unwrap().is_none());
        assert_eq!(
            s.project_for_dir(sub.to_str().unwrap())
                .unwrap()
                .unwrap()
                .slug,
            "other"
        );
        assert!(
            s.project_for_dir(tempfile::tempdir().unwrap().path().to_str().unwrap())
                .unwrap()
                .is_none()
        );
        s.conn().unwrap().execute("INSERT INTO agents (name, runner, model, created_at, sessions, posts, status) VALUES ('ember', 'copilot', 'claude', ?, '[\"s1\",\"s2\"]', 3, 'pruned')", [s.now()]).unwrap();
        let a = s.agent("ember").unwrap().unwrap();
        assert_eq!(a.sessions, vec!["s1", "s2"]);
        assert_eq!(a.posts, 3);
        assert!(s.agent("nobody").unwrap().is_none());
        let agents = s.agents("").unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(s.agents("active").unwrap()[0].name, HUMAN_AGENT);
    }

    #[test]
    fn schema_parity_with_python() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        remove_env("PULSE_NOW");
        let script = Path::new("legacy").join("pulse.py");
        if !script.exists() {
            return;
        }
        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let repo = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q", "--initial-branch=main"],
            vec![
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            let out = Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .envs(pulse_testutil::git_env().into_iter().filter_map(|kv| {
                    kv.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                }))
                .output()
                .unwrap();
            if !out.status.success() {
                return;
            }
        }
        let py_home = tempfile::tempdir().unwrap();
        let out = Command::new("python3")
            .arg(&script)
            .arg("--home")
            .arg(py_home.path())
            .args(["init", "--path"])
            .arg(repo.path())
            .args(["--name", "parity"])
            .env("PULSE_AGENT", "human")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "python init: {}\n{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        let py = open_temp(Options {
            home: py_home.path().display().to_string(),
            skip_records: true,
            ..Options::default()
        });
        let rust = open_temp(Options {
            create: true,
            skip_records: true,
            ..Options::default()
        });
        let dump = |s: &Store| -> String {
            let conn = s.conn().unwrap();
            let mut stmt = conn.prepare("SELECT type, name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name").unwrap();
            stmt.query_map([], |r| {
                Ok(format!(
                    "{} {}\n{}\n",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?.unwrap_or_default()
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("")
        };
        let a = dump(&py);
        let b = dump(&rust);
        assert_eq!(
            a, b,
            "sqlite_master differs\n--- python ---\n{a}\n--- go ---\n{b}"
        );
        let py_settings = py.settings().unwrap();
        let rust_settings = rust.settings().unwrap();
        assert_eq!(
            py_settings.len(),
            rust_settings.len(),
            "settings rows: python {}, go {}",
            py_settings.len(),
            rust_settings.len()
        );
        for (k, v) in py_settings {
            assert_eq!(rust_settings.get(&k), Some(&v), "setting {k}");
        }
        let mut py_human = py.agent(HUMAN_AGENT).unwrap().unwrap();
        let mut rust_human = rust.agent(HUMAN_AGENT).unwrap().unwrap();
        py_human.created_at.clear();
        rust_human.created_at.clear();
        assert!(
            py_human.display == rust_human.display
                && py_human.kind == rust_human.kind
                && py_human.notes == rust_human.notes
                && py_human.status == rust_human.status,
            "human agent: python {py_human:?}, go {rust_human:?}"
        );
        {
            let key = "schema_version";
            let a = py.meta(key).unwrap().0;
            let b = rust.meta(key).unwrap().0;
            assert_eq!(a, b, "meta {key}");
        }
    }

    #[test]
    fn read_only_open() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let home = tempfile::tempdir().unwrap();
        let s = open_temp(Options {
            home: home.path().display().to_string(),
            create: true,
            ..Options::default()
        });
        s.close().unwrap();
        let ro = Store::open(Options {
            home: home.path().display().to_string(),
            read_only: true,
            ..Options::default()
        })
        .unwrap();
        assert_eq!(ro.meta("schema_version").unwrap().0, "3");
        assert!(ro.event("t", "x", "", Option::<&str>::None).is_err());
        assert!(
            Store::open(Options {
                home: tempfile::tempdir().unwrap().path().display().to_string(),
                read_only: true,
                create: true,
                ..Options::default()
            })
            .is_err()
        );
    }

    #[test]
    fn concurrent_open() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let home = tempfile::tempdir().unwrap();
        Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            skip_records: true,
            ..Options::default()
        })
        .unwrap()
        .close()
        .unwrap();
        let mut handles = Vec::new();
        for _ in 0..12 {
            let home = home.path().display().to_string();
            handles.push(thread::spawn(move || -> anyhow::Result<()> {
                let s = Store::open(Options {
                    home,
                    ..Options::default()
                })?;
                s.event("worker", "unit.concurrent", "", Option::<&str>::None)
            }));
        }
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(count_events(&s, "schema.records"), 1);
        assert_eq!(count_events(&s, "unit.concurrent"), 12);
    }

    #[test]
    fn ensure_records_upgrades() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let home = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            ..Options::default()
        })
        .unwrap();
        for table in [
            "record_revisions",
            "tasks",
            "hooks",
            "briefs",
            "harness_sessions",
            "harness_steps",
        ] {
            assert!(table_exists(&s, table));
        }
        s.conn()
            .unwrap()
            .execute("DROP TABLE record_revisions", [])
            .unwrap();
        s.close().unwrap();
        let again = open_temp(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        });
        assert!(table_exists(&again, "record_revisions"));
        assert_eq!(count_events(&again, "schema.records"), 1);
        again
            .conn()
            .unwrap()
            .execute(
                "UPDATE meta SET value = '0' WHERE key = 'records_version'",
                [],
            )
            .unwrap();
        again.close().unwrap();
        let third = Store::open(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(third.meta("records_version").unwrap().0, RECORDS_VERSION);
        assert_eq!(count_events(&third, "schema.records"), 2);
    }

    #[test]
    fn on_event_publishes_after_commit() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let s = open_temp(Options {
            create: true,
            ..Options::default()
        });
        let got = std::sync::Arc::new(Mutex::new(Vec::<EventRecord>::new()));
        let sink = got.clone();
        on_event(move |e| sink.lock().unwrap().push(e));
        s.event(
            "tester",
            "auto.commit",
            "x",
            serde_json::json!({"b":1,"a":"z"}),
        )
        .unwrap();
        assert!(
            got.lock()
                .unwrap()
                .iter()
                .any(|e| e.action == "auto.commit" && e.detail == r#"{"a": "z", "b": 1}"#)
        );
        let tx = s.begin().unwrap();
        tx.event("tester", "rolled.back", "x", Option::<&str>::None)
            .unwrap();
        tx.rollback().unwrap();
        assert!(
            !got.lock()
                .unwrap()
                .iter()
                .any(|e| e.action == "rolled.back")
        );
        let tx = s.begin().unwrap();
        tx.event("tester", "committed", "x", Option::<&str>::None)
            .unwrap();
        tx.commit().unwrap();
        s.set_setting("human", "landing", "human").unwrap();
        let events = got.lock().unwrap();
        assert!(events.iter().any(|e| e.action == "committed"));
        assert!(events.iter().any(|e| e.action == "setting"));
        let n: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = 'rolled.back'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn events_indexes_arrive_at_open_and_serve_job_history_queries() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        set_env("PULSE_JOB", "");
        let home = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            ..Options::default()
        })
        .unwrap();
        let index_exists = |st: &Store, name: &str| -> bool {
            st.conn()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?",
                    [name],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
                == 1
        };
        assert!(index_exists(&s, "events_action_subject"));
        assert!(index_exists(&s, "events_subject_seq"));
        s.conn()
            .unwrap()
            .execute("DROP INDEX events_action_subject", [])
            .unwrap();
        s.close().unwrap();
        let again = Store::open(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        })
        .unwrap();
        assert!(index_exists(&again, "events_action_subject"));
        let plan = |query: &str| -> String {
            let conn = again.conn().unwrap();
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join("\n")
        };
        let got = plan(
            "SELECT e.detail FROM events e WHERE e.action = 'job.recover' AND e.subject = 'job-1' AND e.at >= '2026' AND NOT EXISTS (SELECT 1 FROM events later WHERE later.subject = e.subject AND later.seq > e.seq AND later.action IN ('job.lease', 'job.failed')) ORDER BY e.seq DESC LIMIT 1",
        );
        assert!(
            got.contains("USING INDEX events_action_subject") && !got.contains("SCAN e"),
            "recovery decision plan:\n{got}"
        );
        let got = plan(
            "SELECT detail FROM events WHERE action = 'executor.login' AND subject = 'local' ORDER BY seq DESC LIMIT 1",
        );
        assert!(
            got.contains("USING INDEX events_action_subject"),
            "executor history plan:\n{got}"
        );
    }

    #[test]
    fn git_top_level_ignores_hook_git_dir() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        let hook = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        for dir in [hook.path(), project.path()] {
            let status = Command::new("git")
                .arg("init")
                .arg("-q")
                .arg(dir)
                .envs(pulse_testutil::git_env().into_iter().filter_map(|kv| {
                    kv.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                }))
                .status();
            if status.is_err() || !status.unwrap().success() {
                return;
            }
        }
        set_env("GIT_DIR", hook.path().join(".git").to_str().unwrap());
        set_env("GIT_WORK_TREE", hook.path().to_str().unwrap());
        set_env(
            "GIT_INDEX_FILE",
            hook.path().join(".git/index").to_str().unwrap(),
        );
        let sub = project.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        let got = PathBuf::from(git_top_level(&sub)).canonicalize().unwrap();
        assert_eq!(got, project.path().canonicalize().unwrap());
        remove_env("GIT_DIR");
        remove_env("GIT_WORK_TREE");
        remove_env("GIT_INDEX_FILE");
    }
}
