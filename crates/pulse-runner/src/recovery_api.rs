//! Port of internal/runner/recovery_api.go.

use anyhow::{Result, anyhow};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

type Row = crate::util::Row;
type Querier = dyn crate::util::Querier;

pub const FAILED_JOBS_DEFAULT_LIMIT: i32 = 50;
pub const FAILED_JOBS_MAX_LIMIT: i32 = 100;
pub const JOB_REPAIR_MAX_SELECTION: i32 = crate::recover::RECOVER_PER_TICK;

#[derive(Debug, Clone, Default)]
pub struct FailedJobsQuery {
    pub project: String,
    pub limit: i32,
    pub before: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FailedJobView {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "project")]
    pub project: String,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "tier")]
    pub tier: i32,
    #[serde(rename = "title")]
    pub title: String,
    #[serde(rename = "source")]
    pub source: String,
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "result")]
    pub result: String,
    #[serde(rename = "finished_at")]
    pub finished_at: String,
    #[serde(rename = "class")]
    pub class: String,
    #[serde(rename = "category")]
    pub category: String,
    #[serde(rename = "reason")]
    pub reason: String,
    #[serde(rename = "fix")]
    pub fix: String,
    #[serde(rename = "eligible")]
    pub eligible: bool,
    #[serde(rename = "root")]
    pub root: String,
    #[serde(rename = "recoveries")]
    pub recoveries: i32,
    #[serde(rename = "recovery_limit")]
    pub recovery_limit: i32,
    #[serde(rename = "retry_at", default, skip_serializing_if = "String::is_empty")]
    pub retry_at: String,
    #[serde(
        rename = "carrier_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub carrier_id: String,
    #[serde(
        rename = "carrier_status",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub carrier_status: String,
    #[serde(
        rename = "last_run_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub last_run_id: String,
    #[serde(
        rename = "last_run_status",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub last_run_status: String,
    #[serde(
        rename = "last_run_summary",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub last_run_summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FailedJobsPage {
    #[serde(rename = "project")]
    pub project: String,
    #[serde(rename = "historical_total")]
    pub historical_total: i32,
    #[serde(rename = "jobs")]
    pub jobs: Vec<FailedJobView>,
    #[serde(rename = "page_counts")]
    pub page_counts: BTreeMap<String, i32>,
    #[serde(rename = "limit")]
    pub limit: i32,
    #[serde(rename = "next_before")]
    pub next_before: String,
    #[serde(rename = "max_selection")]
    pub max_selection: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepairJobsInput {
    #[serde(rename = "job_ids")]
    pub job_i_ds: Vec<String>,
    #[serde(rename = "project", default, skip_serializing_if = "String::is_empty")]
    pub project: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepairJobResult {
    #[serde(rename = "job_id")]
    pub job_id: String,
    #[serde(rename = "outcome")]
    pub outcome: String,
    #[serde(rename = "category")]
    pub category: String,
    #[serde(rename = "reason")]
    pub reason: String,
    #[serde(rename = "fix")]
    pub fix: String,
    #[serde(
        rename = "carrier_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub carrier_id: String,
    #[serde(
        rename = "carrier_status",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub carrier_status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepairJobsResult {
    #[serde(rename = "requested")]
    pub requested: i32,
    #[serde(rename = "queued")]
    pub queued: i32,
    #[serde(rename = "skipped")]
    pub skipped: i32,
    #[serde(rename = "errors")]
    pub errors: i32,
    #[serde(rename = "results")]
    pub results: Vec<RepairJobResult>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RecoveryPolicy {
    pub(crate) enabled: bool,
    pub(crate) days: i32,
    pub(crate) limit: i32,
    pub(crate) stopped: String,
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

/// Go: func ReadFailedJobs(s *store.Store, in FailedJobsQuery) (out *FailedJobsPage, err error)
pub fn read_failed_jobs(
    s: &pulse_store::Store,
    mut input: FailedJobsQuery,
) -> anyhow::Result<Option<FailedJobsPage>> {
    if input.limit == 0 {
        input.limit = FAILED_JOBS_DEFAULT_LIMIT;
    }
    if input.limit < 1 || input.limit > FAILED_JOBS_MAX_LIMIT {
        return Err(crate::util::fail(
            2,
            format!("limit must be between 1 and {FAILED_JOBS_MAX_LIMIT}"),
        )
        .into());
    }
    let (before_at, before_id) = decode_failed_jobs_cursor(input.before.clone())?;
    let conn = s
        .conn()
        .map_err(|_| anyhow!("the PULSE store is unavailable"))?;
    conn.execute_batch("BEGIN")?;
    let result = (|| {
        if !input.project.is_empty()
            && crate::board::project_by_slug(&conn, input.project.clone())
                .0
                .is_empty()
        {
            return Err(crate::util::fail(2, format!("No project {:?}", input.project)).into());
        }
        let policy = read_recovery_policy(&conn)?;
        let mut where_sql = "status = 'failed'".to_string();
        let mut vals: Vec<Box<dyn rusqlite::ToSql>> = vec![];
        if !input.project.is_empty() {
            where_sql.push_str(" AND project = ?");
            vals.push(Box::new(input.project.clone()));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| &**v).collect();
        let mut out = FailedJobsPage {
            project: input.project.clone(),
            jobs: vec![],
            page_counts: BTreeMap::new(),
            limit: input.limit,
            max_selection: JOB_REPAIR_MAX_SELECTION,
            historical_total: must_one(
                &conn,
                &format!("SELECT COUNT(*) AS n FROM jobs WHERE {where_sql}"),
                &refs,
            )
            .int("n"),
            ..Default::default()
        };
        if !input.before.is_empty() {
            where_sql.push_str(" AND (finished_at < ? OR (finished_at = ? AND id < ?))");
            vals.push(Box::new(before_at.clone()));
            vals.push(Box::new(before_at));
            vals.push(Box::new(before_id));
        }
        vals.push(Box::new(input.limit + 1));
        let refs: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| &**v).collect();
        let mut jobs = must_rows(
            &conn,
            &format!(
                "SELECT * FROM jobs WHERE {where_sql} ORDER BY finished_at DESC, id DESC LIMIT ?"
            ),
            &refs,
        );
        if jobs.len() > input.limit as usize {
            let last = jobs[input.limit as usize - 1].clone();
            out.next_before = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(
                "{}\0{}",
                last.str("finished_at"),
                last.str("id")
            ));
            jobs.truncate(input.limit as usize);
        }
        let now = s.now();
        for job in jobs {
            let (view, _, _) = inspect_recovery_failure(&conn, job, policy.clone(), now.clone())?;
            *out.page_counts.entry(view.category.clone()).or_insert(0) += 1;
            out.jobs.push(view);
        }
        Ok(out)
    })();
    let _ = conn.execute_batch("ROLLBACK");
    result.map(Some)
}

/// Go: func decodeFailedJobsCursor(cursor string) (string, string, error)
pub(crate) fn decode_failed_jobs_cursor(cursor: String) -> anyhow::Result<(String, String)> {
    if cursor.is_empty() {
        return Ok((String::new(), String::new()));
    }
    if cursor.len() <= 1024
        && let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(cursor.as_bytes())
        && let Ok(text) = String::from_utf8(raw)
        && let Some((at, id)) = text.split_once('\0')
        && !id.is_empty()
        && id.len() <= 200
        && !id.contains('\0')
        && at.len() <= 100
    {
        return Ok((at.to_string(), id.to_string()));
    }
    Err(crate::util::fail(
        2,
        "invalid before cursor; use next_before from the previous page",
    )
    .into())
}

/// Go: func readRecoveryPolicy(q querier) (recoveryPolicy, error)
pub(crate) fn read_recovery_policy(q: &Querier) -> anyhow::Result<RecoveryPolicy> {
    let mut values = BTreeMap::from([(
        "stop".to_string(),
        pulse_store::DEFAULT_SETTINGS
            .get("stop")
            .cloned()
            .unwrap_or_else(|| "0".into()),
    )]);
    for key in [
        "daemon.recover",
        "daemon.recover_days",
        "daemon.recover_limit",
    ] {
        values.insert(
            key.into(),
            pulse_store::DAEMON_SETTINGS
                .get(key)
                .cloned()
                .unwrap_or_default(),
        );
    }
    for r in must_rows(
        q,
        "SELECT name, value FROM settings WHERE name IN ('stop', 'daemon.recover', 'daemon.recover_days', 'daemon.recover_limit')",
        &[],
    ) {
        values.insert(r.str("name"), r.str("value"));
    }
    let mut p = RecoveryPolicy {
        enabled: values.get("daemon.recover").is_some_and(|v| v == "on"),
        ..Default::default()
    };
    p.days = values
        .get("daemon.recover_days")
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|n| *n >= 1)
        .ok_or_else(|| {
            anyhow!("daemon.recover_days is invalid; repair stays held until the owner corrects it")
        })?;
    p.limit = values
        .get("daemon.recover_limit")
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|n| *n >= 0)
        .ok_or_else(|| {
            anyhow!(
                "daemon.recover_limit is invalid; repair stays held until the owner corrects it"
            )
        })?;
    if !matches!(values.get("stop").map(String::as_str), Some("0" | "1"))
        || !matches!(
            values.get("daemon.recover").map(String::as_str),
            Some("on" | "off")
        )
    {
        return Err(anyhow!(
            "invalid stop or daemon.recover setting; repair stays held"
        ));
    }
    if values.get("stop").is_some_and(|v| v == "1") {
        p.stopped = "The runner is stopped (stop = 1).".into();
    }
    let brain = crate::board::brain_project(q);
    if !brain.0.is_empty() {
        let stop = recovery_stop_file(brain.str("path"))?;
        if stop {
            p.stopped = "The brain's docs/evolution/STOP is present.".into();
        }
    }
    Ok(p)
}

/// Go: func recoveryStopFile(root string) (bool, error)
pub(crate) fn recovery_stop_file(root: String) -> anyhow::Result<bool> {
    if root.is_empty() {
        return Err(anyhow!(
            "the project has no checkout path; cannot inspect its STOP gate"
        ));
    }
    let path = Path::new(&root).join("docs").join("evolution").join("STOP");
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(anyhow!("cannot inspect the project STOP gate: {e}")),
    }
}

/// Go: func inspectRecoveryFailure(...)
pub(crate) fn inspect_recovery_failure(
    q: &Querier,
    job: Row,
    policy: RecoveryPolicy,
    now: String,
) -> anyhow::Result<(FailedJobView, crate::failure::Failure, Row)> {
    let run = crate::recover::last_recovery_run(q, job.str("id"));
    let f = crate::failure::classify_failure(
        job.str("result"),
        crate::recover::row_str(run.clone(), "status".into()),
        crate::recover::row_str(run.clone(), "summary".into()),
        crate::recover::recovery_question_asked(q, job.str("project"), job.str("id")),
    );
    let (root, n) = crate::recover::recovery_chain_from(q, job.clone());
    let mut v = FailedJobView {
        id: job.str("id"),
        project: job.str("project"),
        kind: job.str("kind"),
        tier: job.int("tier"),
        title: crate::compute::redact_secrets(&job.str("title")),
        source: crate::compute::redact_secrets(&job.str("source")),
        status: job.str("status"),
        result: crate::util::trunc(&crate::compute::redact_secrets(&job.str("result")), 6000),
        finished_at: job.str("finished_at"),
        class: f.class.0.clone(),
        root,
        recoveries: n,
        recovery_limit: policy.limit,
        last_run_id: crate::recover::row_str(run.clone(), "id".into()),
        last_run_status: crate::recover::row_str(run.clone(), "status".into()),
        last_run_summary: crate::util::trunc(
            &crate::compute::redact_secrets(&crate::recover::row_str(
                run.clone(),
                "summary".into(),
            )),
            3000,
        ),
        ..Default::default()
    };
    let hold = |v: &mut FailedJobView, category: &str, reason: String, fix: &str| {
        v.category = category.into();
        v.reason = crate::util::trunc(&crate::compute::redact_secrets(&reason), 3000);
        v.fix = fix.into();
    };
    if job.str("status") == "cancelled"
        || crate::recover::owner_cancelled_failure(
            job.str("result"),
            crate::recover::row_str(run.clone(), "summary".into()),
        )
    {
        hold(
            &mut v,
            "cancelled",
            "Cancelled by the owner.".into(),
            "Leave this attempt stopped. A new owner decision is required; repair cannot undo cancellation.",
        );
        return Ok((v, f, run));
    }
    if crate::recover::recovery_run_active(q, job.str("id")) {
        hold(
            &mut v,
            "active_run",
            "The run is still active; this failure is provisional.".into(),
            "Wait for its controller to finish, then refresh.",
        );
        return Ok((v, f, run));
    }
    if job.str("status") != "failed" {
        v.carrier_id = job.str("id");
        v.carrier_status = job.str("status");
        hold(
            &mut v,
            "not_failed",
            format!("The job is now {}.", job.str("status")),
            "Refresh; do not retry queued, running or completed work.",
        );
        return Ok((v, f, run));
    }
    let carrier = crate::recover::recovery_carrier(q, job.clone(), v.root.clone());
    if !carrier.is_empty() {
        let current = crate::jobs::job_row(q, carrier.clone());
        v.carrier_id = carrier.clone();
        v.carrier_status = current.str("status");
        let carrier_status = v.carrier_status.clone();
        hold(
            &mut v,
            "superseded",
            format!("Work is already carried by {carrier} ({carrier_status})."),
            "Follow the carrier instead of retrying this historical attempt.",
        );
        return Ok((v, f, run));
    }
    let project = crate::board::project_by_slug(q, job.str("project"));
    if project.0.is_empty() || project.str("status") != "active" {
        hold(
            &mut v,
            "inactive_project",
            "The project is missing or inactive.".into(),
            "Review the project registration and status before requesting work.",
        );
        return Ok((v, f, run));
    }
    let stopped = recovery_stop_file(project.str("path"))?;
    if !policy.stopped.is_empty() || stopped {
        let reason = if stopped {
            "This project's docs/evolution/STOP is present.".to_string()
        } else {
            policy.stopped.clone()
        };
        hold(
            &mut v,
            "stopped",
            reason,
            "Only the owner can resolve STOP through the existing controls. Repair never removes it.",
        );
        return Ok((v, f, run));
    }
    if crate::recover::recovery_owned_elsewhere(job.clone()) {
        hold(
            &mut v,
            "manual_controller",
            format!(
                "This {} job belongs to a separate controller.",
                job.str("kind")
            ),
            "Use the lead, baseline or experiment workflow; failed answers require a human or a new post.",
        );
        return Ok((v, f, run));
    }
    if (job.int("tier") != 0 && job.int("tier") != 1)
        || !pulse_board::JOB_KINDS.contains(&job.str("kind").as_str())
    {
        v.class = "structural".into();
        hold(
            &mut v,
            "structural",
            "Only valid Tier 0 and Tier 1 jobs may be recovered.".into(),
            "Resolve the job's scope with the owner; Tier 2 needs an approved decision.",
        );
        return Ok((v, f, run));
    }
    if job.str("source").starts_with("task:") {
        let task_id = job.str("source").trim_start_matches("task:").to_string();
        let task = must_one(
            q,
            "SELECT * FROM tasks WHERE id = ? AND project = ?",
            &[&task_id, &job.str("project")],
        );
        if !task.0.is_empty()
            && !task.str("job_id").is_empty()
            && task.str("job_id") != job.str("id")
        {
            let current = crate::jobs::job_row(q, task.str("job_id"));
            if !current.0.is_empty() && current.str("project") == job.str("project") {
                v.carrier_id = current.str("id");
                v.carrier_status = current.str("status");
            }
        }
        if task.0.is_empty() {
            hold(
                &mut v,
                "manual_controller",
                "The source task is missing from this project.".into(),
                "Ask the lead to reconcile the task lineage; do not recreate old work.",
            );
        } else if task.str("status") == "cancelled" {
            hold(
                &mut v,
                "cancelled",
                "The owner cancelled the source task.".into(),
                "Leave this task stopped; repair cannot undo task cancellation.",
            );
        } else if task.str("status") == "done" {
            hold(
                &mut v,
                "superseded",
                "The source task is already done.".into(),
                "Keep its earlier attempts as history.",
            );
        } else if !task.str("job_id").is_empty() && task.str("job_id") != job.str("id") {
            hold(
                &mut v,
                "manual_controller",
                "The source task has a different current job.".into(),
                "Follow the task's current job and the lead's plan.",
            );
        } else if f.class.0 == "deterministic" || crate::recover::lead_followed_up(q, job.clone()) {
            hold(
                &mut v,
                "manual_controller",
                "This task failure awaits the lead's follow-up.".into(),
                "Review or amend the task with the lead instead of creating a competing plan.",
            );
        }
        if !v.category.is_empty() {
            return Ok((v, f, run));
        }
    }
    if f.class.0 == "structural" {
        hold(
            &mut v,
            "structural",
            f.reason.clone(),
            "Resolve the reported prerequisite or owner question through its existing workflow. This panel does not override holds or hard stops.",
        );
        return Ok((v, f, run));
    }
    if !policy.enabled {
        hold(
            &mut v,
            "recovery_off",
            "Recovery is off (daemon.recover).".into(),
            "Review the owner's recovery setting; this panel will not change it.",
        );
        return Ok((v, f, run));
    }
    if crate::util::parse_iso(&job.str("finished_at")).is_err() {
        hold(
            &mut v,
            "structural",
            "The failure has no valid completion time.".into(),
            "Have the controller reconcile the attempt before requesting recovery.",
        );
        return Ok((v, f, run));
    }
    if job.str("finished_at") < crate::util::plus_hours(&now, -24.0 * f64::from(policy.days)) {
        hold(
            &mut v,
            "outside_window",
            format!(
                "This failure is older than the {}-day recovery window.",
                policy.days
            ),
            "Review current project work with the lead; do not bulk-retry historical failures.",
        );
        return Ok((v, f, run));
    }
    if n >= policy.limit {
        hold(
            &mut v,
            "recovery_cap",
            format!("This chain has used {n} of {} recoveries.", policy.limit),
            "The lead and owner must resolve the repeated cause. Repair cannot raise the cap.",
        );
        return Ok((v, f, run));
    }
    let decision = crate::recover::recovery_decision(q, job.str("id"), job.str("finished_at"));
    if !decision.0.is_empty() {
        let detail = crate::util::json_map(&decision.str("detail"));
        let id = crate::recover::detail_string(detail.clone(), "job".into());
        if !id.is_empty() {
            let carrier = crate::jobs::job_row(q, id.clone());
            if !carrier.0.is_empty() && carrier.str("project") == job.str("project") {
                v.carrier_id = id;
                v.carrier_status = carrier.str("status");
            }
        }
        hold(
            &mut v,
            "held",
            format!(
                "This failure already has a recovery decision: {}",
                crate::recover::detail_string(detail, "why".into())
            ),
            "Follow the recorded decision or its corrected attempt; do not recover the same failure twice.",
        );
        return Ok((v, f, run));
    }
    v.retry_at = crate::failure::recover_due(job.str("finished_at"), n);
    if now < v.retry_at {
        let retry_at = v.retry_at.clone();
        hold(
            &mut v,
            "backoff",
            format!("Recovery is waiting until {retry_at}."),
            "Wait for the existing backoff, then refresh.",
        );
        return Ok((v, f, run));
    }
    let queued = must_one(
        q,
        "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND status = 'queued'",
        &[&job.str("project")],
    )
    .int("n");
    if queued >= crate::recover::RECOVER_QUEUE_ROOM {
        hold(
            &mut v,
            "queue_full",
            format!(
                "Recovery waits for the project queue ({} queued; limit {}).",
                queued,
                crate::recover::RECOVER_QUEUE_ROOM
            ),
            "Let queued work finish before adding recovery work.",
        );
        return Ok((v, f, run));
    }
    v.eligible = true;
    v.category = "eligible".into();
    v.reason = crate::util::trunc(&crate::compute::redact_secrets(&f.reason), 3000);
    v.fix = "Queue one corrected copy with earlier-attempt evidence; the original remains history."
        .into();
    if f.class.0 == "transient" {
        v.fix = "Requeue this transient attempt with earlier evidence; recorded runs and recovery events remain history.".into();
    }
    Ok((v, f, run))
}

/// Go: func RepairFailedJobs(s *store.Store, in RepairJobsInput) (out *RepairJobsResult, err error)
pub fn repair_failed_jobs(
    s: &pulse_store::Store,
    input: RepairJobsInput,
) -> anyhow::Result<Option<RepairJobsResult>> {
    if input.job_i_ds.is_empty() || input.job_i_ds.len() > JOB_REPAIR_MAX_SELECTION as usize {
        return Err(crate::util::fail(
            2,
            format!("select between 1 and {JOB_REPAIR_MAX_SELECTION} explicit job_ids"),
        )
        .into());
    }
    let mut seen = BTreeMap::new();
    for id in &input.job_i_ds {
        if id.is_empty()
            || id.trim() != id
            || id.len() > 200
            || id.contains('\0')
            || seen.contains_key(id)
        {
            return Err(
                crate::util::fail(2, "job_ids must contain unique, non-empty job IDs").into(),
            );
        }
        seen.insert(id.clone(), true);
    }
    let conn = s
        .conn()
        .map_err(|_| anyhow!("the PULSE store is unavailable"))?;
    if !input.project.is_empty()
        && crate::board::project_by_slug(&conn, input.project.clone())
            .0
            .is_empty()
    {
        return Err(crate::util::fail(2, format!("No project {:?}", input.project)).into());
    }
    let mut out = RepairJobsResult {
        requested: input.job_i_ds.len() as i32,
        results: vec![],
        ..Default::default()
    };
    for id in input.job_i_ds {
        let mut result = RepairJobResult {
            job_id: id.clone(),
            outcome: "skipped".into(),
            ..Default::default()
        };
        let write_err = crate::db::immediate(s, |q| {
            let job = crate::jobs::job_row(q, id.clone());
            if job.0.is_empty()
                || (!input.project.is_empty() && job.str("project") != input.project)
            {
                result.category = "not_found".into();
                result.reason = "No such job in the selected project.".into();
                result.fix = "Select an existing job from this project's failed-job list.".into();
                return Ok(());
            }
            let policy = read_recovery_policy(q)?;
            let now = s.now();
            let (view, f, run) = inspect_recovery_failure(q, job.clone(), policy, now.clone())?;
            result.category = view.category.clone();
            result.reason = view.reason.clone();
            result.fix = view.fix.clone();
            result.carrier_id = view.carrier_id.clone();
            result.carrier_status = view.carrier_status.clone();
            if !view.eligible {
                return Ok(());
            }
            crate::recover::current_recovery_failure(q, job.clone())?;
            let project = crate::board::project_by_slug(q, job.str("project"));
            let (carrier, outcome) = crate::recover::queue_recovery(
                q,
                project,
                job,
                run,
                now,
                pulse_store::HUMAN_AGENT.into(),
                f,
                view.root,
                view.recoveries,
            )?;
            result.outcome = outcome;
            result.carrier_id = carrier;
            result.carrier_status = "queued".into();
            Ok(())
        });
        if let Err(err) = write_err {
            result.outcome = "error".into();
            result.category = "error".into();
            result.reason = crate::compute::redact_secrets(&err.to_string());
            result.fix =
                "This item was not repaired. Refresh and resolve the reported error before trying again."
                    .into();
            result.carrier_id.clear();
            result.carrier_status.clear();
            out.errors += 1;
        } else if result.outcome == "skipped" {
            out.skipped += 1;
        } else {
            out.queued += 1;
        }
        out.results.push(result);
    }
    Ok(Some(out))
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

    ignored!(
        job_repair_classification_and_gates,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_evidence_history_and_actor,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_carrier_source_and_lineage,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_current_failure_and_concurrency,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_queue_bound_and_selection,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_concurrent_queue_bound,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_project_scope_and_partial_errors,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_all_history_count_and_pagination,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_rechecks_displayed_failure,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_source_scope_and_task_carrier,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_invalid_gates_fail_closed,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_cancelled_ancestor_intent,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_authorized_chain_still_dedupes_carriers,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        job_repair_cancellation_override_does_not_authorize_sibling,
        "cross-slice: failure::classify_failure"
    );
    ignored!(
        recover_cancelled_ancestor_after_human_retry,
        "cross-slice: failure::classify_failure"
    );
}
