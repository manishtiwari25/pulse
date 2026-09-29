use crate::board::*;
use anyhow::Result;
use rand::Rng;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::json;

pub(crate) const JOB_COLS: &str = "id, project, kind, tier, title, brief, source, priority, status, created_at, created_by, lease_agent, lease_until, generation, attempts, finished_at, result, branch";

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub project: String,
    pub kind: String,
    pub tier: i64,
    pub title: String,
    pub brief: String,
    pub source: String,
    pub priority: i64,
    pub status: String,
    pub created_at: String,
    pub created_by: String,
    pub lease_agent: String,
    pub lease_until: String,
    pub generation: i64,
    pub attempts: i64,
    pub finished_at: String,
    pub result: String,
    pub branch: String,
}

pub(crate) fn scan_job(row: &Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        project: row.get(1)?,
        kind: row.get(2)?,
        tier: row.get(3)?,
        title: row.get(4)?,
        brief: row.get(5)?,
        source: row.get(6)?,
        priority: row.get(7)?,
        status: row.get(8)?,
        created_at: row.get(9)?,
        created_by: row.get(10)?,
        lease_agent: row.get(11)?,
        lease_until: row.get(12)?,
        generation: row.get(13)?,
        attempts: row.get(14)?,
        finished_at: row.get(15)?,
        result: row.get(16)?,
        branch: row.get(17)?,
    })
}

pub fn job_by_id(s: &pulse_store::Store, id: &str) -> Result<Option<Job>> {
    let conn = s.conn()?;
    job_by_id_conn(&conn, id)
}
pub use job_by_id as JobByID;

pub(crate) fn job_by_id_conn(conn: &Connection, id: &str) -> Result<Option<Job>> {
    Ok(conn
        .query_row(
            &format!("SELECT {JOB_COLS} FROM jobs WHERE id = ?"),
            [id],
            scan_job,
        )
        .optional()?)
}

pub fn new_id(s: &pulse_store::Store, prefix: &str) -> String {
    let mut b = [0u8; 3];
    rand::rng().fill_bytes(&mut b);
    format!("{prefix}-{}-{}", stamp_of(&s.now()), hex::encode(b))
}
pub use new_id as NewID;

#[derive(Debug, Clone, Default)]
pub struct JobInput {
    pub project: String,
    pub kind: String,
    pub title: String,
    pub brief: String,
    pub tier: i64,
    pub priority: i64,
    pub source: String,
    pub created_by: String,
    pub branch: String,
    pub no_dedupe: bool,
}

pub fn add_job(s: &pulse_store::Store, input: JobInput) -> Result<(Job, bool)> {
    let conn = s.conn()?;
    add_job_conn(s, &conn, input)
}
pub use add_job as AddJob;

pub(crate) fn add_job_conn(
    s: &pulse_store::Store,
    conn: &Connection,
    input: JobInput,
) -> Result<(Job, bool)> {
    if !contains(JOB_KINDS, &input.kind) {
        return Err(refuse(
            2,
            format!("Job kind must be one of: {}", JOB_KINDS.join(", ")),
        ));
    }
    if s.project(&input.project)?.is_none() {
        return Err(refuse(2, format!("No project {}", py_repr(&input.project))));
    }
    if input.tier != 0 && input.tier != 1 {
        return Err(refuse(
            2,
            "The runner takes Tier 0 and Tier 1 jobs only; Tier 2 needs an ADR and the human",
        ));
    }
    if !input.no_dedupe && !input.source.is_empty() {
        let id: Option<String> = conn
            .query_row("SELECT id FROM jobs WHERE source = ? AND status IN ('queued', 'leased', 'blocked')", [&input.source], |r| r.get(0))
            .optional()?;
        if let Some(id) = id {
            return Ok((job_by_id_conn(conn, &id)?.expect("dedupe job"), false));
        }
    }
    let id = new_id(s, "job");
    conn.execute(
        "INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by, branch) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?)",
        params![id, input.project, input.kind, input.tier, input.title, input.brief, input.source, input.priority, s.now(), input.created_by, input.branch],
    )?;
    event(
        s,
        &input.created_by,
        "job.add",
        &id,
        pulse_store::Detail::json(
            json!({"project": input.project, "kind": input.kind, "title": input.title}),
        )?,
    )?;
    Ok((job_by_id_conn(conn, &id)?.expect("inserted job"), true))
}

pub fn finish_job(
    s: &pulse_store::Store,
    job_id: &str,
    agent: &str,
    status: &str,
    result: &str,
    branch: &str,
    generation: Option<i64>,
) -> Result<Job> {
    let conn = s.conn()?;
    finish_job_conn(
        s, &conn, job_id, agent, status, result, branch, generation, false,
    )
}
pub use finish_job as FinishJob;

#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_job_conn(
    s: &pulse_store::Store,
    conn: &Connection,
    job_id: &str,
    agent: &str,
    status: &str,
    result: &str,
    branch: &str,
    generation: Option<i64>,
    replace_branch: bool,
) -> Result<Job> {
    if !contains(JOB_END_STATES, status) {
        return Err(refuse(
            2,
            "A job ends done, failed, cancelled, blocked, or back to queued",
        ));
    }
    let job = job_by_id_conn(conn, job_id)?
        .ok_or_else(|| refuse(2, format!("No job {}", py_repr(job_id))))?;
    if job.status == "leased"
        && !job.lease_agent.is_empty()
        && !agent.is_empty()
        && job.lease_agent != agent
        && agent != pulse_store::HUMAN_AGENT
    {
        return Err(refuse(
            2,
            format!("Job {job_id} is leased by {}, not {agent}", job.lease_agent),
        ));
    }
    if let Some(g) = generation
        && job.generation != g
    {
        return Err(refuse(
            2,
            format!(
                "Stale lease: job {job_id} is on generation {}, this result is from generation {g}",
                job.generation
            ),
        ));
    }
    let finished = if status == "queued" {
        String::new()
    } else {
        s.now()
    };
    if replace_branch {
        conn.execute("UPDATE jobs SET status = ?, result = ?, branch = ?, finished_at = ?, lease_agent = CASE WHEN ? = 'queued' THEN '' ELSE lease_agent END, lease_until = '' WHERE id = ?",
            params![status, result, branch, finished, status, job_id])?;
    } else {
        conn.execute("UPDATE jobs SET status = ?, result = ?, branch = COALESCE(NULLIF(?, ''), branch), finished_at = ?, lease_agent = CASE WHEN ? = 'queued' THEN '' ELSE lease_agent END, lease_until = '' WHERE id = ?",
            params![status, result, branch, finished, status, job_id])?;
    }
    event(s, agent, &format!("job.{status}"), job_id, trunc400(result))?;
    Ok(job_by_id_conn(conn, job_id)?.expect("updated job"))
}

#[derive(Debug, Clone, Default)]
pub struct RunJobResult {
    pub run_id: String,
    pub job_id: String,
    pub agent: String,
    pub status: String,
    pub result: String,
    pub branch: String,
    pub generation: i64,
}

pub fn finish_run_job(s: &pulse_store::Store, input: RunJobResult) -> Result<Job> {
    let tx = s.begin()?;
    let status: Option<String> = tx
        .query_row(
            r#"SELECT j.status FROM jobs j JOIN runs r ON r.job_id = j.id
		WHERE r.id = ? AND j.id = ? AND r.agent = ? AND r.project = j.project AND r.status = 'running'
		AND j.lease_agent = ? AND j.generation = ?"#,
            params![
                input.run_id,
                input.job_id,
                input.agent,
                input.agent,
                input.generation
            ],
            |r| r.get(0),
        )
        .optional()?;
    let Some(prev) = status else {
        return Err(refuse(
            2,
            format!(
                "Stale run: job {} is no longer owned by run {}",
                input.job_id, input.run_id
            ),
        ));
    };
    if input.status == "done" && (prev == "cancelled" || prev == "failed") {
        return Err(refuse(
            2,
            format!(
                "Job {} was {prev} before the controller finished",
                input.job_id
            ),
        ));
    }
    let job = finish_job_conn(
        s,
        &tx,
        &input.job_id,
        &input.agent,
        &input.status,
        &input.result,
        &input.branch,
        Some(input.generation),
        true,
    )?;
    tx.commit()?;
    Ok(job)
}
pub use finish_run_job as FinishRunJob;

#[derive(Debug, Clone, Default)]
pub struct RetryInput {
    pub job_id: String,
    pub as_new: bool,
    pub brief: String,
    pub actor: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetryResult {
    pub job: Job,
    pub outcome: String,
    pub root: String,
    #[serde(skip)]
    pub reused: bool,
    #[serde(skip)]
    pub n: i64,
}

pub fn retry_job(s: &pulse_store::Store, input: RetryInput) -> Result<RetryResult> {
    let actor = if input.actor.is_empty() {
        pulse_store::HUMAN_AGENT.to_string()
    } else {
        input.actor.clone()
    };
    let conn = s.conn()?;
    let job = job_by_id_conn(&conn, &input.job_id)?
        .ok_or_else(|| refuse(2, format!("No job {}", py_repr(&input.job_id))))?;
    match job.status.as_str() {
        "failed" | "cancelled" => {}
        "queued" | "leased" => {
            return Err(refuse(
                2,
                format!("Job {} is {}; it is already running", job.id, job.status),
            ));
        }
        "done" => {
            return Err(refuse(
                2,
                format!("Job {} is done; it has nothing to retry", job.id),
            ));
        }
        _ => {
            return Err(refuse(
                2,
                format!(
                    "Job {} is {}; only failed or cancelled jobs can be retried",
                    job.id, job.status
                ),
            ));
        }
    }
    let active: i64 = conn.query_row(
        "SELECT COUNT(*) FROM runs WHERE job_id = ? AND status = 'running'",
        [&job.id],
        |r| r.get(0),
    )?;
    if active != 0 {
        return Err(refuse(
            2,
            format!(
                "Job {} still has an active run; its result is not final",
                job.id
            ),
        ));
    }
    if input.as_new {
        let brief = format!(
            "{}\n\n## Earlier attempt (ADR-036)\n\nJob `{}` failed at {}: {}.\n\n{}Start from the cause, fix it, then deliver the original task above.",
            job.brief,
            job.id,
            job.finished_at,
            job.result,
            if input.brief.is_empty() {
                String::new()
            } else {
                input.brief.clone() + "\n\n"
            }
        );
        let (copy, _) = add_job_conn(
            s,
            &conn,
            JobInput {
                project: job.project.clone(),
                kind: job.kind.clone(),
                tier: job.tier,
                priority: job.priority,
                title: retry_title(&job.id, &job.title),
                brief,
                source: job.source.clone(),
                created_by: actor.clone(),
                no_dedupe: true,
                ..Default::default()
            },
        )?;
        event(
            s,
            &actor,
            JOB_RECOVER_ACTION,
            &job.id,
            pulse_store::Detail::json(
                json!({"class":"human","n":1,"outcome":"replanned","root":job.id,"signature":"","why":format!("pulse job retry by {actor}"),"job":copy.id}),
            )?,
        )?;
        Ok(RetryResult {
            job: copy,
            outcome: "replanned".into(),
            root: job.id,
            reused: false,
            n: 1,
        })
    } else {
        let result = format!("retried by {actor}: {}", trunc(&job.result, 200));
        let append = retry_note(&s.now(), &actor, &input.brief);
        conn.execute("UPDATE jobs SET status = 'queued', result = ?, branch = '', attempts = 0, lease_agent = '', lease_until = '', finished_at = '', brief = brief || ? WHERE id = ?",
            params![result, append, job.id])?;
        event(s, &actor, "job.queued", &job.id, trunc400(&result))?;
        event(
            s,
            &actor,
            JOB_RECOVER_ACTION,
            &job.id,
            pulse_store::Detail::json(
                json!({"class":"human","n":1,"outcome":"retried","root":job.id,"signature":"","why":format!("pulse job retry by {actor}")}),
            )?,
        )?;
        Ok(RetryResult {
            job: job_by_id_conn(&conn, &job.id)?.expect("retried job"),
            outcome: "retried".into(),
            root: job.id,
            reused: false,
            n: 1,
        })
    }
}
pub use retry_job as RetryJob;

pub fn recovery_carrier(_conn: &Connection, _job: &Job, _root: &str) -> Result<String> {
    Ok(String::new())
}
pub use recovery_carrier as RecoveryCarrier;

fn retry_note(now: &str, actor: &str, brief: &str) -> String {
    if brief.is_empty() {
        String::new()
    } else {
        format!("\n\n## Retry note ({actor}, {now})\n\n{brief}")
    }
}
fn retry_title(root: &str, title: &str) -> String {
    let prefix = format!("Retry {root}: ");
    trunc(
        &(prefix.clone() + title.strip_prefix(&prefix).unwrap_or(title)),
        200,
    )
}
pub(crate) fn trunc400(text: &str) -> String {
    text.chars().take(400).collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub post_id: String,
    pub project: String,
    pub answer: String,
    pub words: String,
    pub decided_at: String,
    pub decided_by: String,
    pub result: crate::posts::Post,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<Job>,
}

pub fn lead_name(slug: &str) -> String {
    format!("{slug}-lead")
}
pub use lead_name as LeadName;
pub const DECISION_VALID_DAYS: i64 = 7;

pub fn tier2_proposal(summary: &str, body: &str) -> String {
    static ADR: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if let Some(m) = ADR
        .get_or_init(|| regex::Regex::new(r"\bADR-\d+\b").unwrap())
        .find(summary)
    {
        return format!("names {}", m.as_str());
    }
    let text = format!("{summary}\n{body}");
    for p in pulse_store::OWNER_PATHS {
        if text.contains(p) {
            return format!("points at {p}");
        }
    }
    String::new()
}
pub use tier2_proposal as Tier2Proposal;

pub fn decide(
    _s: &pulse_store::Store,
    _post_id: &str,
    _answer: &str,
    _words: &str,
    _decided_by: &str,
) -> Result<Decision> {
    Err(refuse(
        2,
        "decide is not yet available in this Rust board build",
    ))
}
pub use decide as Decide;
pub fn decision_job(
    _s: &pulse_store::Store,
    _target: &crate::posts::Post,
    _result: &crate::posts::Post,
    _words: &str,
    _decided_by: &str,
) -> Result<Option<Job>> {
    Ok(None)
}
pub use decision_job as DecisionJob;

#[derive(Debug, Clone, Serialize)]
pub struct Authorization {
    pub post_id: String,
    pub answer: String,
    pub decided_at: String,
    pub valid: bool,
}
pub fn authorizing_decision(
    _s: &pulse_store::Store,
    _marker: &str,
    _now: &str,
    _days: i64,
) -> Result<Option<Authorization>> {
    Ok(None)
}
pub use authorizing_decision as AuthorizingDecision;
pub fn agent_gate(
    _s: &pulse_store::Store,
    _marker: &str,
    actor: &str,
    _now: &str,
) -> Result<String> {
    if actor.is_empty() || actor == pulse_store::HUMAN_AGENT {
        Ok(String::new())
    } else {
        Err(refuse(
            2,
            "An agent may run this only after the human's `pulse decide ... yes` on a board proposal whose body carries the line; no decided proposal names it",
        ))
    }
}
pub use agent_gate as AgentGate;

pub fn onboarding_job(s: &pulse_store::Store, slug: &str) -> Result<Option<Job>> {
    let conn = s.conn()?;
    let id: Option<String> = conn.query_row("SELECT id FROM jobs WHERE source = ? AND status IN ('queued', 'leased', 'blocked', 'done') ORDER BY created_at DESC, id DESC LIMIT 1", [format!("onboard:{slug}")], |r| r.get(0)).optional()?;
    id.map_or(Ok(None), |id| job_by_id_conn(&conn, &id))
}
pub use onboarding_job as OnboardingJob;

pub fn queue_onboarding_job(
    s: &pulse_store::Store,
    slug: &str,
    created_by: &str,
    has_recipe: bool,
) -> Result<(Job, bool)> {
    let created_by = if created_by.is_empty() {
        "init"
    } else {
        created_by
    };
    let recipe = if has_recipe {
        format!(
            "The environment recipe is already in the store (`pulse env show --project {slug}`); check it against what you observed and amend it with `pulse env set` when a pin or a setup line is wrong. Never write a token, password, or key into it."
        )
    } else {
        format!(
            "Then fill the environment recipe (ADR-026) from what you observed: the exact tool pins a fresh machine needs (`name@version`), the setup lines that make a clean clone runnable, the plain variables the check needs (names and values that are not secrets), and any running service the project cannot do without. Write it with `pulse env set --project {slug} --file recipe.md`; it lives in the store as record CTX-environment, not in the checkout. Never write a token, password, or key into it."
        )
    };
    add_job(
        s,
        JobInput {
            project: slug.into(),
            kind: "onboard".into(),
            title: format!(
                "Onboard {slug}: read the guide, run the checks, post the first findings"
            ),
            brief: format!(
                "Read `pulse guide`, `pulse board --project {slug}`, the project's records (`pulse record list`), and open decisions. Run its checks. Write FINDING: lines for what the next agent would rediscover and SUMMARY: with the project's state. Change nothing unless a check proves a Tier 0 fix. {recipe}"
            ),
            tier: 0,
            priority: 20,
            source: format!("onboard:{slug}"),
            created_by: created_by.into(),
            ..Default::default()
        },
    )
}
pub use queue_onboarding_job as QueueOnboardingJob;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_title_once() {
        assert_eq!(retry_title("job-1", "Retry job-1: x"), "Retry job-1: x");
    }
}
