use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::json;

pub const STATUSES: &[&str] = &[
    "open",
    "planned",
    "in_progress",
    "review",
    "done",
    "cancelled",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Task {
    pub id: String,
    pub project: String,
    pub title: String,
    pub status: String,
    pub priority: i32,
    pub lead: String,
    pub job_id: String,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    pub current_rev: i32,
    pub brief: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Revision {
    pub rev: i32,
    pub title: String,
    pub brief: String,
    pub note: String,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub agent: String,
    pub branch: String,
    pub priority: i32,
    pub created_at: String,
    pub finished_at: String,
    pub result: String,
}

#[derive(Debug)]
pub struct ErrNotFound;

impl std::fmt::Display for ErrNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not found")
    }
}

impl std::error::Error for ErrNotFound {}

pub(crate) const TASK_COLS: &str = "t.id, t.project, t.title, t.status, t.priority, t.lead, t.job_id, t.created_by, t.created_at, t.updated_at, t.current_rev, COALESCE(r.brief, '')";
pub(crate) const TASK_FROM: &str =
    "FROM tasks t LEFT JOIN task_revisions r ON r.task_id = t.id AND r.rev = t.current_rev";

pub(crate) fn scan_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        project: row.get(1)?,
        title: row.get(2)?,
        status: row.get(3)?,
        priority: row.get(4)?,
        lead: row.get(5)?,
        job_id: row.get(6)?,
        created_by: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        current_rev: row.get(10)?,
        brief: row.get(11)?,
    })
}

pub(crate) fn input(
    code: i32,
    what: impl AsRef<str>,
    why: impl AsRef<str>,
    fix: impl AsRef<str>,
) -> anyhow::Error {
    pulse_cli::errorf(code, what.as_ref(), why.as_ref(), fix.as_ref()).into()
}

pub(crate) fn check_project(s: &pulse_store::Store, project: &str) -> Result<()> {
    if project.is_empty() {
        return Err(input(
            2,
            "A project is needed",
            "tasks, messages and compute requests belong to one project",
            "pass --project SLUG (see pulse projects)",
        ));
    }
    if project == pulse_store::COMMONS_SLUG {
        return Ok(());
    }
    if s.project(project)?.is_none() {
        return Err(input(
            2,
            format!("No project {project:?}"),
            "it is not registered in this store",
            "pulse projects",
        ));
    }
    Ok(())
}

pub(crate) fn next_id(conn: &rusqlite::Connection, table: &str, prefix: &str) -> Result<String> {
    let q = format!(
        "SELECT MAX(CAST(substr(id, {}) AS INTEGER)) FROM {table} WHERE id LIKE ?",
        prefix.len() + 1
    );
    let n: Option<i64> = conn.query_row(&q, [format!("{prefix}%")], |r| r.get(0))?;
    Ok(format!("{prefix}{:03}", n.unwrap_or(0) + 1))
}

pub(crate) fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn new(
    s: &pulse_store::Store,
    project: &str,
    title: &str,
    brief: &str,
    priority: i32,
    author: &str,
) -> Result<Task> {
    let title = clean(title);
    if title.is_empty() {
        return Err(input(
            2,
            "A task needs a title",
            "the title is what the board and the lead see first",
            "pulse task new --project P --title 'what you want' [--brief-file F]",
        ));
    }
    if title.len() > 200 {
        return Err(input(
            2,
            format!("title is {} chars; limit 200", title.len()),
            "keep the title short",
            "move detail into the brief (--brief-file F or stdin)",
        ));
    }
    if author.is_empty() {
        return Err(input(
            2,
            "An author is needed",
            "every task records who gave it",
            format!(
                "pass --agent NAME or set PULSE_AGENT (the owner is {})",
                pulse_store::HUMAN_AGENT
            ),
        ));
    }
    check_project(s, project)?;
    let now = s.now();
    let tx = s.begin()?;
    let id = next_id(&tx, "tasks", "TASK-")?;
    tx.execute("INSERT INTO tasks (id, project, title, status, priority, created_by, created_at, updated_at, current_rev) VALUES (?, ?, ?, 'open', ?, ?, ?, ?, 1)",
        params![id, project, title, priority, author, now, now])?;
    tx.execute(
        "INSERT INTO task_revisions (task_id, rev, title, brief, note, author, at) VALUES (?, 1, ?, ?, '', ?, ?)",
        params![id, title, brief, author, now],
    )?;
    tx.event(
        author,
        "task.created",
        &id,
        pulse_store::Detail::json(
            json!({"project": project, "title": title, "priority": priority}),
        )?,
    )?;
    tx.commit()?;
    Ok(get(s, &id)?.0)
}

pub fn get(s: &pulse_store::Store, id: &str) -> Result<(Task, Vec<Revision>)> {
    let conn = s.conn()?;
    let q = format!("SELECT {TASK_COLS} {TASK_FROM} WHERE t.id = ?");
    let t = conn
        .query_row(&q, [id], scan_task)
        .optional()?
        .ok_or_else(|| {
            input(
                2,
                format!("No task {id:?}"),
                "the id is not in this store",
                "pulse task list",
            )
        })?;
    let mut stmt = conn.prepare("SELECT rev, title, brief, note, author, at FROM task_revisions WHERE task_id = ? ORDER BY rev")?;
    let revs = stmt
        .query_map([id], |r| {
            Ok(Revision {
                rev: r.get(0)?,
                title: r.get(1)?,
                brief: r.get(2)?,
                note: r.get(3)?,
                author: r.get(4)?,
                at: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((t, revs))
}

pub fn list(s: &pulse_store::Store, project: &str, status: &str) -> Result<Vec<Task>> {
    let mut q = format!("SELECT {TASK_COLS} {TASK_FROM} WHERE 1=1");
    let mut args = Vec::new();
    if !project.is_empty() {
        q.push_str(" AND t.project = ?");
        args.push(project.to_string());
    }
    if !status.is_empty() {
        q.push_str(" AND t.status = ?");
        args.push(status.to_string());
    }
    q.push_str(" ORDER BY t.id DESC");
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    Ok(stmt
        .query_map(rusqlite::params_from_iter(refs), scan_task)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn jobs(s: &pulse_store::Store, id: &str) -> Result<Vec<Job>> {
    let conn = s.conn()?;
    let (exact, like) = pulse_store::task_source_params(id);
    let mut stmt = conn.prepare(&format!(
        "SELECT id, kind, title, status, lease_agent, branch, priority, created_at, finished_at, result FROM jobs WHERE {} ORDER BY created_at",
        pulse_store::task_source_clause("source")
    ))?;
    Ok(stmt
        .query_map([exact, like], |r| {
            Ok(Job {
                id: r.get(0)?,
                kind: r.get(1)?,
                title: r.get(2)?,
                status: r.get(3)?,
                agent: r.get(4)?,
                branch: r.get(5)?,
                priority: r.get(6)?,
                created_at: r.get(7)?,
                finished_at: r.get(8)?,
                result: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn terminal(status: &str) -> bool {
    status == "done" || status == "cancelled"
}

pub fn amend(
    s: &pulse_store::Store,
    id: &str,
    title: &str,
    brief: &str,
    note: &str,
    author: &str,
) -> Result<Task> {
    if author.is_empty() {
        return Err(input(
            2,
            "An author is needed",
            "every revision records who wrote it",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    let (cur, revs) = get(s, id)?;
    if terminal(&cur.status) {
        return Err(input(
            2,
            format!("{id} is {} and cannot be amended", cur.status),
            "finished work is history",
            format!(
                "pulse task new --project {} --title '...' (open a new task)",
                cur.project
            ),
        ));
    }
    let mut title = clean(title);
    if title.is_empty() {
        title = cur.title.clone();
    }
    if title.len() > 200 {
        return Err(input(
            2,
            format!("title is {} chars; limit 200", title.len()),
            "keep the title short",
            "move detail into the brief",
        ));
    }
    let last = revs.last().cloned().unwrap_or_default();
    let brief = if brief.is_empty() {
        last.brief
    } else {
        brief.to_string()
    };
    if title == cur.title && brief == cur.brief && clean(note).is_empty() {
        return Err(input(
            2,
            "Nothing changed",
            "the title and brief equal the current revision",
            "pass --title, --brief-file/stdin or --note",
        ));
    }
    let now = s.now();
    let tx = s.begin()?;
    let rev = cur.current_rev + 1;
    tx.execute("INSERT INTO task_revisions (task_id, rev, title, brief, note, author, at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![id, rev, title, brief, note, author, now])?;
    tx.execute(
        "UPDATE tasks SET title = ?, current_rev = ?, updated_at = ? WHERE id = ?",
        params![title, rev, now, id],
    )?;
    let (exact, like) = pulse_store::task_source_params(id);
    let replanned = tx.execute(
        &format!(
            "UPDATE jobs SET brief = brief || ? WHERE {} AND status = 'queued'",
            pulse_store::task_source_clause("source")
        ),
        params![
            format!(
                "\n\n[task amended: rev {rev} - re-read `pulse task show {id}` before starting]"
            ),
            exact,
            like
        ],
    )?;
    tx.event(
        author,
        "task.amended",
        id,
        pulse_store::Detail::json(
            json!({"project": cur.project, "rev": rev, "note": note, "replanned_jobs": replanned}),
        )?,
    )?;
    tx.commit()?;
    Ok(get(s, id)?.0)
}

pub fn cancel(s: &pulse_store::Store, id: &str, reason: &str, actor: &str) -> Result<Task> {
    if actor.is_empty() {
        return Err(input(
            2,
            "An actor is needed",
            "every cancellation records who asked",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    let (cur, _) = get(s, id)?;
    if terminal(&cur.status) {
        return Err(input(
            2,
            format!("{id} is already {}", cur.status),
            "a finished task does not change",
            format!("pulse task show {id}"),
        ));
    }
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE tasks SET status = 'cancelled', updated_at = ? WHERE id = ?",
        params![now, id],
    )?;
    let (exact, like) = pulse_store::task_source_params(id);
    let clause = pulse_store::task_source_clause("source");
    let cancelled = tx.execute(
        &format!("UPDATE jobs SET status = 'cancelled', finished_at = ?, result = ? WHERE {clause} AND status = 'queued'"),
        params![now, format!("task cancelled: {reason}"), exact, like])?;
    let leased: i64 = tx.query_row(
        &format!("SELECT COUNT(*) FROM jobs WHERE {clause} AND status = 'leased'"),
        [exact, like],
        |r| r.get(0),
    )?;
    tx.event(actor, "task.cancelled", id, pulse_store::Detail::json(json!({"project": cur.project, "reason": reason, "cancelled_jobs": cancelled, "leased_jobs": leased, "lead": cur.lead}))?)?;
    tx.commit()?;
    Ok(get(s, id)?.0)
}

pub fn status_index(status: &str) -> isize {
    STATUSES
        .iter()
        .position(|s| *s == status)
        .map_or(-1, |i| i as isize)
}

pub fn set_status(s: &pulse_store::Store, id: &str, status: &str, actor: &str) -> Result<Task> {
    if actor.is_empty() {
        return Err(input(
            2,
            "An actor is needed",
            "every status change records who made it",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    let to = status_index(status);
    if to < 0 {
        return Err(input(
            2,
            format!("Unknown status {status:?}"),
            format!("statuses are {}", STATUSES.join(", ")),
            format!("pulse task status {id} <status>"),
        ));
    }
    if status == "cancelled" {
        return cancel(s, id, "", actor);
    }
    let (cur, _) = get(s, id)?;
    let from = status_index(&cur.status);
    if terminal(&cur.status) {
        return Err(input(
            2,
            format!("{id} is {}", cur.status),
            "a finished task does not change",
            format!("pulse task new --project {} --title '...'", cur.project),
        ));
    }
    // One step back is allowed: a task waiting in review goes on when the
    // owner answers or the lead picks the work up again.
    let back_to_work = cur.status == "review" && status == "in_progress";
    if to <= from && !back_to_work {
        return Err(input(
            2,
            format!("{id} is {}; {status} is not a step forward", cur.status),
            format!(
                "tasks move {} or to cancelled; only review goes back, to in_progress",
                STATUSES[..5].join(" -> ")
            ),
            format!("pulse task status {id} {}", STATUSES[from as usize + 1]),
        ));
    }
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?",
        params![status, now, id],
    )?;
    tx.event(
        actor,
        "task.status",
        id,
        pulse_store::Detail::json(
            json!({"project": cur.project, "from": cur.status, "to": status}),
        )?,
    )?;
    tx.commit()?;
    Ok(get(s, id)?.0)
}

pub fn set_lead(
    s: &pulse_store::Store,
    id: &str,
    lead: &str,
    job_id: &str,
    actor: &str,
) -> Result<Task> {
    let (cur, _) = get(s, id)?;
    let now = s.now();
    let tx = s.begin()?;
    tx.execute("UPDATE tasks SET lead = ?, job_id = CASE WHEN ? = '' THEN job_id ELSE ? END, updated_at = ? WHERE id = ?", params![lead, job_id, job_id, now, id])?;
    tx.event(
        actor,
        "task.lead",
        id,
        pulse_store::Detail::json(json!({"project": cur.project, "lead": lead, "job_id": job_id}))?,
    )?;
    tx.commit()?;
    Ok(get(s, id)?.0)
}
