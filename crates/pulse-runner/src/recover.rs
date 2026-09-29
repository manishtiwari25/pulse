//! Port of internal/runner/recover.go.

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

fn event_at(q: &Querier, at: &str, actor: &str, action: &str, subject: &str, detail: Value) {
    let text = if detail.is_null() {
        String::new()
    } else if let Some(s) = detail.as_str() {
        s.to_string()
    } else {
        crate::util::py_json(&detail)
    };
    exec1(
        q,
        "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
        &[&at, &actor, &action, &subject, &text],
    );
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
            );
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
            );
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
            );
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
            );
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
            );
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
            );
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
            );
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
            );
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
            );
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
        );
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
        );
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
) {
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
    );
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
        Some(Value::Null) | None => String::new(),
        Some(v) => match v {
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::Array(_) | Value::Object(_) => v.to_string(),
            Value::String(_) | Value::Null => unreachable!(),
        },
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
    macro_rules! ignored {
        ($name:ident, $reason:literal) => {
            #[test]
            #[ignore = $reason]
            fn $name() {}
        };
    }

    #[test]
    fn retry_title_prefixes_once() {
        assert_eq!(
            super::retry_title("job-1".into(), "Fix the build".into()),
            "Retry job-1: Fix the build"
        );
        assert_eq!(
            super::retry_title("job-1".into(), "Retry job-1: Fix the build".into()),
            "Retry job-1: Fix the build"
        );
    }

    ignored!(
        post_recovery_finding_dedupe_uses_normalized_thread,
        "cross-slice: board::create_post"
    );
    ignored!(
        post_recovery_finding_dedupe_allows_summary_drift,
        "cross-slice: board::create_post"
    );
    ignored!(
        recover_off_switch_writes_note_only_with_failures,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_transient_waits_then_requeues_and_drops_branch,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_deterministic_non_task_queues_corrected_job_with_log_tail,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_corrected_chain_keeps_root_and_backoff,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_dedupe_open_corrected_job,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_holds_owner_cancelled_jobs,
        "cross-slice: failure::failure_signature"
    );
    ignored!(
        recover_task_owned_deterministic_waits_for_lead,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_structural_held_without_finding,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_limit_held_posts_one_finding_across_ticks,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_skips_owned_kinds_and_old_window,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_per_tick_cap,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_meta_shape_and_removal_preserves_other_projects,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_idempotent_for_handled_failure,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_superseded_failure_is_held_once,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_task_owned_transient_defers_to_lead_follow_up,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_pending_only_for_unanswered_transient_within_limit,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_waits_while_the_queue_is_full,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_never_retries_answer_jobs,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_failed_write_is_atomic,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_does_not_mutate_stale_failures,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_branch_cleanup_failure_remains_retryable,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_cleanup_only_deletes_this_jobs_branches,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_one_corrected_job_without_source,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_chain_has_one_corrected_carrier,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_write_errors_respect_tick_cap,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_held_write_errors_respect_tick_cap,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_same_second_failure_is_not_stranded,
        "cross-slice: jobs::lease_job"
    );
    ignored!(
        recovery_resilience_pending_matches_recovery_gates,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_corrected_brief_is_bounded,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_compacts_legacy_brief_without_dropping_instructions,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_missing_log_is_explicit,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recovery_resilience_cap_finding_retries_after_write_failure,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        sweep_lost_runs_releases_lease,
        "cross-slice: sweep::sweep_lost_runs"
    );
    ignored!(report_reported, "cross-slice: run::parse_result_text");
    ignored!(
        launch_rejection_on_local_is_runner_error,
        "cross-slice: run::work_once"
    );
    ignored!(
        native_not_logged_in_exit_four_is_runner_error_after_slow_start,
        "cross-slice: run::work_once"
    );
    ignored!(
        worker_exit_summary_includes_harness_stop_reason,
        "cross-slice: run::work_once"
    );
    ignored!(
        worker_exit_five_is_owner_cancelled_failure,
        "cross-slice: run::work_once"
    );
    ignored!(
        failure_worker_exit_five_keeps_plain_cancel_summary,
        "cross-slice: run::work_once"
    );
    ignored!(
        run_harness_cancel_watcher_cancels_context,
        "cross-slice: run::watch_run_harness_cancel_every"
    );
    ignored!(
        launch_rejection_on_remote_drains_executor,
        "cross-slice: run::work_once"
    );
    ignored!(
        peer_questions_same_timestamp_answers,
        "cross-slice: lead::open_questions"
    );
    ignored!(
        peer_help_wakes_lead_once_without_duplicate_worker,
        "cross-slice: lead::tend_lead"
    );
    ignored!(
        peer_help_does_not_bypass_owner_gates,
        "cross-slice: lead::tend_lead"
    );
    ignored!(
        peer_help_batches_only_delivered_mentions,
        "cross-slice: lead::tend_lead"
    );
    ignored!(
        incomplete_worker_reports_reach_recovery_and_peer_help,
        "cross-slice: run::work_once"
    );
    ignored!(
        incomplete_report_prefixes_are_explicit,
        "cross-slice: run::report::incomplete"
    );
    ignored!(
        worker_brief_recovery_is_bounded_and_transport_aware,
        "cross-slice: brief::must_write_brief"
    );
}
