//! Where a task's code stands once its steps are done: on the default branch
//! only, pushed to the remote, delivered as a pull request, held because the
//! push did not go through, or still waiting for the landing pass - and the
//! owner's one-time ask for a pull request. The hub's task sheet shows the
//! state with its next step, and the task feed lists the same events, so a
//! lead agent that reads the feed tells the owner the same story.

use anyhow::Result;
use serde::Serialize;

/// The remote branch PULSE pushes the default branch to for a pull request.
pub const PR_HEAD: &str = "pulse/landed";

/// The event the hub or `pulse land --pr` writes when the owner asks for a
/// pull request; the daemon answers it on its next landing pass.
pub const PR_ASKED: &str = "land.pr-asked";

/// The events that answer an ask: a pull request opened or brought up to
/// date, a plain push of the default branch, or the daemon's word that the
/// default branch had nothing the upstream lacks.
pub const PR_ANSWERS: [&str; 3] = ["land.pr", "land.pushed", "land.pr-answered"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TaskDelivery {
    /// `none`: nothing landed yet. `waiting`: a finished step's branch waits
    /// for the landing pass. `landed`: on the default branch, not delivered
    /// yet. `held`: landed, but the push or pull request did not go through
    /// (`note` says why). `asked`: the owner asked for a pull request and
    /// the daemon opens it on its next pass. `pushed`: on the upstream
    /// branch. `pr`: a pull request is open (`url`, `number`).
    pub state: String,
    /// Landings the task's steps made on the default branch.
    pub landed: i64,
    /// Finished steps whose branch has not landed yet.
    pub waiting: i64,
    /// The newest landed commit.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sha: String,
    /// When the state was reached.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub at: String,
    /// `origin/main`: where a push or pull request went.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub number: i64,
    /// The hold note, or why no delivery is coming on its own.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// What GitHub Actions made of the pushed commit, when the daemon watches
    /// pipelines (`pipeline.watch`): `watching` until the runs finish, then
    /// `passed`, `failed`, `none` (nothing ran) or `gave-up`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pipeline: String,
    /// The fix job the daemon planned for a red pipeline.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pipeline_job: String,
    /// Whether asking for a pull request does something now.
    pub can_ask: bool,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// One `land.*` event of a project or branch, as the feed and the state
/// read it.
#[derive(Debug, Clone)]
pub struct LandEvent {
    pub seq: i64,
    pub at: String,
    pub actor: String,
    pub action: String,
    pub detail: String,
}

/// A detail field as text: strings as they are, numbers rendered (a pull
/// request `number` is numeric), anything else empty.
pub(crate) fn field(detail: &str, key: &str) -> String {
    serde_json::from_str::<serde_json::Value>(detail)
        .ok()
        .and_then(|v| match v.get(key)? {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The task's steps and the branches they worked on: the jobs' ids, their
/// current branch and every run's branch (a landed job's branch column is
/// emptied, the run keeps it and the landing event names the job).
struct TaskWork {
    jobs: Vec<String>,
    branches: Vec<String>,
    /// Finished steps whose branch still waits to land.
    waiting: i64,
}

fn task_work(conn: &rusqlite::Connection, task: &crate::Task) -> Result<TaskWork> {
    let (exact, like) = pulse_store::task_source_params(&task.id);
    let mut jobs: Vec<String> = Vec::new();
    let mut branches: Vec<String> = Vec::new();
    let mut waiting = 0;
    let mut stmt = conn.prepare(&format!(
        "SELECT j.id, j.status, j.branch, \
                (SELECT group_concat(r.branch, '\n') FROM runs r WHERE r.job_id = j.id AND r.branch != '') \
         FROM jobs j WHERE j.project = ? AND ({} OR (? != '' AND j.id = ?)) ORDER BY j.created_at, j.id",
        pulse_store::task_source_clause("j.source")
    ))?;
    let rows = stmt
        .query_map(
            rusqlite::params![task.project, exact, like, task.job_id, task.job_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, status, branch, run_branches) in rows {
        jobs.push(id);
        if !branch.is_empty() {
            if status == "done" {
                waiting += 1;
            }
            if !branches.contains(&branch) {
                branches.push(branch);
            }
        }
        for b in run_branches.lines() {
            if !b.is_empty() && !branches.iter().any(|x| x == b) {
                branches.push(b.to_string());
            }
        }
    }
    Ok(TaskWork {
        jobs,
        branches,
        waiting,
    })
}

/// The landings of the task's work: `land.landed` on one of its branches,
/// or naming one of its jobs.
fn landings_of(conn: &rusqlite::Connection, work: &TaskWork) -> Result<Vec<LandEvent>> {
    let mut landings = land_events(conn, &work.branches, &["land.landed"], 0)?;
    if !work.jobs.is_empty() {
        let marks = vec!["?"; work.jobs.len()].join(",");
        let mut stmt = conn.prepare(&format!(
            "SELECT seq, at, actor, action, detail FROM events \
             WHERE action = 'land.landed' AND json_extract(detail, '$.job') IN ({marks}) ORDER BY seq"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(work.jobs.iter()), |r| {
                Ok(LandEvent {
                    seq: r.get(0)?,
                    at: r.get(1)?,
                    actor: r.get(2)?,
                    action: r.get(3)?,
                    detail: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for e in rows {
            if !landings.iter().any(|l| l.seq == e.seq) {
                landings.push(e);
            }
        }
        landings.sort_by_key(|e| e.seq);
    }
    Ok(landings)
}

fn land_events(
    conn: &rusqlite::Connection,
    subjects: &[String],
    actions: &[&str],
    after_seq: i64,
) -> Result<Vec<LandEvent>> {
    if subjects.is_empty() {
        return Ok(Vec::new());
    }
    let marks = |n: usize| vec!["?"; n].join(",");
    let sql = format!(
        "SELECT seq, at, actor, action, subject, detail FROM events \
         WHERE subject IN ({}) AND action IN ({}) AND seq > ? ORDER BY seq",
        marks(subjects.len()),
        marks(actions.len())
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<rusqlite::types::Value> = subjects
        .iter()
        .map(|s| rusqlite::types::Value::Text(s.clone()))
        .collect();
    params.extend(
        actions
            .iter()
            .map(|a| rusqlite::types::Value::Text((*a).to_string())),
    );
    params.push(rusqlite::types::Value::Integer(after_seq));
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params), |r| {
            Ok(LandEvent {
                seq: r.get(0)?,
                at: r.get(1)?,
                actor: r.get(2)?,
                action: r.get(3)?,
                detail: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// A `land.held` about the push or the pull request, not about another
/// job's landing: the daemon marks a push-only hold `push-failed`, and
/// appends a push note to a landing hold with these words.
fn push_related_hold(detail: &str) -> bool {
    let note = field(detail, "note");
    field(detail, "state") == "push-failed"
        || note.contains("not pushed")
        || note.contains("not delivered")
        || note.contains("pull request")
}

/// The project-level delivery events that belong to a task's landings: from
/// the first landing on, up to and including the first push or pull request
/// after the last one. Later events belong to later work.
pub fn delivery_events(
    conn: &rusqlite::Connection,
    project: &str,
    first_landed_seq: i64,
    last_landed_seq: i64,
) -> Result<Vec<LandEvent>> {
    let mut out = Vec::new();
    for e in land_events(
        conn,
        &[project.to_string()],
        &[
            "land.pushed",
            "land.pr",
            PR_ASKED,
            "land.pr-answered",
            "land.held",
        ],
        first_landed_seq,
    )? {
        if e.action == "land.held" && !push_related_hold(&e.detail) {
            continue;
        }
        let delivered = e.action == "land.pushed" || e.action == "land.pr";
        out.push(e.clone());
        if delivered && e.seq > last_landed_seq {
            break;
        }
    }
    Ok(out)
}

/// The `land.landed` events of a task's branches, oldest first.
pub fn task_landings(conn: &rusqlite::Connection, task: &crate::Task) -> Result<Vec<LandEvent>> {
    landings_of(conn, &task_work(conn, task)?)
}

/// Where the task's code stands now.
pub fn delivery_for(s: &pulse_store::Store, task_id: &str) -> Result<TaskDelivery> {
    let (task, _) = crate::get(s, task_id)?;
    let conn = s.conn()?;
    let work = task_work(&conn, &task)?;
    let landings = landings_of(&conn, &work)?;
    let mut d = TaskDelivery {
        landed: landings.len() as i64,
        waiting: work.waiting,
        ..TaskDelivery::default()
    };
    let Some(last) = landings.last() else {
        d.state = if work.waiting > 0 { "waiting" } else { "none" }.into();
        return Ok(d);
    };
    d.sha = serde_json::from_str::<serde_json::Value>(&last.detail)
        .ok()
        .and_then(|v| {
            v.pointer("/code_delivery/after")
                .or_else(|| v.get("after"))
                .and_then(|a| a.as_str())
                .map(ToString::to_string)
        })
        .unwrap_or_default();
    d.at = last.at.clone();
    d.state = "landed".into();
    let events = delivery_events(&conn, &task.project, landings[0].seq, last.seq)?;
    let mut answered_seq = 0;
    let mut asked: Option<&LandEvent> = None;
    for e in &events {
        match e.action.as_str() {
            "land.pushed" | "land.pr" if e.seq > last.seq => {
                d.state = if e.action == "land.pr" {
                    "pr"
                } else {
                    "pushed"
                }
                .into();
                d.at = e.at.clone();
                d.remote = format!(
                    "{}/{}",
                    field(&e.detail, "remote"),
                    if e.action == "land.pr" {
                        field(&e.detail, "base")
                    } else {
                        field(&e.detail, "branch")
                    }
                );
                d.url = field(&e.detail, "url");
                d.number = field(&e.detail, "number").parse().unwrap_or(0);
                d.note = String::new();
                d.can_ask = false;
                (d.pipeline, d.pipeline_job) =
                    pipeline_verdict(s, &conn, &task.project, &field(&e.detail, "to"))?;
                return Ok(d);
            }
            a if PR_ANSWERS.contains(&a) => {
                answered_seq = e.seq;
                asked = None;
            }
            a if a == PR_ASKED => asked = Some(e),
            "land.held" if e.seq > last.seq => {
                d.state = "held".into();
                d.at = e.at.clone();
                d.note = field(&e.detail, "note");
            }
            _ => {}
        }
    }
    if let Some(a) = asked.filter(|a| a.seq > answered_seq) {
        d.state = "asked".into();
        d.at = a.at.clone();
        d.note = String::new();
        d.can_ask = false;
        return Ok(d);
    }
    if d.state == "landed" && s.setting("landing.push", "off") == "off" {
        d.note = "PULSE does not push for this machine (landing.push is off); you push, or ask for a pull request.".into();
    }
    d.can_ask = true;
    Ok(d)
}

/// The daemon's verdict on the pipeline of the pushed commit `sha`
/// (`pipeline.*` events, the runner's pipeline watcher) and the fix job it
/// planned, as (verdict, job). `watching` while no verdict is in and the
/// watcher is on - a rerun of the failed jobs for a fresh log is not a
/// verdict yet; empty when it is off.
fn pipeline_verdict(
    s: &pulse_store::Store,
    conn: &rusqlite::Connection,
    project: &str,
    sha: &str,
) -> Result<(String, String)> {
    if sha.is_empty() {
        return Ok((String::new(), String::new()));
    }
    let mut stmt = conn.prepare(
        "SELECT action, detail FROM events WHERE subject = ?1 AND action LIKE 'pipeline.%' AND action NOT IN ('pipeline.unreadable', 'pipeline.rerun') AND json_extract(detail, '$.sha') = ?2 ORDER BY seq DESC LIMIT 1",
    )?;
    let verdict = stmt
        .query_row(rusqlite::params![project, sha], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .ok();
    Ok(match verdict {
        Some((action, detail)) => (
            action.trim_start_matches("pipeline.").to_string(),
            field(&detail, "job"),
        ),
        None if s.setting("pipeline.watch", "fix") != "off" => ("watching".into(), String::new()),
        None => (String::new(), String::new()),
    })
}

/// The owner asks for the project's landed work as a pull request. The
/// daemon's next landing pass pushes the default branch to `pulse/landed`
/// and opens or updates the pull request, whatever `landing.push` says.
pub fn ask_pull_request(
    s: &pulse_store::Store,
    project: &str,
    task_id: &str,
    actor: &str,
) -> Result<()> {
    pulse_bus::emit(
        s,
        actor,
        PR_ASKED,
        project,
        pulse_store::Detail::json(serde_json::json!({
            "project": project,
            "task": task_id,
            "head": PR_HEAD,
        }))?,
    )?;
    Ok(())
}

/// Whether an ask waits for the daemon: one newer than the project's last
/// push, pull request or "nothing to deliver" answer.
pub fn pull_request_asked(s: &pulse_store::Store, project: &str) -> bool {
    let Ok(conn) = s.conn() else {
        return false;
    };
    let asked: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE action = ? AND subject = ?",
            rusqlite::params![PR_ASKED, project],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if asked == 0 {
        return false;
    }
    let answered: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE action IN (?, ?, ?) AND subject = ?",
            rusqlite::params![PR_ANSWERS[0], PR_ANSWERS[1], PR_ANSWERS[2], project],
            |r| r.get(0),
        )
        .unwrap_or(0);
    asked > answered
}
