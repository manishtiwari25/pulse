use crate::util::{
    Row, event_at, exec1, fail, git_check, json_map, must_one, must_rows, plus_minutes,
    setting_int, slugify, trunc,
};
use anyhow::{Result, anyhow};
use base64::Engine;
use regex::Regex;
use rusqlite::ToSql;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

pub(crate) const LOCAL_EXECUTOR: &str = "local";
pub(crate) const UNREACHABLE_AFTER: i32 = 3;
pub(crate) const SSH_EXIT_UNREACH: i32 = 255;
pub(crate) const REMOTE_EXIT_TIMEOUT: i32 = 124;
pub(crate) const LOGIN_CHECK_PROMPT: &str = "Reply with exactly the word ok and nothing else.";
pub(crate) const NATIVE_LOGIN_PROMPT: &str = "Reply with exactly one line: SUMMARY: ok";
pub(crate) const LOGIN_CHECK_MINUTES: i32 = 2;
pub(crate) const LOGIN_EXPIRY_WARN: i32 = 7;
pub(crate) const GITHUB_API: &str = "https://api.github.com";

static EXECUTOR_STATUSES: &[&str] = &["active", "drained", "unreachable"];
static LOGIN_BACKOFF_MINS: [i32; 2] = [15, 360];
static SSH_OPTIONS: &[&str] = &[
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=15",
    "-o",
    "ClearAllForwardings=yes",
];
static LOGIN_FAULT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|\n)not logged in:|not_logged_in|re-?authenticat|not logged in|authenticat|unauthori[sz]ed|\b401\b|Copilot Requests|log ?in").unwrap()
});
static TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(github_pat_|gho_|ghu_|ghp_|ghs_|ghr_)[A-Za-z0-9_]+").unwrap());
static JOB_DIR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]+$").unwrap());
static SSH_ADDRESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z0-9._-]+)@([A-Za-z0-9._-]+|\[[0-9A-Fa-f:.]+\])(?::([0-9]{1,5}))?$")
        .unwrap()
});
static EXPIRY_DATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4}-\d{2}-\d{2})").unwrap());

#[derive(Debug, Clone)]
pub struct ExecutorUnreachable {
    pub msg: String,
}
impl std::fmt::Display for ExecutorUnreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for ExecutorUnreachable {}

#[derive(Debug, Clone)]
pub struct ExecutorFault {
    pub msg: String,
}
impl std::fmt::Display for ExecutorFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for ExecutorFault {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutorUnfit {
    pub msg: String,
    pub tool: String,
    pub project: String,
    pub detail: String,
}
impl std::fmt::Display for ExecutorUnfit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for ExecutorUnfit {}

fn is_nil(r: &Row) -> bool {
    r.0.is_empty()
}

fn py_repr(s: &str) -> String {
    format!("{:?}", s)
}

fn words_joined(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn login_fault(text: &str) -> bool {
    LOGIN_FAULT_RE.is_match(text)
}

pub(crate) fn redact_tokens(text: &str) -> String {
    TOKEN_RE.replace_all(text, "[redacted]").to_string()
}

pub(crate) fn fault_line(text: &str, limit: usize) -> String {
    let lines = text
        .split('\n')
        .map(words_joined)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>();
    for l in &lines {
        if login_fault(l) {
            return cut(&redact_tokens(l), limit);
        }
    }
    lines
        .first()
        .map(|l| cut(&redact_tokens(l), limit))
        .unwrap_or_default()
}

pub(crate) fn cut(s: &str, n: usize) -> String {
    let mut chars = s.chars();
    let out = chars.by_ref().take(n).collect::<String>();
    if chars.next().is_some() {
        out
    } else {
        s.to_string()
    }
}

pub(crate) fn tail(s: &str, n: usize) -> String {
    let chars = s.chars().collect::<Vec<_>>();
    if chars.len() > n {
        chars[chars.len() - n..].iter().collect()
    } else {
        s.to_string()
    }
}

pub(crate) fn parse_ssh_address(text: &str) -> Result<(String, String, i32)> {
    let trimmed = text.trim();
    let Some(caps) = SSH_ADDRESS_RE.captures(trimmed) else {
        return Err(fail(
            2,
            format!(
                "ssh address must be user@host[:port], got {}",
                py_repr(text)
            ),
        )
        .into());
    };
    let port = caps
        .get(3)
        .map(|m| m.as_str().parse::<i32>().unwrap_or(22))
        .unwrap_or(22);
    if port <= 0 || port >= 65536 {
        return Err(fail(2, format!("ssh port out of range in {}", py_repr(text))).into());
    }
    Ok((caps[1].to_string(), caps[2].to_string(), port))
}

pub(crate) fn normalize_projects(text: &str) -> String {
    let raw = text.trim();
    if raw.is_empty() || raw == "*" || raw == "all" {
        return "*".to_string();
    }
    let mut set = BTreeSet::new();
    for p in raw.split(',') {
        if !p.trim().is_empty() {
            let s = slugify(p, 40);
            if !s.is_empty() {
                set.insert(s);
            }
        }
    }
    set.into_iter().collect::<Vec<_>>().join(",")
}

#[allow(dead_code)]
pub(crate) fn executor_allows(r: &Row, slug: &str) -> bool {
    let projects = if is_nil(r) || r.str("projects").is_empty() {
        "*".to_string()
    } else {
        r.str("projects")
    };
    projects == "*" || projects.split(',').any(|x| x == slug)
}

#[allow(dead_code)]
pub(crate) fn executor_projects(r: &Row) -> Vec<String> {
    if is_nil(r) || r.str("projects").is_empty() || r.str("projects") == "*" {
        return Vec::new();
    }
    r.str("projects")
        .split(',')
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn local_executor_row(s: &pulse_store::Store) -> Row {
    let mut workers = setting_int(s, "workers");
    if workers < 0 {
        workers = 0;
    }
    let mut r = Row::new();
    for (k, v) in [
        ("name", json!(LOCAL_EXECUTOR)),
        ("kind", json!("local")),
        ("address", json!("")),
        ("workdir", json!("")),
        ("workers", json!(workers as i64)),
        ("projects", json!("*")),
        ("status", json!("active")),
        ("last_probe_at", json!("")),
        ("probe_note", json!("this machine")),
        ("errors", json!(0_i64)),
        ("backoff_minutes", json!(0_i64)),
        ("next_probe_at", json!("")),
        ("runs_done", json!(0_i64)),
        ("runs_failed", json!(0_i64)),
        ("created_at", json!("")),
        ("created_by", json!("")),
    ] {
        r.insert(k, v);
    }
    r
}

pub(crate) fn executor_rows(s: &pulse_store::Store, include_local: bool) -> Vec<Row> {
    let conn = s.conn().unwrap();
    let rows = must_rows(
        &conn,
        "SELECT * FROM executors WHERE status != 'removed' ORDER BY name",
        &[],
    );
    if include_local {
        let mut out = vec![local_executor_row(s)];
        out.extend(rows);
        out
    } else {
        rows
    }
}

pub(crate) fn executor_row(s: &pulse_store::Store, name: &str) -> Row {
    if name.is_empty() || name == LOCAL_EXECUTOR {
        return local_executor_row(s);
    }
    let conn = s.conn().unwrap();
    must_one(
        &conn,
        "SELECT * FROM executors WHERE name = ? AND status != 'removed'",
        &[&name],
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn add_executor(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    address: &str,
    workdir: &str,
    workers: i32,
    projects: &str,
    created_by: &str,
) -> Result<Row> {
    let name = slugify(name, 40);
    if name.is_empty() || name == LOCAL_EXECUTOR || name == "x" {
        return Err(fail(
            2,
            format!(
                "Executor name must be lowercase words with dashes and not {}",
                py_repr(LOCAL_EXECUTOR)
            ),
        )
        .into());
    }
    parse_ssh_address(address)?;
    let workdir_trimmed = workdir.trim();
    if !workdir_trimmed.starts_with('/') || workdir_trimmed.split('/').any(|p| p == "..") {
        return Err(fail(
            2,
            "--workdir must be an absolute path on the executor (e.g. /srv/pulse)",
        )
        .into());
    }
    if !is_nil(&executor_row(s, &name)) {
        return Err(fail(2, format!("Executor {name} already exists")).into());
    }
    if workers < 1 {
        return Err(fail(2, "--workers must be at least 1").into());
    }
    let mut wd = workdir_trimmed.trim_end_matches('/').to_string();
    if wd.is_empty() {
        wd = "/".to_string();
    }
    let normalized = normalize_projects(projects);
    let conn = s.conn().unwrap();
    let removed = must_one(
        &conn,
        "SELECT 1 FROM executors WHERE name = ? AND status = 'removed'",
        &[&name],
    );
    if !is_nil(&removed) {
        let args: [&dyn ToSql; 7] = [
            &address.trim(),
            &wd,
            &workers,
            &normalized,
            &now,
            &created_by,
            &name,
        ];
        exec1(
            &conn,
            "UPDATE executors SET kind = 'ssh', address = ?, workdir = ?, workers = ?, projects = ?, status = 'drained', created_at = ?, created_by = ?, last_probe_at = '', probe_note = '', next_probe_at = '' WHERE name = ?",
            &args,
        );
    } else {
        let args: [&dyn ToSql; 7] = [
            &name,
            &address.trim(),
            &wd,
            &workers,
            &normalized,
            &now,
            &created_by,
        ];
        exec1(
            &conn,
            "INSERT INTO executors (name, kind, address, workdir, workers, projects, status, created_at, created_by) VALUES (?, 'ssh', ?, ?, ?, ?, 'drained', ?, ?)",
            &args,
        );
    }
    event_at(
        &conn,
        now,
        created_by,
        "executor.add",
        &name,
        json!({"address": address, "workdir": workdir, "workers": workers, "projects": normalized}),
    );
    Ok(executor_row(s, &name))
}

#[allow(dead_code)]
pub(crate) fn executor_running(s: &pulse_store::Store, name: &str, now: &str) -> i32 {
    let horizon = plus_minutes(now, -f64::from(2 * setting_int(s, "minutes_per_job") + 15));
    let conn = s.conn().unwrap();
    must_one(
        &conn,
        "SELECT COUNT(*) AS n FROM runs WHERE status = 'running' AND executor = ? AND started_at > ?",
        &[&name, &horizon],
    )
    .int("n")
}

#[allow(dead_code)]
pub(crate) fn executor_free_slots(
    s: &pulse_store::Store,
    r: &Row,
    now: &str,
    inflight: i32,
) -> i32 {
    if r.str("status") != "active" {
        return 0;
    }
    let mut running = executor_running(s, &r.str("name"), now);
    if inflight > running {
        running = inflight;
    }
    (r.int("workers") - running).max(0)
}

#[allow(dead_code)]
pub(crate) fn pick_executor(s: &pulse_store::Store, slug: &str, now: &str) -> Row {
    let mut best = Row::new();
    let mut best_running = 0;
    for r in executor_rows(s, false) {
        if r.str("status") != "active" || !executor_allows(&r, slug) {
            continue;
        }
        if executor_free_slots(s, &r, now, 0) <= 0 {
            continue;
        }
        let running = executor_running(s, &r.str("name"), now);
        if is_nil(&best)
            || running < best_running
            || (running == best_running && r.str("name") < best.str("name"))
        {
            best = r;
            best_running = running;
        }
    }
    if !is_nil(&best) {
        return best;
    }
    let local = local_executor_row(s);
    if executor_free_slots(s, &local, now, 0) > 0 {
        local
    } else {
        Row::new()
    }
}

pub(crate) fn set_executor_status(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    status: &str,
    actor: &str,
    note: Option<&str>,
) -> Result<Row> {
    if name == LOCAL_EXECUTOR {
        return Err(fail(
            2,
            "The local executor is this machine; use `pulse settings set workers N` to size it",
        )
        .into());
    }
    if !EXECUTOR_STATUSES.contains(&status) {
        return Err(fail(
            2,
            format!("Status must be one of: {}", EXECUTOR_STATUSES.join(", ")),
        )
        .into());
    }
    if is_nil(&executor_row(s, name)) {
        return Err(fail(2, format!("No executor {name}")).into());
    }
    let note_arg = note;
    let conn = s.conn().unwrap();
    let args: [&dyn ToSql; 7] = [
        &status, &note_arg, &status, &status, &status, &status, &name,
    ];
    exec1(
        &conn,
        "UPDATE executors SET status = ?, probe_note = COALESCE(?, probe_note), errors = CASE WHEN ? = 'active' THEN 0 ELSE errors END, backoff_minutes = CASE WHEN ? = 'active' THEN 0 ELSE backoff_minutes END, next_probe_at = CASE WHEN ? = 'active' THEN '' ELSE next_probe_at END, healed_at = CASE WHEN ? = 'active' THEN '' ELSE healed_at END WHERE name = ?",
        &args,
    );
    event_at(
        &conn,
        now,
        actor,
        &format!("executor.{status}"),
        name,
        note.map(|s| Value::String(s.to_string()))
            .unwrap_or_else(|| Value::String(String::new())),
    );
    Ok(executor_row(s, name))
}

pub(crate) fn note_executor_run(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    status: &str,
    note: &str,
    transport_error: bool,
    fault: bool,
) {
    if name == LOCAL_EXECUTOR {
        return;
    }
    let r = executor_row(s, name);
    if is_nil(&r) {
        return;
    }
    let conn = s.conn().unwrap();
    if fault {
        let probe_note = trunc(&format!("fault: {note}"), 300);
        exec1(
            &conn,
            "UPDATE executors SET errors = errors + 1, probe_note = ?, status = CASE WHEN status = 'active' THEN 'drained' ELSE status END WHERE name = ?",
            &[&probe_note, &name],
        );
        event_at(
            &conn,
            now,
            "runner",
            "executor.drained",
            name,
            json!({"fault": trunc(note, 200)}),
        );
        if login_fault(note) {
            let next = plus_minutes(now, f64::from(LOGIN_BACKOFF_MINS[0]));
            exec1(
                &conn,
                "UPDATE executors SET backoff_minutes = ?, next_probe_at = ? WHERE name = ?",
                &[&LOGIN_BACKOFF_MINS[0], &next, &name],
            );
        }
        return;
    }
    if transport_error {
        let errs = r.int("errors") + 1;
        let failed = if status == "error" { 0 } else { 1 };
        if errs >= UNREACHABLE_AFTER && r.str("status") == "active" {
            let next = plus_minutes(now, 1.0);
            let probe_note = trunc(note, 200);
            let args: [&dyn ToSql; 5] = [&errs, &failed, &next, &probe_note, &name];
            exec1(
                &conn,
                "UPDATE executors SET errors = ?, runs_failed = runs_failed + ?, status = 'unreachable', backoff_minutes = 1, next_probe_at = ?, probe_note = ? WHERE name = ?",
                &args,
            );
            event_at(
                &conn,
                now,
                "runner",
                "executor.unreachable",
                name,
                json!({"errors": errs, "note": trunc(note, 200)}),
            );
        } else {
            exec1(
                &conn,
                "UPDATE executors SET errors = ?, runs_failed = runs_failed + ? WHERE name = ?",
                &[&errs, &failed, &name],
            );
        }
        return;
    }
    if status == "error" {
        return;
    }
    let done = i32::from(status == "done" || status == "dry-run");
    let failed = i32::from(done != 1 && status != "stale");
    exec1(
        &conn,
        "UPDATE executors SET errors = 0, runs_done = runs_done + ?, runs_failed = runs_failed + ? WHERE name = ?",
        &[&done, &failed, &name],
    );
}

pub(crate) fn ssh_command(r: &Row, args: &[String]) -> Result<Vec<String>> {
    let (user, host, port) = parse_ssh_address(&r.str("address"))?;
    let mut payload = Vec::new();
    for a in args {
        payload.extend_from_slice(a.as_bytes());
        payload.push(0);
    }
    let mut cmd = vec!["ssh".to_string()];
    cmd.extend(SSH_OPTIONS.iter().map(|s| s.to_string()));
    cmd.extend([
        "-p".to_string(),
        port.to_string(),
        format!("{user}@{host}"),
        format!(
            "b64 {}",
            base64::engine::general_purpose::STANDARD.encode(payload)
        ),
    ]);
    Ok(cmd)
}

/// The ssh runner tests swap for a fake. Tests install closures that
/// capture state, so the hook is a shared trait object rather than a
/// plain fn pointer.
pub(crate) type SshRunner = std::sync::Arc<
    dyn Fn(
            &pulse_bus::CancellationToken,
            &[String],
            &[u8],
            Duration,
        ) -> Result<(i32, String, String)>
        + Send
        + Sync,
>;
pub(crate) static RUN_SSH: LazyLock<RwLock<SshRunner>> =
    LazyLock::new(|| RwLock::new(std::sync::Arc::new(run_process)));

/// run_ssh_hook returns the current `runSSH` implementation.
pub(crate) fn run_ssh_hook() -> SshRunner {
    RUN_SSH.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// set_run_ssh installs `f` as `runSSH` and returns the previous hook so a
/// test can restore it.
#[allow(dead_code)]
pub(crate) fn set_run_ssh<F>(f: F) -> SshRunner
where
    F: Fn(
            &pulse_bus::CancellationToken,
            &[String],
            &[u8],
            Duration,
        ) -> Result<(i32, String, String)>
        + Send
        + Sync
        + 'static,
{
    std::mem::replace(
        &mut *RUN_SSH.write().unwrap_or_else(|e| e.into_inner()),
        std::sync::Arc::new(f),
    )
}

/// restore_run_ssh puts a hook returned by [`set_run_ssh`] or [`run_ssh_hook`] back.
#[allow(dead_code)]
pub(crate) fn restore_run_ssh(old: SshRunner) {
    *RUN_SSH.write().unwrap_or_else(|e| e.into_inner()) = old;
}

pub(crate) fn run_process(
    cancel: &pulse_bus::CancellationToken,
    argv: &[String],
    stdin: &[u8],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    let out =
        match crate::proc::run_group(cancel, argv, "", &crate::proc::environ(), stdin, timeout) {
            Ok(o) => o,
            Err(e) => {
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|ioe| ioe.kind() == std::io::ErrorKind::NotFound)
                {
                    return Err(
                        std::io::Error::new(std::io::ErrorKind::NotFound, "not found").into(),
                    );
                }
                return Err(e);
            }
        };
    if out.timeout {
        if cancel.is_cancelled() {
            return Err(pulse_bus::ContextError::Canceled.into());
        }
        return Err(pulse_bus::ContextError::DeadlineExceeded.into());
    }
    Ok((out.exit_code, out.stdout, out.stderr))
}

pub(crate) fn ssh_run(
    r: &Row,
    args: &[String],
    stdin: &[u8],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    ssh_run_context(
        &pulse_bus::CancellationToken::new(),
        r,
        args,
        stdin,
        timeout,
    )
}

pub(crate) fn ssh_run_context(
    cancel: &pulse_bus::CancellationToken,
    r: &Row,
    args: &[String],
    stdin: &[u8],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    let argv = ssh_command(r, args)?;
    let runner = run_ssh_hook();
    let res = runner(cancel, &argv, stdin, timeout);
    match res {
        Err(err) => {
            if cancel.is_cancelled() {
                return Err(err);
            }
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|ioe| ioe.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(ExecutorUnreachable {
                    msg: "ssh is not installed on this machine".to_string(),
                }
                .into());
            }
            if err.to_string().contains("deadline exceeded") {
                let verb = args.first().map(String::as_str).unwrap_or("");
                return Err(ExecutorUnreachable {
                    msg: format!("ssh {verb} timed out after {}s", timeout.as_secs()),
                }
                .into());
            }
            Err(err)
        }
        Ok((rc, out, err_text)) => {
            if rc == SSH_EXIT_UNREACH {
                let mut msg = tail(err_text.trim(), 300);
                if msg.is_empty() {
                    msg = "ssh exit 255".to_string();
                }
                return Err(ExecutorUnreachable { msg }.into());
            }
            Ok((rc, out, err_text))
        }
    }
}

pub(crate) fn admin_ssh_command(
    address: &str,
    remote_command: &str,
    control_path: &str,
) -> Result<Vec<String>> {
    let (user, host, port) = parse_ssh_address(address)?;
    let mut cmd = vec!["ssh".to_string()];
    cmd.extend(SSH_OPTIONS.iter().map(|s| s.to_string()));
    if !control_path.is_empty() {
        cmd.extend(["-o".to_string(), format!("ControlPath={control_path}")]);
    }
    cmd.extend([
        "-p".to_string(),
        port.to_string(),
        format!("{user}@{host}"),
        remote_command.to_string(),
    ]);
    Ok(cmd)
}

pub(crate) fn admin_ssh_run(
    address: &str,
    remote_command: &str,
    stdin: &[u8],
    timeout: Duration,
    control_path: &str,
) -> Result<(i32, String, String)> {
    let argv = admin_ssh_command(address, remote_command, control_path)?;
    let cancel = pulse_bus::CancellationToken::new();
    let runner = run_ssh_hook();
    match runner(&cancel, &argv, stdin, timeout) {
        Err(err) => {
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|ioe| ioe.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(ExecutorUnreachable {
                    msg: "ssh is not installed on this machine".to_string(),
                }
                .into());
            }
            if err.to_string().contains("deadline exceeded") {
                let user = address.split('@').next().unwrap_or("");
                return Err(ExecutorUnreachable {
                    msg: format!("ssh to {user}@... timed out after {}s", timeout.as_secs()),
                }
                .into());
            }
            Err(err)
        }
        Ok((rc, out, err_text)) => {
            if rc == SSH_EXIT_UNREACH {
                let mut msg = tail(err_text.trim(), 300);
                if msg.is_empty() {
                    msg = "ssh exit 255".to_string();
                }
                return Err(ExecutorUnreachable { msg }.into());
            }
            Ok((rc, out, err_text))
        }
    }
}

pub(crate) const LOGIN_PREP_SCRIPT: &str = "set -e; d=\"$HOME/.copilot\"; f=\"$d/settings.json\"; umask 077; mkdir -p \"$d\"; chmod 700 \"$d\"; if [ ! -s \"$f\" ]; then printf \"{\\n  \\\"storeTokenPlaintext\\\": true\\n}\\n\" > \"$f\"; echo settings=written; elif grep -q \"\\\"storeTokenPlaintext\\\"[[:space:]]*:[[:space:]]*true\" \"$f\"; then echo settings=present; elif command -v node >/dev/null 2>&1; then node -e \"const fs=require(\\\"fs\\\");const p=process.argv[1];const c=JSON.parse(fs.readFileSync(p,\\\"utf8\\\"));c.storeTokenPlaintext=true;fs.writeFileSync(p,JSON.stringify(c,null,2)+\\\"\\n\\\")\" \"$f\"; echo settings=merged; else echo settings=unmergeable; exit 4; fi";
pub(crate) static LOGIN_PREP_COMMAND: LazyLock<String> =
    LazyLock::new(|| format!("sudo -n -u pulse -H sh -c '{}'", LOGIN_PREP_SCRIPT));
pub(crate) const LOGIN_COMMAND: &str = "sudo -n -u pulse -H timeout 90 copilot login --with-token";

pub(crate) fn classify_token(token: &str) -> Result<String> {
    if token.is_empty() {
        return Err(fail(2, "no token on standard input; pipe it, e.g. `op read op://vault/item/field | pulse executor login ...`").into());
    }
    if token.chars().any(char::is_whitespace) {
        return Err(fail(2, "the token must be one line with no whitespace inside").into());
    }
    if token.starts_with("github_pat_") {
        Ok("pat".to_string())
    } else if token.starts_with("gho_") {
        Ok("oauth".to_string())
    } else if token.starts_with("ghu_") {
        Ok("user".to_string())
    } else if token.starts_with("ghp_") {
        Err(fail(2, "a classic personal access token (ghp_) is rejected by the Copilot CLI; create a fine-grained token with the Copilot Requests permission (github_pat_)").into())
    } else {
        Err(fail(2, "unsupported token prefix; use a fine-grained token (github_pat_), device-flow token (gho_), or user-to-server token (ghu_)").into())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenFacts {
    pub ok: Option<bool>,
    pub user: String,
    pub expires: String,
    pub note: String,
}

pub(crate) fn github_api_base(api: &str) -> String {
    let mut v = api.to_string();
    if v.is_empty() {
        v = std::env::var("PULSE_GITHUB_API").unwrap_or_default();
    }
    if v.is_empty() {
        v = GITHUB_API.to_string();
    }
    v.trim_end_matches('/').to_string()
}

/// Carries every call to the GitHub API; tests point it at a fake transport.
/// (token, url) -> (status, lower-cased headers, body).
pub(crate) type GithubHttpFn =
    dyn Fn(&str, &str) -> Result<(u16, BTreeMap<String, String>, Vec<u8>)> + Send + Sync;
pub(crate) static GITHUB_HTTP: LazyLock<RwLock<Box<GithubHttpFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(github_http_real)));

/// Swaps the GitHub transport, returning the previous one so a test can restore it.
#[cfg(test)]
pub(crate) fn set_github_http(f: Box<GithubHttpFn>) -> Box<GithubHttpFn> {
    std::mem::replace(&mut *GITHUB_HTTP.write().expect("github http poisoned"), f)
}

pub(crate) fn github_http_real(
    token: &str,
    url: &str,
) -> Result<(u16, BTreeMap<String, String>, Vec<u8>)> {
    static CLIENT: LazyLock<reqwest::blocking::Client> = LazyLock::new(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("github http client")
    });
    let resp = CLIENT
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "pulse")
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| anyhow!("Get \"{url}\": {}", redact_tokens(&e.to_string())))?;
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_ascii_lowercase(),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let body = resp.bytes().map(|b| b.to_vec()).unwrap_or_default();
    Ok((status, headers, body))
}

fn github_get_full(token: &str, url: &str) -> Result<(u16, BTreeMap<String, String>, Vec<u8>)> {
    (GITHUB_HTTP.read().expect("github http poisoned"))(token, url)
}

#[allow(dead_code)]
pub(crate) fn github_get(token: &str, url: &str) -> Result<(u16, Vec<u8>)> {
    let (status, _, body) = github_get_full(token, url)?;
    Ok((status, body))
}

pub(crate) fn github_token_facts(token: &str, api: &str) -> TokenFacts {
    match github_get_full(token, &(github_api_base(api) + "/user")) {
        Err(err) => TokenFacts {
            note: format!(
                "GitHub not reached ({}); the token's validity is unknown",
                cut(&redact_tokens(&err.to_string()), 80)
            ),
            ..TokenFacts::default()
        },
        Ok((status, headers, body)) if (200..300).contains(&status) => {
            let parsed: BTreeMap<String, Value> = serde_json::from_slice(&body).unwrap_or_default();
            let user = parsed
                .get("login")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let expires = headers
                .get("github-authentication-token-expiration")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let mut note = "GitHub accepts the token".to_string();
            if !user.is_empty() {
                note.push_str(" for ");
                note.push_str(&user);
            }
            if !expires.is_empty() {
                note.push_str(", expires ");
                note.push_str(&expires);
            }
            TokenFacts {
                ok: Some(true),
                user,
                expires,
                note,
            }
        }
        Ok((status, _, _)) if status == 401 || status == 403 => TokenFacts {
            ok: Some(false),
            note: format!(
                "GitHub refuses the token ({status}): invalid, expired, or revoked - the token was not sent"
            ),
            ..TokenFacts::default()
        },
        Ok((status, _, _)) => TokenFacts {
            note: format!("GitHub answered {status}; the token's validity is unknown"),
            ..TokenFacts::default()
        },
    }
}

#[allow(dead_code)]
pub(crate) const NO_REPOSITORY_REFUSAL: &str = "the executor token must reach no repository: mint a fine-grained token with only the Copilot Requests permission";

#[allow(dead_code)]
pub(crate) fn github_no_repos(token: &str, api: &str) -> (bool, String) {
    match github_get(
        token,
        &(github_api_base(api)
            + "/user/repos?per_page=1&affiliation=owner,collaborator,organization_member"),
    ) {
        Err(err) => (
            false,
            format!(
                "GitHub not reached ({}); the token's reach is unknown",
                cut(&redact_tokens(&err.to_string()), 80)
            ),
        ),
        Ok((status, _)) if !(200..300).contains(&status) => (
            false,
            format!(
                "GitHub answered {status} to the repository check; the token's reach is unknown"
            ),
        ),
        Ok((_, body)) => {
            let repos: serde_json::Result<Vec<Value>> = serde_json::from_slice(&body);
            match repos {
                Err(_) => (
                    false,
                    "GitHub's repository answer was not a list; the token's reach is unknown"
                        .to_string(),
                ),
                Ok(v) if !v.is_empty() => (false, NO_REPOSITORY_REFUSAL.to_string()),
                Ok(_) => (true, "the token reaches no repository".to_string()),
            }
        }
    }
}

pub(crate) fn expiry_warning(expires: &str, now: &str, days: i32) -> String {
    if expires.is_empty() {
        return String::new();
    }
    let Some(caps) = EXPIRY_DATE_RE.captures(expires) else {
        return String::new();
    };
    if now.len() < 10 {
        return String::new();
    }
    let Ok(when) = chrono::NaiveDate::parse_from_str(&caps[1], "%Y-%m-%d") else {
        return String::new();
    };
    let Ok(reference) = chrono::NaiveDate::parse_from_str(&now[..10], "%Y-%m-%d") else {
        return String::new();
    };
    let left = (when - reference).num_days();
    if left < 0 {
        format!("the token expired on {}", &caps[1])
    } else if left <= i64::from(days) {
        format!("the token expires in {left} day(s), on {}", &caps[1])
    } else {
        String::new()
    }
}

pub(crate) fn login_check_command(s: &pulse_store::Store) -> Vec<String> {
    if s.harness_native() {
        let mut cmd = vec![
            "pulse".to_string(),
            "harness".to_string(),
            "run".to_string(),
            "-p".to_string(),
            NATIVE_LOGIN_PROMPT.to_string(),
            "--max-steps".to_string(),
            "2".to_string(),
            "--no-store".to_string(),
            "--quiet".to_string(),
        ];
        let model = s.light_model();
        if !model.is_empty() {
            cmd.extend(["--model".to_string(), model]);
        }
        return cmd;
    }
    let mut cmd = vec![
        runner_cmd(s),
        "-p".to_string(),
        LOGIN_CHECK_PROMPT.to_string(),
        "--no-ask-user".to_string(),
        "--disable-builtin-mcps".to_string(),
        "-s".to_string(),
    ];
    let model = s.light_model();
    if !model.is_empty() {
        cmd.extend(["--model".to_string(), model]);
    }
    cmd
}

pub(crate) fn runner_cmd(s: &pulse_store::Store) -> String {
    let v = s.setting("runner_cmd", "");
    if v.is_empty() {
        "copilot".to_string()
    } else {
        v
    }
}

pub(crate) fn tiny_bundle() -> Result<Vec<u8>> {
    let base = std::env::current_dir()?.join("target");
    std::fs::create_dir_all(&base)?;
    let tmp = base.join(format!("pulse-login-check-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)?;
    let result = (|| -> Result<Vec<u8>> {
        let repo = tmp.join("repo");
        let bundle = tmp.join("check.bundle");
        let tmp_s = tmp.to_string_lossy().to_string();
        let repo_s = repo.to_string_lossy().to_string();
        let bundle_s = bundle.to_string_lossy().to_string();
        git_check(
            &tmp_s,
            &["init", "-q", "--initial-branch=main", repo_s.as_str()],
        )?;
        git_check(
            &repo_s,
            &[
                "-c",
                "user.name=pulse",
                "-c",
                "user.email=pulse@pulse.local",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "login check",
            ],
        )?;
        git_check(
            &repo_s,
            &["bundle", "create", "-q", bundle_s.as_str(), "main"],
        )?;
        Ok(std::fs::read(bundle)?)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

pub(crate) fn login_check(
    s: &pulse_store::Store,
    name: &str,
    actor: &str,
    now: &str,
    apply: bool,
) -> Result<(bool, String)> {
    let mut r = executor_row(s, name);
    if is_nil(&r) || name == LOCAL_EXECUTOR {
        return Err(fail(2, format!("No executor {name}")).into());
    }
    let job = format!(
        "login-check-{}",
        Regex::new(r"[^0-9A-Za-z]").unwrap().replace_all(now, "")
    );
    let started = Instant::now();
    let mut ok = false;
    let kind: String;
    let said: String;
    let mut rc = 0;
    let bundle = tiny_bundle()?;
    match ssh_run(
        &r,
        &["prepare".into(), job.clone(), "main".into(), "pulse".into()],
        &bundle,
        Duration::from_secs(120),
    ) {
        Err(err) if err.downcast_ref::<ExecutorUnreachable>().is_some() => {
            let u = err.downcast_ref::<ExecutorUnreachable>().unwrap();
            kind = "unreachable".to_string();
            said = u.msg.clone();
        }
        Err(err) => return Err(err),
        Ok((code, out, err_text)) if code != 0 => {
            rc = code;
            let src = if err_text.is_empty() { out } else { err_text };
            kind = "hands".to_string();
            said = format!(
                "prepare failed (exit {code}): {}",
                tail(&words_joined(&src), 200)
            );
        }
        Ok(_) => {
            let mut args = vec![
                "run".to_string(),
                job.clone(),
                LOGIN_CHECK_MINUTES.to_string(),
                "--".to_string(),
            ];
            args.extend(login_check_command(s));
            match ssh_run(
                &r,
                &args,
                &[],
                Duration::from_secs((LOGIN_CHECK_MINUTES * 60 + 60) as u64),
            ) {
                Err(err) if err.downcast_ref::<ExecutorUnreachable>().is_some() => {
                    let u = err.downcast_ref::<ExecutorUnreachable>().unwrap();
                    kind = "unreachable".to_string();
                    said = u.msg.clone();
                }
                Err(err) => return Err(err),
                Ok((code, out, err_text)) => {
                    rc = code;
                    said = fault_line(&(err_text.clone() + "\n" + &out), 200);
                    if s.harness_native() {
                        if rc == 0 && out.contains("SUMMARY:") {
                            ok = true;
                            kind = "ok".to_string();
                        } else if rc == 4
                            || err_text.trim().to_lowercase().starts_with("not logged in:")
                            || login_fault(&said)
                        {
                            kind = "refused".to_string();
                        } else {
                            kind = "failed".to_string();
                        }
                    } else if rc == 0 && !login_fault(&said) {
                        ok = true;
                        kind = "ok".to_string();
                    } else if login_fault(&said) {
                        kind = "refused".to_string();
                    } else {
                        kind = "failed".to_string();
                    }
                }
            }
            let _ = ssh_run(&r, &["clean".into(), job], &[], Duration::from_secs(60));
        }
    }
    let seconds = started.elapsed().as_secs() as i32;
    let worker = if s.harness_native() {
        "harness"
    } else {
        "copilot"
    };
    let note = if ok {
        format!("login ok: {worker} answered in {seconds}s")
    } else if kind == "refused" {
        format!(
            "{worker} refuses the login on {name}: {said} - GitHub accepts the token but Copilot does not: a fine-grained token needs the Copilot Requests permission, and the account needs Copilot; fix the token on GitHub or place a new one with `pulse executor login {name} --admin <admin@host> --with-token`"
        )
    } else if kind == "failed" {
        format!("{worker} failed on {name} (exit {rc}): {said}")
    } else {
        format!("{kind}: {}", cut(&redact_tokens(&said), 200))
    };
    let conn = s.conn().unwrap();
    event_at(
        &conn,
        now,
        actor,
        "executor.login-check",
        name,
        json!({"ok": ok, "kind": kind, "note": trunc(&note, 300), "seconds": seconds}),
    );
    if apply {
        r = executor_row(s, name);
        if ok && r.str("status") == "drained" {
            let n = trunc(&format!("ok: {note}"), 300);
            set_executor_status(s, now, name, "active", actor, Some(&n))?;
            exec1(
                &conn,
                "UPDATE executors SET backoff_minutes = 0, next_probe_at = '', errors = 0 WHERE name = ?",
                &[&name],
            );
            event_at(
                &conn,
                now,
                actor,
                "executor.recovered",
                name,
                json!({"note": trunc(&note, 200)}),
            );
        } else if ok {
            exec1(
                &conn,
                "UPDATE executors SET backoff_minutes = 0, next_probe_at = '' WHERE name = ?",
                &[&name],
            );
        } else {
            let wait = (r.int("backoff_minutes") * 2)
                .max(LOGIN_BACKOFF_MINS[0])
                .min(LOGIN_BACKOFF_MINS[1]);
            let next = plus_minutes(now, f64::from(wait));
            if kind == "refused" {
                let probe_note = trunc(&format!("fault: {note}"), 300);
                let args: [&dyn ToSql; 4] = [&probe_note, &wait, &next, &name];
                exec1(
                    &conn,
                    "UPDATE executors SET probe_note = ?, backoff_minutes = ?, next_probe_at = ?, status = CASE WHEN status = 'active' THEN 'drained' ELSE status END WHERE name = ?",
                    &args,
                );
                if r.str("status") == "active" {
                    event_at(
                        &conn,
                        now,
                        actor,
                        "executor.drained",
                        name,
                        json!({"fault": trunc(&note, 200)}),
                    );
                }
            } else if r.str("status") == "drained" {
                exec1(
                    &conn,
                    "UPDATE executors SET backoff_minutes = ?, next_probe_at = ? WHERE name = ?",
                    &[&wait, &next, &name],
                );
            }
        }
    }
    Ok((ok, note))
}

pub(crate) fn set_executor_admin(s: &pulse_store::Store, name: &str, admin: &str) {
    let admin = admin.trim();
    if !admin.is_empty() {
        let conn = s.conn().unwrap();
        exec1(
            &conn,
            "UPDATE executors SET admin = ? WHERE name = ?",
            &[&admin, &name],
        );
    }
}

pub(crate) fn login_drained(r: &Row) -> bool {
    r.str("status") == "drained"
        && login_fault(&r.str("probe_note"))
        && !r.str("next_probe_at").is_empty()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LoginResult {
    pub(crate) ok: bool,
    pub(crate) kind: String,
    pub(crate) rc: i32,
    pub(crate) lines: Vec<String>,
}

pub(crate) fn last_lines(text: &str, n: usize) -> Vec<String> {
    let trimmed = redact_tokens(text).trim_end_matches('\n').to_string();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let lines = trimmed
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

pub(crate) type PlaceLoginFn = fn(
    &pulse_store::Store,
    &str,
    &str,
    &str,
    &mut dyn Read,
    bool,
    bool,
    &str,
) -> Result<LoginResult>;
pub(crate) static PLACE_LOGIN: LazyLock<RwLock<PlaceLoginFn>> =
    LazyLock::new(|| RwLock::new(executor_login));

#[allow(clippy::too_many_arguments)]
pub(crate) fn executor_login(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    admin_address: &str,
    token: &mut dyn Read,
    is_tty: bool,
    remove: bool,
    actor: &str,
) -> Result<LoginResult> {
    let r = executor_row(s, name);
    if is_nil(&r) || name == LOCAL_EXECUTOR {
        return Err(fail(2, format!("No executor {name}")).into());
    }
    parse_ssh_address(admin_address)?;
    set_executor_admin(s, name, admin_address);
    let mut res = LoginResult {
        lines: Vec::new(),
        ..LoginResult::default()
    };
    let conn = s.conn().unwrap();

    if remove {
        res.kind = "removed".to_string();
        let remove_command = if s.harness_native() {
            r#"sudo -n -u pulse -H sh -c 'rm -f -- "$HOME/.pulse/private/providers/copilot.json"'"#
        } else {
            r#"sudo -n -u pulse -H sh -c 'rm -rf -- "$HOME/.copilot"'"#
        };
        match admin_ssh_run(
            admin_address,
            remove_command,
            &[],
            Duration::from_secs(120),
            "",
        ) {
            Err(err) if err.downcast_ref::<ExecutorUnreachable>().is_some() => {
                let u = err.downcast_ref::<ExecutorUnreachable>().unwrap();
                res.rc = SSH_EXIT_UNREACH;
                res.lines.push(format!("{name} unreachable: {}", u.msg));
                log_login_event(&conn, now, actor, name, &res, json!({}));
                return Ok(res);
            }
            Err(err) => return Err(err),
            Ok((rc, out, err_text)) => {
                res.rc = rc;
                res.ok = rc == 0;
                if res.ok {
                    res.lines
                        .push(format!("login removed for user pulse on {name}"));
                    let before = r.str("status");
                    let (probe_ok, note) = probe_executor(s, name, actor, now)?;
                    let mut status = executor_row(s, name).str("status");
                    if !probe_ok && before == "active" {
                        let st = set_executor_status(s, now, name, "drained", actor, Some(&note))?;
                        status = st.str("status");
                    }
                    if probe_ok {
                        res.lines.push(format!("{name}: ok, {status}: {note}"));
                    } else {
                        res.lines
                            .push(format!("{name}: probe FAILED ({status}): {note}"));
                    }
                } else {
                    res.lines.push(format!("login removal failed (exit {rc})"));
                    res.lines.extend(last_lines(&(out + &err_text), 5));
                }
            }
        }
        log_login_event(&conn, now, actor, name, &res, json!({}));
        return Ok(res);
    }

    if is_tty {
        return Err(fail(2, "the token must be piped, not typed").into());
    }
    let reader: &mut dyn Read = token;
    let mut raw = String::new();
    reader.read_to_string(&mut raw)?;
    let mut source = String::new();
    let tok = if raw.trim().is_empty() {
        let (tok, origin) = crate::token::login_source(s)?;
        if tok.is_empty() {
            return Err(fail(
                2,
                format!(
                    "No login source for executor {name} ({}) at {admin_address} workdir {} (no_stored_login). Sign in in Settings -> Harness or run `pulse harness login`, or pass `pulse executor token --with-token` / `pulse executor login {name} --admin {admin_address} --with-token`.",
                    r.str("address"),
                    r.str("workdir")
                ),
            )
            .into());
        }
        source = origin;
        String::from_utf8(tok)?
    } else {
        raw.trim().to_string()
    };
    let kind = classify_token(&tok)?;
    res.kind = kind.clone();
    if !source.is_empty() {
        res.lines.push(format!("login source: {source}"));
    }
    let facts = github_token_facts(&tok, "");
    res.lines.push(facts.note.clone());
    if facts.ok == Some(false) {
        res.rc = 1;
        log_login_event(&conn, now, actor, name, &res, json!({"step": "github"}));
        return Ok(res);
    }
    if !expiry_warning(&facts.expires, now, LOGIN_EXPIRY_WARN).is_empty() {
        res.lines.push(format!(
            "warning: {}",
            expiry_warning(&facts.expires, now, LOGIN_EXPIRY_WARN)
        ));
    }
    let mut settings_note = "settings=unknown".to_string();
    let login_cmd = if s.harness_native() {
        settings_note = "settings=native".to_string();
        "sudo -n -u pulse -H timeout 90 pulse harness login --with-token".to_string()
    } else {
        match admin_ssh_run(
            admin_address,
            &LOGIN_PREP_COMMAND,
            &[],
            Duration::from_secs(120),
            "",
        ) {
            Err(err) if err.downcast_ref::<ExecutorUnreachable>().is_some() => {
                let u = err.downcast_ref::<ExecutorUnreachable>().unwrap();
                res.rc = SSH_EXIT_UNREACH;
                res.lines.push(format!("{name} unreachable: {}", u.msg));
                log_login_event(&conn, now, actor, name, &res, json!({}));
                return Ok(res);
            }
            Err(err) => return Err(err),
            Ok((prep_rc, prep_out, prep_err)) => {
                if prep_rc != 0 {
                    res.rc = prep_rc;
                    res.lines.push(format!("preparing ~/.copilot/settings.json for user pulse on {name} failed (exit {prep_rc}); the token was not sent"));
                    res.lines.extend(last_lines(&(prep_out + &prep_err), 5));
                    log_login_event(&conn, now, actor, name, &res, json!({"step": "settings"}));
                    return Ok(res);
                }
                for l in prep_out.split('\n') {
                    if l.starts_with("settings=") {
                        settings_note = l.trim().to_string();
                        break;
                    }
                }
            }
        }
        LOGIN_COMMAND.to_string()
    };
    let mut input = tok.into_bytes();
    input.push(b'\n');
    match admin_ssh_run(
        admin_address,
        &login_cmd,
        &input,
        Duration::from_secs(120),
        "",
    ) {
        Err(err) if err.downcast_ref::<ExecutorUnreachable>().is_some() => {
            let u = err.downcast_ref::<ExecutorUnreachable>().unwrap();
            res.rc = SSH_EXIT_UNREACH;
            res.lines.push(format!("{name} unreachable: {}", u.msg));
            log_login_event(&conn, now, actor, name, &res, json!({}));
            return Ok(res);
        }
        Err(err) => return Err(err),
        Ok((rc, out, err_text)) => {
            res.rc = rc;
            res.ok = rc == 0;
            if res.ok {
                res.lines.push(format!(
                    "login stored for user pulse on {name} (kind {kind}, {settings_note})"
                ));
                let (probe_ok, note) = probe_executor(s, name, actor, now)?;
                if probe_ok {
                    let (check_ok, check_note) = login_check(s, name, actor, now, true)?;
                    let status = executor_row(s, name).str("status");
                    if check_ok {
                        res.lines
                            .push(format!("{name}: ok, {status}: {check_note}; {note}"));
                    } else {
                        res.ok = false;
                        res.lines.push(format!(
                            "{name}: login check FAILED ({status}): {check_note}"
                        ));
                    }
                } else {
                    let status = executor_row(s, name).str("status");
                    res.lines
                        .push(format!("{name}: probe FAILED ({status}): {note}"));
                }
            } else {
                res.lines.push(format!("login failed (exit {rc})"));
                res.lines.extend(last_lines(&(out + &err_text), 5));
            }
        }
    }
    let mut extra = serde_json::Map::new();
    if !source.is_empty() {
        extra.insert("source".to_string(), json!(source));
    }
    if !facts.expires.is_empty() {
        extra.insert("expires".to_string(), json!(facts.expires));
    }
    if !facts.user.is_empty() {
        extra.insert("user".to_string(), json!(facts.user));
    }
    log_login_event(&conn, now, actor, name, &res, Value::Object(extra));
    Ok(res)
}

fn log_login_event(
    conn: &impl crate::util::Querier,
    now: &str,
    actor: &str,
    name: &str,
    res: &LoginResult,
    extra: Value,
) {
    let mut d = serde_json::Map::new();
    d.insert("kind".to_string(), json!(res.kind));
    d.insert("rc".to_string(), json!(res.rc));
    d.insert("ok".to_string(), json!(res.ok));
    if let Value::Object(m) = extra {
        for (k, v) in m {
            d.insert(k, v);
        }
    }
    event_at(conn, now, actor, "executor.login", name, Value::Object(d));
}

pub(crate) fn parse_probe(text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    for tok in text.split_whitespace() {
        if let Some((k, v)) = tok.split_once('=') {
            fields.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    fields
}

/// The local executor is this machine: no ssh, no hands. Its probe answers what a hand's
/// probe answers - git present, room under `PULSE_HOME/work` - and, as for a hand, a
/// human's passing probe clears the projects marked unfit here, so the owner who fixed
/// the machine does not wait a day for the marks to expire.
pub(crate) fn local_work_root(s: &pulse_store::Store) -> std::path::PathBuf {
    std::path::Path::new(&s.home).join("work")
}

pub(crate) fn free_mb(path: &std::path::Path) -> i32 {
    std::process::Command::new("df")
        .args(["-Pm", &path.display().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|t| {
            t.lines().nth(1).and_then(|l| {
                l.split_whitespace()
                    .nth(3)
                    .and_then(|n| n.parse::<i32>().ok())
            })
        })
        .unwrap_or(0)
}

pub(crate) fn disk_floor_hold(s: &pulse_store::Store) -> Option<(i32, i32, String)> {
    let min_free = setting_int(s, "daemon.min_free_mb");
    if min_free <= 0 {
        return None;
    }
    let work = local_work_root(s);
    let _ = std::fs::create_dir_all(&work);
    let free = free_mb(&work);
    (free < min_free).then(|| (free, min_free, work.display().to_string()))
}

pub(crate) fn disk_floor_note(s: &pulse_store::Store) -> Option<String> {
    disk_floor_hold(s).map(|(free, floor, path)| {
        format!("disk under floor: {free} MB free on {path} (daemon.min_free_mb {floor})")
    })
}

fn probe_local(s: &pulse_store::Store, actor: &str, now: &str) -> (bool, String) {
    let mut problems = Vec::new();
    let gitv = std::process::Command::new("git")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|v| v.split_whitespace().nth(2).map(ToOwned::to_owned))
        .unwrap_or_else(|| "missing".into());
    if gitv == "missing" {
        problems.push("git missing".to_string());
    }
    let work = local_work_root(s);
    let _ = std::fs::create_dir_all(&work);
    let free_mb = free_mb(&work);
    let min_free = setting_int(s, "daemon.min_free_mb");
    if min_free > 0 && free_mb < min_free {
        let want = if min_free % 1024 == 0 {
            format!("{} GiB", min_free / 1024)
        } else {
            format!("{min_free} MB")
        };
        problems.push(format!("under {want} free on {}", work.display()));
    }
    if !problems.is_empty() {
        return (false, problems.join("; "));
    }
    let mut note = format!("ok: git={gitv} disk_free_mb={free_mb}");
    if actor == crate::util::HUMAN_AGENT
        && let Ok(conn) = s.conn()
    {
        let marks = crate::recipe::unfit_projects(&conn, LOCAL_EXECUTOR, now);
        crate::recipe::clear_unfit(&conn, now, LOCAL_EXECUTOR);
        if !marks.is_empty() {
            note.push_str(&format!(
                " unfit_cleared={}",
                marks.keys().cloned().collect::<Vec<_>>().join(",")
            ));
        }
    }
    (true, note)
}

pub(crate) fn probe_executor(
    s: &pulse_store::Store,
    name: &str,
    actor: &str,
    now: &str,
) -> Result<(bool, String)> {
    if name == LOCAL_EXECUTOR {
        return Ok(probe_local(s, actor, now));
    }
    let r = executor_row(s, name);
    if is_nil(&r) {
        return Err(fail(2, format!("No executor {name}")).into());
    }
    let drained_for_login = login_drained(&r);
    let mut ok = false;
    let mut note;
    match ssh_run(&r, &["probe".into()], &[], Duration::from_secs(60)) {
        Err(err) if err.downcast_ref::<ExecutorUnreachable>().is_some() => {
            let u = err.downcast_ref::<ExecutorUnreachable>().unwrap();
            note = format!("unreachable: {}", u.msg);
        }
        Err(err) => return Err(err),
        Ok((rc, out, err_text)) => {
            let mut fields = parse_probe(&out);
            let mut problems = Vec::new();
            if rc != 0 || fields.get("hands").map(String::as_str) != Some("ok") {
                let src = if err_text.is_empty() {
                    out.clone()
                } else {
                    err_text.clone()
                };
                let mut said = tail(&words_joined(&src), 160);
                if said.is_empty() {
                    said = "no output".to_string();
                }
                problems.push(format!("pulse-hands did not answer (exit {rc}: {said})"));
            } else {
                let get = |m: &BTreeMap<String, String>, k: &str, def: &str| {
                    m.get(k).cloned().unwrap_or_else(|| def.to_string())
                };
                if get(&fields, "git", "missing") == "missing" {
                    problems.push("git missing".to_string());
                }
                if s.harness_native() {
                    if get(&fields, "harness", "no") != "yes" {
                        problems.push(format!("harness login missing (run `pulse executor login {name} --admin <admin@host> --with-token`)"));
                    }
                } else if get(&fields, "copilot", "missing") == "missing" {
                    problems.push("copilot missing".to_string());
                }
                if fields.get("keys_ok").map(String::as_str) != Some("yes") {
                    problems.push("authorized key lacks command=/restrict".to_string());
                }
                if get(&fields, "remotes", "0") != "0" {
                    problems.push(format!(
                        "{} clone(s) with remotes",
                        fields.get("remotes").cloned().unwrap_or_default()
                    ));
                }
                if fields.get("workdir").is_some_and(|w| {
                    !w.is_empty()
                        && w.trim_end_matches('/') != r.str("workdir").trim_end_matches('/')
                }) {
                    problems.push(format!(
                        "workdir is {} not {}",
                        fields.get("workdir").cloned().unwrap_or_default(),
                        r.str("workdir")
                    ));
                }
                if !s.harness_native() && get(&fields, "logged_in", "yes") != "yes" {
                    problems.push(
                        "copilot is not logged in (run `copilot login` as the executor user)"
                            .to_string(),
                    );
                }
                if !fields.contains_key("keys") {
                    problems.push(format!("hands predate ADR-022 (no keys= field); refresh them with `pulse pair <admin@host> --name {name}`"));
                }
                let min_free = setting_int(s, "daemon.min_free_mb");
                if min_free > 0
                    && let Ok(n) = get(&fields, "disk_free_mb", "0").parse::<i32>()
                    && n < min_free
                {
                    let want = if min_free % 1024 == 0 {
                        format!("{} GiB", min_free / 1024)
                    } else {
                        format!("{min_free} MB")
                    };
                    let where_ = fields
                        .get("workdir")
                        .filter(|w| !w.is_empty())
                        .cloned()
                        .unwrap_or_else(|| r.str("workdir"));
                    problems.push(format!("under {want} free on {where_}"));
                }
            }
            ok = problems.is_empty();
            if ok {
                let swept = sweep_executor_jobs(
                    s,
                    now,
                    &r,
                    fields.get("jobs").map(String::as_str).unwrap_or(""),
                );
                if !swept.is_empty() {
                    fields.insert("jobs".to_string(), format!("swept:{}", swept.len()));
                } else if !fields.contains_key("jobs") {
                    fields.insert("jobs".to_string(), "-".to_string());
                }
                let mut stale = Vec::new();
                let keys = fields
                    .get("keys")
                    .filter(|k| !k.is_empty())
                    .cloned()
                    .unwrap_or_else(|| "-".to_string());
                let conn = s.conn().unwrap();
                for job_id in keys.split(',') {
                    if job_id.is_empty() || job_id == "-" || !JOB_DIR_RE.is_match(job_id) {
                        continue;
                    }
                    let live = must_one(
                        &conn,
                        "SELECT 1 FROM runs WHERE job_id = ? AND executor = ? AND status = 'running'",
                        &[&job_id, &name],
                    );
                    if is_nil(&live) && !swept.iter().any(|j| j == job_id) {
                        stale.push(job_id.to_string());
                    }
                }
                if !stale.is_empty() {
                    problems.push(format!(
                        "{} signing key(s) left without a live run: {}",
                        stale.len(),
                        stale.join(", ")
                    ));
                    ok = false;
                }
            }
            if ok {
                let mut keys = fields
                    .keys()
                    .filter(|k| k.as_str() != "hands")
                    .cloned()
                    .collect::<Vec<_>>();
                keys.sort();
                let parts = keys
                    .iter()
                    .map(|k| format!("{k}={}", fields.get(k).cloned().unwrap_or_default()))
                    .collect::<Vec<_>>();
                note = format!("ok: {}", parts.join(" "));
                if fields.get("pulse").map(String::as_str) == Some("present") {
                    note.push_str(&format!("; warning: hands predate pulse path reporting (pulse=present); refresh them with `pulse pair <admin@host> --name {name}`"));
                }
            } else {
                note = problems.join("; ");
            }
        }
    }
    note = trunc(&note, 300);
    let conn = s.conn().unwrap();
    if ok && drained_for_login {
        exec1(
            &conn,
            "UPDATE executors SET last_probe_at = ?, probe_note = ?, errors = 0 WHERE name = ?",
            &[
                &now,
                &trunc(
                    &format!("fault: login refused, awaiting the login check; {note}"),
                    300,
                ),
                &name,
            ],
        );
    } else if ok {
        let args: [&dyn ToSql; 4] = [&now, &note, &actor, &name];
        exec1(
            &conn,
            "UPDATE executors SET last_probe_at = ?, probe_note = ?, errors = 0, backoff_minutes = 0, next_probe_at = '', status = CASE WHEN status = 'drained' AND ? = 'human' THEN 'active' WHEN status = 'unreachable' THEN 'active' ELSE status END WHERE name = ?",
            &args,
        );
        if actor == crate::util::HUMAN_AGENT {
            crate::recipe::clear_unfit(&conn, now, name);
        }
    } else {
        exec1(
            &conn,
            "UPDATE executors SET last_probe_at = ?, probe_note = ? WHERE name = ?",
            &[&now, &note, &name],
        );
    }
    event_at(
        &conn,
        now,
        actor,
        "executor.probe",
        name,
        json!({"ok": ok, "note": note}),
    );
    Ok((ok, note))
}

pub(crate) fn sweep_executor_jobs(
    s: &pulse_store::Store,
    now: &str,
    r: &Row,
    jobs: &str,
) -> Vec<String> {
    let mut swept = Vec::new();
    let conn = s.conn().unwrap();
    for job_id in jobs.split(',') {
        if job_id.is_empty() || job_id == "-" || !JOB_DIR_RE.is_match(job_id) {
            continue;
        }
        let live = must_one(
            &conn,
            "SELECT 1 FROM runs WHERE job_id = ? AND executor = ? AND status = 'running'",
            &[&job_id, &r.str("name")],
        );
        if !is_nil(&live) {
            continue;
        }
        match ssh_run(
            r,
            &["clean".to_string(), job_id.to_string()],
            &[],
            Duration::from_secs(60),
        ) {
            Err(_) => break,
            Ok((0, _, _)) => {
                swept.push(job_id.to_string());
                event_at(
                    &conn,
                    now,
                    "runner",
                    "executor.sweep",
                    &r.str("name"),
                    json!({"job": job_id}),
                );
            }
            _ => {}
        }
    }
    swept
}

#[allow(dead_code)]
pub(crate) fn last_login_check(s: &pulse_store::Store, name: &str) -> BTreeMap<String, Value> {
    let conn = s.conn().unwrap();
    let r = must_one(
        &conn,
        "SELECT at, detail FROM events WHERE action = 'executor.login-check' AND subject = ? ORDER BY seq DESC LIMIT 1",
        &[&name],
    );
    if is_nil(&r) {
        return BTreeMap::new();
    }
    let d = json_map(&r.str("detail"));
    let mut out = BTreeMap::new();
    out.insert("at".to_string(), json!(r.str("at")));
    out.insert(
        "ok".to_string(),
        json!(d.get("ok").and_then(Value::as_bool).unwrap_or(false)),
    );
    out.insert(
        "note".to_string(),
        json!(d.get("note").and_then(Value::as_str).unwrap_or("")),
    );
    out
}

#[allow(dead_code)]
pub(crate) fn login_expiry(s: &pulse_store::Store, name: &str) -> String {
    let conn = s.conn().unwrap();
    let r = must_one(
        &conn,
        "SELECT detail FROM events WHERE action = 'executor.login' AND subject = ? ORDER BY seq DESC LIMIT 1",
        &[&name],
    );
    if is_nil(&r) {
        return String::new();
    }
    json_map(&r.str("detail"))
        .get("expires")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}
