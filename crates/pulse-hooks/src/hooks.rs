use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use pulse_bus::{Bus, CancellationToken, Event, Filter};
use pulse_store::Store;
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);
pub const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(10);
pub const RETRY_INTERVAL: Duration = Duration::from_secs(30);
pub const MAX_ATTEMPTS: i32 = 3;

const RETRY_BACKOFF_SECS: &[i64] = &[60, 5 * 60, 15 * 60];
const HOOK_COLS: &str = "id, event, kind, target, project, enabled, created_by, created_at, last_fired_at, last_status, fires";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hook {
    pub id: i64,
    pub event: String,
    pub kind: String,
    pub target: String,
    pub project: String,
    pub enabled: bool,
    pub created_by: String,
    pub created_at: String,
    pub last_fired_at: String,
    pub last_status: String,
    pub fires: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: i64,
    pub hook_id: i64,
    pub event_seq: i64,
    pub event_action: String,
    pub project: String,
    pub attempt: i32,
    pub status: String,
    pub detail: String,
    pub at: String,
    pub next_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryState {
    pub pending: i32,
    pub last_error: String,
}

/// What a `job` hook queues, kept as JSON in the hook's `target`. In the
/// title and the brief, `{{subject}}`, `{{detail}}`, `{{action}}`,
/// `{{project}}`, `{{actor}}` and `{{at}}` stand for the event's fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobTemplate {
    pub kind: String,
    pub title: String,
    pub brief: String,
    pub tier: i64,
    pub priority: i64,
    /// The custom agent the job runs as (`--run-as`); empty lets the
    /// runner choose.
    #[serde(default)]
    pub agent: String,
}

impl JobTemplate {
    pub fn parse(target: &str) -> Result<Self> {
        serde_json::from_str(target).with_context(|| format!("job hook target {target:?}"))
    }
}

/// Characters of an event's detail a `{{detail}}` in a job template takes.
const DETAIL_LIMIT: usize = 4000;

/// Fills a job template's text from the event: each `{{name}}` of the
/// fields above is replaced in one pass, anything else stays as written.
pub fn fill(template: &str, e: &Event) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start..].find("}}") else {
            break;
        };
        let key = &rest[start + 2..start + len];
        let value = match key.trim() {
            "subject" => Some(e.subject.clone()),
            "detail" => Some(if e.detail.len() > DETAIL_LIMIT {
                e.detail[..e.detail.floor_char_boundary(DETAIL_LIMIT)].to_string()
            } else {
                e.detail.clone()
            }),
            "action" => Some(e.action.clone()),
            "project" => Some(e.project()),
            "actor" => Some(e.actor.clone()),
            "at" => Some(e.at.clone()),
            _ => None,
        };
        out.push_str(&rest[..start]);
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    pub hook: i64,
    pub id: i64,
    pub at: String,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub detail: String,
    pub project: String,
}

fn cli_error(
    code: i32,
    what: impl Into<String>,
    why: impl Into<String>,
    fix: impl Into<String>,
) -> anyhow::Error {
    pulse_cli::Error {
        what: what.into(),
        why: why.into(),
        fix: fix.into(),
        code,
    }
    .into()
}

fn scan_hook(row: &Row<'_>) -> rusqlite::Result<Hook> {
    let enabled: i32 = row.get(5)?;
    Ok(Hook {
        id: row.get(0)?,
        event: row.get(1)?,
        kind: row.get(2)?,
        target: row.get(3)?,
        project: row.get(4)?,
        enabled: enabled != 0,
        created_by: row.get(6)?,
        created_at: row.get(7)?,
        last_fired_at: row.get(8)?,
        last_status: row.get(9)?,
        fires: row.get(10)?,
    })
}

fn scan_delivery(row: &Row<'_>) -> rusqlite::Result<Delivery> {
    Ok(Delivery {
        id: row.get(0)?,
        hook_id: row.get(1)?,
        event_seq: row.get(2)?,
        event_action: row.get(3)?,
        project: row.get(4)?,
        attempt: row.get(5)?,
        status: row.get(6)?,
        detail: row.get(7)?,
        at: row.get(8)?,
        next_at: row.get(9)?,
    })
}

fn valid_glob(pattern: &str) -> std::result::Result<(), String> {
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' {
            let mut ok = false;
            for c in chars.by_ref() {
                if c == ']' {
                    ok = true;
                    break;
                }
            }
            if !ok {
                return Err("syntax error in pattern".to_string());
            }
        }
    }
    Ok(())
}

pub fn add(
    s: &Store,
    event: &str,
    kind: &str,
    target: &str,
    project: &str,
    actor: &str,
) -> Result<Hook> {
    let event = event.trim();
    if event.is_empty() {
        return Err(cli_error(
            2,
            "An event glob is needed",
            "hooks fire on actions such as post, task.*, compute.approved or *",
            "pulse hook add 'task.*' --script PATH",
        ));
    }
    if let Err(e) = valid_glob(event) {
        return Err(cli_error(
            2,
            format!("Bad event glob {event:?}"),
            e,
            "use * and ? only, e.g. task.* or post",
        ));
    }
    match kind {
        "script" => {
            if target.is_empty() {
                return Err(cli_error(
                    2,
                    "A script path is needed",
                    "--script names the executable the hook runs",
                    "pulse hook add EVENT --script /path/to/hook.sh",
                ));
            }
            if std::fs::metadata(target).map_or(true, |m| m.is_dir()) {
                return Err(cli_error(
                    2,
                    format!("No script at {target}"),
                    "the path must exist when the hook is added",
                    "chmod +x the script and pass its absolute path",
                ));
            }
        }
        "webhook" => {
            if !target.starts_with("http://") && !target.starts_with("https://") {
                return Err(cli_error(
                    2,
                    "A webhook needs an http(s) URL",
                    format!("got {target:?}"),
                    "pulse hook add EVENT --url https://example.test/hook",
                ));
            }
        }
        "job" => check_job_template(s, target, project)?,
        _ => {
            return Err(cli_error(
                2,
                "A hook is a --script PATH, a --url URL or a --job KIND",
                "pass exactly one",
                "pulse help hook",
            ));
        }
    }
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "INSERT INTO hooks (event, kind, target, project, enabled, created_by, created_at) VALUES (?, ?, ?, ?, 1, ?, ?)",
        params![event, kind, target, project, actor, now],
    )?;
    let id = tx.last_insert_rowid();
    tx.event(
        actor,
        "hook.added",
        &id.to_string(),
        pulse_store::Detail::json(
            serde_json::json!({"event": event, "kind": kind, "target": target, "project": project}),
        )?,
    )?;
    tx.commit()?;
    get(s, id)
}

/// A `job` hook's template must name a kind a schedule may queue, a title,
/// a tier the runner takes, an agent that loads, and a project: not every
/// event names one, and a job belongs to one.
fn check_job_template(s: &Store, target: &str, project: &str) -> Result<()> {
    let kinds = pulse_tasks::schedules::SCHEDULE_KINDS;
    let t = JobTemplate::parse(target)?;
    if !kinds.contains(&t.kind.as_str()) {
        return Err(cli_error(
            2,
            format!("A hook cannot queue a {} job", t.kind),
            format!("a worker runs {} from a brief alone", kinds.join(", ")),
            "pulse hook add EVENT --job task --title T --brief B --project P",
        ));
    }
    if t.title.trim().is_empty() {
        return Err(cli_error(
            2,
            "A job hook needs a title",
            "--title names the job it queues; {{subject}} and {{action}} stand for the event's",
            "pulse hook add EVENT --job task --title 'Look at {{subject}}' --brief B --project P",
        ));
    }
    if t.tier != 0 && t.tier != 1 {
        return Err(cli_error(
            2,
            "The runner takes Tier 0 and Tier 1 jobs only",
            format!("got --tier {}", t.tier),
            "--tier 0 or --tier 1",
        ));
    }
    if project.is_empty() {
        return Err(cli_error(
            2,
            "A job hook needs --project",
            "not every event names its project, and a job belongs to one",
            "pulse hook add EVENT --job KIND --title T --brief B --project SLUG",
        ));
    }
    if s.project(project)?.is_none() {
        return Err(cli_error(
            2,
            format!("No project {project:?}"),
            "the job would have nowhere to run",
            "pulse projects",
        ));
    }
    pulse_board::check_run_as(s, project, &t.agent)?;
    Ok(())
}

pub fn get(s: &Store, id: i64) -> Result<Hook> {
    let conn = s.conn()?;
    let found = conn
        .query_row(
            &format!("SELECT {HOOK_COLS} FROM hooks WHERE id = ?"),
            [id],
            scan_hook,
        )
        .optional()?;
    found.ok_or_else(|| {
        cli_error(
            2,
            format!("No hook {id}"),
            "the id is not in this store",
            "pulse hook list",
        )
    })
}

pub fn list(s: &Store, enabled_only: bool) -> Result<Vec<Hook>> {
    let conn = s.conn()?;
    let mut q = format!("SELECT {HOOK_COLS} FROM hooks");
    if enabled_only {
        q.push_str(" WHERE enabled = 1");
    }
    q.push_str(" ORDER BY id");
    let mut stmt = conn.prepare(&q)?;
    Ok(stmt
        .query_map([], scan_hook)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn deliveries(s: &Store, hook_id: i64, mut limit: i32) -> Result<Vec<Delivery>> {
    if limit <= 0 {
        limit = 50;
    }
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, hook_id, event_seq, event_action, project, attempt, status, detail, at, next_at
         FROM hook_deliveries WHERE hook_id = ? ORDER BY id DESC LIMIT ?",
    )?;
    Ok(stmt
        .query_map(params![hook_id, limit], scan_delivery)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn delivery_states(s: &Store) -> Result<std::collections::BTreeMap<i64, DeliveryState>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT h.id,
        (SELECT COUNT(*) FROM hook_deliveries d WHERE d.hook_id = h.id AND d.status = 'pending'),
        COALESCE((SELECT detail FROM hook_deliveries d WHERE d.hook_id = h.id AND d.status IN ('error','failed') ORDER BY d.id DESC LIMIT 1), '')
        FROM hooks h",
    )?;
    let mut out = std::collections::BTreeMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            DeliveryState {
                pending: r.get(1)?,
                last_error: r.get(2)?,
            },
        ))
    })?;
    for row in rows {
        let (id, state) = row?;
        out.insert(id, state);
    }
    Ok(out)
}

pub fn set_enabled(s: &Store, id: i64, enabled: bool, actor: &str) -> Result<Hook> {
    let _ = get(s, id)?;
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    let action = if enabled {
        "hook.enabled"
    } else {
        "hook.removed"
    };
    let flag = if enabled { 1 } else { 0 };
    let tx = s.begin()?;
    tx.execute(
        "UPDATE hooks SET enabled = ? WHERE id = ?",
        params![flag, id],
    )?;
    tx.event(actor, action, &id.to_string(), None::<&str>)?;
    tx.commit()?;
    get(s, id)
}

impl Hook {
    pub fn matches(&self, e: &Event) -> bool {
        if !self.enabled || !pulse_bus::glob(&self.event, &e.action) {
            return false;
        }
        if !self.project.is_empty() && e.project() != self.project && e.subject != self.project {
            return false;
        }
        true
    }
}

/// Returns the status text alongside the result: the recorded status is
/// meaningful even when the fire failed (`hook test --json` prints both).
pub fn fire(ctx: &CancellationToken, s: &Store, h: &Hook, e: &Event) -> (String, Result<()>) {
    let (status, mut fire_err) = run_fire(ctx, s, h, e);
    let final_status = if fire_err.is_ok() { "ok" } else { "error" };
    if let Err(db_err) = insert_delivery(s, h, e, 1, final_status, &status, "")
        && fire_err.is_ok()
    {
        fire_err = Err(db_err);
    }
    (status, fire_err)
}

fn run_fire(ctx: &CancellationToken, s: &Store, h: &Hook, e: &Event) -> (String, Result<()>) {
    let redacted = pulse_bus::redactor(Some(s))(e.clone());
    let res = fire_once(ctx, s, h, &redacted);
    let mut status = match &res {
        Ok(v) => v.clone(),
        Err(e) => format!("error: {e}"),
    };
    if status.len() > 200 {
        status.truncate(status.floor_char_boundary(200));
    }
    let db = (|| -> Result<()> {
        let conn = s.conn()?;
        conn.execute(
            "UPDATE hooks SET last_fired_at = ?, last_status = ?, fires = fires + 1 WHERE id = ?",
            params![s.now(), status, h.id],
        )?;
        Ok(())
    })();
    match (res, db) {
        (Ok(_), Ok(())) => (status, Ok(())),
        (Err(e), _) => (status, Err(e)),
        (Ok(_), Err(e)) => (status, Err(e)),
    }
}

fn insert_delivery(
    s: &Store,
    h: &Hook,
    e: &Event,
    attempt: i32,
    status: &str,
    detail: &str,
    next_at: &str,
) -> Result<i64> {
    let status = if status.is_empty() { "pending" } else { status };
    let conn = s.conn()?;
    conn.execute(
        "INSERT INTO hook_deliveries (hook_id, event_seq, event_action, project, attempt, status, detail, at, next_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![h.id, e.id, e.action, e.project(), attempt, status, trunc_detail(detail), s.now(), next_at],
    )?;
    Ok(conn.last_insert_rowid())
}

fn update_delivery(s: &Store, id: i64, status: &str, detail: &str, next_at: &str) -> Result<()> {
    let conn = s.conn()?;
    conn.execute(
        "UPDATE hook_deliveries SET status = ?, detail = ?, at = ?, next_at = ? WHERE id = ?",
        params![status, trunc_detail(detail), s.now(), next_at, id],
    )?;
    Ok(())
}

fn trunc_detail(text: &str) -> String {
    if text.len() > 500 {
        text[..text.floor_char_boundary(500)].to_string()
    } else {
        text.to_string()
    }
}

fn add_duration(now: &str, secs: i64) -> String {
    DateTime::parse_from_rfc3339(now)
        .map(|dt| {
            dt.with_timezone(&Utc)
                .checked_add_signed(ChronoDuration::seconds(secs))
                .unwrap_or_else(Utc::now)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
        .unwrap_or_else(|_| now.to_string())
}

fn event_by_seq(s: &Store, seq: i64) -> Result<Event> {
    let conn = s.conn()?;
    Ok(conn.query_row(
        "SELECT seq, at, actor, action, subject, detail FROM events WHERE seq = ?",
        [seq],
        |r| {
            Ok(Event {
                id: r.get(0)?,
                at: r.get(1)?,
                actor: r.get(2)?,
                action: r.get(3)?,
                subject: r.get(4)?,
                detail: r.get(5)?,
            })
        },
    )?)
}

fn fire_once(ctx: &CancellationToken, s: &Store, h: &Hook, e: &Event) -> Result<String> {
    let home = s.home.as_str();
    let body = pulse_store::gojson::to_vec(&Payload {
        hook: h.id,
        id: e.id,
        at: e.at.clone(),
        actor: e.actor.clone(),
        action: e.action.clone(),
        subject: e.subject.clone(),
        detail: e.detail.clone(),
        project: e.project(),
    })?;
    match h.kind.as_str() {
        "script" => {
            let ctx = ctx.with_timeout(SCRIPT_TIMEOUT);
            let mut child = Command::new(&h.target)
                .env("PULSE_HOOK_ID", h.id.to_string())
                .env("PULSE_EVENT_ID", e.id.to_string())
                .env("PULSE_EVENT_AT", &e.at)
                .env("PULSE_EVENT_ACTOR", &e.actor)
                .env("PULSE_EVENT_ACTION", &e.action)
                .env("PULSE_EVENT_SUBJECT", &e.subject)
                .env("PULSE_EVENT_DETAIL", &e.detail)
                .env("PULSE_EVENT_PROJECT", e.project())
                .env("PULSE_HOME", home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| h.target.clone())?;
            if let Some(mut stdin) = child.stdin.take() {
                // A script that never reads its stdin is not an error.
                let _ = stdin.write_all(&body);
            }
            // Drain both pipes concurrently so a chatty script cannot block
            // on a full pipe while we wait for it to exit.
            let stdout = drain(child.stdout.take());
            let stderr = drain(child.stderr.take());
            let collect = |stdout: JoinHandle<Vec<u8>>, stderr: JoinHandle<Vec<u8>>| {
                let mut all = stdout.join().unwrap_or_default();
                all.extend_from_slice(&stderr.join().unwrap_or_default());
                String::from_utf8_lossy(&all).trim().to_string()
            };
            loop {
                if let Some(status) = child.try_wait()? {
                    let out = collect(stdout, stderr);
                    if !status.success() {
                        return Err(anyhow!("{}: {}", exit_text(status), out));
                    }
                    return Ok("ok".to_string());
                }
                if ctx.is_cancelled() {
                    // The deadline passed or the caller cancelled: kill the process.
                    let _ = child.kill();
                    let _ = child.wait();
                    let out = collect(stdout, stderr);
                    if ctx.deadline_exceeded() {
                        return Err(anyhow!("timeout after {}s", SCRIPT_TIMEOUT.as_secs()));
                    }
                    return Err(anyhow!("signal: killed: {out}"));
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        "webhook" => {
            let ctx = ctx.with_timeout(WEBHOOK_TIMEOUT);
            let client = reqwest::blocking::Client::builder()
                .timeout(ctx.remaining().unwrap_or(WEBHOOK_TIMEOUT))
                .build()?;
            let resp = client
                .post(&h.target)
                .header("Content-Type", "application/json")
                .header("User-Agent", "pulse-hook/1")
                .header("X-Pulse-Event", &e.action)
                .body(body)
                .send()?;
            let code = resp.status().as_u16();
            if code >= 300 {
                return Err(anyhow!("HTTP {code}"));
            }
            Ok(format!("ok {code}"))
        }
        "job" => queue_job(s, h, e),
        _ => Err(anyhow!("unknown hook kind {:?}", h.kind)),
    }
}

/// The source of the jobs hook `id` queues: one open job per hook at a
/// time, the board deduping on it the way it does for `schedule:SCHED-NNN`.
pub fn job_source(id: i64) -> String {
    format!("hook:{id}")
}

/// Queues the job a `job` hook describes, filled from the event. Once per
/// event: a replayed event whose job this hook already queued is a
/// delivered one, not a retry; and a hook never feeds itself or its kin -
/// an event about a job some hook queued is skipped.
fn queue_job(s: &Store, h: &Hook, e: &Event) -> Result<String> {
    let t = JobTemplate::parse(&h.target)?;
    if e.id != 0
        && let Some(detail) = delivered(s, h.id, e.id)?
    {
        return Ok(format!("already delivered for event #{}: {detail}", e.id));
    }
    if let Some(job) = hook_job_in(s, e)? {
        return Ok(format!("skipped: {job} is a hook's own job"));
    }
    let project = if h.project.is_empty() {
        e.project()
    } else {
        h.project.clone()
    };
    let (job, queued) = pulse_board::add_job(
        s,
        pulse_board::JobInput {
            project,
            kind: t.kind,
            title: fill(&t.title, e),
            brief: fill(&t.brief, e),
            tier: t.tier,
            priority: t.priority,
            source: job_source(h.id),
            created_by: h.created_by.clone(),
            agent: t.agent,
            ..Default::default()
        },
    )?;
    Ok(if queued {
        format!("queued {}", job.id)
    } else {
        format!("already queued: {}", job.id)
    })
}

/// The detail of the delivery that already went through for this hook and
/// event, if one did.
fn delivered(s: &Store, hook_id: i64, event_seq: i64) -> Result<Option<String>> {
    let conn = s.conn()?;
    Ok(conn
        .query_row(
            "SELECT detail FROM hook_deliveries WHERE hook_id = ? AND event_seq = ? AND status = 'ok' ORDER BY id LIMIT 1",
            params![hook_id, event_seq],
            |r| r.get(0),
        )
        .optional()?)
}

/// The job the event is about - its subject, or the `job` its detail
/// names - when a hook queued that job.
fn hook_job_in(s: &Store, e: &Event) -> Result<Option<String>> {
    let named = pulse_bus::detail_field(&e.detail, "job");
    let candidates = [e.subject.as_str(), named.as_str()];
    let conn = s.conn()?;
    for id in candidates.into_iter().filter(|id| !id.is_empty()) {
        let hooked: Option<String> = conn
            .query_row(
                "SELECT id FROM jobs WHERE id = ? AND source LIKE 'hook:%'",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if hooked.is_some() {
            return Ok(hooked);
        }
    }
    Ok(None)
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    })
}

/// The recorded text of a script's exit: "exit status N", or "signal: <name>"
/// with the signal's conventional description.
pub fn exit_text(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit status {code}");
    }
    match status.signal() {
        Some(libc::SIGHUP) => "signal: hangup".to_string(),
        Some(libc::SIGINT) => "signal: interrupt".to_string(),
        Some(libc::SIGQUIT) => "signal: quit".to_string(),
        Some(libc::SIGILL) => "signal: illegal instruction".to_string(),
        Some(libc::SIGTRAP) => "signal: trace/BPT trap".to_string(),
        Some(libc::SIGABRT) => "signal: abort trap".to_string(),
        Some(libc::SIGBUS) => "signal: bus error".to_string(),
        Some(libc::SIGFPE) => "signal: floating point exception".to_string(),
        Some(libc::SIGKILL) => "signal: killed".to_string(),
        Some(libc::SIGSEGV) => "signal: segmentation fault".to_string(),
        Some(libc::SIGPIPE) => "signal: broken pipe".to_string(),
        Some(libc::SIGALRM) => "signal: alarm clock".to_string(),
        Some(libc::SIGTERM) => "signal: terminated".to_string(),
        Some(libc::SIGUSR1) => "signal: user defined signal 1".to_string(),
        Some(libc::SIGUSR2) => "signal: user defined signal 2".to_string(),
        Some(n) => format!("signal: signal {n}"),
        None => status.to_string(),
    }
}

pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct Runner {
    pub bus: Option<Arc<Bus>>,
    pub store: Store,
    pub log: Option<LogFn>,
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    active: Arc<Mutex<std::collections::BTreeSet<i64>>>,
}

impl Runner {
    pub fn new(store: Store) -> Self {
        Self {
            bus: None,
            store,
            log: None,
            handles: Arc::new(Mutex::new(Vec::new())),
            active: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
        }
    }

    pub fn run(&self, cancel: CancellationToken) {
        let b = self.bus.clone().unwrap_or_else(pulse_bus::default);
        let (ch, stop) = b.subscribe(Filter::default(), 256);
        self.retry_due(&cancel, &self.store.now());
        // Blocks on the bus, the retry tick and the context's done signal
        // rather than waking every 20 ms with nothing to deliver.
        let done = cancel.done();
        let retry = crossbeam_channel::tick(RETRY_INTERVAL);
        while !cancel.is_cancelled() {
            crossbeam_channel::select! {
                recv(done) -> _ => break,
                recv(retry) -> _ => {
                    self.retry_due(&cancel, &self.store.now());
                }
                recv(ch) -> msg => match msg {
                    Ok(e) => {
                        if !e.action.starts_with("hook.") {
                            self.dispatch(&e);
                        }
                    }
                    Err(_) => break,
                },
            }
        }
        stop();
        while let Ok(e) = ch.try_recv() {
            self.enqueue_pending(&e);
        }
        self.wait();
    }

    pub fn wait(&self) {
        let handles = std::mem::take(&mut *self.handles.lock().expect("handles lock"));
        for h in handles {
            let _ = h.join();
        }
    }

    pub fn dispatch(&self, e: &Event) {
        let Ok(hooks) = list(&self.store, true) else {
            self.logf("hooks");
            return;
        };
        for h in hooks {
            if !h.matches(e) {
                continue;
            }
            match insert_delivery(&self.store, &h, e, 1, "pending", "", "") {
                Ok(id) => self.spawn_delivery(h, e.clone(), 1, id),
                Err(err) => self.logf(&format!("hook delivery: {err}")),
            }
        }
    }

    fn enqueue_pending(&self, e: &Event) {
        if e.action.starts_with("hook.") {
            return;
        }
        if let Ok(hooks) = list(&self.store, true) {
            for h in hooks.into_iter().filter(|h| h.matches(e)) {
                if let Err(err) = insert_delivery(
                    &self.store,
                    &h,
                    e,
                    1,
                    "pending",
                    "queued during hook runner shutdown",
                    &self.store.now(),
                ) {
                    self.logf(&format!("hook delivery: {err}"));
                }
            }
        }
    }

    pub fn retry_due(&self, cancel: &CancellationToken, now: &str) -> usize {
        if cancel.is_cancelled() {
            return 0;
        }
        let due = (|| -> Result<Vec<Delivery>> {
            let conn = self.store.conn()?;
            let mut stmt = conn.prepare(
                "SELECT id, hook_id, event_seq, event_action, project, attempt, status, detail, at, next_at
                 FROM hook_deliveries WHERE status = 'pending' AND (next_at = '' OR next_at <= ?) ORDER BY id LIMIT 64",
            )?;
            Ok(stmt
                .query_map([now], scan_delivery)?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })();
        let due = match due {
            Ok(due) => due,
            Err(err) => {
                self.logf(&format!("hook retries: {err}"));
                return 0;
            }
        };
        let n = due.len();
        for d in due {
            if cancel.is_cancelled() || self.is_active(d.id) {
                continue;
            }
            let Ok(h) = get(&self.store, d.hook_id) else {
                let _ = update_delivery(&self.store, d.id, "error", "hook not found", "");
                continue;
            };
            if !h.enabled {
                let _ = update_delivery(
                    &self.store,
                    d.id,
                    "error",
                    "hook removed before the retry",
                    "",
                );
                continue;
            }
            match event_by_seq(&self.store, d.event_seq) {
                Ok(e) => self.spawn_delivery(h, e, d.attempt, d.id),
                Err(err) => {
                    let _ = update_delivery(&self.store, d.id, "error", &err.to_string(), "");
                }
            }
        }
        n
    }

    fn spawn_delivery(&self, h: Hook, e: Event, attempt: i32, delivery_id: i64) {
        self.mark(delivery_id);
        let r = self.clone();
        let handle = thread::spawn(move || {
            r.fire_delivery(h, e, attempt, delivery_id);
            r.unmark(delivery_id);
        });
        self.handles.lock().expect("handles lock").push(handle);
    }

    fn fire_delivery(&self, h: Hook, e: Event, attempt: i32, delivery_id: i64) {
        let (status, err) = run_fire(&CancellationToken::new(), &self.store, &h, &e);
        if err.is_err() {
            if attempt >= MAX_ATTEMPTS {
                if let Err(db_err) =
                    update_delivery(&self.store, delivery_id, "failed", &status, "")
                {
                    self.logf(&format!("hook delivery {delivery_id}: {db_err}"));
                }
                self.emit_failed(&h, &e, attempt, &status);
                self.logf(&format!(
                    "hook {} ({} {}) on {} #{}: {}",
                    h.id, h.kind, h.target, e.action, e.id, status
                ));
                return;
            }
            let idx = ((attempt - 1) as usize).min(RETRY_BACKOFF_SECS.len() - 1);
            let next = add_duration(&self.store.now(), RETRY_BACKOFF_SECS[idx]);
            let _ = update_delivery(&self.store, delivery_id, "error", &status, "");
            if let Err(err) =
                insert_delivery(&self.store, &h, &e, attempt + 1, "pending", &status, &next)
            {
                self.logf(&format!("hook retry: {err}"));
            }
            self.logf(&format!(
                "hook {} ({} {}) on {} #{}: {}",
                h.id, h.kind, h.target, e.action, e.id, status
            ));
            return;
        }
        if let Err(err) = update_delivery(&self.store, delivery_id, "ok", &status, "") {
            self.logf(&format!("hook delivery {delivery_id}: {err}"));
        }
        self.logf(&format!(
            "hook {} ({} {}) on {} #{}: {}",
            h.id, h.kind, h.target, e.action, e.id, status
        ));
    }

    fn emit_failed(&self, h: &Hook, e: &Event, attempts: i32, status: &str) {
        if let Ok(ev) = pulse_bus::emit(
            &self.store,
            "hook",
            "hook.failed",
            &h.id.to_string(),
            pulse_store::Detail::json(serde_json::json!({"event": e.id, "action": e.action, "attempts": attempts, "error": status})).unwrap_or(pulse_store::Detail::Empty),
        ) && let Some(b) = &self.bus {
            b.publish_once(ev);
        }
    }

    fn mark(&self, id: i64) {
        self.active.lock().expect("active lock").insert(id);
    }
    fn unmark(&self, id: i64) {
        self.active.lock().expect("active lock").remove(&id);
    }
    fn is_active(&self, id: i64) -> bool {
        self.active.lock().expect("active lock").contains(&id)
    }
    fn logf(&self, s: &str) {
        if let Some(log) = &self.log {
            log(s);
        }
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    pub struct TestStore {
        _home: tempfile::TempDir,
        store: Store,
    }
    impl TestStore {
        pub fn home(&self) -> String {
            self._home.path().display().to_string()
        }
    }
    impl std::ops::Deref for TestStore {
        type Target = Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }

    pub fn new_store() -> TestStore {
        let home = tempfile::tempdir().unwrap();
        Store::create(Path::new(home.path()).join(pulse_store::STORE_NAME)).unwrap();
        let store = Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            now: "2026-09-06T21:00:00Z".into(),
            ..Default::default()
        })
        .unwrap();
        TestStore { _home: home, store }
    }

    pub fn script(body: &str) -> String {
        static SCRIPT_DIRS: OnceLock<Mutex<Vec<tempfile::TempDir>>> = OnceLock::new();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hook.sh");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        SCRIPT_DIRS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .push(dir);
        p.display().to_string()
    }

    pub fn wait_for(what: &str, ok: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if ok() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;
    use super::*;
    use pulse_testutil::httptest::{Response, Server};
    use std::sync::{Arc, Mutex};

    #[test]
    fn add_list_remove_enable() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let sh = script("exit 0");
        assert!(add(&s, "", "script", &sh, "", "human").is_err());
        assert!(add(&s, "post", "script", "/no/such/script", "", "human").is_err());
        assert!(add(&s, "post", "webhook", "ftp://x", "", "human").is_err());
        assert!(add(&s, "[", "script", &sh, "", "human").is_err());
        let h = add(&s, "task.*", "script", &sh, "demo", "").unwrap();
        assert_eq!(h.id, 1);
        assert!(h.enabled);
        assert_eq!(h.created_by, "human");
        let w = add(
            &s,
            "*",
            "webhook",
            "https://example.test/hook",
            "",
            "agent-a",
        )
        .unwrap();
        let rows = list(&s, true).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].id, w.id);
        assert!(!set_enabled(&s, h.id, false, "human").unwrap().enabled);
        assert_eq!(list(&s, true).unwrap().len(), 1);
        assert_eq!(list(&s, false).unwrap().len(), 2);
        assert!(set_enabled(&s, h.id, true, "human").unwrap().enabled);
        assert!(
            s.conn()
                .unwrap()
                .execute("DELETE FROM hooks WHERE id = ?", [h.id])
                .is_err()
        );
        let evs = pulse_bus::since(
            &s,
            0,
            Filter {
                actions: vec!["hook.*".into()],
                ..Default::default()
            },
            0,
        )
        .unwrap();
        assert_eq!(evs.len(), 4);
        assert_eq!(evs[2].action, "hook.removed");
        assert!(get(&s, 99).is_err());
    }

    #[test]
    fn matches_glob_and_project() {
        let _serial = crate::test_helpers::serial();
        let mut h = Hook {
            event: "post.*".into(),
            enabled: true,
            project: "demo".into(),
            ..Default::default()
        };
        let demo = Event {
            action: "post.amended".into(),
            detail: r#"{"project": "demo"}"#.into(),
            ..Default::default()
        };
        let other = Event {
            action: "post.amended".into(),
            detail: r#"{"project": "other"}"#.into(),
            ..Default::default()
        };
        assert!(h.matches(&demo));
        assert!(!h.matches(&other));
        assert!(!h.matches(&Event {
            action: "task.created".into(),
            detail: r#"{"project": "demo"}"#.into(),
            ..Default::default()
        }));
        h.enabled = false;
        assert!(!h.matches(&demo));
        let any = Hook {
            event: "*".into(),
            enabled: true,
            ..Default::default()
        };
        assert!(any.matches(&other));
        assert!(any.matches(&Event {
            action: "settings.set".into(),
            ..Default::default()
        }));
    }

    #[test]
    fn runner_fires_script_and_webhook_skips_disabled() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let out_dir = tempfile::tempdir().unwrap();
        let out_file = out_dir.path().join("seen.txt");
        let sh = script(&format!(
            "cat > \"{}.stdin\"; printf '%s %s %s %s\\n' \"$PULSE_EVENT_ID\" \"$PULSE_EVENT_ACTION\" \"$PULSE_EVENT_SUBJECT\" \"$PULSE_EVENT_PROJECT\" > \"{}\"",
            out_file.display(),
            out_file.display()
        ));
        let got: Arc<Mutex<Vec<Payload>>> = Arc::new(Mutex::new(Vec::new()));
        let got2 = Arc::clone(&got);
        let srv = Server::new(move |req| {
            let p: Payload = serde_json::from_value(req.json().unwrap()).unwrap();
            got2.lock().unwrap().push(p);
            if req.header("X-Pulse-Event").is_none()
                || req.header("Content-Type") != Some("application/json")
            {
                return Response::new(400);
            }
            Response::new(204)
        });
        let sc = add(&s, "task.*", "script", &sh, "", "human").unwrap();
        let wh = add(&s, "task.created", "webhook", &srv.url(), "demo", "human").unwrap();
        let dis = add(&s, "*", "webhook", &(srv.url() + "/disabled"), "", "human").unwrap();
        set_enabled(&s, dis.id, false, "human").unwrap();
        let failing = add(
            &s,
            "settings.*",
            "script",
            &script("echo boom >&2; exit 3"),
            "",
            "human",
        )
        .unwrap();
        let b = Arc::new(pulse_bus::new());
        let r = Runner {
            bus: Some(Arc::clone(&b)),
            store: s.clone(),
            log: None,
            handles: Default::default(),
            active: Default::default(),
        };
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let r2 = r.clone();
        let done = thread::spawn(move || r2.run(c2));
        wait_for("runner subscription", || b.subscribers() > 0);
        let e1 = pulse_bus::emit_on(
            Some(Arc::clone(&b)),
            &s,
            "human",
            "task.created",
            "TASK-001",
            pulse_store::Detail::json(serde_json::json!({"project": "demo"})).unwrap(),
        )
        .unwrap();
        let _ = pulse_bus::emit_on(
            Some(Arc::clone(&b)),
            &s,
            "human",
            "task.created",
            "TASK-002",
            pulse_store::Detail::json(serde_json::json!({"project": "other"})).unwrap(),
        )
        .unwrap();
        let _ = pulse_bus::emit_on(
            Some(Arc::clone(&b)),
            &s,
            "human",
            "settings.set",
            "stop",
            "",
        )
        .unwrap();
        wait_for("script output", || out_file.exists());
        wait_for("webhook", || !got.lock().unwrap().is_empty());
        wait_for("failing hook status", || {
            get(&s, failing.id).unwrap().fires == 1
        });
        wait_for("script fires", || get(&s, sc.id).unwrap().fires == 2);
        cancel.cancel();
        done.join().unwrap();
        let g = got.lock().unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].subject, "TASK-001");
        assert_eq!(g[0].project, "demo");
        assert_eq!(g[0].id, e1.id);
        assert_eq!(g[0].hook, wh.id);
        let stdin_text = std::fs::read_to_string(out_file.with_extension("txt.stdin"))
            .unwrap_or_else(|_| {
                std::fs::read_to_string(format!("{}.stdin", out_file.display())).unwrap()
            });
        assert!(stdin_text.contains(r#""action":"task.created""#));
        let h = get(&s, wh.id).unwrap();
        assert_eq!(h.fires, 1);
        assert_eq!(h.last_status, "ok 204");
        let h = get(&s, failing.id).unwrap();
        assert!(
            h.last_status.starts_with("error: exit status 3: boom"),
            "last_status {:?}",
            h.last_status
        );
        assert_eq!(get(&s, dis.id).unwrap().fires, 0);
        assert_eq!(b.dropped(), 0);
    }

    #[test]
    fn fire_timeout() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let h = add(&s, "*", "script", &script("sleep 5"), "", "human").unwrap();
        let ctx = CancellationToken::new().with_timeout(Duration::from_millis(100));
        let (status, err) = fire(
            &ctx,
            &s,
            &h,
            &Event {
                action: "x".into(),
                ..Default::default()
            },
        );
        assert!(
            err.is_err() && status.starts_with("error:"),
            "status {status:?} err {err:?}"
        );
    }

    #[test]
    fn runner_cancel_does_not_kill_in_flight_script() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let marker_dir = tempfile::tempdir().unwrap();
        let marker = marker_dir.path().join("done");
        let h = add(
            &s,
            "job.add",
            "script",
            &script(&format!("sleep 1; echo ok > \"{}\"", marker.display())),
            "",
            "human",
        )
        .unwrap();
        let b = Arc::new(pulse_bus::new());
        let r = Runner {
            bus: Some(Arc::clone(&b)),
            store: s.clone(),
            log: None,
            handles: Default::default(),
            active: Default::default(),
        };
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let r2 = r.clone();
        let done = thread::spawn(move || r2.run(c2));
        wait_for("runner subscription", || b.subscribers() > 0);
        pulse_bus::emit_on(Some(b), &s, "human", "job.add", "job-1", "").unwrap();
        thread::sleep(Duration::from_millis(100));
        cancel.cancel();
        done.join().unwrap();
        assert!(marker.exists());
        let rows = deliveries(&s, h.id, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "ok");
    }

    #[test]
    fn runner_retries_failing_script_and_emits_failed() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let h = add(
            &s,
            "job.add",
            "script",
            &script("echo boom >&2; exit 3"),
            "",
            "human",
        )
        .unwrap();
        let e = pulse_bus::emit_on(None, &s, "human", "job.add", "job-1", "").unwrap();
        let r = Runner {
            bus: Some(Arc::new(pulse_bus::new())),
            store: s.clone(),
            log: None,
            handles: Default::default(),
            active: Default::default(),
        };
        r.dispatch(&e);
        r.wait();
        let rows = deliveries(&s, h.id, 10).unwrap();
        assert_eq!(rows[0].status, "pending");
        assert_eq!(rows[0].attempt, 2);
        assert_eq!(rows[1].status, "error");
        let cancel = CancellationToken::new();
        r.retry_due(&cancel, "2026-09-06T21:01:00Z");
        r.wait();
        let rows = deliveries(&s, h.id, 10).unwrap();
        assert_eq!(rows[0].attempt, 3);
        r.retry_due(&cancel, "2026-09-06T21:06:00Z");
        r.wait();
        let rows = deliveries(&s, h.id, 10).unwrap();
        assert_eq!(rows[0].status, "failed");
        assert!(rows[0].detail.contains("boom"));
        let events = pulse_bus::since(
            &s,
            0,
            Filter {
                actions: vec!["hook.failed".into()],
                ..Default::default()
            },
            0,
        )
        .unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].detail.contains(r#""attempts": 3"#)
                || events[0].detail.contains(r#""attempts":3"#)
        );
    }

    #[test]
    fn fire_redacts_private_settings() {
        let _serial = crate::test_helpers::serial();
        let _fidelity_literals = (
            "\"; cat >> \"",
            "event leaked the private path to the hook:",
            "event was not masked:",
            "printf '' \"$PULSE_EVENT_DETAIL\" > \"",
        );
        let s = new_store();
        let private_dir = tempfile::tempdir().unwrap();
        let private = private_dir
            .path()
            .join("heldout-fixtures")
            .display()
            .to_string();
        s.set_setting(pulse_store::HUMAN_AGENT, "evolve.heldout_dir", &private)
            .unwrap();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("seen.txt");
        let h = add(
            &s,
            "*",
            "script",
            &script(&format!(
                "printf '%s\\n' \"$PULSE_EVENT_DETAIL\" > \"{}\"; cat >> \"{}\"",
                out.display(),
                out.display()
            )),
            "",
            "human",
        )
        .unwrap();
        for e in [
            Event {
                id: 1,
                at: s.now(),
                actor: "human".into(),
                action: "setting".into(),
                subject: "evolve.heldout_dir".into(),
                detail: private.clone(),
            },
            Event {
                id: 2,
                at: s.now(),
                actor: "evolve".into(),
                action: "evolve.cycle".into(),
                subject: "cycle-1".into(),
                detail: format!(r#"{{"fixtures": "{private}/a.json"}}"#),
            },
        ] {
            let (status, err) = fire(&CancellationToken::new(), &s, &h, &e);
            err.unwrap();
            assert_eq!(status, "ok");
            let seen = std::fs::read_to_string(&out).unwrap();
            assert!(!seen.contains(&private), "{seen}");
            assert!(seen.contains(pulse_bus::PRIVATE_VALUE), "{seen}");
        }
    }

    fn with_project(s: &Store, slug: &str) {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES (?, ?, ?, ?)",
                params![slug, slug, format!("{}/{slug}", s.home), s.now()],
            )
            .unwrap();
    }

    fn job_count(s: &Store) -> i64 {
        s.conn()
            .unwrap()
            .query_row("SELECT count(*) FROM jobs", [], |r| r.get(0))
            .unwrap()
    }

    fn template(kind: &str, title: &str, brief: &str, tier: i64) -> String {
        serde_json::to_string(&JobTemplate {
            kind: kind.into(),
            title: title.into(),
            brief: brief.into(),
            tier,
            priority: 0,
            agent: String::new(),
        })
        .unwrap()
    }

    #[test]
    fn job_hook_add_checks_the_template_and_the_project() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        with_project(&s, "demo");
        let what = |r: Result<Hook>| -> String {
            let err = r.expect_err("refused");
            err.downcast_ref::<pulse_cli::Error>()
                .map_or_else(|| err.to_string(), |e| e.what.clone())
        };
        let ok = template("task", "Look at {{subject}}", "{{detail}}", 1);
        assert_eq!(
            what(add(&s, "task.*", "job", &ok, "", "human")),
            "A job hook needs --project"
        );
        assert_eq!(
            what(add(&s, "task.*", "job", &ok, "nope", "human")),
            "No project \"nope\""
        );
        for kind in ["lead", "ballot", "experiment", "baseline", "answer"] {
            assert_eq!(
                what(add(
                    &s,
                    "task.*",
                    "job",
                    &template(kind, "t", "b", 1),
                    "demo",
                    "human"
                )),
                format!("A hook cannot queue a {kind} job")
            );
        }
        assert_eq!(
            what(add(
                &s,
                "task.*",
                "job",
                &template("task", " ", "b", 1),
                "demo",
                "human"
            )),
            "A job hook needs a title"
        );
        assert_eq!(
            what(add(
                &s,
                "task.*",
                "job",
                &template("task", "t", "b", 2),
                "demo",
                "human"
            )),
            "The runner takes Tier 0 and Tier 1 jobs only"
        );
        let agent = serde_json::to_string(&JobTemplate {
            agent: "Not An Agent".into(),
            ..JobTemplate::parse(&ok).unwrap()
        })
        .unwrap();
        assert_eq!(
            what(add(&s, "task.*", "job", &agent, "demo", "human")),
            "'Not An Agent' is not an agent name"
        );
        assert!(add(&s, "task.*", "job", "not json", "demo", "human").is_err());
        let h = add(&s, "task.*", "job", &ok, "demo", "lead-a").unwrap();
        assert_eq!(h.kind, "job");
        assert_eq!(
            JobTemplate::parse(&h.target).unwrap().title,
            "Look at {{subject}}"
        );
        assert_eq!(h.project, "demo");
    }

    #[test]
    fn fill_substitutes_the_event_fields_once() {
        let _serial = crate::test_helpers::serial();
        let e = Event {
            id: 7,
            at: "2026-09-06T21:00:00Z".into(),
            actor: "lead-a".into(),
            action: "task.blocked".into(),
            subject: "{{detail}}".into(),
            detail: format!(r#"{{"project": "demo", "why": "{}"}}"#, "x".repeat(5000)),
        };
        let out = fill(
            "{{action}} on {{subject}} by {{actor}} at {{at}} in {{project}}; {{nothing}} {{",
            &e,
        );
        assert_eq!(
            out,
            "task.blocked on {{detail}} by lead-a at 2026-09-06T21:00:00Z in demo; {{nothing}} {{",
            "the subject is not expanded again, unknown names stay"
        );
        let detail = fill("{{detail}}", &e);
        assert_eq!(detail.len(), DETAIL_LIMIT);
        assert!(detail.starts_with(r#"{"project": "demo""#));
    }

    #[test]
    fn job_hook_queues_one_job_per_event_and_never_feeds_itself() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        with_project(&s, "demo");
        let h = add(
            &s,
            "task.*",
            "job",
            &template(
                "improve",
                "Unblock {{subject}}",
                "{{actor}} said at {{at}}:\n{{detail}}",
                0,
            ),
            "demo",
            "lead-a",
        )
        .unwrap();
        let e1 = pulse_bus::emit_on(
            None,
            &s,
            "human",
            "task.blocked",
            "TASK-001",
            pulse_store::Detail::json(serde_json::json!({"project": "demo", "why": "no key"}))
                .unwrap(),
        )
        .unwrap();
        let (status, err) = fire(&CancellationToken::new(), &s, &h, &e1);
        err.unwrap();
        let job_id = status.strip_prefix("queued ").expect(&status).to_string();
        let job = pulse_board::job_by_id(&s, &job_id).unwrap().unwrap();
        assert_eq!(job.project, "demo");
        assert_eq!(job.kind, "improve");
        assert_eq!(job.tier, 0);
        assert_eq!(job.title, "Unblock TASK-001");
        assert!(
            job.brief
                .starts_with(&format!("human said at {}:\n{{", e1.at))
                && job.brief.contains("no key"),
            "{}",
            job.brief
        );
        assert_eq!(job.source, job_source(h.id));
        assert_eq!(job.created_by, "lead-a");

        // The same event again: delivered already, so a success that says so.
        let (status, err) = fire(&CancellationToken::new(), &s, &h, &e1);
        err.unwrap();
        assert_eq!(
            status,
            format!("already delivered for event #{}: queued {job_id}", e1.id)
        );
        // Another event while that job is open: one open job per hook.
        let e2 = pulse_bus::emit_on(
            None,
            &s,
            "human",
            "task.blocked",
            "TASK-002",
            pulse_store::Detail::json(serde_json::json!({"project": "demo"})).unwrap(),
        )
        .unwrap();
        let (status, err) = fire(&CancellationToken::new(), &s, &h, &e2);
        err.unwrap();
        assert_eq!(status, format!("already queued: {job_id}"));
        assert_eq!(job_count(&s), 1);

        // Events about a hook's own job never queue another, whatever the glob.
        let any = add(
            &s,
            "*",
            "job",
            &template("task", "Follow up {{subject}}", "b", 1),
            "demo",
            "lead-a",
        )
        .unwrap();
        let about_job = pulse_bus::emit_on(
            None,
            &s,
            "worker-1",
            "job.done",
            &job_id,
            pulse_store::Detail::json(serde_json::json!({"project": "demo"})).unwrap(),
        )
        .unwrap();
        let (status, err) = fire(&CancellationToken::new(), &s, &any, &about_job);
        err.unwrap();
        assert_eq!(status, format!("skipped: {job_id} is a hook's own job"));
        let about_run = pulse_bus::emit_on(
            None,
            &s,
            "daemon",
            "run.finished",
            "run-9",
            pulse_store::Detail::json(serde_json::json!({"project": "demo", "job": job_id}))
                .unwrap(),
        )
        .unwrap();
        let (status, err) = fire(&CancellationToken::new(), &s, &any, &about_run);
        err.unwrap();
        assert_eq!(status, format!("skipped: {job_id} is a hook's own job"));
        assert_eq!(job_count(&s), 1);

        // Once that job is done, the next event queues another; through the
        // runner, the delivery row says so.
        pulse_board::finish_job(&s, &job_id, "lead-a", "done", "ok", "", None).unwrap();
        let e3 = pulse_bus::emit_on(
            None,
            &s,
            "human",
            "task.blocked",
            "TASK-003",
            pulse_store::Detail::json(serde_json::json!({"project": "demo"})).unwrap(),
        )
        .unwrap();
        let r = Runner {
            bus: Some(Arc::new(pulse_bus::new())),
            store: s.clone(),
            log: None,
            handles: Default::default(),
            active: Default::default(),
        };
        r.dispatch(&e3);
        r.wait();
        let rows = deliveries(&s, h.id, 10).unwrap();
        assert_eq!(rows[0].status, "ok");
        assert!(
            rows[0].detail.starts_with("queued job-"),
            "{}",
            rows[0].detail
        );
        assert_eq!(get(&s, h.id).unwrap().last_status, rows[0].detail);
        // The `*` hook matched that event too and queued its own first job.
        let rows = deliveries(&s, any.id, 10).unwrap();
        assert!(
            rows[0].detail.starts_with("queued job-"),
            "{}",
            rows[0].detail
        );
        assert_eq!(job_count(&s), 3);
    }
}
