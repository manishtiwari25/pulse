//! Suggested work: any agent - a worker that saw a gap, a role lead, the
//! chat, the owner - suggests the next work item for a project, and a
//! different agent reviews it before it becomes a task. The daemon queues
//! the review job; the reviewer's verdict is `accept` (the suggestion becomes
//! a task the lead plans), `decline` (closed with the reason, which the
//! suggester can read) or `owner` (a decision only the owner can take; it
//! waits on the owner's For-you list). Nobody reviews their own suggestion.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::json;

use crate::tasks::{Task, check_project, clean, input, next_id};

pub const STATUSES: &[&str] = &[
    "suggested",
    "reviewing",
    "accepted",
    "declined",
    "owner",
    "withdrawn",
];
pub const VERDICTS: &[&str] = &["accept", "decline", "owner"];
/// The `source` of a review job: `suggestion:SUG-001`.
pub const SOURCE_PREFIX: &str = "suggestion:";
/// Review jobs that may end without a verdict before the suggestion stops
/// waiting for agents and goes to the owner.
pub const MAX_REVIEWS: i32 = 3;
/// How many suggestions one agent may have waiting for a review in a
/// project; the owner has no limit.
pub const OPEN_PER_AGENT: i64 = 5;
const TITLE_LIMIT: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub id: String,
    pub project: String,
    pub title: String,
    pub brief: String,
    pub priority: i32,
    pub suggested_by: String,
    pub status: String,
    pub reviewer: String,
    pub verdict: String,
    pub job_id: String,
    pub reviews: i32,
    pub task_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub reviewed_at: String,
}

impl Suggestion {
    /// Still waiting for somebody's verdict.
    pub fn is_open(&self) -> bool {
        matches!(self.status.as_str(), "suggested" | "reviewing" | "owner")
    }

    /// The source of its review jobs.
    pub fn source(&self) -> String {
        source(&self.id)
    }
}

/// A reviewer's verdict with the optional sharpening of what gets accepted.
#[derive(Debug, Clone, Default)]
pub struct Verdict {
    /// `accept`, `decline` or `owner`.
    pub verdict: String,
    pub reason: String,
    /// A sharper title for the task (accept); empty keeps the suggestion's.
    pub title: String,
    /// A rewritten brief for the task (accept); empty keeps the suggestion's.
    pub brief: String,
    /// The task's priority (accept); None keeps the suggestion's.
    pub priority: Option<i32>,
}

pub fn source(id: &str) -> String {
    format!("{SOURCE_PREFIX}{id}")
}

/// The suggestion id a review job's source names, if it is one.
pub fn split_source(source: &str) -> Option<&str> {
    source
        .strip_prefix(SOURCE_PREFIX)
        .filter(|id| !id.is_empty() && !id.contains(':'))
}

pub fn is_id(id: &str) -> bool {
    id.starts_with("SUG-")
}

const COLS: &str = "id, project, title, brief, priority, suggested_by, status, reviewer, verdict, job_id, reviews, task_id, created_at, updated_at, reviewed_at";

fn scan(row: &rusqlite::Row<'_>) -> rusqlite::Result<Suggestion> {
    Ok(Suggestion {
        id: row.get(0)?,
        project: row.get(1)?,
        title: row.get(2)?,
        brief: row.get(3)?,
        priority: row.get(4)?,
        suggested_by: row.get(5)?,
        status: row.get(6)?,
        reviewer: row.get(7)?,
        verdict: row.get(8)?,
        job_id: row.get(9)?,
        reviews: row.get(10)?,
        task_id: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        reviewed_at: row.get(14)?,
    })
}

fn is_human(actor: &str) -> bool {
    actor == pulse_store::HUMAN_AGENT
}

/// Suggests a work item. Agents say why (the brief is what the reviewer
/// judges) and may have `OPEN_PER_AGENT` waiting per project; a title that
/// is already an open suggestion or an open task is refused and named.
pub fn suggest(
    s: &pulse_store::Store,
    project: &str,
    title: &str,
    brief: &str,
    priority: i32,
    author: &str,
) -> Result<Suggestion> {
    let title = clean(title);
    if title.is_empty() {
        return Err(input(
            2,
            "A suggestion needs a title",
            "the title is what the reviewer and the owner see first",
            "pulse task suggest --project P --title 'what the project needs next' --brief-file F",
        ));
    }
    if title.len() > TITLE_LIMIT {
        return Err(input(
            2,
            format!("title is {} chars; limit {TITLE_LIMIT}", title.len()),
            "keep the title short",
            "move detail into the brief (--brief-file F or stdin)",
        ));
    }
    if author.is_empty() {
        return Err(input(
            2,
            "An author is needed",
            "every suggestion records who made it",
            format!(
                "pass --agent NAME or set PULSE_AGENT (the owner is {})",
                pulse_store::HUMAN_AGENT
            ),
        ));
    }
    let brief = brief.trim().to_string();
    if brief.is_empty() && !is_human(author) {
        return Err(input(
            2,
            "A suggestion needs a brief",
            "the why - a failing check, a step the user does by hand, a promise the README makes - is what the reviewer judges",
            "pass --brief-file F, --brief TEXT or the brief on stdin",
        ));
    }
    check_project(s, project)?;
    let now = s.now();
    let tx = s.begin()?;
    let same: Option<String> = tx
        .query_row(
            "SELECT id FROM task_suggestions WHERE project = ? AND lower(title) = lower(?) AND status IN ('suggested', 'reviewing', 'owner') LIMIT 1",
            params![project, title],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = same {
        return Err(input(
            2,
            format!("Already suggested as {id}"),
            "an open suggestion in this project has the same title",
            format!(
                "pulse task show {id}; add your evidence on its thread or suggest a different slice"
            ),
        ));
    }
    let task: Option<String> = tx
        .query_row(
            "SELECT id FROM tasks WHERE project = ? AND lower(title) = lower(?) AND status NOT IN ('done', 'cancelled') LIMIT 1",
            params![project, title],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = task {
        return Err(input(
            2,
            format!("Already a task: {id}"),
            "an open task in this project has the same title",
            format!("pulse task show {id}"),
        ));
    }
    if !is_human(author) {
        let open: i64 = tx.query_row(
            "SELECT COUNT(*) FROM task_suggestions WHERE project = ? AND suggested_by = ? AND status IN ('suggested', 'reviewing')",
            params![project, author],
            |r| r.get(0),
        )?;
        if open >= OPEN_PER_AGENT {
            return Err(input(
                2,
                format!("{author} has {open} suggestions waiting for a review in {project}"),
                format!(
                    "an agent may have {OPEN_PER_AGENT} waiting per project; a reviewer is another agent's time"
                ),
                format!(
                    "wait for the verdicts (pulse task suggestions --project {project}) or withdraw one"
                ),
            ));
        }
    }
    let id = next_id(&tx, "task_suggestions", "SUG-")?;
    tx.execute(
        "INSERT INTO task_suggestions (id, project, title, brief, priority, suggested_by, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 'suggested', ?, ?)",
        params![id, project, title, brief, priority, author, now, now],
    )?;
    tx.event(
        author,
        "suggestion.new",
        &id,
        pulse_store::Detail::json(
            json!({"project": project, "title": title, "priority": priority}),
        )?,
    )?;
    tx.commit()?;
    get(s, &id)
}

pub fn get(s: &pulse_store::Store, id: &str) -> Result<Suggestion> {
    let conn = s.conn()?;
    conn.query_row(
        &format!("SELECT {COLS} FROM task_suggestions WHERE id = ?"),
        [id],
        scan,
    )
    .optional()?
    .ok_or_else(|| {
        input(
            2,
            format!("No suggestion {id:?}"),
            "the id is not in this store",
            "pulse task suggestions",
        )
    })
}

/// Suggestions of a project (every project when empty), newest first;
/// `status` narrows to one state, `open` to the three that still wait.
pub fn list(s: &pulse_store::Store, project: &str, status: &str) -> Result<Vec<Suggestion>> {
    let mut q = format!("SELECT {COLS} FROM task_suggestions WHERE 1=1");
    let mut args = Vec::new();
    if !project.is_empty() {
        q.push_str(" AND project = ?");
        args.push(project.to_string());
    }
    if status == "open" {
        q.push_str(" AND status IN ('suggested', 'reviewing', 'owner')");
    } else if !status.is_empty() {
        if !STATUSES.contains(&status) {
            return Err(input(
                2,
                format!("Unknown status {status:?}"),
                format!("statuses are {} (or open)", STATUSES.join(", ")),
                "pulse task suggestions --status open",
            ));
        }
        q.push_str(" AND status = ?");
        args.push(status.to_string());
    }
    q.push_str(" ORDER BY id DESC");
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    Ok(stmt
        .query_map(rusqlite::params_from_iter(refs), scan)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Records a verdict. An agent reviews another agent's suggestion while it
/// is `suggested` or `reviewing`; the owner may rule on any open one, and is
/// the only one who may rule on one marked `owner`. `accept` creates the
/// task and returns it.
pub fn review(
    s: &pulse_store::Store,
    id: &str,
    v: &Verdict,
    actor: &str,
) -> Result<(Suggestion, Option<Task>)> {
    if actor.is_empty() {
        return Err(input(
            2,
            "A reviewer is needed",
            "every verdict records who gave it",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    if !VERDICTS.contains(&v.verdict.as_str()) {
        return Err(input(
            2,
            format!("Unknown verdict {:?}", v.verdict),
            format!("verdicts are {}", VERDICTS.join(", ")),
            format!("pulse task review {id} accept|decline|owner --reason WHY"),
        ));
    }
    let reason = clean(&v.reason);
    let human = is_human(actor);
    if reason.is_empty() && !human {
        return Err(input(
            2,
            "A verdict needs a reason",
            "the suggester and the owner read why",
            format!("pulse task review {id} {} --reason '...'", v.verdict),
        ));
    }
    if v.verdict == "owner" && human {
        return Err(input(
            2,
            "You are the owner",
            "`owner` is how an agent hands a decision to you",
            format!("pulse task review {id} accept|decline"),
        ));
    }
    let cur = get(s, id)?;
    if !cur.is_open() {
        return Err(input(
            2,
            format!("{id} is already {}", cur.status),
            "a reviewed suggestion does not change",
            format!("pulse task show {id}"),
        ));
    }
    if cur.status == "owner" && !human {
        return Err(input(
            2,
            format!("{id} waits for the owner"),
            format!("{} handed it to the owner: {}", cur.reviewer, cur.verdict),
            "leave it; the owner rules on it from the console",
        ));
    }
    if actor == cur.suggested_by && !human {
        return Err(input(
            2,
            "Nobody reviews their own suggestion",
            format!("{actor} suggested {id}"),
            "leave it to the review job the daemon queues for another agent",
        ));
    }
    // The task first, so a refused task leaves the suggestion open rather
    // than accepted with nothing behind it.
    let task = if v.verdict == "accept" {
        let title = {
            let t = clean(&v.title);
            if t.is_empty() { cur.title.clone() } else { t }
        };
        let body = if v.brief.trim().is_empty() {
            cur.brief.clone()
        } else {
            v.brief.trim().to_string()
        };
        let why = if reason.is_empty() {
            String::new()
        } else {
            format!(": {reason}")
        };
        let brief = format!(
            "{body}\n\n---\nSuggested by {} as {id}; accepted by {actor}{why}.\n",
            cur.suggested_by
        );
        Some(crate::tasks::new(
            s,
            &cur.project,
            &title,
            &brief,
            v.priority.unwrap_or(cur.priority),
            &cur.suggested_by,
        )?)
    } else {
        None
    };
    let now = s.now();
    let status = match v.verdict.as_str() {
        "accept" => "accepted",
        "decline" => "declined",
        _ => "owner",
    };
    let task_id = task.as_ref().map(|t| t.id.clone()).unwrap_or_default();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE task_suggestions SET status = ?, reviewer = ?, verdict = ?, task_id = ?, reviewed_at = ?, updated_at = ? WHERE id = ?",
        params![status, actor, reason, task_id, now, now, id],
    )?;
    if !cur.job_id.is_empty() {
        tx.execute(
            "UPDATE jobs SET status = 'cancelled', finished_at = ?, result = ? WHERE id = ? AND status = 'queued'",
            params![now, format!("suggestion {id} {status} by {actor}"), cur.job_id],
        )?;
    }
    tx.event(
        actor,
        "suggestion.review",
        id,
        pulse_store::Detail::json(json!({
            "project": cur.project,
            "verdict": v.verdict,
            "status": status,
            "reason": reason,
            "suggested_by": cur.suggested_by,
            "task": task_id,
        }))?,
    )?;
    tx.commit()?;
    Ok((get(s, id)?, task))
}

/// The suggester takes a suggestion back (the owner may too); a queued
/// review job is cancelled with it.
pub fn withdraw(s: &pulse_store::Store, id: &str, reason: &str, actor: &str) -> Result<Suggestion> {
    if actor.is_empty() {
        return Err(input(
            2,
            "An actor is needed",
            "every withdrawal records who asked",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    let cur = get(s, id)?;
    if !cur.is_open() {
        return Err(input(
            2,
            format!("{id} is already {}", cur.status),
            "a closed suggestion does not change",
            format!("pulse task show {id}"),
        ));
    }
    if actor != cur.suggested_by && !is_human(actor) {
        return Err(input(
            2,
            format!("{id} was suggested by {}", cur.suggested_by),
            "only the suggester or the owner withdraws a suggestion",
            format!("pulse task review {id} decline --reason WHY (as its reviewer)"),
        ));
    }
    let now = s.now();
    let reason = clean(reason);
    let tx = s.begin()?;
    tx.execute(
        "UPDATE task_suggestions SET status = 'withdrawn', verdict = ?, reviewed_at = ?, updated_at = ? WHERE id = ?",
        params![reason, now, now, id],
    )?;
    if !cur.job_id.is_empty() {
        tx.execute(
            "UPDATE jobs SET status = 'cancelled', finished_at = ?, result = ? WHERE id = ? AND status = 'queued'",
            params![now, format!("suggestion {id} withdrawn by {actor}"), cur.job_id],
        )?;
    }
    tx.event(
        actor,
        "suggestion.withdrawn",
        id,
        pulse_store::Detail::json(json!({"project": cur.project, "reason": reason}))?,
    )?;
    tx.commit()?;
    get(s, id)
}

/// The daemon queued a review job for it.
pub fn reviewing(
    s: &pulse_store::Store,
    id: &str,
    job_id: &str,
    actor: &str,
) -> Result<Suggestion> {
    let cur = get(s, id)?;
    if cur.status != "suggested" {
        return Err(input(
            2,
            format!("{id} is {}", cur.status),
            "only a suggestion waiting for a review gets a review job",
            format!("pulse task show {id}"),
        ));
    }
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE task_suggestions SET status = 'reviewing', job_id = ?, updated_at = ? WHERE id = ?",
        params![job_id, now, id],
    )?;
    tx.event(
        actor,
        "suggestion.reviewing",
        id,
        pulse_store::Detail::json(
            json!({"project": cur.project, "job": job_id, "review": cur.reviews + 1}),
        )?,
    )?;
    tx.commit()?;
    get(s, id)
}

/// The review job ended without a verdict: the suggestion waits for another
/// reviewer, or - after `MAX_REVIEWS` such endings - for the owner.
pub fn review_ended(s: &pulse_store::Store, id: &str, actor: &str) -> Result<Suggestion> {
    let cur = get(s, id)?;
    if cur.status != "reviewing" {
        return Err(input(
            2,
            format!("{id} is {}", cur.status),
            "only a suggestion under review has a review to end",
            format!("pulse task show {id}"),
        ));
    }
    let now = s.now();
    let reviews = cur.reviews + 1;
    let to_owner = reviews >= MAX_REVIEWS;
    let tx = s.begin()?;
    if to_owner {
        tx.execute(
            "UPDATE task_suggestions SET status = 'owner', reviews = ?, reviewer = '', verdict = ?, reviewed_at = ?, updated_at = ? WHERE id = ?",
            params![
                reviews,
                format!("{reviews} review jobs ended without a verdict; your call"),
                now,
                now,
                id
            ],
        )?;
    } else {
        tx.execute(
            "UPDATE task_suggestions SET status = 'suggested', reviews = ?, updated_at = ? WHERE id = ?",
            params![reviews, now, id],
        )?;
    }
    tx.event(
        actor,
        "suggestion.unreviewed",
        id,
        pulse_store::Detail::json(json!({"project": cur.project, "job": cur.job_id, "reviews": reviews, "owner": to_owner}))?,
    )?;
    tx.commit()?;
    get(s, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_round_trip() {
        assert_eq!(source("SUG-007"), "suggestion:SUG-007");
        assert_eq!(split_source("suggestion:SUG-007"), Some("SUG-007"));
        assert_eq!(split_source("suggestion:"), None);
        assert_eq!(split_source("suggestion:SUG-1:extra"), None);
        assert_eq!(split_source("task:TASK-001"), None);
        assert!(is_id("SUG-001"));
        assert!(!is_id("TASK-001"));
    }
}
