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
    event_conn(
        conn,
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
    event_conn(
        conn,
        s,
        agent,
        &format!("job.{status}"),
        job_id,
        trunc400(result),
    )?;
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
    let input = RetryInput {
        actor: if input.actor.is_empty() {
            pulse_store::HUMAN_AGENT.to_string()
        } else {
            input.actor
        },
        ..input
    };
    let tx = s.begin()?;
    let out = retry_job_conn(s, &tx, input)?;
    tx.commit()?;
    Ok(out)
}

fn retry_job_conn(
    s: &pulse_store::Store,
    conn: &Connection,
    input: RetryInput,
) -> Result<RetryResult> {
    let job = job_by_id_conn(conn, &input.job_id)?
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
    let root = recover_root(conn, &job)?;
    let mut n = recover_n(conn, &root)?;
    let carrier = recovery_carrier(conn, &job, &root)?;
    if !carrier.is_empty() {
        let current = job_by_id_conn(conn, &carrier)?.expect("recovery carrier job");
        if current.status != "queued" && current.status != "leased" && current.status != "blocked" {
            return Err(refuse(
                2,
                format!(
                    "Job {} is superseded by {} ({}); it must not be retried",
                    job.id, carrier, current.status
                ),
            ));
        }
        return Ok(RetryResult {
            job: current,
            outcome: "replanned".into(),
            root,
            reused: true,
            n,
        });
    }
    n += 1;
    if input.as_new {
        return retry_as_new(s, conn, &job, &root, n, input);
    }
    retry_in_place(s, conn, &job, &root, n, input)
}

fn retry_as_new(
    s: &pulse_store::Store,
    conn: &Connection,
    job: &Job,
    root: &str,
    n: i64,
    input: RetryInput,
) -> Result<RetryResult> {
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
        conn,
        JobInput {
            project: job.project.clone(),
            kind: job.kind.clone(),
            tier: job.tier,
            priority: job.priority,
            title: retry_title(&job.id, &job.title),
            brief,
            source: job.source.clone(),
            created_by: input.actor.clone(),
            no_dedupe: true,
            ..Default::default()
        },
    )?;
    event_conn(
        conn,
        s,
        &input.actor,
        JOB_RECOVER_ACTION,
        &job.id,
        pulse_store::Detail::json(
            json!({"class":"human","n":n,"outcome":"replanned","root":root,"signature":"","why":format!("pulse job retry by {}", input.actor),"job":copy.id}),
        )?,
    )?;
    Ok(RetryResult {
        job: copy,
        outcome: "replanned".into(),
        root: root.into(),
        reused: false,
        n,
    })
}

fn retry_in_place(
    s: &pulse_store::Store,
    conn: &Connection,
    job: &Job,
    root: &str,
    n: i64,
    input: RetryInput,
) -> Result<RetryResult> {
    let result = format!("retried by {}: {}", input.actor, trunc(&job.result, 200));
    let append = retry_note(&s.now(), &input.actor, &input.brief);
    conn.execute("UPDATE jobs SET status = 'queued', result = ?, branch = '', attempts = 0, lease_agent = '', lease_until = '', finished_at = '', brief = brief || ? WHERE id = ?",
            params![result, append, job.id])?;
    event_conn(
        conn,
        s,
        &input.actor,
        "job.queued",
        &job.id,
        trunc400(&result),
    )?;
    event_conn(
        conn,
        s,
        &input.actor,
        JOB_RECOVER_ACTION,
        &job.id,
        pulse_store::Detail::json(
            json!({"class":"human","n":n,"outcome":"retried","root":root,"signature":"","why":format!("pulse job retry by {}", input.actor)}),
        )?,
    )?;
    Ok(RetryResult {
        job: job_by_id_conn(conn, &job.id)?.expect("retried job"),
        outcome: "retried".into(),
        root: root.into(),
        reused: false,
        n,
    })
}

pub fn recovery_carrier(conn: &Connection, job: &Job, root: &str) -> Result<String> {
    let id: Option<String> = conn.query_row(
        r#"WITH RECURSIVE recoveries AS (
		SELECT seq, at, actor, subject, json_extract(detail, '$.job') AS job,
			json_extract(detail, '$.class') AS class, json_extract(detail, '$.outcome') AS outcome
		FROM events WHERE action = ? AND json_valid(detail) AND json_extract(detail, '$.root') = ?
	), chain(id) AS (
		SELECT ?
		UNION SELECT job FROM recoveries WHERE outcome = 'replanned'
	), ancestors(id) AS (
		SELECT ?
		UNION SELECT r.subject FROM recoveries r JOIN ancestors a ON r.job = a.id
			WHERE r.outcome = 'replanned'
	)
	SELECT j.id FROM jobs j WHERE j.id != ? AND j.project = ? AND (
		(j.id IN (SELECT id FROM chain) AND (
			j.status IN ('queued', 'leased', 'blocked', 'done', 'cancelled')
			OR EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.status = 'running')))
		OR (? != '' AND j.source = ? AND (
			j.status IN ('queued', 'leased', 'blocked')
			OR EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.status = 'running')
			OR (j.status IN ('done', 'cancelled') AND (j.created_at >= ? OR j.finished_at >= ?)))))
		AND (j.status != 'cancelled' OR NOT EXISTS (
			SELECT 1 FROM recoveries h WHERE h.subject = j.id AND h.outcome = 'replanned' AND h.class = 'human'
			AND (h.actor = ? OR h.actor GLOB ?) AND h.job IN (SELECT id FROM ancestors)
			AND j.finished_at != '' AND (h.at > j.finished_at OR (h.at = j.finished_at AND EXISTS (
				SELECT 1 FROM events c WHERE c.subject = j.id AND c.action = 'job.cancelled'
				AND c.at = j.finished_at AND c.seq < h.seq)))
			AND NOT EXISTS (SELECT 1 FROM events c WHERE c.subject = j.id AND c.action = 'job.cancelled' AND c.seq > h.seq)))
		ORDER BY j.created_at DESC, j.id DESC LIMIT 1"#,
        params![
            JOB_RECOVER_ACTION,
            root,
            root,
            job.id,
            job.id,
            job.project,
            job.source,
            job.source,
            job.finished_at,
            job.finished_at,
            pulse_store::HUMAN_AGENT,
            format!("{}:*", pulse_store::HUMAN_AGENT)
        ],
        |r| r.get(0),
    ).optional()?;
    Ok(id.unwrap_or_default())
}

fn recover_root(conn: &Connection, job: &Job) -> Result<String> {
    let root: Option<String> = conn.query_row(
        "SELECT json_extract(detail, '$.root') FROM events WHERE action = ? AND json_valid(detail) AND json_extract(detail, '$.outcome') = 'replanned' AND json_extract(detail, '$.job') = ? ORDER BY seq LIMIT 1",
        params![JOB_RECOVER_ACTION, job.id],
        |r| r.get(0),
    ).optional()?;
    Ok(root.unwrap_or_else(|| job.id.clone()))
}

fn recover_n(conn: &Connection, root: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM events WHERE action = ? AND json_valid(detail) AND json_extract(detail, '$.root') = ? AND json_extract(detail, '$.outcome') IN ('requeued', 'replanned', 'retried')",
        params![JOB_RECOVER_ACTION, root],
        |r| r.get(0),
    )?)
}

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
pub fn decide(
    s: &pulse_store::Store,
    post_id: &str,
    answer: &str,
    words: &str,
    decided_by: &str,
) -> Result<Decision> {
    if answer != "yes" && answer != "no" {
        return Err(refuse(2, "The answer is yes or no"));
    }
    let decided_by = if decided_by.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        decided_by
    };
    let tx = s.begin()?;
    let target = crate::posts::post_by_id_conn(&tx, post_id)?
        .ok_or_else(|| refuse(2, format!("No post {}", py_repr(post_id))))?;
    if tx
        .query_row(
            "SELECT 1 FROM decisions WHERE post_id = ?",
            [post_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .is_some()
    {
        return Err(refuse(
            2,
            format!("Post {post_id} was already decided; a new proposal needs a new post"),
        ));
    }
    let decider = decision_decider(s, &tx, &target, post_id, decided_by)?;
    let resolution = if answer == "yes" {
        "accepted"
    } else {
        "declined"
    };
    let said = if words.is_empty() {
        "(no words recorded)"
    } else {
        words
    };
    let body = if decided_by == pulse_store::HUMAN_AGENT {
        format!("The human answered **{answer}**.\n\n{said}\n\nProposal: {post_id}\n")
    } else {
        format!(
            "The owner is away (owner.presence = away); **{decided_by}** answered **{answer}** on the owner's behalf.\n\n{said}\n\nProposal: {post_id}\n"
        )
    };
    let post = crate::posts::create_post_conn(
        s,
        &tx,
        crate::posts::PostInput {
            kind: "result".into(),
            project: target.project.clone(),
            area: target.area.clone(),
            thread: target.thread.clone(),
            summary: format!("{resolution}: {}", trunc(&target.summary, 200)),
            body,
            agent_row: Some(decider),
            on: post_id.into(),
            resolution: resolution.into(),
            branch: Some(String::new()),
            ..Default::default()
        },
    )?;
    let now = s.now();
    tx.execute(
        "INSERT INTO decisions (post_id, project, answer, words, decided_at, decided_by, result_post) VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![post_id, target.project, answer, words, now, decided_by, post.id],
    )?;
    let mut detail = serde_json::Map::new();
    detail.insert("answer".into(), json!(answer));
    detail.insert("result".into(), json!(post.id));
    if decided_by != pulse_store::HUMAN_AGENT {
        detail.insert("on_behalf".into(), json!("owner-away"));
    }
    event_conn(
        &tx,
        s,
        decided_by,
        "decide",
        post_id,
        pulse_store::Detail::json(serde_json::Value::Object(detail))?,
    )?;
    let mut out = Decision {
        post_id: post_id.into(),
        project: target.project.clone(),
        answer: answer.into(),
        words: words.into(),
        decided_at: now,
        decided_by: decided_by.into(),
        result: post.clone(),
        job: None,
    };
    if answer == "yes" {
        out.job = decision_job_conn(s, &tx, &target, &post, words, decided_by)?;
    }
    tx.commit()?;
    Ok(out)
}

fn decision_decider(
    s: &pulse_store::Store,
    conn: &Connection,
    target: &crate::posts::Post,
    post_id: &str,
    decided_by: &str,
) -> Result<pulse_store::Agent> {
    if decided_by == pulse_store::HUMAN_AGENT {
        return Ok(s
            .agent(pulse_store::HUMAN_AGENT)?
            .unwrap_or_else(|| pulse_store::Agent {
                name: pulse_store::HUMAN_AGENT.into(),
                display: "the human".into(),
                sessions: vec![],
                ..Default::default()
            }));
    }
    if !s.owner_away() {
        return Err(refuse(
            2,
            "pulse decide is the human's call; a lead decides only while owner.presence is away",
        ));
    }
    let lead = s.agent(decided_by)?;
    if !matches!(lead, Some(ref l) if l.kind == "lead" && l.status == "active") {
        return Err(refuse(2, "only a project's lead decides for the owner"));
    }
    if decided_by != lead_name(&target.project) {
        return Err(refuse(
            2,
            "only the proposal project's lead decides for the owner",
        ));
    }
    if conn
        .query_row(
            "SELECT 1 FROM compute_requests WHERE proposal_post = ?",
            [post_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .is_some()
    {
        return Err(refuse(
            2,
            "compute is money: it waits for the owner; compute.auto_approve and the caps pre-authorize it",
        ));
    }
    if decision_marker_re().is_match(&target.body) {
        return Err(refuse(
            2,
            "a proposal that authorizes a command waits for the human (ADR-021)",
        ));
    }
    if is_stop_question(target) {
        return Err(refuse(
            2,
            "a stop question from the evolution loop is the owner's alone (ADR-043)",
        ));
    }
    let what = tier2_proposal(&target.summary, &target.body);
    if !what.is_empty() {
        return Err(refuse(
            2,
            format!("a proposal that {what} is a Tier 2 decision: it waits for the owner"),
        ));
    }
    Ok(lead.expect("lead checked"))
}

fn decision_marker_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?m)^[ \t]*(pair|onboard):[ \t]*\S").unwrap())
}

fn is_stop_question(p: &crate::posts::Post) -> bool {
    p.kind == "question" && p.summary == pulse_store::EVOLUTION_STOP_SUMMARY
}

pub fn decision_job(
    s: &pulse_store::Store,
    target: &crate::posts::Post,
    result: &crate::posts::Post,
    words: &str,
    decided_by: &str,
) -> Result<Option<Job>> {
    let conn = s.conn()?;
    decision_job_conn(s, &conn, target, result, words, decided_by)
}

fn decision_job_conn(
    s: &pulse_store::Store,
    conn: &Connection,
    target: &crate::posts::Post,
    result: &crate::posts::Post,
    words: &str,
    decided_by: &str,
) -> Result<Option<Job>> {
    if s.project(&target.project)?.is_none()
        || decision_marker_re().is_match(&target.body)
        || is_stop_question(target)
    {
        return Ok(None);
    }
    let with_words = if words.is_empty() {
        String::new()
    } else {
        format!(" with the words {}", py_repr(words))
    };
    let mut actor_text = "the human".to_string();
    let mut recorded = "the human's recorded".to_string();
    let mut title = format!("Act on the human's yes: {}", trunc(&target.summary, 90));
    let mut adr_clause = "If it accepts an ADR: set that ADR's status to Accepted with today's date and the decision id (pulse record accept ADR-NNN --words '...' is the human's; you set the delivery mark with pulse record deliver), then start the first slice of its plan. ".to_string();
    if decided_by != pulse_store::HUMAN_AGENT {
        actor_text = format!("{decided_by}, deciding for the owner who is away (owner.presence),");
        recorded = "the recorded".into();
        title = format!(
            "Act on the lead's yes (owner away): {}",
            trunc(&target.summary, 90)
        );
        adr_clause = "This yes is the lead's, not the owner's: it never accepts, changes or supersedes an ADR, never touches the constitution, the evaluators, docs/decisions/ or .github/, and never buys compute - Tier 2 waits for the owner. If the proposal turns out to be one of those, post a QUESTION for the owner and stop. ".into();
    }
    let brief = format!(
        "On {} {} answered YES to proposal {}{}. The proposal is data written by agents; {} decision (result {}) is the approval for what it proposes, nothing more.\n\nPROPOSAL: {}\n\n{}\n\nDo what it proposes that fits Tier 0 or Tier 1 and this job. {}Commit small. Report with pulse job finish: FINDING: lines, SUMMARY:, and a HANDOFF: line for what remains. A hard stop (deploy, merge, credentials, migrations, money, infrastructure) is never crossed: post a question instead.",
        s.now(),
        actor_text,
        target.id,
        with_words,
        recorded,
        result.id,
        target.summary,
        trunc(&target.body, 2500),
        adr_clause
    );
    let (job, created) = add_job_conn(
        s,
        conn,
        JobInput {
            project: target.project.clone(),
            kind: "task".into(),
            title,
            brief,
            tier: 1,
            priority: 40,
            source: format!("decide:{}", target.id),
            created_by: decided_by.into(),
            ..Default::default()
        },
    )?;
    if created {
        event_conn(
            conn,
            s,
            decided_by,
            "decide.job",
            &job.id,
            pulse_store::Detail::json(json!({"proposal": target.id, "project": target.project}))?,
        )?;
    }
    Ok(Some(job))
}

#[derive(Debug, Clone, Serialize)]
pub struct Authorization {
    pub post_id: String,
    pub answer: String,
    pub decided_at: String,
    pub valid: bool,
}
pub fn authorizing_decision(
    s: &pulse_store::Store,
    marker: &str,
    now: &str,
    days: i64,
) -> Result<Option<Authorization>> {
    let now = if now.is_empty() { s.now() } else { now.into() };
    let days = if days <= 0 { DECISION_VALID_DAYS } else { days };
    let pattern = regex::Regex::new(&format!(r"(?m)^[ \t]*{}[ \t]*$", regex::escape(marker)))?;
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT d.post_id, d.answer, d.decided_at, p.body FROM decisions d JOIN posts p ON p.id = d.post_id WHERE d.decided_by = ? ORDER BY d.decided_at DESC, d.post_id DESC")?;
    let mut rows = stmt.query([pulse_store::HUMAN_AGENT])?;
    while let Some(row) = rows.next()? {
        let body: String = row.get(3)?;
        if !pattern.is_match(&body) {
            continue;
        }
        let decided_at: String = row.get(2)?;
        let t0 = parse_iso(&now)?;
        let t1 = parse_iso(&decided_at)?;
        let answer: String = row.get(1)?;
        let age = t0 - t1;
        return Ok(Some(Authorization {
            post_id: row.get(0)?,
            valid: answer == "yes"
                && age >= chrono::Duration::zero()
                && age <= chrono::Duration::days(days),
            answer,
            decided_at,
        }));
    }
    Ok(None)
}
pub fn agent_gate(s: &pulse_store::Store, marker: &str, actor: &str, now: &str) -> Result<String> {
    if actor.is_empty() || actor == pulse_store::HUMAN_AGENT {
        Ok(String::new())
    } else {
        let found = authorizing_decision(s, marker, now, 0)?;
        let Some(found) = found else {
            return Err(refuse(
                2,
                format!(
                    "An agent may run this only after the human's `pulse decide ... yes` on a board proposal whose body carries the line `{marker}`; no decided proposal names it"
                ),
            ));
        };
        if !found.valid {
            return Err(refuse(
                2,
                format!(
                    "The newest decision naming `{}` ({}: {} on {}) does not authorize it; a yes at most {} days old is needed",
                    marker,
                    found.post_id,
                    found.answer,
                    &found.decided_at[..10],
                    DECISION_VALID_DAYS
                ),
            ));
        }
        Ok(found.post_id)
    }
}

pub fn onboarding_job(s: &pulse_store::Store, slug: &str) -> Result<Option<Job>> {
    let conn = s.conn()?;
    let id: Option<String> = conn.query_row("SELECT id FROM jobs WHERE source = ? AND status IN ('queued', 'leased', 'blocked', 'done') ORDER BY created_at DESC, id DESC LIMIT 1", [format!("onboard:{slug}")], |r| r.get(0)).optional()?;
    id.map_or(Ok(None), |id| job_by_id_conn(&conn, &id))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use serde_json::Value;
    use tempfile::TempDir;

    const TEST_NOW: &str = "2026-09-06T10:00:00Z";

    fn fixture() -> (TempDir, pulse_store::Store, pulse_store::Project) {
        let dir = tempfile::tempdir().unwrap();
        let s = pulse_store::Store::open(pulse_store::Options {
            home: dir.path().display().to_string(),
            create: true,
            now: TEST_NOW.into(),
            ..Default::default()
        })
        .unwrap();
        let conn = s.conn().unwrap();
        conn.execute("INSERT INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) VALUES ('demo', 'demo', ?, '', 0, '', 'main', 'active', ?, 'human', ?)",
            params![dir.path().join("repo").display().to_string(), TEST_NOW, TEST_NOW]).unwrap();
        let p = s.project("demo").unwrap().unwrap();
        drop(conn);
        (dir, s, p)
    }

    fn make_failed_job(s: &pulse_store::Store, project: &str, source: &str) -> Job {
        let (job, _) = add_job(
            s,
            JobInput {
                project: project.into(),
                kind: "task".into(),
                title: "Fix the failing check".into(),
                brief: "Original brief".into(),
                tier: 1,
                priority: 7,
                source: source.into(),
                created_by: "alice".into(),
                ..Default::default()
            },
        )
        .unwrap();
        finish_job(
            s,
            &job.id,
            "alice",
            "failed",
            &"x".repeat(220),
            &format!("pulse/alice/{}", job.id),
            None,
        )
        .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET attempts = 4, lease_agent = 'alice', lease_until = '2026-09-06T11:00:00Z' WHERE id = ?",
                [&job.id],
            )
            .unwrap();
        job_by_id(s, &job.id).unwrap().unwrap()
    }

    fn recover_event(s: &pulse_store::Store, job_id: &str) -> serde_json::Map<String, Value> {
        let detail: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT detail FROM events WHERE action = ? AND subject = ? ORDER BY seq DESC LIMIT 1",
                params![JOB_RECOVER_ACTION, job_id],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str::<Value>(&detail)
            .unwrap()
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn retry_title_once() {
        assert_eq!(retry_title("job-1", "Retry job-1: x"), "Retry job-1: x");
    }

    #[test]
    fn retry_job_in_place_queues_failed_job() {
        let (_dir, s, p) = fixture();
        let job = make_failed_job(&s, &p.slug, "task:one");

        let result = retry_job(
            &s,
            RetryInput {
                job_id: job.id.clone(),
                brief: "Run the check first.".into(),
                actor: "human".into(),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.outcome, "retried");
        assert_eq!(result.root, job.id);
        assert_eq!(result.job.id, job.id);
        assert_eq!(result.job.status, "queued");
        assert_eq!(result.job.attempts, 0);
        assert!(result.job.branch.is_empty());
        assert!(result.job.finished_at.is_empty());
        assert!(result.job.lease_agent.is_empty());
        assert!(result.job.lease_until.is_empty());
        assert!(result.job.result.starts_with("retried by human: "));
        assert_eq!(
            result
                .job
                .result
                .trim_start_matches("retried by human: ")
                .chars()
                .count(),
            200
        );
        assert!(
            result
                .job
                .brief
                .contains("## Retry note (human, 2026-09-06T10:00:00Z)\n\nRun the check first.")
        );
        let detail = recover_event(&s, &job.id);
        assert_eq!(detail["outcome"], "retried");
        assert_eq!(detail["root"], job.id);
        assert_eq!(detail["class"], "human");
        assert_eq!(detail["why"], "pulse job retry by human");
        assert_eq!(detail["signature"], "");
        assert_eq!(detail["n"], 1);
    }

    #[test]
    fn retry_job_keeps_recover_root_and_increments() {
        let (_dir, s, p) = fixture();
        let job = make_failed_job(&s, &p.slug, "task:chain");
        event(
            &s,
            "daemon",
            JOB_RECOVER_ACTION,
            "job-root",
            pulse_store::Detail::json(serde_json::json!({
                "outcome": "replanned",
                "root": "job-root",
                "job": job.id
            }))
            .unwrap(),
        )
        .unwrap();

        let result = retry_job(
            &s,
            RetryInput {
                job_id: job.id.clone(),
                actor: "human".into(),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(result.root, "job-root");
        assert_eq!(result.n, 2);
        let detail = recover_event(&s, &job.id);
        assert_eq!(detail["root"], "job-root");
        assert_eq!(detail["n"], 2);
    }

    #[test]
    fn retry_job_as_new_queues_one_corrected_copy() {
        let (_dir, s, p) = fixture();
        let job = make_failed_job(&s, &p.slug, "task:copy");

        let result = retry_job(
            &s,
            RetryInput {
                job_id: job.id.clone(),
                as_new: true,
                brief: "Correct the test fixture.".into(),
                actor: "human".into(),
            },
        )
        .unwrap();
        let copy = result.job;
        assert_eq!(result.outcome, "replanned");
        assert_eq!(result.root, job.id);
        assert_ne!(copy.id, job.id);
        assert_eq!(copy.source, job.source);
        assert_eq!(copy.project, job.project);
        assert_eq!(copy.kind, job.kind);
        assert_eq!(copy.tier, job.tier);
        assert_eq!(copy.priority, job.priority);
        assert_eq!(copy.status, "queued");
        assert!(
            copy.title
                .starts_with(&format!("Retry {}: Fix the failing check", job.id))
        );
        assert!(copy.title.chars().count() <= 200);
        for want in [
            "Original brief",
            "## Earlier attempt (ADR-036)",
            "Correct the test fixture.",
            "Start from the cause, fix it, then deliver the original task above.",
        ] {
            assert!(copy.brief.contains(want), "missing {want}: {}", copy.brief);
        }
        let detail = recover_event(&s, &job.id);
        assert_eq!(detail["outcome"], "replanned");
        assert_eq!(detail["root"], job.id);
        assert_eq!(detail["job"], copy.id);
        assert_eq!(detail["n"], 1);

        let again = retry_job(
            &s,
            RetryInput {
                job_id: job.id,
                as_new: true,
                actor: "human".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(again.job.id, copy.id);
        assert!(again.reused);
    }

    #[test]
    fn decide_yes_queues_job() {
        let (_dir, s, p) = fixture();
        crate::agents::join_agent(
            &s,
            crate::agents::JoinInput {
                name: "alice".into(),
                runner: "copilot".into(),
                model: "m".into(),
                session: "sess-alice".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let proposal = crate::posts::create_post(
            &s,
            crate::posts::PostInput {
                kind: "suggestion".into(),
                project: p.slug.clone(),
                area: "plan".into(),
                thread: "next".into(),
                summary: "Ship the next slice".into(),
                body: "Do a useful thing.".into(),
                agent: "alice".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let d = decide(&s, &proposal.id, "yes", "approved", "human").unwrap();

        assert_eq!(d.post_id, proposal.id);
        assert_eq!(d.project, p.slug);
        assert_eq!(d.answer, "yes");
        assert_eq!(d.words, "approved");
        assert_eq!(d.decided_at, TEST_NOW);
        assert_eq!(d.result.kind, "result");
        assert_eq!(d.result.resolution, "accepted");
        assert_eq!(d.result.on, proposal.id);
        let job = d.job.expect("yes queues job");
        assert_eq!(job.project, p.slug);
        assert_eq!(job.kind, "task");
        assert_eq!(job.tier, 1);
        assert_eq!(job.priority, 40);
        assert_eq!(job.source, format!("decide:{}", proposal.id));
        assert!(job.title.starts_with("Act on the human's yes: "));
        assert!(job.brief.contains("answered YES"));
        let row: (String, String, String) = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT answer, words, result_post FROM decisions WHERE post_id = ?",
                [&proposal.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("yes".into(), "approved".into(), d.result.id));
    }
}
