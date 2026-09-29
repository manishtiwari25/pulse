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
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::exec1;
    use crate::test_helpers_f7::*;
    use crate::util::Row;
    use std::path::PathBuf;

    fn conn(s: &pulse_store::Store) -> pulse_store::PooledConn {
        s.conn().expect("conn")
    }

    fn repair_fixture() -> (crate::test_helpers::TestStore, PathBuf) {
        let s = new_store();
        let (_, root) = new_repo(&s);
        (s, root.into())
    }

    fn repair_failure(s: &pulse_store::Store, id: &str, result: &str) -> Row {
        insert_recover_failed_job(
            s,
            id,
            "improve",
            "",
            result,
            &crate::util::plus_minutes(TEST_NOW, -30.0),
        );
        exec1(
            s,
            "UPDATE jobs SET branch = '', lease_agent = '' WHERE id = ?",
            &[&id],
        );
        crate::jobs::job_row(&conn(s), id.to_string())
    }

    fn repair_one(s: &pulse_store::Store, id: &str) -> RepairJobResult {
        let out = super::repair_failed_jobs(
            s,
            RepairJobsInput {
                project: "proj".into(),
                job_i_ds: vec![id.into()],
            },
        )
        .expect("repair")
        .expect("repair output");
        assert_eq!(out.results.len(), 1, "repair output = {out:?}");
        out.results.into_iter().next().unwrap()
    }

    fn repair_store_at(s: &pulse_store::Store, hours: f64) -> pulse_store::Store {
        pulse_store::Store::open(pulse_store::Options {
            home: s.home.clone(),
            now: crate::util::plus_hours(TEST_NOW, hours),
            skip_records: true,
            ..Default::default()
        })
        .expect("open shifted store")
    }

    fn finish_at(s: &pulse_store::Store, hours: f64, id: &str, status: &str, result: &str) {
        let later = repair_store_at(s, hours);
        crate::jobs::finish_job(
            &later,
            id.into(),
            pulse_store::HUMAN_AGENT.into(),
            status.into(),
            result.into(),
            String::new(),
            -1,
        )
        .expect("finish job");
    }

    #[test]
    fn job_repair_classification_and_gates() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for (name, result_text, category, class, outcome) in [
            (
                "deterministic",
                "verify exit 1",
                "eligible",
                "deterministic",
                "replanned",
            ),
            (
                "transient",
                "runner error: connection reset",
                "eligible",
                "transient",
                "requeued",
            ),
            (
                "missing tool",
                "verify exit 127",
                "structural",
                "structural",
                "skipped",
            ),
            (
                "hard stop",
                "needs a real decision",
                "structural",
                "structural",
                "skipped",
            ),
            (
                "cancelled result",
                "cancelled by the owner: stop",
                "cancelled",
                "",
                "skipped",
            ),
            (
                "cancelled summary",
                "runner error",
                "cancelled",
                "",
                "skipped",
            ),
            (
                "cancelled status",
                "verify exit 1",
                "cancelled",
                "",
                "skipped",
            ),
            ("active run", "verify exit 1", "active_run", "", "skipped"),
            (
                "inactive",
                "verify exit 1",
                "inactive_project",
                "",
                "skipped",
            ),
            ("stop", "verify exit 1", "stopped", "", "skipped"),
            ("project stop", "verify exit 1", "stopped", "", "skipped"),
            (
                "project dangling stop",
                "verify exit 1",
                "stopped",
                "",
                "skipped",
            ),
            ("brain stop", "verify exit 1", "stopped", "", "skipped"),
            (
                "recovery off",
                "verify exit 1",
                "recovery_off",
                "",
                "skipped",
            ),
            ("cap", "verify exit 1", "recovery_cap", "", "skipped"),
            ("backoff", "verify exit 1", "backoff", "", "skipped"),
            ("history", "verify exit 1", "outside_window", "", "skipped"),
            (
                "bad timestamp",
                "verify exit 1",
                "structural",
                "",
                "skipped",
            ),
            (
                "tier 2",
                "verify exit 1",
                "structural",
                "structural",
                "skipped",
            ),
            ("lead", "verify exit 1", "manual_controller", "", "skipped"),
            (
                "baseline",
                "verify exit 1",
                "manual_controller",
                "",
                "skipped",
            ),
            (
                "experiment",
                "verify exit 1",
                "manual_controller",
                "",
                "skipped",
            ),
            (
                "answer",
                "verify exit 1",
                "manual_controller",
                "",
                "skipped",
            ),
            (
                "question",
                "verify exit 1",
                "structural",
                "structural",
                "skipped",
            ),
            ("held", "verify exit 1", "held", "", "skipped"),
            (
                "task deterministic",
                "verify exit 1",
                "manual_controller",
                "",
                "skipped",
            ),
            (
                "task transient",
                "connection reset",
                "eligible",
                "transient",
                "requeued",
            ),
            (
                "task followed up",
                "connection reset",
                "manual_controller",
                "",
                "skipped",
            ),
            (
                "task missing",
                "connection reset",
                "manual_controller",
                "",
                "skipped",
            ),
            (
                "task cancelled",
                "connection reset",
                "cancelled",
                "",
                "skipped",
            ),
            ("task done", "connection reset", "superseded", "", "skipped"),
        ] {
            let (s, mut root) = repair_fixture();
            let id = "job-repair";
            repair_failure(&s, id, result_text);
            match name {
                "cancelled summary" => {
                    insert_recover_run(
                        &s,
                        id,
                        "run-repair",
                        "failed",
                        "cancelled by the owner",
                        "",
                        "",
                    );
                }
                "cancelled status" => exec1(
                    &s,
                    "UPDATE jobs SET status = 'cancelled' WHERE id = ?",
                    &[&id],
                ),
                "active run" => {
                    insert_recover_run(&s, id, "run-repair", "running", "", "", "");
                }
                "inactive" => exec1(
                    &s,
                    "UPDATE projects SET status = 'inactive' WHERE slug = 'proj'",
                    &[],
                ),
                "stop" => set_recover_setting(&s, "stop", "1"),
                "project stop" | "project dangling stop" | "brain stop" => {
                    if name == "brain stop" {
                        root = tempfile::tempdir().expect("tempdir").keep();
                        exec1(
                            &s,
                            "INSERT INTO projects(slug,name,path,brain,added_at) VALUES ('brain','Brain',?,1,?)",
                            &[&root.to_string_lossy().to_string(), &TEST_NOW],
                        );
                    }
                    let dir = root.join("docs").join("evolution");
                    std::fs::create_dir_all(&dir).expect("mkdir stop dir");
                    if name == "project dangling stop" {
                        std::os::unix::fs::symlink("missing-stop-note", dir.join("STOP"))
                            .expect("symlink STOP");
                    } else {
                        std::fs::write(dir.join("STOP"), b"owner stop").expect("write STOP");
                    }
                }
                "recovery off" => set_recover_setting(&s, "daemon.recover", "off"),
                "cap" => set_recover_setting(&s, "daemon.recover_limit", "0"),
                "backoff" => exec1(
                    &s,
                    "UPDATE jobs SET finished_at = ? WHERE id = ?",
                    &[&crate::util::plus_minutes(TEST_NOW, -2.0), &id],
                ),
                "history" => exec1(
                    &s,
                    "UPDATE jobs SET finished_at = ? WHERE id = ?",
                    &[&crate::util::plus_hours(TEST_NOW, -24.0 * 30.0), &id],
                ),
                "bad timestamp" => {
                    exec1(&s, "UPDATE jobs SET finished_at = '' WHERE id = ?", &[&id])
                }
                "tier 2" => exec1(&s, "UPDATE jobs SET tier = 2 WHERE id = ?", &[&id]),
                "lead" | "baseline" | "experiment" | "answer" => {
                    exec1(&s, "UPDATE jobs SET kind = ? WHERE id = ?", &[&name, &id]);
                }
                "question" => {
                    crate::board::create_post(
                        &s,
                        TEST_NOW.into(),
                        crate::board::PostOpts {
                            kind: "question".into(),
                            project: "proj".into(),
                            area: "jobs".into(),
                            thread: id.into(),
                            summary: "Owner approval required".into(),
                            agent: crate::board::actor_agent(&s, "daemon".into()),
                            for_owner: true,
                            ..Default::default()
                        },
                    )
                    .expect("create question");
                }
                "held" => crate::recover::recovery_event(
                    &conn(&s),
                    TEST_NOW.into(),
                    "daemon".into(),
                    id.into(),
                    "held".into(),
                    crate::failure::Failure {
                        class: crate::failure::FailureClass(
                            crate::failure::FAILURE_DETERMINISTIC.into(),
                        ),
                        reason: "owner review".into(),
                        ..Default::default()
                    },
                    id.into(),
                    0,
                    String::new(),
                )
                .expect("recovery event"),
                _ => {}
            }
            if name.starts_with("task ") {
                exec1(
                    &s,
                    "UPDATE jobs SET source = 'task:TASK-1' WHERE id = ?",
                    &[&id],
                );
                if name != "task missing" {
                    let status = if name == "task cancelled" || name == "task done" {
                        name.trim_start_matches("task ")
                    } else {
                        "in_progress"
                    };
                    exec1(
                        &s,
                        "INSERT INTO tasks(id,project,title,status,job_id,created_by,created_at,updated_at) VALUES ('TASK-1','proj','Task',?,?,'human',?,?)",
                        &[&status, &id, &TEST_NOW, &TEST_NOW],
                    );
                }
                if name == "task followed up" {
                    exec1(
                        &s,
                        "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'proj-lead', ?, 'TASK-1', 'followed up')",
                        &[&TEST_NOW, &crate::recover::FOLLOW_UP_ACTION],
                    );
                }
            }
            let before = crate::jobs::job_row(&conn(&s), id.to_string());
            let events = count(&s, "SELECT COUNT(*) AS n FROM events", &[]);
            let page = super::read_failed_jobs(
                &s,
                FailedJobsQuery {
                    project: "proj".into(),
                    ..Default::default()
                },
            )
            .expect("read failed jobs")
            .expect("page");
            if name != "cancelled status" {
                assert_eq!(page.jobs.len(), 1, "{name}: page = {page:?}");
                assert_eq!(page.jobs[0].category, category, "{name}: page = {page:?}");
                assert_eq!(
                    page.jobs[0].eligible,
                    category == "eligible",
                    "{name}: page = {page:?}"
                );
                if !class.is_empty() {
                    assert_eq!(page.jobs[0].class, class, "{name}: page = {page:?}");
                }
                assert!(
                    !page.jobs[0].reason.is_empty() && !page.jobs[0].fix.is_empty(),
                    "{name}: no explanation"
                );
            }
            assert_eq!(
                count(&s, "SELECT COUNT(*) AS n FROM events", &[]),
                events,
                "{name}: GET wrote an event"
            );
            let result = repair_one(&s, id);
            assert_eq!(
                result.outcome, outcome,
                "{name}: repair result = {result:?}"
            );
            assert_eq!(
                result.category, category,
                "{name}: repair result = {result:?}"
            );
            if outcome == "skipped" {
                let after = crate::jobs::job_row(&conn(&s), id.to_string());
                for key in [
                    "status",
                    "result",
                    "branch",
                    "finished_at",
                    "lease_agent",
                    "attempts",
                ] {
                    assert_eq!(
                        before.str(key),
                        after.str(key),
                        "{name}: skipped repair mutated {key}"
                    );
                }
                assert_eq!(
                    count(&s, "SELECT COUNT(*) AS n FROM events", &[]),
                    events,
                    "{name}: skipped repair wrote audit"
                );
            }
        }
    }
    #[test]
    fn job_repair_evidence_history_and_actor() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, root) = repair_fixture();
        let original = repair_failure(&s, "job-evidence", "verify exit 1");
        let log = root.join("evidence.log");
        std::fs::write(
            &log,
            b"exact failing assertion\ngithub_pat_abcdefghijklmnopqrstuvwxyz",
        )
        .expect("write log");
        let run = insert_recover_run(
            &s,
            &original.str("id"),
            "run-evidence",
            "failed",
            "verify exit 1",
            &log.to_string_lossy(),
            "",
        );
        let repaired = repair_one(&s, &original.str("id"));
        assert_eq!(repaired.outcome, "replanned", "repair = {repaired:?}");
        assert_ne!(
            repaired.carrier_id,
            original.str("id"),
            "repair = {repaired:?}"
        );
        let copy = crate::jobs::job_row(&conn(&s), repaired.carrier_id.clone());
        for want in [
            original.str("brief"),
            "verify exit 1".into(),
            "run-evidence".into(),
            "exact failing assertion".into(),
            "[redacted]".into(),
            "Never weaken a check".into(),
        ] {
            assert!(
                copy.str("brief").contains(&want),
                "corrected brief lacks {want:?}: {}",
                copy.str("brief")
            );
        }
        assert!(
            !copy
                .str("brief")
                .contains("github_pat_abcdefghijklmnopqrstuvwxyz")
        );
        let historic = crate::jobs::job_row(&conn(&s), original.str("id"));
        for key in ["status", "result", "finished_at", "brief"] {
            assert_eq!(
                historic.str(key),
                original.str(key),
                "repair rewrote historical failure {key}"
            );
        }
        let last = crate::recover::last_recovery_run(&conn(&s), original.str("id"));
        assert_eq!(
            last.str("id"),
            run.str("id"),
            "repair rewrote historical run evidence"
        );
        for e in crate::util::query_rows(
            &conn(&s),
            "SELECT * FROM events WHERE action = ? OR subject = ?",
            &[&pulse_board::JOB_RECOVER_ACTION, &copy.str("id")],
        )
        .expect("events")
        {
            assert_eq!(e.str("actor"), pulse_store::HUMAN_AGENT);
        }
        assert_eq!(copy.str("created_by"), pulse_store::HUMAN_AGENT);
        for status in ["queued", "done"] {
            exec1(
                &s,
                "UPDATE jobs SET status = ?, finished_at = ? WHERE id = ?",
                &[&status, &TEST_NOW, &copy.str("id")],
            );
            let again = repair_one(&s, &original.str("id"));
            assert_eq!(
                again.outcome, "skipped",
                "repeated repair with {status} carrier = {again:?}"
            );
            assert_eq!(
                again.category, "superseded",
                "repeated repair with {status} carrier = {again:?}"
            );
            assert_eq!(
                again.carrier_id,
                copy.str("id"),
                "repeated repair with {status} carrier = {again:?}"
            );
        }
        assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM jobs", &[]), 2);
    }
    #[test]
    fn job_repair_carrier_source_and_lineage() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for source in ["", "suggestion:shared"] {
            for status in [
                "queued",
                "leased",
                "blocked",
                "done",
                "cancelled",
                "active provisional",
            ] {
                let (s, _) = repair_fixture();
                let job = repair_failure(&s, "job-old", "connection reset");
                let carrier = repair_failure(&s, "job-carrier", "verify exit 1");
                exec1(&s, "UPDATE jobs SET source = ?", &[&source]);
                let actual = if status == "active provisional" {
                    insert_recover_run(&s, &carrier.str("id"), "run-active", "running", "", "", "");
                    "failed"
                } else {
                    status
                };
                exec1(
                    &s,
                    "UPDATE jobs SET status = ?, finished_at = ? WHERE id = ?",
                    &[&actual, &TEST_NOW, &carrier.str("id")],
                );
                if source.is_empty() {
                    crate::recover::recovery_event(
                        &conn(&s),
                        crate::util::plus_hours(TEST_NOW, -2.0),
                        "human".into(),
                        job.str("id"),
                        "replanned".into(),
                        crate::failure::Failure {
                            class: crate::failure::FailureClass(
                                crate::failure::FAILURE_DETERMINISTIC.into(),
                            ),
                            ..Default::default()
                        },
                        job.str("id"),
                        1,
                        carrier.str("id"),
                    )
                    .expect("recovery event");
                }
                let got = repair_one(&s, &job.str("id"));
                assert_eq!(got.outcome, "skipped", "{source}/{status}: {got:?}");
                assert_eq!(got.category, "superseded", "{source}/{status}: {got:?}");
                assert_eq!(
                    got.carrier_id,
                    carrier.str("id"),
                    "{source}/{status}: {got:?}"
                );
            }
        }
    }
    #[test]
    fn job_repair_current_failure_and_concurrency() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for mode in ["same failure", "new failure", "completed", "queued"] {
            let (s, _) = repair_fixture();
            let id = "job-race";
            repair_failure(&s, id, "verify exit 1");
            if mode == "new failure" {
                crate::recover::recovery_event(
                    &conn(&s),
                    crate::util::plus_hours(TEST_NOW, -2.0),
                    "daemon".into(),
                    id.into(),
                    "requeued".into(),
                    crate::failure::Failure {
                        class: crate::failure::FailureClass(
                            crate::failure::FAILURE_TRANSIENT.into(),
                        ),
                        ..Default::default()
                    },
                    id.into(),
                    1,
                    String::new(),
                )
                .expect("recovery event");
                exec1(
                    &s,
                    "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'worker', 'job.failed', ?, 'fresh failure')",
                    &[&crate::util::plus_minutes(TEST_NOW, -30.0), &id],
                );
            }
            if mode == "completed" || mode == "queued" {
                let status = if mode == "queued" { "queued" } else { "done" };
                exec1(
                    &s,
                    "UPDATE jobs SET status = ? WHERE id = ?",
                    &[&status, &id],
                );
            }
            let _ = super::repair_failed_jobs(
                &s,
                RepairJobsInput {
                    job_i_ds: vec![id.into()],
                    ..Default::default()
                },
            )
            .expect("repair")
            .expect("repair output");
            let want = if mode == "completed" { 0 } else { 1 };
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'",
                    &[]
                ),
                want,
                "{mode}"
            );
        }
    }
    #[test]
    fn job_repair_queue_bound_and_selection() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, _) = repair_fixture();
        for i in 0..6 {
            repair_failure(&s, &format!("job-{i}"), "verify exit 1");
        }
        let events = count(&s, "SELECT COUNT(*) AS n FROM events", &[]);
        for ids in [
            vec![],
            vec!["job-0", "job-1", "job-2", "job-3", "job-4", "job-5"],
            vec!["job-0", "job-0"],
            vec![""],
            vec![" job-0"],
        ] {
            let got = super::repair_failed_jobs(
                &s,
                RepairJobsInput {
                    job_i_ds: ids.iter().map(|s| s.to_string()).collect(),
                    ..Default::default()
                },
            );
            assert!(got.is_err(), "invalid selection accepted: {ids:?} {got:?}");
        }
        assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM events", &[]), events);
        let out = super::repair_failed_jobs(
            &s,
            RepairJobsInput {
                job_i_ds: ["job-0", "job-1", "job-2", "job-3", "job-4"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                ..Default::default()
            },
        )
        .expect("bounded selection")
        .expect("out");
        assert_eq!(out.queued, 5, "bounded selection: {out:?}");
        assert_eq!(out.errors, 0, "bounded selection: {out:?}");
        let got = repair_one(&s, "job-5");
        assert_eq!(got.outcome, "skipped", "queue overflow: {got:?}");
        assert_eq!(got.category, "queue_full", "queue overflow: {got:?}");
    }
    #[test]
    fn job_repair_concurrent_queue_bound() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, _) = repair_fixture();
        for i in 0..6 {
            let id = format!("job-{i}");
            repair_failure(&s, &id, "verify exit 1");
            if i < 4 {
                exec1(&s, "UPDATE jobs SET status = 'queued' WHERE id = ?", &[&id]);
            }
        }
        for id in ["job-4", "job-5"] {
            let _ = super::repair_failed_jobs(
                &s,
                RepairJobsInput {
                    job_i_ds: vec![id.into()],
                    ..Default::default()
                },
            )
            .expect("repair");
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'",
                &[]
            ),
            crate::recover::RECOVER_QUEUE_ROOM
        );
    }
    #[test]
    fn job_repair_project_scope_and_partial_errors() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, _) = repair_fixture();
        let other = tempfile::tempdir().expect("tempdir").keep();
        exec1(
            &s,
            "INSERT INTO projects(slug,name,path,added_at) VALUES ('other','Other',?,?)",
            &[&other.to_string_lossy().to_string(), &TEST_NOW],
        );
        for id in ["job-good", "job-error", "job-other"] {
            repair_failure(&s, id, "verify exit 1");
        }
        exec1(
            &s,
            "UPDATE jobs SET project = 'other' WHERE id = 'job-other'",
            &[],
        );
        assert!(
            super::repair_failed_jobs(
                &s,
                RepairJobsInput {
                    project: "unknown".into(),
                    job_i_ds: vec!["job-good".into()]
                }
            )
            .is_err()
        );
        assert!(
            super::read_failed_jobs(
                &s,
                FailedJobsQuery {
                    project: "unknown".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        exec1(
            &s,
            "CREATE TRIGGER reject_one_repair BEFORE INSERT ON events
\t\tWHEN NEW.action = 'job.recover' AND NEW.subject = 'job-error'
\t\tBEGIN SELECT RAISE(ABORT, 'injected ledger failure'); END",
            &[],
        );
        let out = super::repair_failed_jobs(
            &s,
            RepairJobsInput {
                project: "proj".into(),
                job_i_ds: ["job-good", "missing", "job-other", "job-error"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            },
        )
        .expect("partial")
        .expect("out");
        assert_eq!(out.queued, 1, "partial result: {out:?}");
        assert_eq!(out.skipped, 2, "partial result: {out:?}");
        assert_eq!(out.errors, 1, "partial result: {out:?}");
        for i in [1_usize, 2] {
            assert_eq!(
                out.results[i].category, "not_found",
                "scope leak: {:?}",
                out.results[i]
            );
            assert!(
                out.results[i].carrier_id.is_empty(),
                "scope leak: {:?}",
                out.results[i]
            );
        }
        assert!(
            out.results[3].reason.contains("injected ledger failure"),
            "failed write claimed success: {:?}",
            out.results[3]
        );
        assert!(
            out.results[3].carrier_id.is_empty(),
            "failed write claimed success: {:?}",
            out.results[3]
        );
        assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM jobs", &[]), 4);
        exec1(&s, "DROP TRIGGER reject_one_repair", &[]);
        assert_eq!(repair_one(&s, "job-error").outcome, "replanned");
    }
    #[test]
    fn job_repair_all_history_count_and_pagination() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, _) = repair_fixture();
        for i in 0..1644 {
            exec1(
                &s,
                "INSERT INTO jobs(id,project,kind,tier,title,status,created_at,finished_at,result)
\t\t\t\tVALUES (?,'proj','improve',1,'Historical failure','failed',?,?,'verify exit 1')",
                &[
                    &format!("job-{i:04}"),
                    &crate::util::plus_hours(TEST_NOW, -24.0 * 31.0),
                    &crate::util::plus_hours(TEST_NOW, -24.0 * 30.0),
                ],
            );
        }
        let events = count(&s, "SELECT COUNT(*) AS n FROM events", &[]);
        let first = super::read_failed_jobs(
            &s,
            FailedJobsQuery {
                project: "proj".into(),
                limit: 7,
                ..Default::default()
            },
        )
        .expect("first")
        .expect("page");
        assert_eq!(first.historical_total, 1644, "first page: {first:?}");
        assert_eq!(first.jobs.len(), 7, "first page: {first:?}");
        assert!(!first.next_before.is_empty(), "first page: {first:?}");
        assert_eq!(
            first.page_counts.get("outside_window").copied(),
            Some(7),
            "first page: {first:?}"
        );
        let next = super::read_failed_jobs(
            &s,
            FailedJobsQuery {
                project: "proj".into(),
                limit: 7,
                before: first.next_before.clone(),
            },
        )
        .expect("next")
        .expect("page");
        assert_eq!(next.historical_total, 1644, "next page: {next:?}");
        assert_eq!(next.jobs.len(), 7, "next page: {next:?}");
        assert_ne!(next.jobs[0].id, first.jobs[0].id, "next page: {next:?}");
        assert!(
            first.jobs[6].id > next.jobs[0].id,
            "tied timestamps were not paginated by ID"
        );
        for query in [
            FailedJobsQuery {
                limit: -1,
                ..Default::default()
            },
            FailedJobsQuery {
                limit: FAILED_JOBS_MAX_LIMIT + 1,
                ..Default::default()
            },
            FailedJobsQuery {
                before: "not-a-cursor".into(),
                ..Default::default()
            },
        ] {
            assert!(
                super::read_failed_jobs(&s, query).is_err(),
                "unbounded or invalid page accepted"
            );
        }
        assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM events", &[]), events);
    }
    #[test]
    fn job_repair_rechecks_displayed_failure() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for change in [
            "owner cancellation",
            "structural result",
            "active run",
            "STOP",
            "inactive project",
        ] {
            let (s, _) = repair_fixture();
            repair_failure(&s, "job-changed", "verify exit 1");
            let page = super::read_failed_jobs(
                &s,
                FailedJobsQuery {
                    project: "proj".into(),
                    ..Default::default()
                },
            )
            .expect("initial")
            .expect("page");
            assert_eq!(page.jobs.len(), 1, "{change}: {page:?}");
            assert!(page.jobs[0].eligible, "{change}: {page:?}");
            match change {
                "owner cancellation" => exec1(
                    &s,
                    "UPDATE jobs SET result = 'cancelled by the owner' WHERE id = 'job-changed'",
                    &[],
                ),
                "structural result" => exec1(
                    &s,
                    "UPDATE jobs SET result = 'verify exit 127' WHERE id = 'job-changed'",
                    &[],
                ),
                "active run" => {
                    insert_recover_run(&s, "job-changed", "run-changed", "running", "", "", "");
                }
                "STOP" => set_recover_setting(&s, "stop", "1"),
                "inactive project" => exec1(
                    &s,
                    "UPDATE projects SET status = 'inactive' WHERE slug = 'proj'",
                    &[],
                ),
                _ => unreachable!(),
            }
            assert_eq!(
                repair_one(&s, "job-changed").outcome,
                "skipped",
                "{change}: stale eligibility used"
            );
            assert_eq!(
                count(&s, "SELECT COUNT(*) AS n FROM jobs", &[]),
                1,
                "{change}: changed failure was copied"
            );
        }
    }
    #[test]
    fn job_repair_source_scope_and_task_carrier() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, _) = repair_fixture();
        let other = tempfile::tempdir().expect("tempdir").keep();
        exec1(
            &s,
            "INSERT INTO projects(slug,name,path,added_at) VALUES ('other','Other',?,?)",
            &[&other.to_string_lossy().to_string(), &TEST_NOW],
        );
        repair_failure(&s, "job-source", "verify exit 1");
        repair_failure(&s, "job-other", "verify exit 1");
        exec1(&s, "UPDATE jobs SET source = 'shared-source'", &[]);
        exec1(
            &s,
            "UPDATE jobs SET project = 'other', status = 'queued' WHERE id = 'job-other'",
            &[],
        );
        let repaired = repair_one(&s, "job-source");
        assert_eq!(
            repaired.outcome, "replanned",
            "cross-project source collision: {repaired:?}"
        );
        assert_eq!(
            crate::jobs::job_row(&conn(&s), repaired.carrier_id).str("project"),
            "proj"
        );
        repair_failure(&s, "job-task-old", "connection reset");
        repair_failure(&s, "job-task-current", "verify exit 1");
        exec1(
            &s,
            "UPDATE jobs SET source = 'task:TASK-1' WHERE id = 'job-task-old'",
            &[],
        );
        exec1(
            &s,
            "INSERT INTO tasks(id,project,title,status,job_id,created_by,created_at,updated_at)
\t\tVALUES ('TASK-1','proj','Task','in_progress','job-task-current','human',?,?)",
            &[&TEST_NOW, &TEST_NOW],
        );
        let got = repair_one(&s, "job-task-old");
        assert_eq!(
            got.outcome, "skipped",
            "current task carrier missing: {got:?}"
        );
        assert_eq!(
            got.carrier_id, "job-task-current",
            "current task carrier missing: {got:?}"
        );
        assert_eq!(
            got.carrier_status, "failed",
            "current task carrier missing: {got:?}"
        );
    }
    #[test]
    fn job_repair_invalid_gates_fail_closed() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for gate in [
            "stop",
            "daemon.recover",
            "daemon.recover_days",
            "daemon.recover_limit",
            "checkout path",
        ] {
            let (s, _) = repair_fixture();
            repair_failure(&s, "job-invalid-gate", "verify exit 1");
            if gate == "checkout path" {
                exec1(&s, "UPDATE projects SET path = '' WHERE slug = 'proj'", &[]);
            } else {
                exec1(
                    &s,
                    "INSERT OR REPLACE INTO settings(name,value,updated_at,updated_by) VALUES (?,'invalid',?,'human')",
                    &[&gate, &TEST_NOW],
                );
            }
            assert!(
                super::read_failed_jobs(
                    &s,
                    FailedJobsQuery {
                        project: "proj".into(),
                        ..Default::default()
                    }
                )
                .is_err(),
                "{gate}: invalid gate ignored"
            );
            let got = repair_one(&s, "job-invalid-gate");
            assert_eq!(
                got.outcome, "error",
                "{gate}: invalid gate queued work: {got:?}"
            );
            assert!(
                !got.reason.is_empty(),
                "{gate}: invalid gate error lacked reason"
            );
            assert_eq!(
                count(&s, "SELECT COUNT(*) AS n FROM jobs", &[]),
                1,
                "{gate}: invalid gate mutated jobs"
            );
        }
    }
    #[test]
    fn job_repair_cancelled_ancestor_intent() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for source in ["", "manual:cancelled-chain"] {
            for mode in [
                "earlier cancellation",
                "earlier same second",
                "newer same second",
                "newer before failure",
                "newer after failure",
                "API is not an override",
                "agent is not an override",
            ] {
                let (s, _) = repair_fixture();
                let root = repair_failure(&s, "job-cancelled-root", "verify exit 1");
                exec1(
                    &s,
                    "UPDATE jobs SET source = ? WHERE id = ?",
                    &[&source, &root.str("id")],
                );
                crate::jobs::finish_job(
                    &s,
                    root.str("id"),
                    pulse_store::HUMAN_AGENT.into(),
                    "cancelled".into(),
                    "cancelled by the owner".into(),
                    String::new(),
                    -1,
                )
                .expect("finish root cancelled");
                let (intent_hour, failure_hour) = if mode == "earlier same second" {
                    (0.0, 0.0)
                } else {
                    (1.0, 2.0)
                };
                let child_id = if mode == "API is not an override" {
                    let later = repair_store_at(&s, intent_hour);
                    let (copy, _) = crate::jobs::add_job(
                        &later,
                        "proj".into(),
                        "improve".into(),
                        "Corrected attempt".into(),
                        root.str("brief"),
                        1,
                        0,
                        source.into(),
                        pulse_store::HUMAN_AGENT.into(),
                        false,
                    )
                    .expect("add copy");
                    crate::recover::recovery_event(
                        &conn(&s),
                        later.now(),
                        pulse_store::HUMAN_AGENT.into(),
                        root.str("id"),
                        "replanned".into(),
                        crate::failure::Failure {
                            class: crate::failure::FailureClass(
                                crate::failure::FAILURE_DETERMINISTIC.into(),
                            ),
                            reason: "verify exit 1".into(),
                            ..Default::default()
                        },
                        root.str("id"),
                        1,
                        copy.str("id"),
                    )
                    .expect("recovery event");
                    copy.str("id")
                } else {
                    let later = repair_store_at(&s, intent_hour);
                    let actor = if mode == "agent is not an override" {
                        "worker"
                    } else {
                        pulse_store::HUMAN_AGENT
                    };
                    pulse_board::retry_job(
                        &later,
                        pulse_board::RetryInput {
                            job_id: root.str("id"),
                            as_new: true,
                            actor: actor.into(),
                            ..Default::default()
                        },
                    )
                    .expect("retry root")
                    .job
                    .id
                };
                match mode {
                    "newer same second" | "newer before failure" | "newer after failure" => {
                        let at = if mode == "newer same second" {
                            intent_hour
                        } else if mode == "newer after failure" {
                            2.5
                        } else {
                            1.5
                        };
                        finish_at(
                            &s,
                            at,
                            &root.str("id"),
                            "cancelled",
                            "cancelled by the owner again",
                        );
                    }
                    _ => {}
                }
                finish_at(&s, failure_hour, &child_id, "failed", "verify exit 1");
                let before = crate::jobs::job_row(&conn(&s), root.str("id"));
                let later = repair_store_at(&s, 3.0);
                let page = super::read_failed_jobs(
                    &later,
                    FailedJobsQuery {
                        project: "proj".into(),
                        ..Default::default()
                    },
                )
                .expect("child view")
                .expect("page");
                assert_eq!(page.jobs.len(), 1, "{source}/{mode}: child view = {page:?}");
                let authorized = mode.starts_with("earlier");
                assert_eq!(
                    page.jobs[0].eligible, authorized,
                    "{source}/{mode}: cancellation chronology was lost: {:?}",
                    page.jobs[0]
                );
                let root_repair = repair_one(&later, &root.str("id"));
                assert_eq!(
                    root_repair.outcome, "skipped",
                    "{source}/{mode}: API undid root cancellation: {root_repair:?}"
                );
                assert_eq!(
                    root_repair.category, "cancelled",
                    "{source}/{mode}: API undid root cancellation: {root_repair:?}"
                );
                let got = repair_one(&later, &child_id);
                if authorized {
                    assert_eq!(
                        got.outcome, "replanned",
                        "{source}/{mode}: authorized child remained blocked: {got:?}"
                    );
                } else {
                    assert_eq!(
                        got.outcome, "skipped",
                        "{source}/{mode}: cancellation was overridden: {got:?}"
                    );
                    assert_eq!(
                        got.category, "superseded",
                        "{source}/{mode}: cancellation was overridden: {got:?}"
                    );
                    assert_eq!(
                        got.carrier_id,
                        root.str("id"),
                        "{source}/{mode}: cancellation was overridden: {got:?}"
                    );
                }
                let root_after = crate::jobs::job_row(&conn(&s), root.str("id"));
                for key in ["status", "result", "finished_at", "source"] {
                    assert_eq!(
                        before.str(key),
                        root_after.str(key),
                        "{source}/{mode}: cancelled ancestor history changed {key}"
                    );
                }
            }
        }
    }
    #[test]
    fn job_repair_authorized_chain_still_dedupes_carriers() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for source in ["", "manual:cancelled-chain"] {
            for status in [
                "queued",
                "leased",
                "blocked",
                "done",
                "cancelled",
                "active provisional",
            ] {
                let (s, _) = repair_fixture();
                let root = repair_failure(&s, "job-cancelled-root", "verify exit 1");
                exec1(
                    &s,
                    "UPDATE jobs SET source = ? WHERE id = ?",
                    &[&source, &root.str("id")],
                );
                crate::jobs::finish_job(
                    &s,
                    root.str("id"),
                    pulse_store::HUMAN_AGENT.into(),
                    "cancelled".into(),
                    "cancelled by the owner".into(),
                    String::new(),
                    -1,
                )
                .expect("cancel root");
                let child = pulse_board::retry_job(
                    &repair_store_at(&s, 1.0),
                    pulse_board::RetryInput {
                        job_id: root.str("id"),
                        as_new: true,
                        actor: pulse_store::HUMAN_AGENT.into(),
                        ..Default::default()
                    },
                )
                .expect("retry")
                .job
                .id;
                finish_at(&s, 2.0, &child, "failed", "verify exit 1");
                let repaired = repair_one(&repair_store_at(&s, 3.0), &child);
                assert_eq!(
                    repaired.outcome, "replanned",
                    "{source}/{status}: initial recovery = {repaired:?}"
                );
                let actual = if status == "active provisional" {
                    insert_recover_run(
                        &s,
                        &repaired.carrier_id,
                        "run-current",
                        "running",
                        "",
                        "",
                        "",
                    );
                    "failed"
                } else {
                    status
                };
                exec1(
                    &s,
                    "UPDATE jobs SET status = ?, finished_at = ? WHERE id = ?",
                    &[
                        &actual,
                        &crate::util::plus_hours(TEST_NOW, 4.0),
                        &repaired.carrier_id,
                    ],
                );
                let events = count(&s, "SELECT COUNT(*) AS n FROM events", &[]);
                let got = repair_one(&repair_store_at(&s, 5.0), &child);
                assert_eq!(
                    got.outcome, "skipped",
                    "{source}/{status}: current carrier ignored: {got:?}"
                );
                assert_eq!(
                    got.category, "superseded",
                    "{source}/{status}: current carrier ignored: {got:?}"
                );
                assert_eq!(
                    got.carrier_id, repaired.carrier_id,
                    "{source}/{status}: current carrier ignored: {got:?}"
                );
                assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM jobs", &[]), 3);
                assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM events", &[]), events);
            }
        }
    }
    #[test]
    fn job_repair_cancellation_override_does_not_authorize_sibling() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for source in ["", "manual:cancelled-branches"] {
            let (s, _) = repair_fixture();
            let root = repair_failure(&s, "job-root", "verify exit 1");
            exec1(
                &s,
                "UPDATE jobs SET source = ? WHERE id = ?",
                &[&source, &root.str("id")],
            );
            let old_child = pulse_board::retry_job(
                &s,
                pulse_board::RetryInput {
                    job_id: root.str("id"),
                    as_new: true,
                    actor: pulse_store::HUMAN_AGENT.into(),
                    ..Default::default()
                },
            )
            .expect("old retry")
            .job
            .id;
            finish_at(&s, 0.5, &old_child, "failed", "verify exit 1");
            finish_at(
                &s,
                1.0,
                &root.str("id"),
                "cancelled",
                "cancelled by the owner",
            );
            let new_child = pulse_board::retry_job(
                &repair_store_at(&s, 2.0),
                pulse_board::RetryInput {
                    job_id: root.str("id"),
                    as_new: true,
                    actor: pulse_store::HUMAN_AGENT.into(),
                    ..Default::default()
                },
            )
            .expect("new retry")
            .job
            .id;
            finish_at(&s, 3.0, &new_child, "failed", "verify exit 1");
            let later = repair_store_at(&s, 4.0);
            let old = repair_one(&later, &old_child);
            assert_eq!(
                old.outcome, "skipped",
                "{source}: new retry authorized cancelled sibling: {old:?}"
            );
            assert_eq!(
                old.carrier_id,
                root.str("id"),
                "{source}: new retry authorized cancelled sibling: {old:?}"
            );
            let new = repair_one(&later, &new_child);
            assert_eq!(
                new.outcome, "replanned",
                "{source}: explicitly authorized branch stayed held: {new:?}"
            );
        }
    }
    #[test]
    fn recover_cancelled_ancestor_after_human_retry() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for source in ["", "manual:cancelled-chain"] {
            let (s, _) = repair_fixture();
            let root = repair_failure(&s, "job-root", "verify exit 1");
            exec1(
                &s,
                "UPDATE jobs SET source = ? WHERE id = ?",
                &[&source, &root.str("id")],
            );
            crate::jobs::finish_job(
                &s,
                root.str("id"),
                pulse_store::HUMAN_AGENT.into(),
                "cancelled".into(),
                "cancelled by the owner".into(),
                String::new(),
                -1,
            )
            .expect("cancel root");
            let child = pulse_board::retry_job(
                &repair_store_at(&s, 1.0),
                pulse_board::RetryInput {
                    job_id: root.str("id"),
                    as_new: true,
                    actor: pulse_store::HUMAN_AGENT.into(),
                    ..Default::default()
                },
            )
            .expect("retry")
            .job
            .id;
            finish_at(&s, 2.0, &child, "failed", "verify exit 1");
            let later = repair_store_at(&s, 3.0);
            let project = crate::board::project_by_slug(&conn(&s), "proj".into());
            let lines =
                crate::recover::recover_failed_jobs(&later, project, later.now(), "daemon".into());
            assert!(
                lines.len() == 1 && lines[0].contains(&format!("re-planned {child}")),
                "{source}: daemon rejected authorized child: {lines:?}"
            );
            let carrier = recover_detail(&s, &child)
                .get("job")
                .and_then(|v| v.as_str())
                .expect("carrier job")
                .to_string();
            finish_at(
                &s,
                4.0,
                &root.str("id"),
                "cancelled",
                "cancelled by the owner again",
            );
            finish_at(&s, 5.0, &carrier, "failed", "verify exit 1");
            let later = repair_store_at(&s, 6.0);
            let project = crate::board::project_by_slug(&conn(&s), "proj".into());
            crate::recover::recover_failed_jobs(&later, project, later.now(), "daemon".into());
            let detail = recover_detail(&s, &carrier);
            assert_eq!(
                detail.get("outcome").and_then(|v| v.as_str()),
                Some("held"),
                "{source}: {detail:?}"
            );
            assert_eq!(
                detail.get("job").and_then(|v| v.as_str()),
                Some(root.str("id").as_str()),
                "{source}: {detail:?}"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'",
                    &[]
                ),
                0
            );
        }
    }
}
