use crate::messages::stamp;
use crate::tasks::{check_project, clean, input, next_id};
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::json;
use std::sync::RwLock;

pub const COMPUTE_STATUSES: &[&str] = &[
    "pending",
    "approved",
    "provisioning",
    "ready",
    "denied",
    "released",
    "failed",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ComputeRequest {
    pub id: String,
    pub project: String,
    pub requested_by: String,
    pub size: String,
    pub hours: i32,
    pub reason: String,
    pub status: String,
    pub decided_by: String,
    pub executor: String,
    pub words: String,
    pub proposal_post: String,
    pub at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Caps {
    pub provider: String,
    pub max_vms: i32,
    pub max_hours_per_day: i32,
    pub auto_approve: bool,
    pub budget_credits: i32,
    pub vms_now: i32,
    pub hours_today: i32,
    pub login_source: bool,
}

const COMPUTE_COLS: &str = "id, project, requested_by, size, hours, reason, status, decided_by, executor, words, proposal_post, at, updated_at";

fn scan_compute(row: &rusqlite::Row<'_>) -> rusqlite::Result<ComputeRequest> {
    Ok(ComputeRequest {
        id: row.get(0)?,
        project: row.get(1)?,
        requested_by: row.get(2)?,
        size: row.get(3)?,
        hours: row.get(4)?,
        reason: row.get(5)?,
        status: row.get(6)?,
        decided_by: row.get(7)?,
        executor: row.get(8)?,
        words: row.get(9)?,
        proposal_post: row.get(10)?,
        at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

pub fn get_compute(s: &pulse_store::Store, id: &str) -> Result<ComputeRequest> {
    let conn = s.conn()?;
    conn.query_row(
        &format!("SELECT {COMPUTE_COLS} FROM compute_requests WHERE id = ?"),
        [id],
        scan_compute,
    )
    .optional()?
    .ok_or_else(|| {
        input(
            2,
            format!("No compute request {id:?}"),
            "the id is not in this store",
            "pulse compute list",
        )
    })
}

pub fn list_compute(
    s: &pulse_store::Store,
    project: &str,
    status: &str,
) -> Result<Vec<ComputeRequest>> {
    let mut q = format!("SELECT {COMPUTE_COLS} FROM compute_requests WHERE 1=1");
    let mut args = Vec::new();
    if !project.is_empty() {
        q.push_str(" AND project = ?");
        args.push(project.to_string());
    }
    if !status.is_empty() {
        q.push_str(" AND status = ?");
        args.push(status.to_string());
    }
    q.push_str(" ORDER BY id DESC");
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    Ok(stmt
        .query_map(rusqlite::params_from_iter(refs), scan_compute)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub type LoginSourceCheckFn = fn(&pulse_store::Store) -> bool;
pub static LOGIN_SOURCE_CHECK: RwLock<Option<LoginSourceCheckFn>> = RwLock::new(None);

pub const NO_LOGIN_SOURCE: &str = "no executor login source";

pub fn get_caps(s: &pulse_store::Store) -> Result<Caps> {
    let login_source = LOGIN_SOURCE_CHECK
        .read()
        .expect("login source lock poisoned")
        .is_none_or(|f| f(s));
    let mut c = Caps {
        provider: s.setting("compute.provider", ""),
        max_vms: s.setting_int("compute.max_vms") as i32,
        max_hours_per_day: s.setting_int("compute.max_hours_per_day") as i32,
        auto_approve: s.setting("compute.auto_approve", "") == "1",
        budget_credits: s.setting_int("compute.budget_credits") as i32,
        login_source,
        ..Caps::default()
    };
    let conn = s.conn()?;
    c.vms_now = conn.query_row(
        "SELECT COUNT(*) FROM compute_requests WHERE status IN ('approved','provisioning','ready')",
        [],
        |r| r.get(0),
    )?;
    // Machine-hours the day has spent or reserved: every request of the day
    // but a denied one and one that failed before a machine was ready
    // (its executor never set) - that one consumed nothing, as its own
    // finding says, so it does not block the retry that fixes the cause.
    c.hours_today = conn.query_row(
        "SELECT COALESCE(SUM(hours), 0) FROM compute_requests WHERE substr(at, 1, 10) = ? AND status NOT IN ('denied') AND NOT (status = 'failed' AND executor = '')",
        [&s.now()[..10]],
        |r| r.get(0),
    )?;
    Ok(c)
}

pub fn request_compute(
    s: &pulse_store::Store,
    project: &str,
    agent: &str,
    size: &str,
    hours: i32,
    reason: &str,
) -> Result<ComputeRequest> {
    if agent.is_empty() {
        return Err(input(
            2,
            "An agent is needed",
            "compute is requested by an agent",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    let mut size = size.trim().to_string();
    let size_setting = pulse_store::compute_size_setting(&s.setting("compute.provider", ""));
    if size.is_empty() {
        size = s.setting(&size_setting, "");
    }
    if size.is_empty() {
        return Err(input(
            2,
            "A machine size is needed",
            format!("{size_setting} is empty and --size was not given"),
            format!(
                "pulse compute request --size <size> ... or pulse settings set {size_setting} <size>"
            ),
        ));
    }
    if hours <= 0 {
        return Err(input(
            2,
            "Hours must be at least 1",
            "a request names how long the machine is needed",
            "pulse compute request --hours 4 ...",
        ));
    }
    let reason = clean(reason);
    if reason.is_empty() {
        return Err(input(
            2,
            "A reason is needed",
            "the owner reads it before approving",
            "pulse compute request --reason 'why' ...",
        ));
    }
    check_project(s, project)?;
    let caps = get_caps(s)?;
    let auto = caps.auto_approve
        && caps.vms_now < caps.max_vms
        && (caps.max_hours_per_day == 0 || caps.hours_today + hours <= caps.max_hours_per_day)
        && caps.login_source;
    let now = s.now();
    let tx = s.begin()?;
    let id = next_id(&tx, "compute_requests", "CR-")?;
    let (status, decided_by) = if auto {
        ("approved", "pulse")
    } else {
        ("pending", "")
    };
    tx.execute("INSERT INTO compute_requests (id, project, requested_by, size, hours, reason, status, decided_by, at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![id, project, agent, size, hours, reason, status, decided_by, now, now])?;
    tx.event(
        agent,
        "compute.requested",
        &id,
        pulse_store::Detail::json(
            json!({"project": project, "size": size, "hours": hours, "status": status}),
        )?,
    )?;
    if auto {
        tx.event("pulse", "compute.approved", &id, pulse_store::Detail::json(json!({"project": project, "auto": true, "vms_now": caps.vms_now + 1, "max_vms": caps.max_vms}))?)?;
    } else {
        let summary = format!("compute request {id}: {size} for {hours}h - {reason}");
        let mut body = format!(
            "{agent} asks for one {size} machine for {hours} hours.\n\nReason: {reason}\n\nCaps: {} of {} machines in use, auto_approve={}.\n\nApprove with `pulse compute approve {id} --words '...'` or deny with `pulse compute deny {id} --words '...'` (or from the hub's Compute page).\n",
            caps.vms_now, caps.max_vms, caps.auto_approve
        );
        if !caps.login_source {
            body.push_str("\nHeld: ");
            body.push_str(NO_LOGIN_SOURCE);
            body.push_str(" - a provisioned machine could not log in. Place a Copilot-only fine-grained token with `pulse executor token --with-token`; an approval waits for one (ADR-034).\n");
        }
        let post_id = insert_post(
            &tx, s, project, "runner", "compute", "proposal", agent, &summary, &body, "", "",
        )?;
        tx.execute(
            "UPDATE compute_requests SET proposal_post = ? WHERE id = ?",
            params![post_id, id],
        )?;
    }
    tx.commit()?;
    get_compute(s, &id)
}

pub fn approve(
    s: &pulse_store::Store,
    id: &str,
    words: &str,
    actor: &str,
) -> Result<ComputeRequest> {
    decide_compute(s, id, words, actor, true)
}

pub fn deny(s: &pulse_store::Store, id: &str, words: &str, actor: &str) -> Result<ComputeRequest> {
    decide_compute(s, id, words, actor, false)
}

fn decide_compute(
    s: &pulse_store::Store,
    id: &str,
    words: &str,
    actor: &str,
    yes: bool,
) -> Result<ComputeRequest> {
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    let cur = get_compute(s, id)?;
    if cur.status != "pending" {
        return Err(input(
            2,
            format!("{id} is {}, not pending", cur.status),
            "only a pending request takes an answer",
            "pulse compute list",
        ));
    }
    let (status, action, answer) = if yes {
        ("approved", "compute.approved", "yes")
    } else {
        ("denied", "compute.denied", "no")
    };
    let now = s.now();
    let tx = s.begin()?;
    tx.execute("UPDATE compute_requests SET status = ?, decided_by = ?, words = ?, updated_at = ? WHERE id = ?", params![status, actor, words, now, id])?;
    if !cur.proposal_post.is_empty() {
        let decided: i64 = tx.query_row(
            "SELECT COUNT(*) FROM decisions WHERE post_id = ?",
            [&cur.proposal_post],
            |r| r.get(0),
        )?;
        if decided == 0 {
            let resolution = if yes { "accepted" } else { "declined" };
            let mut summary: String = tx.query_row(
                "SELECT summary FROM posts WHERE id = ?",
                [&cur.proposal_post],
                |r| r.get(0),
            )?;
            if summary.len() > 200 {
                summary.truncate(197);
                summary.push_str("...");
            }
            let text = if words.is_empty() {
                "(no words recorded)"
            } else {
                words
            };
            let body = format!(
                "The human answered **{answer}**.\n\n{text}\n\nProposal: {}\n",
                cur.proposal_post
            );
            let result_id = insert_post(
                &tx,
                s,
                &cur.project,
                "runner",
                "compute",
                "result",
                actor,
                &format!("{resolution}: {summary}"),
                &body,
                &cur.proposal_post,
                resolution,
            )?;
            tx.execute("INSERT INTO decisions (post_id, project, answer, words, decided_at, decided_by, result_post) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![cur.proposal_post, cur.project, answer, words, now, actor, result_id])?;
            tx.event(
                actor,
                "decide",
                &cur.proposal_post,
                pulse_store::Detail::json(json!({"answer": answer, "result": result_id}))?,
            )?;
        }
    }
    tx.event(
        actor,
        action,
        id,
        pulse_store::Detail::json(
            json!({"project": cur.project, "words": words, "requested_by": cur.requested_by}),
        )?,
    )?;
    tx.commit()?;
    get_compute(s, id)
}

pub fn set_compute_status(
    s: &pulse_store::Store,
    id: &str,
    status: &str,
    executor: &str,
    actor: &str,
) -> Result<ComputeRequest> {
    if !COMPUTE_STATUSES.contains(&status) {
        return Err(input(
            2,
            format!("Unknown compute status {status:?}"),
            format!("statuses are {}", COMPUTE_STATUSES.join(", ")),
            "",
        ));
    }
    let cur = get_compute(s, id)?;
    let now = s.now();
    let tx = s.begin()?;
    tx.execute("UPDATE compute_requests SET status = ?, executor = CASE WHEN ? = '' THEN executor ELSE ? END, updated_at = ? WHERE id = ?", params![status, executor, executor, now, id])?;
    tx.event(
        actor,
        &format!("compute.{status}"),
        id,
        pulse_store::Detail::json(
            json!({"project": cur.project, "executor": executor, "from": cur.status}),
        )?,
    )?;
    tx.commit()?;
    get_compute(s, id)
}

#[allow(clippy::too_many_arguments)]
fn insert_post(
    tx: &pulse_store::Tx,
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    thread: &str,
    kind: &str,
    agent: &str,
    summary: &str,
    body: &str,
    on: &str,
    resolution: &str,
) -> Result<String> {
    let mut summary = clean(summary);
    if summary.len() > pulse_board::MAX_SUMMARY_CHARS {
        summary.truncate(pulse_board::MAX_SUMMARY_CHARS - 3);
        summary.push_str("...");
    }
    let now = s.now();
    let base = format!("{}-{agent}", stamp(&now));
    let mut id = base.clone();
    for n in 2.. {
        let exists: Option<i64> = tx
            .query_row("SELECT 1 FROM posts WHERE id = ?", [&id], |r| r.get(0))
            .optional()?;
        if exists.is_none() {
            break;
        }
        id = format!("{base}-{n}");
    }
    let (runner, model): (String, String) = tx
        .query_row(
            "SELECT runner, model FROM agents WHERE name = ?",
            [agent],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or_default();
    tx.execute("INSERT INTO posts (id, project, area, thread, kind, agent, runner, model, session, date, branch, expires, on_post, vote, text_hash, has_read_line, resolution, summary, body, links, files, name, origin_project, extra, origin, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, '', ?, '', '', ?, '', '', 0, ?, ?, ?, '', '', '', '', '{}', 'store', ?)",
        params![id, project, area, thread, kind, agent, runner, model, now, on, resolution, summary, body, now])
        .context("post")?;
    tx.execute(
        "UPDATE agents SET posts = posts + 1, last_active_at = ? WHERE name = ?",
        params![now, agent],
    )?;
    tx.event(
        agent,
        "post",
        &id,
        pulse_store::Detail::json(
            json!({"kind": kind, "project": project, "thread": format!("{area}/{thread}")}),
        )?,
    )?;
    Ok(id)
}

pub type ComputeProvidersFn = fn(&mut pulse_cli::Context) -> Result<()>;
pub static COMPUTE_PROVIDERS: RwLock<Option<ComputeProvidersFn>> = RwLock::new(None);
