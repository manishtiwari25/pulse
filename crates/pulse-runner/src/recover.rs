//! Port of internal/runner/recover.go.
#![allow(dead_code)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::time::Duration;

type Row = crate::util::Row;
type Querier = dyn crate::util::Querier;

pub(crate) const RECOVER_PER_TICK: i32 = 5;
pub(crate) const RECOVER_QUEUE_ROOM: i32 = 5;
pub(crate) const ENVIRONMENT_BLOCK_BACKOFF: Duration = Duration::from_secs(60 * 60);
const RECOVERY_BRIEF_HEADING: &str = "\n\n## Earlier attempt (ADR-036)\n\n";
const RECOVERY_BRIEF_END: &str =
    "When the cause is not yours to fix at Tier 0 or 1, stop and say so with a QUESTION line.";
const RECOVERY_LOG_BYTES: u64 = 64 * 1024;
pub(crate) const FOLLOW_UP_ACTION: &str = "task.followup";

#[derive(Debug, Clone, Default)]
pub(crate) struct RecoveryStats {
    pub(crate) total: i32,
    pub(crate) days: i32,
    pub(crate) with_lead: i32,
    pub(crate) once: i32,
    pub(crate) cancelled: i32,
    pub(crate) capped: i32,
    pub(crate) queue_full: i32,
    pub(crate) blocked: i32,
    pub(crate) errors: i32,
    pub(crate) ledger: BTreeMap<String, i32>,
    pub(crate) next: String,
}

fn query_rows(q: &Querier, query: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Row>> {
    let mut stmt = q.conn().prepare(query)?;
    let names = stmt
        .column_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut rows = stmt.query(args)?;
    let mut out = Vec::new();
    while let Some(rr) = rows.next()? {
        let mut r = Row::new();
        for (i, c) in names.iter().enumerate() {
            use rusqlite::types::ValueRef::*;
            let v = match rr.get_ref(i)? {
                Null => Value::Null,
                Integer(i) => json!(i),
                Real(f) => json!(f),
                Text(t) => Value::String(String::from_utf8_lossy(t).to_string()),
                Blob(b) => Value::String(String::from_utf8_lossy(b).to_string()),
            };
            r.insert(c.clone(), v);
        }
        out.push(r);
    }
    Ok(out)
}

fn query_one(q: &Querier, query: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Row> {
    Ok(query_rows(q, query, args)?
        .into_iter()
        .next()
        .unwrap_or_default())
}

fn must_rows(q: &Querier, query: &str, args: &[&dyn rusqlite::ToSql]) -> Vec<Row> {
    query_rows(q, query, args).unwrap()
}

fn must_one(q: &Querier, query: &str, args: &[&dyn rusqlite::ToSql]) -> Row {
    query_one(q, query, args).unwrap()
}

fn exec1(q: &Querier, query: &str, args: &[&dyn rusqlite::ToSql]) {
    q.conn().execute(query, args).unwrap();
}

fn event_at(
    q: &Querier,
    at: &str,
    actor: &str,
    action: &str,
    subject: &str,
    detail: Value,
) -> Result<()> {
    let text = if detail.is_null() {
        String::new()
    } else if let Some(s) = detail.as_str() {
        s.to_string()
    } else {
        crate::util::py_json(&detail)
    };
    q.conn().execute(
        "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
        [
            &at as &dyn rusqlite::ToSql,
            &actor,
            &action,
            &subject,
            &text,
        ],
    )?;
    Ok(())
}

fn err_message(err: &anyhow::Error) -> String {
    err.to_string()
}

/// Go: func recoverFailedJobs(s *store.Store, project row, now, actor string) []string
pub(crate) fn recover_failed_jobs(
    s: &pulse_store::Store,
    project: Row,
    now: String,
    actor: String,
) -> Vec<String> {
    let slug = project.str("slug");
    let days = crate::util::setting_int(s, "daemon.recover_days");
    let failed = recovery_window_jobs(s, slug.clone(), now.clone(), days);
    if failed.is_empty() {
        write_recovery_note(s, slug, now, String::new(), false);
        return vec![];
    }
    if s.setting("daemon.recover", "on") == "off" {
        write_recovery_note(
            s,
            slug,
            now,
            "recovery off (daemon.recover)".to_string(),
            true,
        );
        return vec![];
    }
    let limit = crate::util::setting_int(s, "daemon.recover_limit");
    let mut stats = RecoveryStats {
        total: failed.len() as i32,
        days,
        ..Default::default()
    };
    let conn = s.conn().expect("conn");
    let mut queued = must_one(
        &conn,
        "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND status = 'queued'",
        &[&slug],
    )
    .int("n");
    let mut lines = Vec::new();
    let mut actions = 0;
    for job in failed {
        if job.str("kind") == "answer" {
            stats.once += 1;
            continue;
        }
        if actions >= RECOVER_PER_TICK {
            stats.capped += 1;
            continue;
        }
        let decision = recovery_decision(&conn, job.str("id"), job.str("finished_at"));
        if !decision.0.is_empty() {
            match complete_recovery_finding(s, slug.clone(), decision, now.clone(), actor.clone()) {
                Ok(created) => {
                    if created {
                        actions += 1;
                    }
                }
                Err(err) => {
                    actions += 1;
                    lines.push(format!(
                        "recovery finding for {} deferred: {}",
                        job.str("id"),
                        err_message(&err)
                    ));
                    stats.errors += 1;
                }
            }
            continue;
        }
        let run = last_recovery_run(&conn, job.str("id"));
        if owner_cancelled_failure(job.str("result"), row_str(run.clone(), "summary".into())) {
            stats.cancelled += 1;
            let f = crate::failure::Failure {
                class: crate::failure::FailureClass("structural".into()),
                reason: "cancelled by the owner".into(),
                signature: crate::failure::failure_signature(
                    crate::failure::FailureClass("structural".into()),
                    "cancelled by the owner".into(),
                ),
            };
            let (root, n) = recovery_chain(s, job.clone());
            actions += 1;
            if let Err(err) = hold_recovery_failure(
                s,
                job.clone(),
                now.clone(),
                actor.clone(),
                f,
                root,
                n,
                String::new(),
            ) {
                lines.push(format!(
                    "recovery hold for {} deferred: {}",
                    job.str("id"),
                    err_message(&err)
                ));
                stats.errors += 1;
            }
            continue;
        }
        let f = crate::failure::classify_failure(
            job.str("result"),
            row_str(run.clone(), "status".into()),
            row_str(run.clone(), "summary".into()),
            recovery_question_asked(&conn, slug.clone(), job.str("id")),
        );
        let (root, n) = recovery_chain(s, job.clone());
        let env_reason = crate::failure::environment_failure_reason(
            job.str("result"),
            row_str(run.clone(), "summary".into()),
        );
        if !env_reason.is_empty() {
            actions += 1;
            let (line, acted) = block_environment_failure(
                s,
                job.clone(),
                run.clone(),
                now.clone(),
                actor.clone(),
                env_reason,
                root,
                n,
            );
            if !line.is_empty() {
                lines.push(line);
                if acted {
                    stats.blocked += 1;
                } else {
                    stats.errors += 1;
                }
            }
            continue;
        }
        let later = recovery_carrier(&conn, job.clone(), root.clone());
        if !later.is_empty() {
            let mut held = f.clone();
            held.reason = format!("superseded by {later}");
            actions += 1;
            if let Err(err) = hold_recovery_failure(
                s,
                job.clone(),
                now.clone(),
                actor.clone(),
                held,
                root,
                n,
                later,
            ) {
                lines.push(format!(
                    "recovery hold for {} deferred: {}",
                    job.str("id"),
                    err_message(&err)
                ));
                stats.errors += 1;
            }
            continue;
        }
        if job.str("source").starts_with("task:")
            && (f.class.0 == "deterministic" || lead_followed_up(&conn, job.clone()))
        {
            stats.with_lead += 1;
            continue;
        }
        if f.class.0 != "structural" && n < limit {
            let due = crate::failure::recover_due(job.str("finished_at"), n);
            if now < due {
                stats.next_at(due);
                continue;
            }
        }
        let adds = f.class.0 != "structural" && n < limit;
        if adds && queued >= RECOVER_QUEUE_ROOM {
            stats.queue_full = queued;
            continue;
        }
        let (line, acted) = recover_one(
            s,
            project.clone(),
            job.clone(),
            run,
            now.clone(),
            actor.clone(),
            f,
            root,
            n,
            limit,
        );
        if !line.is_empty() {
            lines.push(line);
            if !acted {
                stats.errors += 1;
            }
        }
        if acted || !lines.last().is_none_or(String::is_empty) {
            actions += 1;
        }
        if acted && adds {
            queued += 1;
        }
    }
    stats.ledger = recovery_ledger(
        s,
        slug.clone(),
        crate::util::plus_hours(&now, -24.0 * f64::from(days)),
    );
    let note = stats.note();
    write_recovery_note(s, slug, now, note, true);
    lines
}

/// Go: func environmentBlockReason(job row) string
pub(crate) fn environment_block_reason(job: Row) -> String {
    if job.0.is_empty() || job.str("status") != "blocked" {
        return String::new();
    }
    if job.str("result").starts_with("environment: ") {
        return job.str("result");
    }
    String::new()
}

/// Go: func environmentBlockBackoffDue(q querier, jobID, now string) string
pub(crate) fn environment_block_backoff_due(q: &Querier, job_id: String, _now: String) -> String {
    let r = must_one(
        q,
        "SELECT e.at FROM events e WHERE e.action = ? AND e.subject = ?
\t\tAND json_valid(detail) AND json_extract(detail, '$.outcome') = 'blocked'
\t\tORDER BY e.seq DESC LIMIT 1",
        &[&pulse_board::JOB_RECOVER_ACTION, &job_id],
    );
    if r.0.is_empty() {
        String::new()
    } else {
        crate::util::format_iso(
            crate::util::must_iso(&r.str("at")) + chrono::Duration::seconds(3600),
        )
    }
}

/// Go: func environmentHealthBlock(q querier, executor, project, now string) (string, bool)
pub(crate) fn environment_health_block(
    q: &Querier,
    mut executor: String,
    project: String,
    now: String,
) -> (String, bool) {
    if executor.is_empty() {
        executor = crate::executor::LOCAL_EXECUTOR.into();
    }
    let since =
        crate::util::format_iso(crate::util::must_iso(&now) - chrono::Duration::seconds(3600));
    let local = crate::executor::LOCAL_EXECUTOR.to_string();
    let r = must_one(
        q,
        "SELECT j.result, r.executor FROM jobs j
\t\tJOIN runs r ON r.job_id = j.id
\t\tWHERE j.project = ? AND j.status = 'blocked' AND j.result LIKE 'environment:%'
\t\t\tAND j.finished_at >= ?
\t\t\tAND (r.executor = ? OR (? = ? AND (r.executor = '' OR r.executor IS NULL)))
\t\tORDER BY j.finished_at DESC LIMIT 1",
        &[&project, &since, &executor, &executor, &local],
    );
    if r.0.is_empty() {
        (String::new(), false)
    } else {
        (r.str("result"), true)
    }
}

/// Go: func ownerCancelledFailure(result, summary string) bool
pub(crate) fn owner_cancelled_failure(result: String, summary: String) -> bool {
    result.trim_start().starts_with("cancelled by the owner")
        || summary.trim_start().starts_with("cancelled by the owner")
}

/// Go: func recoveryLedger(s *store.Store, slug, since string) map[string]int
pub(crate) fn recovery_ledger(
    s: &pulse_store::Store,
    slug: String,
    since: String,
) -> BTreeMap<String, i32> {
    let conn = s.conn().expect("conn");
    let mut out = BTreeMap::new();
    for r in must_rows(
        &conn,
        "SELECT e.detail FROM events e JOIN jobs j ON j.id = e.subject WHERE e.action = ? AND j.project = ? AND e.at >= ?",
        &[&pulse_board::JOB_RECOVER_ACTION, &slug, &since],
    ) {
        let outcome = detail_string(crate::util::json_map(&r.str("detail")), "outcome".into());
        if !outcome.is_empty() {
            *out.entry(outcome).or_insert(0) += 1;
        }
    }
    out
}

/// Go: func recoveryWindowJobs(s *store.Store, slug, now string, days int) []row
pub(crate) fn recovery_window_jobs(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    days: i32,
) -> Vec<Row> {
    let conn = s.conn().expect("conn");
    must_rows(
        &conn,
        "SELECT j.* FROM jobs j WHERE j.project = ? AND j.status = 'failed' AND j.finished_at >= ? AND j.kind NOT IN ('lead', 'baseline', 'experiment') AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.status = 'running') ORDER BY j.finished_at, j.id",
        &[
            &slug,
            &crate::util::plus_hours(&now, -24.0 * f64::from(days)),
        ],
    )
}

/// Go: func recoveredAfter(s *store.Store, id, finished string) bool
pub(crate) fn recovered_after(s: &pulse_store::Store, id: String, finished: String) -> bool {
    let conn = s.conn().expect("conn");
    !recovery_decision(&conn, id, finished).0.is_empty()
}

/// Go: func recoveryDecision(q querier, id, finished string) row
pub(crate) fn recovery_decision(q: &Querier, id: String, finished: String) -> Row {
    must_one(
        q,
        "SELECT e.detail FROM events e WHERE e.action = ? AND e.subject = ? AND e.at >= ?
\t\tAND NOT EXISTS (SELECT 1 FROM events later WHERE later.subject = e.subject AND later.seq > e.seq
\t\t\tAND later.action IN ('job.lease', 'job.failed'))
\t\tORDER BY e.seq DESC LIMIT 1",
        &[&pulse_board::JOB_RECOVER_ACTION, &id, &finished],
    )
}

/// Go: func lastRecoveryRun(q querier, jobID string) row
pub(crate) fn last_recovery_run(q: &Querier, job_id: String) -> Row {
    must_one(
        q,
        "SELECT * FROM runs WHERE job_id = ? ORDER BY started_at DESC, id DESC LIMIT 1",
        &[&job_id],
    )
}

/// Go: func recoveryQuestionAsked(q querier, slug, jobID string) bool
pub(crate) fn recovery_question_asked(q: &Querier, slug: String, job_id: String) -> bool {
    !must_one(
        q,
        "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'question' AND area = 'jobs' AND thread = ? LIMIT 1",
        &[&slug, &pulse_board::normalize_thread(&job_id)],
    )
    .0
    .is_empty()
}

/// Go: func recoveryCarrier(q querier, job row, root string) string
pub(crate) fn recovery_carrier(q: &Querier, job: Row, root: String) -> String {
    pulse_board::recovery_carrier(
        q.conn(),
        &pulse_board::Job {
            id: job.str("id"),
            project: job.str("project"),
            kind: job.str("kind"),
            tier: job.int("tier") as i64,
            title: job.str("title"),
            brief: job.str("brief"),
            source: job.str("source"),
            priority: job.int("priority") as i64,
            status: job.str("status"),
            created_at: job.str("created_at"),
            created_by: job.str("created_by"),
            lease_agent: job.str("lease_agent"),
            lease_until: job.str("lease_until"),
            generation: job.int("generation") as i64,
            attempts: job.int("attempts") as i64,
            finished_at: job.str("finished_at"),
            result: job.str("result"),
            branch: job.str("branch"),
        },
        &root,
    )
    .unwrap()
}

/// Go: func leadFollowedUp(q querier, job row) bool
pub(crate) fn lead_followed_up(q: &Querier, job: Row) -> bool {
    let task = job.str("source").trim_start_matches("task:").to_string();
    !must_one(
        q,
        "SELECT 1 AS x FROM events WHERE action = ? AND subject = ? AND at >= ? LIMIT 1",
        &[&FOLLOW_UP_ACTION, &task, &job.str("finished_at")],
    )
    .0
    .is_empty()
}

/// Go: func recoveryPending(s *store.Store, jobID string) bool
pub(crate) fn recovery_pending(s: &pulse_store::Store, job_id: String) -> bool {
    if s.setting("daemon.recover", "on") == "off" {
        return false;
    }
    let conn = s.conn().expect("conn");
    let job = crate::jobs::job_row(&conn, job_id.clone());
    if job.0.is_empty()
        || job.str("status") != "failed"
        || recovery_owned_elsewhere(job.clone())
        || (job.int("tier") != 0 && job.int("tier") != 1)
        || job.str("finished_at")
            < crate::util::plus_hours(
                &s.now(),
                -24.0 * f64::from(crate::util::setting_int(s, "daemon.recover_days")),
            )
        || recovered_after(s, job_id.clone(), job.str("finished_at"))
        || recovery_run_active(&conn, job_id.clone())
    {
        return false;
    }
    let run = last_recovery_run(&conn, job_id.clone());
    if owner_cancelled_failure(job.str("result"), row_str(run.clone(), "summary".into()))
        || (job.str("source").starts_with("task:") && lead_followed_up(&conn, job.clone()))
    {
        return false;
    }
    let f = crate::failure::classify_failure(
        job.str("result"),
        row_str(run.clone(), "status".into()),
        row_str(run, "summary".into()),
        recovery_question_asked(&conn, job.str("project"), job_id),
    );
    if f.class.0 != "transient" {
        return false;
    }
    let (root, n) = recovery_chain(s, job.clone());
    n < crate::util::setting_int(s, "daemon.recover_limit")
        && recovery_carrier(&conn, job, root).is_empty()
}

/// Go: func retryTitle(root, title string) string
pub(crate) fn retry_title(root: String, title: String) -> String {
    let prefix = format!("Retry {root}: ");
    crate::util::trunc(
        &(prefix.clone() + title.strip_prefix(&prefix).unwrap_or(&title)),
        200,
    )
}

/// Go: func recoveryChain(s *store.Store, job row) (string, int)
pub(crate) fn recovery_chain(s: &pulse_store::Store, job: Row) -> (String, i32) {
    let conn = s.conn().expect("conn");
    recovery_chain_from(&conn, job)
}

/// Go: func recoveryChainFrom(q querier, job row) (string, int)
pub(crate) fn recovery_chain_from(q: &Querier, job: Row) -> (String, i32) {
    let mut root = job.str("id");
    let r = must_one(
        q,
        "SELECT json_extract(detail, '$.root') AS root FROM events
\t\tWHERE action = ? AND json_valid(detail) AND json_extract(detail, '$.outcome') = 'replanned'
\t\tAND json_extract(detail, '$.job') = ? AND COALESCE(json_extract(detail, '$.root'), '') != ''
\t\tORDER BY seq LIMIT 1",
        &[&pulse_board::JOB_RECOVER_ACTION, &root],
    );
    if !r.0.is_empty() {
        root = r.str("root");
    }
    let n = must_one(
        q,
        "SELECT COUNT(*) AS n FROM events WHERE action = ? AND json_valid(detail)
\t\tAND json_extract(detail, '$.root') = ?
\t\tAND json_extract(detail, '$.outcome') IN ('requeued', 'replanned', 'retried')",
        &[&pulse_board::JOB_RECOVER_ACTION, &root],
    );
    (root, n.int("n"))
}

/// Go: func recoverOne(...)
#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_one(
    s: &pulse_store::Store,
    mut project: Row,
    mut job: Row,
    mut run: Row,
    now: String,
    actor: String,
    mut f: crate::failure::Failure,
    mut root: String,
    mut n: i32,
    mut limit: i32,
) -> (String, bool) {
    let mut line = String::new();
    let mut acted = false;
    let mut capped = false;
    let err = crate::db::immediate(s, |q| {
        let current = current_recovery_failure(q, job.clone())?;
        job = current;
        if recovery_owned_elsewhere(job.clone())
            || !recovery_decision(q, job.str("id"), job.str("finished_at"))
                .0
                .is_empty()
        {
            return Ok(());
        }
        if s.setting("daemon.recover", "on") == "off" {
            return Err(crate::util::fail(2, "recovery is off (daemon.recover)").into());
        }
        if job.str("finished_at")
            < crate::util::plus_hours(
                &now,
                -24.0 * f64::from(crate::util::setting_int(s, "daemon.recover_days")),
            )
        {
            return Err(crate::util::fail(2, "failure is outside daemon.recover_days").into());
        }
        project = crate::board::project_by_slug(q, job.str("project"));
        if project.0.is_empty() || project.str("status") != "active" {
            return Err(crate::util::fail(2, "project is no longer active").into());
        }
        run = last_recovery_run(q, job.str("id"));
        f = crate::failure::classify_failure(
            job.str("result"),
            row_str(run.clone(), "status".into()),
            row_str(run.clone(), "summary".into()),
            recovery_question_asked(q, job.str("project"), job.str("id")),
        );
        (root, n) = recovery_chain_from(q, job.clone());
        limit = crate::util::setting_int(s, "daemon.recover_limit");
        if owner_cancelled_failure(job.str("result"), row_str(run.clone(), "summary".into())) {
            f = crate::failure::Failure {
                class: crate::failure::FailureClass("structural".into()),
                reason: "cancelled by the owner".into(),
                signature: crate::failure::failure_signature(
                    crate::failure::FailureClass("structural".into()),
                    "cancelled by the owner".into(),
                ),
            };
            recovery_event(
                q,
                now.clone(),
                actor.clone(),
                job.str("id"),
                "held".into(),
                f.clone(),
                root.clone(),
                n,
                String::new(),
            )?;
            return Ok(());
        }
        let later = recovery_carrier(q, job.clone(), root.clone());
        if !later.is_empty() {
            f.reason = format!("superseded by {later}");
            recovery_event(
                q,
                now.clone(),
                actor.clone(),
                job.str("id"),
                "held".into(),
                f.clone(),
                root.clone(),
                n,
                later,
            )?;
            return Ok(());
        }
        if job.str("source").starts_with("task:")
            && (f.class.0 == "deterministic" || lead_followed_up(q, job.clone()))
        {
            return Ok(());
        }
        if job.int("tier") != 0 && job.int("tier") != 1 {
            f = crate::failure::Failure {
                class: crate::failure::FailureClass("structural".into()),
                reason: "The runner takes Tier 0 and Tier 1 jobs only; Tier 2 needs an ADR and the human".into(),
                signature: String::new(),
            };
            f.signature = crate::failure::failure_signature(f.class.clone(), f.reason.clone());
        }
        if !pulse_board::JOB_KINDS.contains(&job.str("kind").as_str()) {
            return Err(
                crate::util::fail(2, format!("invalid job kind {:?}", job.str("kind"))).into(),
            );
        }
        if n >= limit {
            f.reason = format!("recovered {n} times; the lead and the owner decide");
            recovery_event(
                q,
                now.clone(),
                actor.clone(),
                job.str("id"),
                "held".into(),
                f.clone(),
                root.clone(),
                n,
                String::new(),
            )?;
            line = format!("held {}: {}", job.str("id"), f.reason);
            acted = true;
            capped = true;
            return Ok(());
        }
        if f.class.0 == "structural" {
            recovery_event(
                q,
                now.clone(),
                actor.clone(),
                job.str("id"),
                "held".into(),
                f.clone(),
                root.clone(),
                n,
                String::new(),
            )?;
            line = format!("held {} (structural): {}", job.str("id"), f.reason);
            acted = true;
            return Ok(());
        }
        if now < crate::failure::recover_due(job.str("finished_at"), n) {
            return Ok(());
        }
        if must_one(
            q,
            "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND status = 'queued'",
            &[&job.str("project")],
        )
        .int("n")
            >= RECOVER_QUEUE_ROOM
        {
            return Err(crate::util::fail(
                2,
                format!("recovery waits for the queue ({RECOVER_QUEUE_ROOM} queued)"),
            )
            .into());
        }
        let (copy_id, _) = queue_recovery(
            q,
            project.clone(),
            job.clone(),
            run.clone(),
            now.clone(),
            actor.clone(),
            f.clone(),
            root.clone(),
            n,
        )?;
        match f.class.0.as_str() {
            "transient" => {
                line = format!(
                    "recovered {} (transient): queued again after a {} pause",
                    job.str("id"),
                    short_duration(crate::failure::recover_backoff(n))
                );
                acted = true;
            }
            "deterministic" => {
                line = format!(
                    "re-planned {} as {} (deterministic: {})",
                    job.str("id"),
                    copy_id,
                    f.reason
                );
                acted = true;
            }
            _ => {}
        }
        Ok(())
    });
    if let Err(err) = err {
        return (
            format!("recovery {} deferred: {}", job.str("id"), err_message(&err)),
            false,
        );
    }
    if capped && let Err(err) = post_recovery_finding(s, project.str("slug"), root, n, now, actor) {
        line.push_str("; recovery finding deferred: ");
        line.push_str(&err_message(&err));
    }
    (line, acted)
}

/// Go: func queueRecovery(...)
#[allow(clippy::too_many_arguments)]
pub(crate) fn queue_recovery(
    q: &Querier,
    project: Row,
    job: Row,
    run: Row,
    now: String,
    actor: String,
    f: crate::failure::Failure,
    root: String,
    n: i32,
) -> anyhow::Result<(String, String)> {
    match f.class.0.as_str() {
        "transient" => {
            let result = format!("recovered: {}", f.reason);
            exec1(
                q,
                "UPDATE jobs SET status = 'queued', result = ?, branch = '', attempts = 0,
\t\t\tfinished_at = '', lease_agent = '', lease_until = '', brief = ? WHERE id = ?",
                &[
                    &result,
                    &corrected_brief(job.clone(), run.clone(), f.clone()),
                    &job.str("id"),
                ],
            );
            event_at(
                q,
                &now,
                &actor,
                "job.queued",
                &job.str("id"),
                Value::String(crate::util::trunc(&result, 400)),
            )?;
            recovery_event(
                q,
                now,
                actor,
                job.str("id"),
                "requeued".into(),
                f,
                root,
                n + 1,
                String::new(),
            )?;
            drop_stale_branch(project.str("path"), job.clone(), run)?;
            Ok((job.str("id"), "requeued".into()))
        }
        "deterministic" => {
            let copy_id = crate::util::new_id("job", &now);
            let title = retry_title(root.clone(), job.str("title"));
            exec1(
                q,
                "INSERT INTO jobs (id, project, kind, title, brief, tier, priority, source, created_by, created_at, status)
\t\t\tVALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued')",
                &[
                    &copy_id,
                    &job.str("project"),
                    &job.str("kind"),
                    &title,
                    &corrected_brief(job.clone(), run, f.clone()),
                    &job.int("tier"),
                    &job.int("priority"),
                    &job.str("source"),
                    &actor,
                    &now,
                ],
            );
            event_at(
                q,
                &now,
                &actor,
                "job.add",
                &copy_id,
                json!({"project": job.str("project"), "kind": job.str("kind"), "title": title}),
            )?;
            recovery_event(
                q,
                now,
                actor,
                job.str("id"),
                "replanned".into(),
                f,
                root,
                n + 1,
                copy_id.clone(),
            )?;
            Ok((copy_id, "replanned".into()))
        }
        _ => Err(crate::util::fail(2, "structural failures must remain held").into()),
    }
}

/// Go: func recoveryOwnedElsewhere(job row) bool
pub(crate) fn recovery_owned_elsewhere(job: Row) -> bool {
    matches!(
        job.str("kind").as_str(),
        "answer" | "lead" | "baseline" | "experiment"
    )
}

/// Go: func recoveryRunActive(q querier, jobID string) bool
pub(crate) fn recovery_run_active(q: &Querier, job_id: String) -> bool {
    !must_one(
        q,
        "SELECT 1 AS x FROM runs WHERE job_id = ? AND status = 'running' LIMIT 1",
        &[&job_id],
    )
    .0
    .is_empty()
}

/// Go: func currentRecoveryFailure(q querier, snapshot row) (row, error)
pub(crate) fn current_recovery_failure(q: &Querier, snapshot: Row) -> anyhow::Result<Row> {
    let current = crate::jobs::job_row(q, snapshot.str("id"));
    if current.0.is_empty()
        || current.str("status") != "failed"
        || current.int("generation") != snapshot.int("generation")
        || current.str("finished_at") != snapshot.str("finished_at")
        || current.str("result") != snapshot.str("result")
    {
        return Err(crate::util::fail(2, "job changed since its failure was inspected").into());
    }
    if recovery_run_active(q, current.str("id")) {
        return Err(
            crate::util::fail(2, "the run is still active; its result is not final").into(),
        );
    }
    Ok(current)
}

/// Go: func holdRecoveryFailure(...)
#[allow(clippy::too_many_arguments)]
pub(crate) fn hold_recovery_failure(
    s: &pulse_store::Store,
    job: Row,
    now: String,
    actor: String,
    f: crate::failure::Failure,
    root: String,
    n: i32,
    new_job: String,
) -> anyhow::Result<()> {
    crate::db::immediate(s, |q| {
        current_recovery_failure(q, job.clone())?;
        if recovery_decision(q, job.str("id"), job.str("finished_at"))
            .0
            .is_empty()
        {
            recovery_event(
                q,
                now,
                actor,
                job.str("id"),
                "held".into(),
                f,
                root,
                n,
                new_job,
            )?;
        }
        Ok(())
    })
}

/// Go: func blockEnvironmentFailure(...)
#[allow(clippy::too_many_arguments)]
pub(crate) fn block_environment_failure(
    s: &pulse_store::Store,
    mut job: Row,
    run: Row,
    now: String,
    actor: String,
    reason: String,
    root: String,
    n: i32,
) -> (String, bool) {
    let err = crate::db::immediate(s, |q| {
        let current = current_recovery_failure(q, job.clone())?;
        job = current;
        if !recovery_decision(q, job.str("id"), job.str("finished_at"))
            .0
            .is_empty()
        {
            return Ok(());
        }
        let due = environment_block_backoff_due(q, job.str("id"), now.clone());
        if !due.is_empty() && now < due {
            return Err(
                crate::util::fail(2, format!("environment block backoff until {due}")).into(),
            );
        }
        let mut executor = row_str(run.clone(), "executor".into());
        if executor.is_empty() {
            executor = crate::executor::LOCAL_EXECUTOR.into();
        }
        exec1(
            q,
            "UPDATE jobs SET status = 'blocked', result = ?, lease_agent = '', lease_until = '', finished_at = ? WHERE id = ?",
            &[&reason, &now, &job.str("id")],
        );
        event_at(
            q,
            &now,
            &actor,
            "job.blocked",
            &job.str("id"),
            Value::String(crate::util::trunc(&reason, 400)),
        )?;
        let f = crate::failure::Failure {
            class: crate::failure::FailureClass("structural".into()),
            reason: reason.clone(),
            signature: crate::failure::failure_signature(
                crate::failure::FailureClass("structural".into()),
                reason.clone(),
            ),
        };
        recovery_event(
            q,
            now.clone(),
            actor.clone(),
            job.str("id"),
            "blocked".into(),
            f,
            root.clone(),
            n,
            String::new(),
        )?;
        let _ = executor;
        Ok(())
    });
    if let Err(err) = err {
        return (
            format!(
                "environment block for {} deferred: {}",
                job.str("id"),
                err_message(&err)
            ),
            false,
        );
    }
    (format!("blocked {} ({})", job.str("id"), reason), true)
}

/// Go: func recoveryEvent(q querier, now, actor, jobID, outcome string, f failure, root string, n int, newJob string)
#[allow(clippy::too_many_arguments)]
pub(crate) fn recovery_event(
    q: &Querier,
    now: String,
    actor: String,
    job_id: String,
    outcome: String,
    f: crate::failure::Failure,
    root: String,
    n: i32,
    new_job: String,
) -> Result<()> {
    let mut detail = Map::new();
    detail.insert("outcome".into(), json!(outcome));
    detail.insert("class".into(), json!(f.class.0));
    detail.insert("why".into(), json!(f.reason));
    detail.insert("root".into(), json!(root));
    detail.insert("n".into(), json!(n));
    detail.insert("signature".into(), json!(f.signature));
    if !new_job.is_empty() {
        detail.insert("job".into(), json!(new_job));
    }
    event_at(
        q,
        &now,
        &actor,
        pulse_board::JOB_RECOVER_ACTION,
        &job_id,
        Value::Object(detail),
    )
}

/// Go: func correctedBrief(job, run row, f failure) string
pub(crate) fn corrected_brief(job: Row, run: Row, f: crate::failure::Failure) -> String {
    let mut run_id = "none".to_string();
    let mut executor = "unknown".to_string();
    if !run.0.is_empty() {
        run_id = run.str("id");
        executor = run.str("executor");
    }
    let (base, mut previous) = compact_recovery_brief(job.str("brief"));
    previous.push(format!(
        "Job `{}` (run `{}`, executor `{}`) failed at {}: {}.",
        job.str("id"),
        run_id,
        executor,
        job.str("finished_at"),
        crate::util::trunc(
            &crate::compute::redact_secrets(
                &f.reason.split_whitespace().collect::<Vec<_>>().join(" ")
            ),
            600
        )
    ));
    base + RECOVERY_BRIEF_HEADING
        + &previous.join("\n")
        + "\n\nLast lines of its run log:\n\n```\n"
        + &recovery_log_evidence(run)
        + "\n```\n\n"
        + "First look at the default branch: when the work above already landed another way, stop and say so in SUMMARY:. "
        + "Inspect the earlier attempts and start from the cause: reproduce the failing check, make a small safe correction, then deliver the original task above. "
        + "For a transient failure, first check whether the temporary cause has cleared rather than rewriting the work. "
        + "If a correction fails, try a distinct evidence-based alternative, then ask the current peer or lead once through the existing peer-help instructions and thread, including the exact error and attempted fixes. "
        + "Do not repeat an unchanged failure, duplicate another agent's work, or poll while waiting for help. "
        + "If work remains unfinished, report SUMMARY: blocked: <cause and attempted fixes> or SUMMARY: failed: <cause and attempted fixes>, and name the remaining failing check. "
        + "Use QUESTION: only for an owner-only decision or a hard stop, never an ordinary debugging question. "
        + "Never weaken a check or bypass owner approvals, protected paths, credentials, environment restrictions, or hard stops. "
        + RECOVERY_BRIEF_END
}

/// Go: func compactRecoveryBrief(brief string) (string, []string)
pub(crate) fn compact_recovery_brief(mut brief: String) -> (String, Vec<String>) {
    let mut base = String::new();
    let mut previous = Vec::new();
    loop {
        let Some(pos) = brief.find(RECOVERY_BRIEF_HEADING) else {
            base.push_str(&brief);
            break;
        };
        let before = brief[..pos].to_string();
        let after = brief[pos + RECOVERY_BRIEF_HEADING.len()..].to_string();
        base.push_str(&before);
        let end = after.find(RECOVERY_BRIEF_END);
        let next = after.find(RECOVERY_BRIEF_HEADING);
        if end.is_none() || next.is_some_and(|n| n < end.unwrap()) || !after.starts_with("Job `") {
            base.push_str(RECOVERY_BRIEF_HEADING);
            brief = after;
            continue;
        }
        let end = end.unwrap();
        let evidence = &after[..end];
        let Some((evidence, _)) = evidence.split_once("Last lines of its run log:") else {
            base.push_str(RECOVERY_BRIEF_HEADING);
            brief = after;
            continue;
        };
        for line in evidence.split('\n') {
            if line.starts_with("Job `") {
                previous.push(crate::util::trunc(
                    &crate::compute::redact_secrets(line),
                    700,
                ));
            }
        }
        brief = after[end + RECOVERY_BRIEF_END.len()..].to_string();
    }
    if previous.len() > 3 {
        previous = previous[previous.len() - 3..].to_vec();
    }
    (base, previous)
}

/// Go: func recoveryLogEvidence(run row) string
pub(crate) fn recovery_log_evidence(run: Row) -> String {
    if run.0.is_empty() {
        return "Run log unavailable: no run was recorded.".into();
    }
    let mut note = "Run log unavailable: no log path was recorded.".to_string();
    let path = run.str("log").trim().to_string();
    if !path.is_empty() {
        match recovery_log_tail(path) {
            Ok(text) if !text.is_empty() => return text,
            Ok(_) => note = "Run log unavailable: the recorded log is empty.".into(),
            Err(err) => {
                note = format!(
                    "Run log unavailable: {}",
                    crate::compute::redact_secrets(&err.to_string())
                );
            }
        }
    }
    let summary = run.str("summary").trim().to_string();
    if !summary.is_empty() {
        return note
            + "\nRecorded summary:\n"
            + &tail_log(crate::compute::redact_secrets(&summary), 40, 3000);
    }
    note + "\nNo run summary was recorded."
}

/// Go: func recoveryLogTail(path string) (string, error)
pub(crate) fn recovery_log_tail(path: String) -> anyhow::Result<String> {
    let info = std::fs::metadata(&path)?;
    if !info.is_file() {
        return Err(anyhow!("{path} is not a regular log file"));
    }
    let mut f = std::fs::File::open(&path)?;
    let start = info.len().saturating_sub(RECOVERY_LOG_BYTES);
    f.seek(std::io::SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(RECOVERY_LOG_BYTES).read_to_end(&mut buf)?;
    Ok(tail_log(
        crate::compute::redact_secrets(&String::from_utf8_lossy(&buf)),
        40,
        3000,
    ))
}

/// Go: func tailLog(text string, lines, chars int) string
pub(crate) fn tail_log(text: String, lines: i32, chars: i32) -> String {
    let text = text.trim_end_matches('\n');
    let mut parts = text.split('\n').collect::<Vec<_>>();
    if parts.len() > lines as usize {
        parts = parts[parts.len() - lines as usize..].to_vec();
    }
    crate::executor::tail(&parts.join("\n"), chars as usize)
}

/// Go: func dropStaleBranch(root string, job, run row) error
pub(crate) fn drop_stale_branch(root: String, job: Row, run: Row) -> anyhow::Result<()> {
    let mut seen = BTreeMap::<String, bool>::new();
    let mut branches = Vec::<String>::new();
    let mut add = |branch: String| {
        let branch = branch.trim().to_string();
        if !branch.is_empty() && !seen.contains_key(&branch) {
            seen.insert(branch.clone(), true);
            branches.push(branch);
        }
    };
    if !job.str("lease_agent").is_empty() {
        add(format!(
            "pulse/{}/{}",
            job.str("lease_agent"),
            job.str("id")
        ));
    }
    add(job.str("branch"));
    if !run.0.is_empty() {
        add(run.str("branch"));
    }
    for branch in &branches {
        let Some(rest) = branch.strip_prefix("pulse/") else {
            return Err(anyhow!(
                "stale branch {} is not owned by job {}; refusing cleanup",
                branch,
                job.str("id")
            ));
        };
        let Some((agent, id)) = rest.split_once('/') else {
            return Err(anyhow!(
                "stale branch {} is not owned by job {}; refusing cleanup",
                branch,
                job.str("id")
            ));
        };
        if agent.is_empty() || id != job.str("id") {
            return Err(anyhow!(
                "stale branch {} is not owned by job {}; refusing cleanup",
                branch,
                job.str("id")
            ));
        }
    }
    for branch in branches {
        let mut result = crate::util::git(
            &root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        );
        if result.err.is_none() && result.code == 1 {
            continue;
        }
        if result.ok() {
            result = crate::util::git_timeout(
                &root,
                Duration::from_secs(30),
                &["branch", "-D", "--", &branch],
            );
        }
        if let Some(err) = result.err {
            return Err(anyhow!("stale branch {branch}: {err}"));
        }
        if !result.ok() {
            return Err(anyhow!(
                "stale branch {} (exit {}): {}",
                branch,
                result.code,
                result.stderr.trim()
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RecoveryDecisionDetail {
    #[serde(rename = "outcome", default)]
    outcome: String,
    #[serde(rename = "root", default)]
    root: String,
    #[serde(rename = "why", default)]
    why: String,
    #[serde(rename = "n", default)]
    n: i32,
}

/// Go: func completeRecoveryFinding(s *store.Store, slug string, decision row, now, actor string) (bool, error)
pub(crate) fn complete_recovery_finding(
    s: &pulse_store::Store,
    slug: String,
    decision: Row,
    now: String,
    actor: String,
) -> anyhow::Result<bool> {
    let detail: RecoveryDecisionDetail = serde_json::from_str(&decision.str("detail"))
        .map_err(|e| anyhow!("read recovery decision: {e}"))?;
    if detail.outcome != "held"
        || !detail
            .why
            .starts_with(&format!("recovered {} times;", detail.n))
    {
        return Ok(false);
    }
    if detail.root.is_empty() {
        return Err(anyhow!("capped recovery decision has no chain root"));
    }
    post_recovery_finding(s, slug, detail.root, detail.n, now, actor)
}

/// Go: func postRecoveryFinding(s *store.Store, slug, root string, n int, now, actor string) (bool, error)
pub(crate) fn post_recovery_finding(
    s: &pulse_store::Store,
    slug: String,
    root: String,
    n: i32,
    now: String,
    actor: String,
) -> anyhow::Result<bool> {
    let summary = format!("Recovery held for {root}");
    let conn = s.conn()?;
    let existing = query_one(
        &conn,
        "SELECT 1 AS x FROM posts
\t\tWHERE project = ? AND kind = 'finding' AND area = ? AND thread = ? AND summary LIKE ? LIMIT 1",
        &[
            &slug,
            &pulse_board::normalize_area("jobs"),
            &pulse_board::normalize_thread(&root),
            &"Recovery held for%",
        ],
    )
    .map_err(|e| anyhow!("look up recovery finding: {e}"))?;
    if !existing.0.is_empty() {
        return Ok(false);
    }
    let mut body = format!(
        "Chain `{root}` was recovered {n} times and now waits for the lead or the owner.\n\n`pulse job retry {root} --as-new`"
    );
    let latest = query_one(
        &conn,
        "SELECT j.id, j.result FROM events e JOIN jobs j ON j.id = e.subject
\t\tWHERE e.action = ? AND j.project = ? AND json_valid(e.detail) AND json_extract(e.detail, '$.root') = ?
\t\tAND json_extract(e.detail, '$.outcome') = 'held' ORDER BY e.seq DESC LIMIT 1",
        &[&pulse_board::JOB_RECOVER_ACTION, &slug, &root],
    )
    .map_err(|e| anyhow!("read capped failure: {e}"))?;
    if !latest.0.is_empty() {
        body.push_str(&format!(
            "\n\nLatest failed job `{}`: {}\n\nInspect its run log and attempted fixes before deciding the next step.",
            latest.str("id"),
            crate::util::trunc(&crate::compute::redact_secrets(&latest.str("result")), 1200)
        ));
    }
    crate::board::create_post(
        s,
        now,
        crate::board::PostOpts {
            kind: "finding".into(),
            project: slug,
            area: "jobs".into(),
            thread: root,
            summary,
            body,
            agent: crate::board::actor_agent(s, actor),
            ..Default::default()
        },
    )?;
    Ok(true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecoveryMetaNote {
    at: String,
    note: String,
}

/// Go: func writeRecoveryNote(s *store.Store, slug, now, text string, keep bool)
pub(crate) fn write_recovery_note(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    text: String,
    keep: bool,
) {
    let conn = s.conn().expect("conn");
    let r = must_one(&conn, "SELECT value FROM meta WHERE key = 'recovery'", &[]);
    let mut notes: BTreeMap<String, RecoveryMetaNote> =
        serde_json::from_str(&r.str("value")).unwrap_or_default();
    if keep {
        notes.insert(
            slug,
            RecoveryMetaNote {
                at: now,
                note: text,
            },
        );
    } else {
        notes.remove(&slug);
    }
    let data = pulse_store::gojson::to_string(&notes).unwrap();
    exec1(
        &conn,
        "INSERT INTO meta (key, value) VALUES ('recovery', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        &[&data],
    );
}

/// Go: func rowStr(r row, key string) string
pub(crate) fn row_str(r: Row, key: String) -> String {
    if r.0.is_empty() {
        String::new()
    } else {
        r.str(&key)
    }
}

/// Go: func detailString(m map[string]any, key string) string
pub(crate) fn detail_string(m: BTreeMap<String, Value>, key: String) -> String {
    match m.get(&key) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => pulse_store::gojson::sprint(v),
        None => pulse_store::gojson::sprint(&Value::Null),
    }
}

/// Go: func shortDuration(d time.Duration) string
pub(crate) fn short_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs > 3600 && secs.is_multiple_of(60) {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

impl RecoveryStats {
    /// Go: func (st *recoveryStats) nextAt(due string)
    pub(crate) fn next_at(&mut self, due: String) {
        if self.next.is_empty() || due < self.next {
            self.next = due;
        }
    }

    /// Go: func (st recoveryStats) note() string
    pub(crate) fn note(&mut self) -> String {
        let mut parts = Vec::new();
        for (outcome, label) in [
            ("requeued", "run again"),
            ("replanned", "re-planned"),
            ("held", "held"),
            ("retried", "retried by hand"),
            ("blocked", "blocked"),
        ] {
            if let Some(n) = self.ledger.get(outcome)
                && *n > 0
            {
                parts.push(format!("{n} {label}"));
            }
        }
        if self.with_lead > 0 {
            parts.push(format!("{} with the lead", self.with_lead));
        }
        if self.once > 0 {
            parts.push(format!("{} answer(s) tried once", self.once));
        }
        if self.cancelled > 0 {
            parts.push(format!("{} cancelled by the owner", self.cancelled));
        }
        if !self.next.is_empty() {
            parts.push(format!("next at {}", self.next));
        }
        if self.capped > 0 {
            parts.push(format!("cap {} reached", RECOVER_PER_TICK));
        }
        if self.queue_full > 0 {
            parts.push(format!("waits for the queue ({} queued)", self.queue_full));
        }
        if self.errors > 0 {
            parts.push(format!("{} recovery error(s); see daemon log", self.errors));
        }
        if parts.is_empty() {
            parts.push("waiting".into());
        }
        format!(
            "{} failed in {}d: {}",
            self.total,
            self.days,
            parts.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::{exec1, must_one, must_rows};
    use crate::test_helpers_f7::*;
    use base64::Engine;
    use serde_json::{Value, json};
    use std::path::Path;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use std::time::{Duration, SystemTime};

    static TEST_WALL_CLOCK_CALLS: AtomicU64 = AtomicU64::new(0);
    static FAKE_SSH_CALLS: AtomicU64 = AtomicU64::new(0);

    fn advancing_wall_clock() -> SystemTime {
        SystemTime::UNIX_EPOCH
            + Duration::from_secs(
                TEST_WALL_CLOCK_CALLS
                    .fetch_add(crate::run::LAUNCH_SECONDS as u64 + 1, Ordering::SeqCst),
            )
    }

    fn ample_tmp_space(_: &str) -> anyhow::Result<u64> {
        Ok(crate::tmpspace::MIN_TMP_SPACE * 4)
    }

    fn use_ample_tmp_space() -> crate::tmpspace::FreeSpaceFn {
        let old = *crate::tmpspace::FREE_SPACE.read().expect("free space");
        *crate::tmpspace::FREE_SPACE.write().expect("free space") = ample_tmp_space;
        old
    }

    fn fake_login_fault_ssh(
        _cancel: &pulse_bus::CancellationToken,
        argv: &[String],
        _stdin: &[u8],
        _timeout: Duration,
    ) -> anyhow::Result<(i32, String, String)> {
        let n = FAKE_SSH_CALLS.fetch_add(1, Ordering::SeqCst);
        let text = argv
            .last()
            .and_then(|v| v.strip_prefix("b64 "))
            .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
            .map(|v| {
                String::from_utf8_lossy(&v)
                    .trim_end_matches('\0')
                    .replace('\0', " ")
            })
            .unwrap_or_else(|| argv.join(" "));
        if n == 0 || text.contains("probe") {
            Ok((
                0,
                "hands=ok pulse=ok heartbeat=1 git=ok harness=yes keys_ok=yes keys=- remotes=0 disk_free_mb=4096".into(),
                String::new(),
            ))
        } else if n == 1 || text.contains("prepare") {
            Ok((
                0,
                "prepared aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n".into(),
                String::new(),
            ))
        } else if n < 5 || text.contains("verify") {
            Ok((0, String::new(), String::new()))
        } else if n >= 3 || text.contains("run") {
            Ok((
                1,
                String::new(),
                "Error: Authentication token found but could not be validated. GitHub returned: Sorry. Your account was suspended".into(),
            ))
        } else {
            Ok((0, String::new(), String::new()))
        }
    }

    fn conn(s: &pulse_store::Store) -> pulse_store::PooledConn {
        s.conn().expect("conn")
    }

    #[test]
    fn retry_title_prefixes_once() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            super::retry_title("job-1".into(), "Fix the build".into()),
            "Retry job-1: Fix the build"
        );
        assert_eq!(
            super::retry_title("job-1".into(), "Retry job-1: Fix the build".into()),
            "Retry job-1: Fix the build"
        );
    }

    #[test]
    fn post_recovery_finding_dedupe_uses_normalized_thread() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        let root = "job-20260912T062520Z-b434fc";
        let normalized = pulse_board::normalize_thread(root);

        let created = super::post_recovery_finding(
            &s,
            "proj".into(),
            root.into(),
            3,
            TEST_NOW.into(),
            "daemon".into(),
        )
        .expect("first finding");
        assert!(created, "first finding was not created");
        let created = super::post_recovery_finding(
            &s,
            "proj".into(),
            root.into(),
            3,
            crate::util::plus_minutes(TEST_NOW, 1.0),
            "daemon".into(),
        )
        .expect("second finding");
        assert!(!created, "second finding was created");
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE project = 'proj' AND kind = 'finding' AND area = 'jobs' AND thread = ?",
            &[&normalized],
        );
        assert_eq!(n, 1, "findings on normalized thread");
        let post = must_one(
            &s,
            "SELECT thread FROM posts WHERE project = 'proj' AND kind = 'finding'",
            &[],
        );
        assert_eq!(post.str("thread"), normalized);
    }

    #[test]
    fn post_recovery_finding_dedupe_allows_summary_drift() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        let root = "job-20260912T062520Z-b434fc";
        crate::board::create_post(
            &s,
            TEST_NOW.into(),
            crate::board::PostOpts {
                kind: "finding".into(),
                project: "proj".into(),
                area: "jobs".into(),
                thread: root.into(),
                summary: "Recovery held for an older summary shape".into(),
                body: "already reported".into(),
                agent: crate::board::actor_agent(&s, "planner".into()),
                ..Default::default()
            },
        )
        .expect("create existing post");
        let created = super::post_recovery_finding(
            &s,
            "proj".into(),
            root.into(),
            3,
            crate::util::plus_minutes(TEST_NOW, 1.0),
            "daemon".into(),
        )
        .expect("finding after drifted summary");
        assert!(
            !created,
            "drifted recovery finding summary was not treated as existing"
        );
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE project = 'proj' AND kind = 'finding' AND area = 'jobs' AND thread = ?",
            &[&pulse_board::normalize_thread(root)],
        );
        assert_eq!(n, 1, "findings after drifted summary");
    }

    #[test]
    fn recover_off_switch_writes_note_only_with_failures() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        set_recover_setting(&s, "daemon.recover", "off");
        insert_recover_failed_job(
            &s,
            "job-off",
            "task",
            "manual",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        let lines = super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert!(lines.is_empty(), "lines = {lines:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&pulse_board::JOB_RECOVER_ACTION],
            ),
            0
        );
        assert_eq!(
            recover_meta(&s).get("proj").expect("proj note").note,
            "recovery off (daemon.recover)"
        );

        exec1(
            &s,
            "UPDATE jobs SET status = 'done' WHERE id = 'job-off'",
            &[],
        );
        super::recover_failed_jobs(
            &s,
            must_one(&s, "SELECT * FROM projects WHERE slug = 'proj'", &[]),
            TEST_NOW.into(),
            "daemon".into(),
        );
        assert!(
            !recover_meta(&s).contains_key("proj"),
            "recovery note kept for a project with no failed jobs"
        );
    }

    #[test]
    fn recover_transient_waits_then_requeues_and_drops_branch() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, root) = new_bare_repo(&s);
        let job = insert_recover_failed_job(
            &s,
            "job-transient",
            "task",
            "manual",
            "runner error: connection reset",
            &crate::util::plus_minutes(TEST_NOW, -2.0),
        );
        let branch = format!("pulse/w1/{}", job.str("id"));
        run(&root, &["git", "branch", &branch]);
        insert_recover_run(
            &s,
            &job.str("id"),
            "run-transient",
            "error",
            "runner error: connection reset",
            "",
            &branch,
        );

        super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&pulse_board::JOB_RECOVER_ACTION],
            ),
            0
        );
        let notes = recover_meta(&s);
        let note = &notes.get("proj").expect("proj note").note;
        assert!(
            note.contains(&format!(
                "next at {}",
                crate::failure::recover_due(job.str("finished_at"), 0)
            )),
            "note before due = {note}"
        );

        let lines = super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 4.0),
            "daemon".into(),
        );
        assert_eq!(lines.len(), 1, "lines = {lines:?}");
        assert!(
            lines[0].contains("recovered job-transient (transient): queued again after a 5m pause"),
            "lines = {lines:?}"
        );
        let again = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(again.str("status"), "queued");
        assert_eq!(again.int("attempts"), 0);
        assert_eq!(again.str("branch"), "");
        assert!(
            !crate::util::git(
                &root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}")
                ]
            )
            .ok(),
            "stale branch {branch} still exists",
        );
        let detail = recover_detail(&s, &job.str("id"));
        assert_eq!(detail.get("outcome"), Some(&json!("requeued")));
        assert_eq!(detail.get("class"), Some(&json!("transient")));
        assert_eq!(detail.get("root"), Some(&json!(job.str("id"))));
        assert_eq!(detail.get("n"), Some(&json!(1)));
    }

    #[test]
    fn recover_deterministic_non_task_queues_corrected_job_with_log_tail() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, root) = new_bare_repo(&s);
        let job = insert_recover_failed_job(
            &s,
            "job-det",
            "improve",
            "manual",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        let log_path = Path::new(&root).join("run-det.log");
        let mut log_lines = (0..45)
            .map(|i| format!("log line {}", char::from(b'A' + (i % 26) as u8)))
            .collect::<Vec<_>>();
        log_lines.push("last important github_pat_abcdefghijklmnopqrstuvwxyz".into());
        std::fs::write(&log_path, log_lines.join("\n")).expect("write log");
        insert_recover_run(
            &s,
            &job.str("id"),
            "run-det",
            "failed",
            "verify exit 1",
            &log_path.display().to_string(),
            "",
        );

        let out = super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert_eq!(out.len(), 1, "lines = {out:?}");
        assert!(out[0].contains("re-planned job-det as"), "lines = {out:?}");
        let copy = must_one(
            &s,
            "SELECT * FROM jobs WHERE source = 'manual' AND status = 'queued'",
            &[],
        );
        assert!(!copy.0.is_empty(), "corrected job missing");
        assert_eq!(copy.str("status"), "queued");
        assert!(
            copy.str("title")
                .starts_with("Retry job-det: Original job-det"),
            "copy = {copy:?}"
        );
        let brief = copy.str("brief");
        for want in [
            "original brief job-det",
            "verify exit 1",
            "Job `job-det` (run `run-det`, executor `local`)",
            "last important",
            "[redacted]",
        ] {
            assert!(brief.contains(want), "brief missing {want:?}:\n{brief}");
        }
        assert!(
            !brief.contains("github_pat_abcdefghijklmnopqrstuvwxyz"),
            "brief leaked a token-shaped value"
        );
        let detail = recover_detail(&s, &job.str("id"));
        assert_eq!(detail.get("outcome"), Some(&json!("replanned")));
        assert_eq!(detail.get("job"), Some(&json!(copy.str("id"))));
        assert_eq!(detail.get("n"), Some(&json!(1)));
    }

    #[test]
    fn recover_corrected_chain_keeps_root_and_backoff() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        insert_recover_failed_job(
            &s,
            "job-root",
            "improve",
            "manual",
            "verify exit 1",
            &crate::util::plus_hours(TEST_NOW, -2.0),
        );
        crate::util::event_at(
            &conn(&s),
            &crate::util::plus_hours(TEST_NOW, -1.0),
            "daemon",
            pulse_board::JOB_RECOVER_ACTION,
            "job-root",
            json!({"outcome": "replanned", "class": "deterministic", "why": "verify exit 1", "root": "job-root", "n": 1, "signature": "deterministic:verify exit #", "job": "job-corrected"}),
        );
        let corrected = insert_recover_failed_job(
            &s,
            "job-corrected",
            "improve",
            "manual",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -5.0),
        );
        insert_recover_run(
            &s,
            &corrected.str("id"),
            "run-corrected",
            "failed",
            "verify exit 1",
            "",
            "",
        );

        super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = 'job-corrected'",
                &[&pulse_board::JOB_RECOVER_ACTION],
            ),
            0
        );
        let notes = recover_meta(&s);
        let note = &notes.get("proj").expect("proj note").note;
        assert!(
            note.contains(&format!(
                "next at {}",
                crate::failure::recover_due(corrected.str("finished_at"), 1)
            )),
            "note = {note}"
        );

        super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 11.0),
            "daemon".into(),
        );
        let detail = recover_detail(&s, &corrected.str("id"));
        assert_eq!(detail.get("root"), Some(&json!("job-root")));
        assert_eq!(detail.get("n"), Some(&json!(2)));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source = 'manual' AND status = 'queued'",
                &[]
            ),
            1
        );
        let copy_title = crate::jobs::job_row(
            &conn(&s),
            detail
                .get("job")
                .and_then(Value::as_str)
                .unwrap()
                .to_string(),
        )
        .str("title");
        assert_eq!(copy_title, "Retry job-root: Original job-corrected");
    }

    #[test]
    fn recover_dedupe_open_corrected_job() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        insert_recover_failed_job(
            &s,
            "job-dedupe",
            "improve",
            "manual",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        exec1(
            &s,
            "INSERT INTO jobs (id, project, kind, tier, title, brief, source, status, created_at)
\t\tVALUES ('job-open-recover', 'proj', 'improve', 1, 'open', 'open', 'manual', 'queued', ?)",
            &[&TEST_NOW],
        );

        super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source = 'manual'",
                &[]
            ),
            2
        );
        let detail = recover_detail(&s, "job-dedupe");
        assert_eq!(detail.get("outcome"), Some(&json!("held")));
        assert_eq!(
            detail.get("why"),
            Some(&json!("superseded by job-open-recover"))
        );
    }

    #[test]
    fn recover_holds_owner_cancelled_jobs() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        let job = insert_recover_failed_job(
            &s,
            "job-cancelled",
            "task",
            "manual",
            "cancelled by the owner: stopped after cancel",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        insert_recover_run(
            &s,
            &job.str("id"),
            "run-cancelled",
            "failed",
            "cancelled by the owner: stopped after cancel",
            "",
            "",
        );

        let lines = super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert!(lines.is_empty(), "lines = {lines:?}");
        assert_eq!(
            crate::jobs::job_row(&conn(&s), job.str("id")).str("status"),
            "failed"
        );
        let detail = recover_detail(&s, &job.str("id"));
        assert_eq!(detail.get("outcome"), Some(&json!("held")));
        assert_eq!(detail.get("why"), Some(&json!("cancelled by the owner")));
        let notes = recover_meta(&s);
        let note = &notes.get("proj").expect("proj note").note;
        assert!(note.contains("cancelled by the owner"), "note = {note}");
    }

    #[test]
    fn recover_task_owned_deterministic_waits_for_lead() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        let job = insert_recover_failed_job(
            &s,
            "job-task",
            "task",
            "task:TASK-1",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert_eq!(
            crate::jobs::job_row(&conn(&s), job.str("id")).str("status"),
            "failed"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&pulse_board::JOB_RECOVER_ACTION]
            ),
            0
        );
        let notes = recover_meta(&s);
        let note = &notes.get("proj").expect("proj note").note;
        assert!(note.contains("with the lead"), "note = {note}");
    }

    #[test]
    fn recover_structural_held_without_finding() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        let job = insert_recover_failed_job(
            &s,
            "job-struct",
            "task",
            "manual",
            "needs a real decision",
            &crate::util::plus_minutes(TEST_NOW, -1.0),
        );
        super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert_eq!(
            crate::jobs::job_row(&conn(&s), job.str("id")).str("status"),
            "failed"
        );
        let detail = recover_detail(&s, &job.str("id"));
        assert_eq!(detail.get("outcome"), Some(&json!("held")));
        assert_eq!(detail.get("class"), Some(&json!("structural")));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'",
                &[]
            ),
            0
        );
    }

    #[test]
    fn recover_limit_held_posts_one_finding_across_ticks() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        set_recover_setting(&s, "daemon.recover_limit", "1");
        insert_recover_failed_job(
            &s,
            "job-limit",
            "improve",
            "manual",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -40.0),
        );
        insert_recover_failed_job(
            &s,
            "job-limit-copy",
            "improve",
            "manual",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -39.0),
        );
        crate::util::event_at(
            &conn(&s),
            &crate::util::plus_hours(TEST_NOW, -2.0),
            "daemon",
            pulse_board::JOB_RECOVER_ACTION,
            "job-old",
            json!({"outcome": "replanned", "class": "deterministic", "why": "verify exit 1", "root": "job-limit", "n": 1, "signature": "deterministic:verify exit #", "job": "job-limit-copy"}),
        );

        super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 1.0),
            "daemon".into(),
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ? AND json_extract(detail, '$.outcome') = 'held'",
                &[&pulse_board::JOB_RECOVER_ACTION]
            ),
            2
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND area = 'jobs' AND thread = 'job-limit'",
                &[]
            ),
            1
        );
        let post = must_one(
            &s,
            "SELECT body FROM posts WHERE kind = 'finding' AND thread = 'job-limit'",
            &[],
        );
        assert!(
            post.str("body")
                .contains("pulse job retry job-limit --as-new"),
            "finding body = {:?}",
            post.str("body")
        );
    }

    #[test]
    fn recover_skips_owned_kinds_and_old_window() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        for (id, kind) in [
            ("job-lead", "lead"),
            ("job-baseline", "baseline"),
            ("job-experiment", "experiment"),
        ] {
            insert_recover_failed_job(
                &s,
                id,
                kind,
                "manual",
                "runner error",
                &crate::util::plus_minutes(TEST_NOW, -30.0),
            );
        }
        insert_recover_failed_job(
            &s,
            "job-old",
            "task",
            "manual",
            "runner error",
            &crate::util::plus_hours(TEST_NOW, -24.0 * 8.0),
        );
        super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&pulse_board::JOB_RECOVER_ACTION]
            ),
            0
        );
        assert!(
            !recover_meta(&s).contains_key("proj"),
            "note written for skipped jobs"
        );
    }

    #[test]
    fn recover_per_tick_cap() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        for i in 0..6 {
            let id = format!("job-cap-{}", char::from(b'a' + i));
            insert_recover_failed_job(
                &s,
                &id,
                "task",
                &format!("manual:{id}"),
                "runner error",
                &crate::util::plus_minutes(TEST_NOW, -30.0),
            );
        }
        super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&pulse_board::JOB_RECOVER_ACTION]
            ),
            RECOVER_PER_TICK
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'",
                &[]
            ),
            RECOVER_PER_TICK
        );
        let notes = recover_meta(&s);
        let note = &notes.get("proj").expect("proj note").note;
        assert!(note.contains("cap 5 reached"), "note = {note}");
    }

    #[test]
    fn recover_meta_shape_and_removal_preserves_other_projects() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        exec1(
            &s,
            "INSERT INTO meta (key, value) VALUES ('recovery', ?)",
            &[&r#"{"other":{"at":"2026-09-06T09:00:00Z","note":"kept"}}"#],
        );
        insert_recover_failed_job(
            &s,
            "job-meta",
            "task",
            "manual",
            "runner error",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        let notes = recover_meta(&s);
        assert_eq!(notes["proj"].at, TEST_NOW);
        assert!(notes["proj"].note.contains("1 failed in 7d"));
        assert_eq!(notes["other"].note, "kept");

        exec1(
            &s,
            "UPDATE jobs SET status = 'done' WHERE id = 'job-meta'",
            &[],
        );
        super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 1.0),
            "daemon".into(),
        );
        let notes = recover_meta(&s);
        assert!(!notes.contains_key("proj"));
        assert_eq!(notes["other"].note, "kept");
    }

    #[test]
    fn recover_idempotent_for_handled_failure() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        insert_recover_failed_job(
            &s,
            "job-idem",
            "improve",
            "manual",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 1.0),
            "daemon".into(),
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&pulse_board::JOB_RECOVER_ACTION]
            ),
            1
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source = 'manual' AND status = 'queued'",
                &[]
            ),
            1
        );
    }

    #[test]
    fn recover_superseded_failure_is_held_once() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        let failed = insert_recover_failed_job(
            &s,
            "job-old",
            "improve",
            "suggestion:POST-1",
            "runner error: connection reset",
            &crate::util::plus_hours(TEST_NOW, -3.0),
        );
        exec1(
            &s,
            "INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by)
\t\tVALUES ('job-later', 'proj', 'improve', 1, 'Later carrier', 'brief', 'suggestion:POST-1', 0, 'done', ?, 'lead')",
            &[&crate::util::plus_hours(TEST_NOW, -2.0)],
        );

        let lines = super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        assert!(lines.is_empty(), "lines = {lines:?}");
        assert_eq!(
            crate::jobs::job_row(&conn(&s), failed.str("id")).str("status"),
            "failed"
        );
        let detail = recover_detail(&s, &failed.str("id"));
        assert_eq!(detail.get("outcome"), Some(&json!("held")));
        assert_eq!(detail.get("why"), Some(&json!("superseded by job-later")));
        assert_eq!(detail.get("job"), Some(&json!("job-later")));
        assert_eq!(recover_meta(&s)["proj"].note, "1 failed in 7d: 1 held");
        super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 1.0),
            "daemon".into(),
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = 'job-old'",
                &[&pulse_board::JOB_RECOVER_ACTION]
            ),
            1
        );
    }

    #[test]
    fn recover_task_owned_transient_defers_to_lead_follow_up() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        let job = insert_recover_failed_job(
            &s,
            "job-task",
            "task",
            "task:TASK-1",
            "runner error: connection reset",
            &crate::util::plus_hours(TEST_NOW, -1.0),
        );
        assert!(super::recovery_pending(&s, job.str("id")));
        crate::util::event_at(
            &conn(&s),
            &crate::util::plus_minutes(TEST_NOW, -30.0),
            "daemon",
            FOLLOW_UP_ACTION,
            "TASK-1",
            json!({"job": "lead-1", "why": "failed"}),
        );
        super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert_eq!(
            crate::jobs::job_row(&conn(&s), job.str("id")).str("status"),
            "failed"
        );
        let notes = recover_meta(&s);
        let note = &notes.get("proj").expect("proj note").note;
        assert!(note.contains("1 with the lead"), "note = {note}");
    }

    #[test]
    fn recovery_pending_only_for_unanswered_transient_within_limit() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        insert_recover_failed_job(
            &s,
            "job-det",
            "task",
            "task:TASK-2",
            "verify exit 1",
            &crate::util::plus_hours(TEST_NOW, -1.0),
        );
        assert!(!super::recovery_pending(&s, "job-det".into()));
        insert_recover_failed_job(
            &s,
            "job-tr",
            "task",
            "task:TASK-3",
            "runner error: connection reset",
            &crate::util::plus_hours(TEST_NOW, -1.0),
        );
        assert!(super::recovery_pending(&s, "job-tr".into()));
        crate::util::event_at(
            &conn(&s),
            TEST_NOW,
            "daemon",
            pulse_board::JOB_RECOVER_ACTION,
            "job-tr",
            json!({"outcome": "held", "class": "transient", "why": "recovered 3 times", "root": "job-tr", "n": 3, "signature": "transient:runner error"}),
        );
        assert!(!super::recovery_pending(&s, "job-tr".into()));
        set_recover_setting(&s, "daemon.recover", "off");
        insert_recover_failed_job(
            &s,
            "job-off",
            "task",
            "task:TASK-4",
            "runner error: connection reset",
            &crate::util::plus_hours(TEST_NOW, -1.0),
        );
        assert!(!super::recovery_pending(&s, "job-off".into()));
    }

    #[test]
    fn recover_waits_while_the_queue_is_full() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        for i in 0..RECOVER_QUEUE_ROOM {
            exec1(
                &s,
                "INSERT INTO jobs (id, project, kind, tier, title, brief, source, status, created_at)
\t\t\tVALUES (?, 'proj', 'improve', 1, 'fresh', 'fresh', ?, 'queued', ?)",
                &[&format!("job-fresh-{i}"), &format!("fresh:{i}"), &TEST_NOW],
            );
        }
        insert_recover_failed_job(
            &s,
            "job-waits",
            "improve",
            "manual",
            "runner error: connection reset",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        insert_recover_failed_job(
            &s,
            "job-held",
            "improve",
            "manual-2",
            "1 commit(s) touch owner-owned paths (.github/x.yml); nothing landed",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );

        let lines = super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        assert_eq!(lines.len(), 1, "lines = {lines:?}");
        assert!(
            lines[0].contains("held job-held (structural)"),
            "lines = {lines:?}"
        );
        assert_eq!(
            crate::jobs::job_row(&conn(&s), "job-waits".into()).str("status"),
            "failed"
        );
        let notes = recover_meta(&s);
        let note = &notes.get("proj").expect("proj note").note;
        assert!(
            note.contains(&format!(
                "waits for the queue ({RECOVER_QUEUE_ROOM} queued)"
            )),
            "note = {note}"
        );

        exec1(
            &s,
            "UPDATE jobs SET status = 'done' WHERE id LIKE 'job-fresh-%'",
            &[],
        );
        super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 1.0),
            "daemon".into(),
        );
        assert_eq!(
            crate::jobs::job_row(&conn(&s), "job-waits".into()).str("status"),
            "queued"
        );
    }

    #[test]
    fn recover_never_retries_answer_jobs() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        insert_recover_failed_job(
            &s,
            "job-ans-1",
            "answer",
            "question:q-one",
            "runner error: connection reset",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        insert_recover_failed_job(
            &s,
            "job-ans-2",
            "answer",
            "question:q-two",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        insert_recover_failed_job(
            &s,
            "job-imp",
            "improve",
            "manual",
            "runner error: connection reset",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        let lines = super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert_eq!(lines.len(), 1, "lines = {lines:?}");
        assert!(
            lines[0].contains("recovered job-imp (transient)"),
            "lines = {lines:?}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'answer'",
                &[]
            ),
            2
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'answer' AND status <> 'failed'",
                &[]
            ),
            0
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject LIKE 'job-ans-%'",
                &[&pulse_board::JOB_RECOVER_ACTION]
            ),
            0
        );
        let notes = recover_meta(&s);
        let note = &notes.get("proj").expect("proj note").note;
        assert!(note.contains("3 failed in"), "note = {note}");
        assert!(note.contains("2 answer(s) tried once"), "note = {note}");
    }

    // Assigned recovery_resilience/peer_recovery/lost tests that still depend on non-F7 behavior.
    #[test]
    fn recovery_resilience_failed_write_is_atomic() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for (class, result, action) in [
            ("transient", "runner error: connection reset", "job.queued"),
            (
                "transient",
                "runner error: connection reset",
                pulse_board::JOB_RECOVER_ACTION,
            ),
            ("deterministic", "verify exit 1", "job.add"),
            (
                "deterministic",
                "verify exit 1",
                pulse_board::JOB_RECOVER_ACTION,
            ),
        ] {
            let s = new_store();
            let (p, root) = new_bare_repo(&s);
            let job = insert_recover_failed_job(
                &s,
                "job-write-failure",
                "improve",
                "",
                result,
                &crate::util::plus_minutes(TEST_NOW, -30.0),
            );
            run(&root, &["git", "branch", &job.str("branch")]);
            exec1(
                &s,
                &format!(
                    "CREATE TRIGGER reject_recovery_write BEFORE INSERT ON events
\t\tWHEN NEW.action = '{}' BEGIN SELECT RAISE(ABORT, 'injected recovery write failure'); END",
                    action
                ),
                &[],
            );
            let f = crate::failure::classify_failure(
                job.str("result"),
                String::new(),
                String::new(),
                false,
            );
            let (line, acted) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::recover_one(
                    &s,
                    p.clone(),
                    job.clone(),
                    Row::new(),
                    TEST_NOW.into(),
                    "daemon".into(),
                    f.clone(),
                    job.str("id"),
                    0,
                    3,
                )
            }))
            .expect("recovery panicked instead of reporting the failed write");
            assert!(
                !acted && line.contains("injected recovery write failure"),
                "{class}/{action}: failed write claimed recovery or lost its error: acted={acted} line={line:?}"
            );
            let after = crate::jobs::job_row(&conn(&s), job.str("id"));
            for key in [
                "status",
                "result",
                "branch",
                "finished_at",
                "lease_agent",
                "brief",
            ] {
                assert_eq!(
                    after.str(key),
                    job.str(key),
                    "{class}/{action}: {key} changed after rollback"
                );
            }
            assert_eq!(
                after.int("attempts"),
                job.int("attempts"),
                "{class}/{action}: attempts reset"
            );
            assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM jobs", &[]), 1);
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action IN ('job.queued', 'job.add', ?)",
                    &[&pulse_board::JOB_RECOVER_ACTION]
                ),
                0
            );
            assert!(
                crate::util::git(
                    &root,
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        &format!("refs/heads/{}", job.str("branch"))
                    ]
                )
                .ok(),
                "{class}/{action}: branch was deleted before recovery writes succeeded"
            );
            exec1(&s, "DROP TRIGGER reject_recovery_write", &[]);
            let (line, acted) = super::recover_one(
                &s,
                p,
                job.clone(),
                Row::new(),
                TEST_NOW.into(),
                "daemon".into(),
                f,
                job.str("id"),
                0,
                3,
            );
            assert!(
                acted && !line.is_empty(),
                "{class}/{action}: recovery did not resume after write fault cleared: {line}"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                    &[&pulse_board::JOB_RECOVER_ACTION]
                ),
                1
            );
        }
    }

    #[test]
    fn recovery_resilience_does_not_mutate_stale_failures() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for mode in [
            "leased",
            "done",
            "cancelled",
            "owner result",
            "new failure",
            "active run",
            "owner question",
        ] {
            let s = new_store();
            let (p, root) = new_bare_repo(&s);
            let job = insert_recover_failed_job(
                &s,
                "job-stale",
                "improve",
                "",
                "runner error: connection reset",
                &crate::util::plus_minutes(TEST_NOW, -30.0),
            );
            run(&root, &["git", "branch", &job.str("branch")]);
            let f = crate::failure::classify_failure(
                job.str("result"),
                String::new(),
                String::new(),
                false,
            );
            match mode {
                "leased" | "done" | "cancelled" => exec1(
                    &s,
                    "UPDATE jobs SET status = ?, lease_agent = 'other', generation = generation + 1 WHERE id = ?",
                    &[&mode, &job.str("id")],
                ),
                "owner result" => exec1(
                    &s,
                    "UPDATE jobs SET result = 'cancelled by the owner: stop retrying' WHERE id = ?",
                    &[&job.str("id")],
                ),
                "new failure" => exec1(
                    &s,
                    "UPDATE jobs SET finished_at = ?, generation = generation + 1 WHERE id = ?",
                    &[&crate::util::plus_minutes(TEST_NOW, -10.0), &job.str("id")],
                ),
                "active run" => {
                    insert_recover_run(
                        &s,
                        &job.str("id"),
                        "run-active",
                        "running",
                        "",
                        "",
                        &job.str("branch"),
                    );
                }
                "owner question" => {
                    crate::board::create_post(
                        &s,
                        TEST_NOW.into(),
                        crate::board::PostOpts {
                            project: "proj".into(),
                            kind: "question".into(),
                            area: "jobs".into(),
                            thread: job.str("id"),
                            summary: "Owner approval required".into(),
                            agent: crate::board::actor_agent(&s, "daemon".into()),
                            for_owner: true,
                            ..Default::default()
                        },
                    )
                    .expect("create question");
                }
                _ => unreachable!(),
            }
            let before = crate::jobs::job_row(&conn(&s), job.str("id"));
            let _ = super::recover_one(
                &s,
                p,
                job.clone(),
                Row::new(),
                TEST_NOW.into(),
                "daemon".into(),
                f,
                job.str("id"),
                0,
                3,
            );
            let after = crate::jobs::job_row(&conn(&s), job.str("id"));
            for key in ["status", "result", "branch", "finished_at", "lease_agent"] {
                assert_eq!(
                    after.str(key),
                    before.str(key),
                    "{mode}: stale recovery changed {key}"
                );
            }
            assert_eq!(
                after.int("attempts"),
                before.int("attempts"),
                "{mode}: stale recovery reset attempts"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = ? AND json_extract(detail, '$.outcome') IN ('requeued', 'replanned')",
                    &[&pulse_board::JOB_RECOVER_ACTION]
                ),
                0,
                "{mode}: stale recovery recorded a successful action"
            );
            assert!(
                crate::util::git(
                    &root,
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        &format!("refs/heads/{}", job.str("branch"))
                    ]
                )
                .ok(),
                "{mode}: stale recovery deleted the branch"
            );
        }
    }

    #[test]
    fn recovery_resilience_branch_cleanup_failure_remains_retryable() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, root) = new_bare_repo(&s);
        let job = insert_recover_failed_job(
            &s,
            "job-checked-out",
            "improve",
            "",
            "runner error: connection reset",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        run(&root, &["git", "checkout", "-q", "-b", &job.str("branch")]);
        let f = crate::failure::classify_failure(
            job.str("result"),
            String::new(),
            String::new(),
            false,
        );
        let (line, acted) = super::recover_one(
            &s,
            p.clone(),
            job.clone(),
            Row::new(),
            TEST_NOW.into(),
            "daemon".into(),
            f.clone(),
            job.str("id"),
            0,
            3,
        );
        assert!(!acted, "cleanup failure claimed action: {line}");
        assert!(
            line.contains(&job.str("branch")),
            "cleanup failure was not reported: {line}"
        );
        let after = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(after.str("status"), "failed");
        assert_eq!(after.str("branch"), job.str("branch"));
        assert_eq!(after.int("attempts"), 3);
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&pulse_board::JOB_RECOVER_ACTION]
            ),
            0
        );
        run(&root, &["git", "checkout", "-q", "main"]);
        let (line, acted) = super::recover_one(
            &s,
            p,
            job.clone(),
            Row::new(),
            TEST_NOW.into(),
            "daemon".into(),
            f,
            job.str("id"),
            0,
            3,
        );
        assert!(acted, "cleanup did not resume after checkout moved: {line}");
    }

    #[test]
    fn recovery_resilience_cleanup_only_deletes_this_jobs_branches() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for branch in ["main", "feature/human-work", "pulse/other/job-unrelated"] {
            for source in ["job", "run"] {
                let s = new_store();
                let (p, root) = new_bare_repo(&s);
                let mut job = insert_recover_failed_job(
                    &s,
                    "job-cleanup-scope",
                    "improve",
                    "",
                    "runner error: connection reset",
                    &crate::util::plus_minutes(TEST_NOW, -30.0),
                );
                let canonical = job.str("branch");
                run(&root, &["git", "branch", &canonical]);
                if branch != "main" {
                    run(&root, &["git", "branch", branch]);
                }
                run(
                    &root,
                    &["git", "checkout", "-q", "-b", "unrelated-checkout"],
                );
                if source == "job" {
                    exec1(
                        &s,
                        "UPDATE jobs SET branch = ? WHERE id = ?",
                        &[&branch, &job.str("id")],
                    );
                    job = crate::jobs::job_row(&conn(&s), job.str("id"));
                } else {
                    insert_recover_run(
                        &s,
                        &job.str("id"),
                        "run-scoped-cleanup",
                        "failed",
                        "runner error",
                        "",
                        branch,
                    );
                }
                let f = crate::failure::classify_failure(
                    job.str("result"),
                    String::new(),
                    String::new(),
                    false,
                );
                let (line, acted) = super::recover_one(
                    &s,
                    p,
                    job.clone(),
                    Row::new(),
                    TEST_NOW.into(),
                    "daemon".into(),
                    f,
                    job.str("id"),
                    0,
                    3,
                );
                assert!(
                    !acted,
                    "{branch}/{source}: unrelated branch was accepted for cleanup: {line}"
                );
                assert!(
                    line.contains("not owned by job"),
                    "{branch}/{source}: line = {line}"
                );
                for kept in [branch, canonical.as_str()] {
                    assert!(
                        crate::util::git(
                            &root,
                            &[
                                "rev-parse",
                                "--verify",
                                "--quiet",
                                &format!("refs/heads/{kept}")
                            ]
                        )
                        .ok(),
                        "{branch}/{source}: cleanup deleted {kept} before validating every branch",
                    );
                }
                assert_eq!(
                    crate::jobs::job_row(&conn(&s), job.str("id")).str("status"),
                    "failed"
                );
                assert_eq!(
                    count(
                        &s,
                        "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                        &[&pulse_board::JOB_RECOVER_ACTION]
                    ),
                    0
                );
            }
        }
    }

    #[test]
    fn recovery_resilience_one_corrected_job_without_source() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        let job = insert_recover_failed_job(
            &s,
            "job-no-source",
            "improve",
            "",
            "verify exit 1",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        let f = crate::failure::classify_failure(
            job.str("result"),
            String::new(),
            String::new(),
            false,
        );
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let s_ref = &s;
                let p_ref = p.clone();
                let job_ref = job.clone();
                let f_ref = f.clone();
                scope.spawn(move || {
                    let _ = super::recover_one(
                        s_ref,
                        p_ref,
                        job_ref.clone(),
                        Row::new(),
                        TEST_NOW.into(),
                        "daemon".into(),
                        f_ref,
                        job_ref.str("id"),
                        0,
                        3,
                    );
                });
            }
        });
        let _ = super::recover_one(
            &s,
            p,
            job.clone(),
            Row::new(),
            TEST_NOW.into(),
            "daemon".into(),
            f,
            job.str("id"),
            0,
            3,
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'",
                &[]
            ),
            1,
            "one failed job created duplicate corrected jobs"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                &[&pulse_board::JOB_RECOVER_ACTION]
            ),
            1,
            "one failed job wrote duplicate recovery events"
        );
    }

    #[test]
    fn recovery_resilience_chain_has_one_corrected_carrier() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        set_recover_setting(&s, "daemon.recover_limit", "5");
        insert_recover_failed_job(
            &s,
            "job-chain-root",
            "improve",
            "",
            "verify exit 1",
            &crate::util::plus_hours(TEST_NOW, -7.0),
        );
        for id in ["job-chain-first", "job-chain-second"] {
            insert_recover_failed_job(
                &s,
                id,
                "improve",
                "",
                "verify exit 1",
                &crate::util::plus_hours(TEST_NOW, -4.0),
            );
            super::recovery_event(
                &conn(&s),
                crate::util::plus_hours(TEST_NOW, -5.0),
                "human".into(),
                "job-chain-root".into(),
                "replanned".into(),
                crate::failure::Failure {
                    class: crate::failure::FailureClass(
                        crate::failure::FAILURE_DETERMINISTIC.into(),
                    ),
                    ..Default::default()
                },
                "job-chain-root".into(),
                1,
                id.into(),
            )
            .expect("recovery event");
        }
        super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'",
                &[]
            ),
            1,
            "one chain produced duplicate corrected jobs"
        );
        let carrier = must_one(&s, "SELECT id FROM jobs WHERE status = 'queued'", &[]);
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?",
            &[&TEST_NOW, &carrier.str("id")],
        );
        super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 1.0),
            "daemon".into(),
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'",
                &[]
            ),
            0,
            "superseded sibling queued another copy after carrier completed"
        );
    }

    #[test]
    fn recovery_resilience_write_errors_respect_tick_cap() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        for i in 0..=RECOVER_PER_TICK {
            insert_recover_failed_job(
                &s,
                &format!("job-write-cap-{i}"),
                "improve",
                "",
                "runner error",
                &crate::util::plus_hours(TEST_NOW, -1.0),
            );
        }
        exec1(
            &s,
            "CREATE TRIGGER reject_recovery_write BEFORE INSERT ON events
\t\tWHEN NEW.action = 'job.recover' BEGIN SELECT RAISE(ABORT, 'injected recovery write failure'); END",
            &[],
        );
        let lines = super::recover_failed_jobs(&s, p, TEST_NOW.into(), "daemon".into());
        assert_eq!(
            lines.len(),
            RECOVER_PER_TICK as usize,
            "write failures bypassed tick budget: {lines:?}"
        );
        let notes = recover_meta(&s);
        let note = &notes.get("proj").expect("proj note").note;
        assert!(note.contains("cap 5 reached"), "note = {note}");
        assert!(note.contains("5 recovery error(s)"), "note = {note}");
    }

    #[test]
    fn recovery_resilience_held_write_errors_respect_tick_cap() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for mode in ["cancelled", "superseded", "cap finding"] {
            let s = new_store();
            let (p, _) = new_bare_repo(&s);
            for i in 0..=RECOVER_PER_TICK {
                let id = format!("job-held-write-{i}");
                let result = if mode == "cancelled" {
                    "cancelled by the owner"
                } else {
                    "verify exit 1"
                };
                let job = insert_recover_failed_job(
                    &s,
                    &id,
                    "improve",
                    &format!("test:{id}"),
                    result,
                    &crate::util::plus_hours(TEST_NOW, -1.0),
                );
                match mode {
                    "superseded" => {
                        let (copy, _) = crate::jobs::add_job(
                            &s,
                            "proj".into(),
                            "improve".into(),
                            "New carrier".into(),
                            "brief".into(),
                            0,
                            0,
                            job.str("source"),
                            "daemon".into(),
                            true,
                        )
                        .expect("add copy");
                        crate::jobs::finish_job(
                            &s,
                            copy.str("id"),
                            "daemon".into(),
                            "done".into(),
                            "already repaired".into(),
                            String::new(),
                            -1,
                        )
                        .expect("finish copy");
                    }
                    "cap finding" => super::recovery_event(
                        &conn(&s),
                        TEST_NOW.into(),
                        "daemon".into(),
                        id.clone(),
                        "held".into(),
                        crate::failure::Failure {
                            class: crate::failure::FailureClass(
                                crate::failure::FAILURE_DETERMINISTIC.into(),
                            ),
                            reason: "recovered 3 times; the lead and the owner decide".into(),
                            ..Default::default()
                        },
                        id,
                        3,
                        String::new(),
                    )
                    .expect("recovery event"),
                    _ => {}
                }
            }
            if mode == "cap finding" {
                exec1(
                    &s,
                    "CREATE TRIGGER reject_held_write BEFORE INSERT ON posts
\t\t\t\t\tWHEN NEW.kind = 'finding' BEGIN SELECT RAISE(ABORT, 'injected held write failure'); END",
                    &[],
                );
            } else {
                exec1(
                    &s,
                    "CREATE TRIGGER reject_held_write BEFORE INSERT ON events
\t\t\t\t\tWHEN NEW.action = 'job.recover' BEGIN SELECT RAISE(ABORT, 'injected held write failure'); END",
                    &[],
                );
            }
            let lines = super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
            assert_eq!(
                lines.len(),
                RECOVER_PER_TICK as usize,
                "{mode}: held write failures bypassed recovery budget: {lines:?}"
            );
            let notes = recover_meta(&s);
            let note = &notes.get("proj").expect("proj note").note;
            assert!(note.contains("cap 5 reached"), "{mode}: note = {note}");
            assert!(
                note.contains("5 recovery error(s)"),
                "{mode}: note = {note}"
            );
            if mode == "cap finding" {
                exec1(&s, "DROP TRIGGER reject_held_write", &[]);
                for (i, want) in [RECOVER_PER_TICK, RECOVER_PER_TICK + 1]
                    .into_iter()
                    .enumerate()
                {
                    super::recover_failed_jobs(
                        &s,
                        p.clone(),
                        crate::util::plus_minutes(TEST_NOW, (i + 1) as f64),
                        "daemon".into(),
                    );
                    assert_eq!(
                        count(
                            &s,
                            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'",
                            &[]
                        ),
                        want,
                        "{mode}: notification repair exceeded its cap or was starved"
                    );
                }
            }
        }
    }

    #[test]
    fn recovery_resilience_same_second_failure_is_not_stranded() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        let job = insert_recover_failed_job(
            &s,
            "job-fast-failure",
            "improve",
            "",
            "runner error",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        let leased = crate::jobs::lease_job(
            &s,
            "w1".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .expect("lease recovered job");
        assert!(!leased.0.is_empty(), "lease recovered job returned no row");
        crate::jobs::finish_job(
            &s,
            job.str("id"),
            "w1".into(),
            "failed".into(),
            "runner error: connection reset".into(),
            String::new(),
            leased.int("generation"),
        )
        .expect("finish recovered job");
        assert!(
            super::recovery_pending(&s, job.str("id")),
            "new failure was mistaken for recovery recorded in the same second"
        );
        super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 16.0),
            "daemon".into(),
        );
        assert_eq!(
            crate::jobs::job_row(&conn(&s), job.str("id")).str("status"),
            "queued",
            "new failure did not recover after its next backoff"
        );
        let detail = recover_detail(&s, &job.str("id"));
        assert_eq!(
            detail.get("n").and_then(|v| v.as_i64()),
            Some(2),
            "same-second retry lost chain budget: {detail:?}"
        );
    }

    #[test]
    fn recovery_resilience_pending_matches_recovery_gates() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for mode in [
            "answer",
            "lead",
            "baseline",
            "experiment",
            "old",
            "cancelled",
            "lead followup",
            "superseded",
            "active run",
        ] {
            let s = new_store();
            new_bare_repo(&s);
            let job = insert_recover_failed_job(
                &s,
                "job-not-pending",
                "task",
                "task:TASK-guard",
                "runner error: connection reset",
                &crate::util::plus_hours(TEST_NOW, -1.0),
            );
            match mode {
                "answer" | "lead" | "baseline" | "experiment" => {
                    exec1(
                        &s,
                        "UPDATE jobs SET kind = ? WHERE id = ?",
                        &[&mode, &job.str("id")],
                    );
                }
                "old" => {
                    exec1(
                        &s,
                        "UPDATE jobs SET finished_at = ? WHERE id = ?",
                        &[
                            &crate::util::plus_hours(TEST_NOW, -24.0 * 8.0),
                            &job.str("id"),
                        ],
                    );
                }
                "cancelled" => {
                    exec1(
                        &s,
                        "UPDATE jobs SET result = 'cancelled by the owner: runner error' WHERE id = ?",
                        &[&job.str("id")],
                    );
                }
                "lead followup" => crate::util::event_at(
                    &conn(&s),
                    TEST_NOW,
                    "daemon",
                    FOLLOW_UP_ACTION,
                    "TASK-guard",
                    json!({"job": "job-lead"}),
                ),
                "superseded" => {
                    crate::jobs::add_job(
                        &s,
                        "proj".into(),
                        "task".into(),
                        "New carrier".into(),
                        "brief".into(),
                        1,
                        5,
                        job.str("source"),
                        "daemon".into(),
                        true,
                    )
                    .expect("add carrier");
                }
                "active run" => {
                    insert_recover_run(&s, &job.str("id"), "run-active", "running", "", "", "");
                }
                _ => unreachable!(),
            }
            assert!(
                !super::recovery_pending(&s, job.str("id")),
                "{mode}: lead would wait for a recovery the daemon will not perform"
            );
        }
    }

    #[test]
    fn recovery_resilience_corrected_brief_is_bounded() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (_, root) = new_bare_repo(&s);
        let log_path = Path::new(&root).join("recovery.log");
        std::fs::write(
            &log_path,
            format!(
                "{}last diagnostic detail\n",
                "prior debug output\n".repeat(500)
            ),
        )
        .expect("write log");
        let original = "Fix the parser. Preserve the owner's protected files and use the existing test fixture.";
        let retry_note =
            "\n\n## Retry note (human)\n\nUse the approved parser boundary; do not edit fixtures.";
        let mut brief = original.to_string();
        for i in 0..12 {
            let mut job = Row::new();
            job.insert("id", json!(format!("job-chain-{i}")));
            job.insert("brief", json!(brief));
            job.insert("finished_at", json!(TEST_NOW));
            let mut run = Row::new();
            run.insert("id", json!(format!("run-chain-{i}")));
            run.insert("executor", json!("local"));
            run.insert("log", json!(log_path.display().to_string()));
            run.insert("summary", json!("verify failed"));
            let f = crate::failure::classify_failure(
                format!("verify exit 1: attempt {i} tried a different parser guard"),
                String::new(),
                String::new(),
                false,
            );
            brief = super::corrected_brief(job, run, f);
            if i == 4 {
                brief.push_str(retry_note);
            }
        }
        for want in [
            original,
            retry_note,
            "job-chain-11",
            "run-chain-11",
            "attempt 11",
            "last diagnostic detail",
            "ask the current peer or lead once",
            "attempted fixes",
            "SUMMARY: blocked:",
            "SUMMARY: failed:",
            "QUESTION: only",
        ] {
            assert!(
                brief.contains(want),
                "corrected brief lost {want:?}:\n{brief}"
            );
        }
        assert_eq!(brief.matches("## Earlier attempt (ADR-036)").count(), 1);
        assert_eq!(brief.matches("Last lines of its run log:").count(), 1);
        assert!(
            brief.len() <= original.len() + retry_note.len() + 8000,
            "recovery context grew beyond its bounded evidence budget: {} bytes",
            brief.len()
        );
    }

    #[test]
    fn recovery_resilience_compacts_legacy_brief_without_dropping_instructions() {
        let _serial = crate::test_helpers::serial();
        let original = format!(
            "Fix the parser.\n\n## Earlier attempt (ADR-036)\n\nJob `example` is documentation, not a generated failure.\n{}",
            RECOVERY_BRIEF_END
        );
        let legacy = original.clone()
            + RECOVERY_BRIEF_HEADING
            + "Job `job-old` (run `run-old`, executor `local`) failed at "
            + TEST_NOW
            + ": verify exit 1.\n\nLast lines of its run log:\n\n```\nold log\n```\n\n"
            + "First look at the default branch: when the work above already landed another way, stop and say so in SUMMARY:. "
            + "Otherwise start from the cause: reproduce the failing check, fix it, then deliver the original task above. "
            + RECOVERY_BRIEF_END
            + "\n\n## Retry note (human)\n\nKeep the fixture unchanged.";
        let mut job = Row::new();
        job.insert("id", json!("job-current"));
        job.insert("brief", json!(legacy));
        job.insert("finished_at", json!(TEST_NOW));
        let brief = super::corrected_brief(
            job,
            Row::new(),
            crate::failure::classify_failure(
                "verify exit 1".into(),
                String::new(),
                String::new(),
                false,
            ),
        );
        for want in [
            original.as_str(),
            "job-old",
            "job-current",
            "## Retry note (human)\n\nKeep the fixture unchanged.",
        ] {
            assert!(
                brief.contains(want),
                "compaction lost original instructions or evidence {want:?}:\n{brief}"
            );
        }
        assert!(
            !brief.contains("old log"),
            "legacy log was recursively copied into the corrected brief"
        );
    }

    #[test]
    fn recovery_resilience_missing_log_is_explicit() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let root = tempfile::tempdir().expect("temp root");
        for mode in ["no run", "no path", "missing", "directory"] {
            let mut job = Row::new();
            job.insert("id", json!("job-log"));
            job.insert("brief", json!("Fix the parser."));
            job.insert("finished_at", json!(TEST_NOW));
            let mut run = Row::new();
            run.insert("id", json!("run-log"));
            run.insert("executor", json!("local"));
            run.insert("summary", json!("recorded summary"));
            match mode {
                "no run" => run = Row::new(),
                "missing" => run.insert(
                    "log",
                    json!(root.path().join("missing.log").display().to_string()),
                ),
                "directory" => run.insert("log", json!(root.path().display().to_string())),
                _ => {}
            }
            let f = crate::failure::classify_failure(
                "worker exited 1 github_pat_abcdefghijklmnopqrstuvwxyz".into(),
                String::new(),
                String::new(),
                false,
            );
            let brief = super::corrected_brief(job, run.clone(), f);
            assert!(
                brief.to_lowercase().contains("unavailable"),
                "missing failure evidence was silently dropped:\n{brief}"
            );
            if !run.0.is_empty() {
                assert!(
                    brief.contains("recorded summary"),
                    "recorded summary was lost:\n{brief}"
                );
            }
            assert!(
                !brief.contains("github_pat_abcdefghijklmnopqrstuvwxyz"),
                "failure reason leaked a token-shaped value"
            );
        }
    }

    #[test]
    fn recovery_resilience_cap_finding_retries_after_write_failure() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (p, _) = new_bare_repo(&s);
        set_recover_setting(&s, "daemon.recover_limit", "1");
        let job = insert_recover_failed_job(
            &s,
            "job-cap-finding",
            "improve",
            "",
            "verify exit 1: parser still rejects empty input",
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        crate::util::event_at(
            &conn(&s),
            &crate::util::plus_hours(TEST_NOW, -2.0),
            "daemon",
            pulse_board::JOB_RECOVER_ACTION,
            &job.str("id"),
            json!({"outcome": "requeued", "root": job.str("id"), "n": 1}),
        );
        exec1(
            &s,
            "CREATE TRIGGER reject_cap_finding BEFORE INSERT ON posts
\t\tWHEN NEW.kind = 'finding' BEGIN SELECT RAISE(ABORT, 'injected finding write failure'); END",
            &[],
        );
        let lines = super::recover_failed_jobs(&s, p.clone(), TEST_NOW.into(), "daemon".into());
        assert!(
            lines.join("\n").contains("injected finding write failure"),
            "capped finding write failure was hidden: {lines:?}"
        );
        exec1(&s, "DROP TRIGGER reject_cap_finding", &[]);
        super::recover_failed_jobs(
            &s,
            p.clone(),
            crate::util::plus_minutes(TEST_NOW, 1.0),
            "daemon".into(),
        );
        super::recover_failed_jobs(
            &s,
            p,
            crate::util::plus_minutes(TEST_NOW, 2.0),
            "daemon".into(),
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = ?",
                &[&job.str("id")]
            ),
            1
        );
        let post = must_one(
            &s,
            "SELECT body FROM posts WHERE kind = 'finding' AND thread = ?",
            &[&job.str("id")],
        );
        for want in [
            "parser still rejects empty input",
            &format!("pulse job retry {} --as-new", job.str("id")),
        ] {
            assert!(
                post.str("body").contains(want),
                "capped finding lacks actionable evidence {want:?}: {}",
                post.str("body")
            );
        }
    }

    #[test]
    fn sweep_lost_runs_releases_lease() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "w0");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "t".into(),
            "do it".into(),
            1,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .expect("add job");
        let leased = crate::jobs::lease_job(
            &s,
            "w0".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                minutes: 1000,
                ..Default::default()
            },
        )
        .expect("lease");
        assert!(!leased.0.is_empty(), "lease returned no row");
        exec1(
            &s,
            "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) VALUES ('run-1', ?, 'w0', 'proj', ?, 'running', '', '', 'real', 'local')",
            &[&job.str("id"), &TEST_NOW],
        );
        exec1(
            &s,
            "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) VALUES ('run-0', 'job-other', 'w0', 'proj', ?, 'done', '', '', 'real', 'local')",
            &[&TEST_NOW],
        );
        assert_eq!(crate::run::sweep_lost_runs(&s, TEST_NOW), 1);
        assert_eq!(
            must_one(&s, "SELECT status FROM runs WHERE id = 'run-1'", &[]).str("status"),
            "lost"
        );
        let j = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(j.str("status"), "queued", "lost run left job: {j:?}");
        assert_eq!(j.str("lease_agent"), "", "lost run left job leased: {j:?}");
        assert_eq!(j.str("lease_until"), "", "lost run left lease_until: {j:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'job.requeue' AND subject = ?",
                &[&job.str("id")]
            ),
            1
        );
        exec1(
            &s,
            "UPDATE jobs SET status = 'leased', lease_agent = 'w0', lease_until = '2099-01-01T00:00:00Z', attempts = ? WHERE id = ?",
            &[&crate::run::MAX_ATTEMPTS, &job.str("id")],
        );
        exec1(
            &s,
            "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) VALUES ('run-2', ?, 'w0', 'proj', ?, 'running', '', '', 'real', 'local')",
            &[&job.str("id"), &TEST_NOW],
        );
        assert_eq!(crate::run::sweep_lost_runs(&s, TEST_NOW), 1);
        let j = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(j.str("status"), "failed", "third loss: {j:?}");
        assert!(j.str("result").contains("3 times"), "third loss: {j:?}");
        assert_eq!(j.str("lease_agent"), "", "third loss: {j:?}");
        exec1(
            &s,
            "UPDATE jobs SET status = 'leased', lease_agent = 'w1', attempts = 1 WHERE id = ?",
            &[&job.str("id")],
        );
        exec1(
            &s,
            "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) VALUES ('run-3', ?, 'w0', 'proj', ?, 'running', '', '', 'real', 'local')",
            &[&job.str("id"), &TEST_NOW],
        );
        crate::run::sweep_lost_runs(&s, TEST_NOW);
        let j = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(
            j.str("status"),
            "leased",
            "another worker's lease was released: {j:?}"
        );
        assert_eq!(
            j.str("lease_agent"),
            "w1",
            "another worker's lease was released: {j:?}"
        );
    }

    #[test]
    fn report_reported() {
        let _serial = crate::test_helpers::serial();
        let text = "Error: Authentication token found but could not be validated.\n\nTo authenticate, you can use any of the following methods:\n";
        let mut r = crate::brief::parse_result_text(Some(text.into()));
        assert!(r.found, "harness error text was not parsed");
        assert!(
            !r.reported(),
            "harness error text was treated as a worker report"
        );
        for line in [
            "SUMMARY: did it",
            "- `FINDING`: a thing",
            "VOTE: 20260907T1-x up fine",
            "QUESTION: which one?",
            "SUGGESTION: area/thread - card",
        ] {
            let mut r = crate::brief::parse_result_text(Some(line.into()));
            assert!(r.reported(), "{line:?} not seen as a report");
        }
        let mut empty = crate::brief::parse_result_text(None);
        assert!(!empty.reported(), "nil text reported");
    }

    #[test]
    fn launch_rejection_on_local_is_runner_error() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let old_space = use_ample_tmp_space();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "t".into(),
            "do it".into(),
            1,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .expect("add job");
        let cmd = "sh -c 'echo \"Error: Authentication token found but could not be validated.\" >&2; exit 1'";
        let (r, reason) = crate::run::work_once(
            &s,
            "aurora",
            "",
            "",
            TEST_NOW,
            crate::run::RunOpts {
                worker_cmd: cmd.into(),
                ..Default::default()
            },
        )
        .expect("work");
        assert!(!r.0.is_empty(), "work returned no run: {reason}");
        assert_eq!(r.str("status"), "error", "run = {r:?}");
        assert!(
            r.str("summary").contains("rejected at launch"),
            "run = {r:?}"
        );
        assert!(
            r.str("summary").contains("login on this machine"),
            "run = {r:?}"
        );
        assert_eq!(
            crate::jobs::job_row(&conn(&s), job.str("id")).str("status"),
            "queued",
            "job after launch rejection"
        );

        let old = *crate::run::WALL_CLOCK.read().expect("wall clock");
        TEST_WALL_CLOCK_CALLS.store(0, Ordering::SeqCst);
        *crate::run::WALL_CLOCK.write().expect("wall clock") = advancing_wall_clock;
        let (r, _) = crate::run::work_once(
            &s,
            "aurora",
            "",
            "",
            TEST_NOW,
            crate::run::RunOpts {
                worker_cmd: "sh -c 'sleep 0.2; echo \"Error: Authentication token found but could not be validated.\" >&2; exit 1'".into(),
                ..Default::default()
            },
        )
        .expect("slow work");
        *crate::run::WALL_CLOCK.write().expect("wall clock") = old;
        assert_eq!(r.str("status"), "failed", "slow rejection: {r:?}");
        assert_eq!(r.str("summary"), "worker exited 1", "slow rejection: {r:?}");
        *crate::tmpspace::FREE_SPACE.write().expect("free space") = old_space;
    }

    #[test]
    fn native_not_logged_in_exit_four_is_runner_error_after_slow_start() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let old_space = use_ample_tmp_space();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "t".into(),
            "do it".into(),
            1,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .expect("add job");
        let old = *crate::run::WALL_CLOCK.read().expect("wall clock");
        TEST_WALL_CLOCK_CALLS.store(0, Ordering::SeqCst);
        *crate::run::WALL_CLOCK.write().expect("wall clock") = advancing_wall_clock;
        let (r, reason) = crate::run::work_once(
            &s,
            "aurora",
            "",
            "",
            TEST_NOW,
            crate::run::RunOpts {
                worker_cmd: "sh -c 'echo \"not logged in: sign in first\" >&2; exit 4'".into(),
                ..Default::default()
            },
        )
        .expect("work");
        *crate::run::WALL_CLOCK.write().expect("wall clock") = old;
        assert!(!r.0.is_empty(), "work returned no run: {reason}");
        assert_eq!(r.str("status"), "error", "run = {r:?}");
        assert!(
            r.str("summary").contains("provider login on this machine"),
            "run = {r:?}"
        );
        let j = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(j.str("status"), "queued", "job after login fault: {j:?}");
        assert_eq!(j.int("attempts"), 0, "job after login fault: {j:?}");
        *crate::tmpspace::FREE_SPACE.write().expect("free space") = old_space;
    }

    #[test]
    fn worker_exit_summary_includes_harness_stop_reason() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let old_space = use_ample_tmp_space();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "t".into(),
            "do it".into(),
            1,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .expect("add job");
        let (r, reason) = crate::run::work_once(
            &s,
            "aurora",
            "",
            "",
            TEST_NOW,
            crate::run::RunOpts {
                worker_cmd:
                    "sh -c 'echo \"SUMMARY: harness stopped: step_limit after 2 steps\"; exit 1'"
                        .into(),
                ..Default::default()
            },
        )
        .expect("work");
        assert!(!r.0.is_empty(), "work returned no run: {reason}");
        let want = "worker exited 1: harness stopped: step_limit after 2 steps";
        assert_eq!(r.str("status"), "failed", "run = {r:?}");
        assert_eq!(r.str("summary"), want, "run = {r:?}");
        *crate::tmpspace::FREE_SPACE.write().expect("free space") = old_space;
    }

    #[test]
    fn worker_exit_five_is_owner_cancelled_failure() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let old_space = use_ample_tmp_space();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "t".into(),
            "do it".into(),
            1,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .expect("add job");
        let (r, reason) = crate::run::work_once(
            &s,
            "aurora",
            "",
            "",
            TEST_NOW,
            crate::run::RunOpts {
                worker_cmd: "sh -c 'echo \"SUMMARY: stopped after cancel\"; exit 5'".into(),
                ..Default::default()
            },
        )
        .expect("work");
        assert!(!r.0.is_empty(), "work returned no run: {reason}");
        assert_eq!(r.str("status"), "failed", "run = {r:?}");
        assert_eq!(
            r.str("summary"),
            "cancelled by the owner: stopped after cancel",
            "run = {r:?}"
        );
        assert_eq!(r.int("exit_code"), 5, "run = {r:?}");
        let j = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(j.str("status"), "failed", "job = {j:?}");
        assert_eq!(
            j.str("result"),
            "cancelled by the owner: stopped after cancel",
            "job = {j:?}"
        );
        *crate::tmpspace::FREE_SPACE.write().expect("free space") = old_space;
    }

    #[test]
    fn failure_worker_exit_five_keeps_plain_cancel_summary() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let old_space = use_ample_tmp_space();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "t".into(),
            "do it".into(),
            1,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .expect("add job");
        let want = "harness stopped: cancelled after 1 model calls and 0 commands; error: interrupted: context canceled";
        let (r, reason) = crate::run::work_once(
            &s,
            "aurora",
            "",
            "",
            TEST_NOW,
            crate::run::RunOpts {
                worker_cmd: format!("sh -c 'echo \"SUMMARY: {want}\"; exit 5'"),
                ..Default::default()
            },
        )
        .expect("work");
        assert!(!r.0.is_empty(), "work returned no run: {reason}");
        assert_eq!(r.str("status"), "failed", "run = {r:?}");
        assert_eq!(r.str("summary"), want, "run = {r:?}");
        assert_eq!(r.int("exit_code"), 5, "run = {r:?}");
        let j = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(j.str("status"), "failed", "job = {j:?}");
        assert_eq!(j.str("result"), want, "job = {j:?}");
        *crate::tmpspace::FREE_SPACE.write().expect("free space") = old_space;
    }

    #[test]
    fn run_harness_cancel_watcher_cancels_context() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        s.begin_harness_session(pulse_store::HarnessSession {
            id: "run-cancel".into(),
            ..Default::default()
        })
        .expect("begin harness session");
        let worker_cancel = pulse_bus::CancellationToken::new();
        let owner_cancelled = Arc::new(AtomicBool::new(false));
        let done = crate::run::watch_run_harness_cancel_every(
            &worker_cancel,
            &s,
            "run-cancel",
            Arc::clone(&owner_cancelled),
            Duration::from_millis(1),
        );
        let ok = s
            .request_harness_cancel("run-cancel")
            .expect("request harness cancel");
        assert!(ok, "request cancel = {ok}");
        let start = SystemTime::now();
        while !worker_cancel.is_cancelled() {
            assert!(
                start.elapsed().unwrap_or_default() < Duration::from_secs(10),
                "worker context was not cancelled"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            owner_cancelled.load(Ordering::SeqCst),
            "owner cancellation flag was not set"
        );
        done();
    }

    #[test]
    fn launch_rejection_on_remote_drains_executor() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        crate::commands::register();
        let old_space = use_ample_tmp_space();
        let old_ssh = crate::executor::run_ssh_hook();
        FAKE_SSH_CALLS.store(0, Ordering::SeqCst);
        crate::executor::set_run_ssh(fake_login_fault_ssh);
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        s.set_setting(pulse_store::HUMAN_AGENT, "minutes_per_job", "1")
            .expect("set minutes");
        let workdir = tempfile::tempdir().expect("workdir");
        crate::executor::add_executor(
            &s,
            TEST_NOW,
            "hands",
            "pulse@hands",
            &workdir.path().display().to_string(),
            1,
            "*",
            pulse_store::HUMAN_AGENT,
        )
        .expect("add executor");
        let (ok, note) =
            crate::executor::probe_executor(&s, "hands", pulse_store::HUMAN_AGENT, TEST_NOW)
                .expect("probe");
        assert!(ok, "probe: {ok} {note}");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "remote".into(),
            "say hi remotely".into(),
            1,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .expect("add job");
        let cmd = "sh -c 'echo \"Error: Authentication token found but could not be validated. GitHub returned: Sorry. Your account was suspended\" >&2; exit 1'";
        let (r, reason) = crate::run::work_once(
            &s,
            "aurora",
            "",
            "",
            TEST_NOW,
            crate::run::RunOpts {
                executor: "hands".into(),
                worker_cmd: cmd.into(),
                ..Default::default()
            },
        )
        .expect("work");
        crate::executor::restore_run_ssh(old_ssh);
        *crate::tmpspace::FREE_SPACE.write().expect("free space") = old_space;
        assert!(!r.0.is_empty(), "work returned no run: {reason}");
        assert_eq!(r.str("status"), "error", "run = {r:?}");
        assert!(
            r.str("summary").contains("the login on hands, not the job"),
            "run = {r:?}"
        );
        let j = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(j.str("status"), "queued", "job = {j:?}");
        assert_eq!(j.int("attempts"), 0, "job = {j:?}");
        let ex = crate::executor::executor_row(&s, "hands");
        assert_eq!(ex.str("status"), "drained", "executor = {ex:?}");
        assert!(ex.str("probe_note").contains("fault:"), "executor = {ex:?}");
    }

    #[test]
    fn peer_questions_same_timestamp_answers() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for mode in ["linked", "same-time thread reply", "older same-time reply"] {
            let s = new_store();
            new_repo(&s);
            join(&s, "aurora");
            join(&s, "borealis");
            let post = |id: &str, kind: &str, on: &str| {
                pulse_board::create_post(
                    &s,
                    pulse_board::PostInput {
                        post_id: id.into(),
                        project: "proj".into(),
                        kind: kind.into(),
                        area: "parser".into(),
                        thread: "empty-input".into(),
                        agent: "aurora".into(),
                        agent_row: crate::board::actor_agent(&s, "aurora".into()).map(|a| *a),
                        summary: id.into(),
                        on: on.into(),
                        ..Default::default()
                    },
                )
                .expect("create post");
            };
            if mode == "older same-time reply" {
                post("a-before", "answer", "");
            }
            post("q-first", "question", "");
            post("q-second", "question", "");
            match mode {
                "linked" => {
                    pulse_board::create_post(
                        &s,
                        pulse_board::PostInput {
                            post_id: "a-linked".into(),
                            project: "proj".into(),
                            kind: "answer".into(),
                            area: "parser".into(),
                            thread: "empty-input".into(),
                            agent: "borealis".into(),
                            agent_row: crate::board::actor_agent(&s, "borealis".into()).map(|a| *a),
                            summary: "First question answered".into(),
                            on: "q-first".into(),
                            ..Default::default()
                        },
                    )
                    .expect("create linked answer");
                }
                "same-time thread reply" => post("a-after", "answer", ""),
                _ => {}
            }
            let open = crate::plan::open_questions(
                &s,
                "proj".into(),
                crate::util::plus_minutes(TEST_NOW, 2.0),
            );
            match mode {
                "linked" => {
                    assert_eq!(
                        open.len(),
                        1,
                        "linked reply consumed unrelated question: {open:?}"
                    );
                    assert_eq!(open[0].str("id"), "q-second");
                }
                "same-time thread reply" => assert!(
                    open.is_empty(),
                    "answered questions would waste new worker runs: {open:?}"
                ),
                "older same-time reply" => assert_eq!(
                    open.len(),
                    2,
                    "older reply consumed fresh questions: {open:?}"
                ),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn peer_help_wakes_lead_once_without_duplicate_worker() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_repo(&s);
        join(&s, "aurora");
        crate::lead::ensure_lead(&s, "proj".into()).expect("ensure lead");
        let q = pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                post_id: "q-peer-help".into(),
                project: "proj".into(),
                kind: "question".into(),
                area: "parser".into(),
                thread: "empty-input".into(),
                agent: "aurora".into(),
                agent_row: crate::board::actor_agent(&s, "aurora".into()).map(|a| *a),
                summary: "@proj-lead which existing fixture covers empty input?".into(),
                body: "I reproduced the failure and tried the parser guard.".into(),
                ..Default::default()
            },
        )
        .expect("create question");
        let lines = crate::lead::tend_lead(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
        let jobs = must_rows(
            &s,
            "SELECT * FROM jobs WHERE kind = ?",
            &[&crate::lead::LEAD_KIND],
        );
        assert_eq!(
            jobs.len(),
            1,
            "peer blocker did not reach lead: jobs={jobs:?} lines={lines:?}"
        );
        assert!(
            jobs[0].str("brief").contains(&q.id),
            "lead brief lacks question id: {:?}",
            jobs[0]
        );
        assert!(
            jobs[0].str("title").contains("help with 1 peer request(s)"),
            "lead title = {}",
            jobs[0].str("title")
        );
        crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into())
            .expect("plan jobs");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source = ?",
                &[&format!("question:{}", q.id)]
            ),
            0,
            "lead-directed question also dispatched ordinary workers"
        );
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?",
            &[&TEST_NOW, &jobs[0].str("id")],
        );
        for now in [
            crate::util::plus_minutes(TEST_NOW, 1.0),
            crate::util::plus_hours(TEST_NOW, 7.0),
        ] {
            crate::lead::tend_lead(&s, "proj".into(), now, "daemon".into());
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?",
                    &[&crate::lead::LEAD_KIND]
                ),
                1,
                "unchanged peer request spun up another lead"
            );
        }
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                post_id: "q-new-evidence".into(),
                project: "proj".into(),
                kind: "question".into(),
                area: "parser".into(),
                thread: "empty-input".into(),
                agent: "aurora".into(),
                agent_row: crate::board::actor_agent(&s, "aurora".into()).map(|a| *a),
                summary: "@proj-lead new evidence: the fixture also fails on main".into(),
                date: crate::util::plus_hours(TEST_NOW, 8.0),
                ..Default::default()
            },
        )
        .expect("create new evidence");
        crate::lead::tend_lead(
            &s,
            "proj".into(),
            crate::util::plus_hours(TEST_NOW, 8.0),
            "daemon".into(),
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?",
                &[&crate::lead::LEAD_KIND]
            ),
            2,
            "new peer evidence did not get a fresh lead turn"
        );
    }

    #[test]
    fn peer_help_does_not_bypass_owner_gates() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for mode in [
            "owner flag",
            "owner paths",
            "tier two",
            "hard stop",
            "loop stop",
            "daemon",
            "answered",
            "closed",
            "owner report",
        ] {
            let s = new_store();
            new_repo(&s);
            join(&s, "aurora");
            crate::lead::ensure_lead(&s, "proj".into()).expect("ensure lead");
            let mut input = pulse_board::PostInput {
                post_id: "q-guarded".into(),
                project: "proj".into(),
                kind: "question".into(),
                area: "parser".into(),
                thread: "empty-input".into(),
                agent: "aurora".into(),
                agent_row: crate::board::actor_agent(&s, "aurora".into()).map(|a| *a),
                summary: "@proj-lead help with the parser".into(),
                ..Default::default()
            };
            match mode {
                "owner flag" => input.for_owner = true,
                "owner paths" => input.body = "Edit docs/evolution/constitution.md".into(),
                "tier two" => input.summary = "@proj-lead should ADR-021 change?".into(),
                "hard stop" => input.body = "This needs a hard stop overridden.".into(),
                "loop stop" => {
                    input.summary = pulse_evolve::STOP_SUMMARY.into();
                    input.body = "@proj-lead".into();
                }
                "daemon" => {
                    join(&s, "daemon");
                    input.agent = "daemon".into();
                    input.agent_row = crate::board::actor_agent(&s, "daemon".into()).map(|a| *a);
                }
                "closed" => input.resolution = "answered".into(),
                "owner report" => {
                    input.kind = "result".into();
                    input.summary = "failed: requires owner approval".into();
                    input.body = "- Finding: @proj-lead need approval".into();
                }
                _ => {}
            }
            pulse_board::create_post(&s, input.clone()).expect("create guarded post");
            if mode == "answered" {
                pulse_board::create_post(
                    &s,
                    pulse_board::PostInput {
                        post_id: "a-guarded".into(),
                        project: "proj".into(),
                        kind: "answer".into(),
                        area: input.area,
                        thread: input.thread,
                        agent: "aurora".into(),
                        agent_row: crate::board::actor_agent(&s, "aurora".into()).map(|a| *a),
                        summary: "I found the fixture.".into(),
                        on: input.post_id,
                        ..Default::default()
                    },
                )
                .expect("create answer");
            }
            let lines = crate::lead::tend_lead(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?",
                    &[&crate::lead::LEAD_KIND]
                ),
                0,
                "{mode}: guarded mention woke lead: {lines:?}"
            );
        }
    }

    #[test]
    fn peer_help_batches_only_delivered_mentions() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_repo(&s);
        join(&s, "aurora");
        crate::lead::ensure_lead(&s, "proj".into()).expect("ensure lead");
        for i in 0..14 {
            pulse_board::create_post(
                &s,
                pulse_board::PostInput {
                    post_id: format!("q-peer-{i:02}"),
                    project: "proj".into(),
                    kind: "question".into(),
                    area: "parser".into(),
                    thread: format!("fixture-{i:02}"),
                    agent: "aurora".into(),
                    agent_row: crate::board::actor_agent(&s, "aurora".into()).map(|a| *a),
                    summary: "@proj-lead help diagnose this fixture".into(),
                    ..Default::default()
                },
            )
            .expect("create peer post");
        }
        for (i, want) in [10, 4].into_iter().enumerate() {
            let now = crate::util::plus_minutes(TEST_NOW, i as f64);
            crate::lead::tend_lead(&s, "proj".into(), now.clone(), "daemon".into());
            let jobs = must_rows(
                &s,
                "SELECT * FROM jobs WHERE kind = ? ORDER BY rowid",
                &[&crate::lead::LEAD_KIND],
            );
            assert_eq!(jobs.len(), i + 1, "batch {i}: jobs={jobs:?}");
            assert!(
                jobs[i]
                    .str("title")
                    .contains(&format!("help with {want} peer request(s)")),
                "batch {i}: jobs={jobs:?}"
            );
            assert_eq!(
                jobs[i].str("brief").matches("- q-peer-").count(),
                10,
                "batch {i}: brief = {}",
                jobs[i].str("brief")
            );
            exec1(
                &s,
                "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?",
                &[&now, &jobs[i].str("id")],
            );
        }
        crate::lead::tend_lead(
            &s,
            "proj".into(),
            crate::util::plus_minutes(TEST_NOW, 2.0),
            "daemon".into(),
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?",
                &[&crate::lead::LEAD_KIND]
            ),
            2,
            "delivered help requests churned"
        );
    }

    #[test]
    fn incomplete_worker_reports_reach_recovery_and_peer_help() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let old_space = use_ample_tmp_space();
        for prefix in ["blocked:", "FAILED:"] {
            let s = new_store();
            new_repo(&s);
            join(&s, "aurora");
            crate::lead::ensure_lead(&s, "proj".into()).expect("ensure lead");
            let (job, _) = crate::jobs::add_job(
                &s,
                "proj".into(),
                "task".into(),
                "Fix parser".into(),
                "Fix the parser guard.".into(),
                0,
                0,
                String::new(),
                pulse_store::HUMAN_AGENT.into(),
                false,
            )
            .expect("add job");
            let summary = format!("{prefix} tried the parser guard; the fixture still fails");
            let cmd = format!(
                "sh -c {}",
                crate::util::shell_quote(&format!(
                    "printf '%s\\n' {} {}",
                    crate::util::shell_quote(&format!("SUMMARY: {summary}")),
                    crate::util::shell_quote(
                        "FINDING: @proj-lead need a second diagnosis of the parser fixture"
                    )
                ))
            );
            let (r, reason) = crate::run::work_once(
                &s,
                "aurora",
                "proj",
                &job.str("id"),
                TEST_NOW,
                crate::run::RunOpts {
                    worker_cmd: cmd,
                    ..Default::default()
                },
            )
            .expect("work");
            assert!(!r.0.is_empty(), "work returned no run: {reason}");
            assert_eq!(r.str("status"), "failed", "{prefix}: run = {r:?}");
            assert_eq!(r.str("summary"), summary, "{prefix}: run = {r:?}");
            let finished = crate::jobs::job_row(&conn(&s), job.str("id"));
            assert_eq!(
                finished.str("status"),
                "failed",
                "{prefix}: job = {finished:?}"
            );
            assert_eq!(finished.str("branch"), "", "{prefix}: job = {finished:?}");
            crate::lead::tend_lead(
                &s,
                "proj".into(),
                crate::util::plus_minutes(TEST_NOW, 1.0),
                "daemon".into(),
            );
            let leads = must_rows(
                &s,
                "SELECT * FROM jobs WHERE kind = ?",
                &[&crate::lead::LEAD_KIND],
            );
            assert_eq!(leads.len(), 1, "{prefix}: leads = {leads:?}");
            assert!(
                leads[0].str("brief").contains("need a second diagnosis"),
                "{prefix}: leads = {leads:?}"
            );
        }
        *crate::tmpspace::FREE_SPACE.write().expect("free space") = old_space;
    }

    #[test]
    fn incomplete_report_prefixes_are_explicit() {
        let _serial = crate::test_helpers::serial();
        for (summary, failed) in [
            ("blocked: fixture still fails", true),
            ("FAILED: tried both parser paths", true),
            ("Blocked : missing evidence", true),
            ("fixed the previously blocked fixture", false),
            ("failed tests now pass", false),
            ("diagnosis: failed parser guard", false),
            ("", false),
        ] {
            let mut report = crate::brief::Report {
                summary: summary.into(),
                ..Default::default()
            };
            assert_eq!(report.incomplete(), failed, "incomplete({summary:?})");
        }
    }

    #[test]
    fn worker_brief_recovery_is_bounded_and_transport_aware() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (project, _) = new_repo(&s);
        for remote in [false, true] {
            let mut job = Row::new();
            job.insert("id", json!("job-peer"));
            job.insert("kind", json!("task"));
            job.insert("tier", json!(0));
            let brief = crate::test_helpers_f4::must_write_brief(crate::brief::BriefOpts {
                job,
                project: project.clone(),
                agent: "aurora".into(),
                clone: "/clone".into(),
                minutes: 10,
                remote,
                store: Some((*s).clone()),
                now: TEST_NOW.into(),
                ..Default::default()
            });
            for want in [
                "## Working through a blocker",
                "different safe approach",
                "Do not poll",
                "SUMMARY: blocked:",
                "Never weaken a check",
                "never bypass",
            ] {
                assert!(
                    brief.contains(want),
                    "remote={remote} missing recovery guidance {want:?}: {brief}"
                );
            }
            if remote {
                assert!(
                    brief.contains("FINDING: @proj-lead"),
                    "storeless worker cannot ask its lead for help: {brief}"
                );
                for invalid in [
                    "Claim your thread before you start (pulse claim",
                    "Reply on that thread with `pulse post answer",
                ] {
                    assert!(
                        !brief.contains(invalid),
                        "remote worker instructed to use unavailable store: {invalid:?}"
                    );
                }
            } else {
                assert!(
                    brief.contains("pulse post question --project proj"),
                    "local worker has no actionable peer help command: {brief}"
                );
            }
        }
    }
}
