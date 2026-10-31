//! The task groups the Chat page lists beside the owner's own
//! conversations. A task is a group of conversations: the owner's thread
//! with the project's lead agent, and one agent session per step (job) the
//! lead planned. The page shows every conversation PULSE holds, grouped by
//! the task it serves, so giving a task and talking to one agent start
//! from the same place and read the same way.
use anyhow::Result;
use pulse_store::Store;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

/// How many tasks a list returns.
const TASK_LIMIT: i64 = 200;
/// A finished or cancelled task stays in the list this long after its last
/// change; `all` lists every task.
const KEEP_FINISHED_DAYS: i64 = 7;

/// The tasks the Chat page groups conversations under: every task still in
/// progress, plus those finished in the last week (all of them with
/// `all`), newest change first. An older store without the task tables
/// yields none.
pub(crate) fn task_groups(store: &Store, all: bool) -> Result<Vec<Value>> {
    let conn = store.conn()?;
    for table in ["tasks", "messages", "jobs", "harness_sessions"] {
        let n: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
            [table],
            |r| r.get(0),
        )?;
        if n == 0 {
            return Ok(Vec::new());
        }
    }
    let since = if all {
        String::new()
    } else {
        keep_since(&store.now())
    };
    let mut stmt = conn.prepare(
        "SELECT id, project, title, status, priority, lead, created_by, created_at, updated_at \
         FROM tasks \
         WHERE ?1 = '' OR status IN ('open','planned','in_progress','review') OR updated_at >= ?1 \
         ORDER BY updated_at DESC, id DESC LIMIT ?2",
    )?;
    let tasks = stmt
        .query_map(params![since, TASK_LIMIT], |r| {
            Ok(TaskRow {
                id: r.get(0)?,
                project: r.get(1)?,
                title: r.get(2)?,
                status: r.get(3)?,
                priority: r.get(4)?,
                lead: r.get(5)?,
                created_by: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::with_capacity(tasks.len());
    for t in tasks {
        out.push(group_json(&conn, &t)?);
    }
    Ok(out)
}

struct TaskRow {
    id: String,
    project: String,
    title: String,
    status: String,
    priority: i64,
    lead: String,
    created_by: String,
    created_at: String,
    updated_at: String,
}

fn group_json(conn: &Connection, t: &TaskRow) -> Result<Value> {
    let (messages, last_message_at, last_author): (i64, String, String) = conn.query_row(
        "SELECT count(*), coalesce(max(at), ''), \
         coalesce((SELECT author FROM messages WHERE task_id = ?1 ORDER BY at DESC, rowid DESC LIMIT 1), '') \
         FROM messages WHERE task_id = ?1",
        [&t.id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let (exact, like) = pulse_store::task_source_params(&t.id);
    let mut stmt = conn.prepare(&format!(
        "SELECT id, kind, title, status, coalesce(nullif(lease_agent, ''), agent), created_at, coalesce(finished_at, ''), \
         coalesce((SELECT hs.id FROM harness_sessions hs WHERE hs.job_id = jobs.id ORDER BY hs.started_at DESC LIMIT 1), '') \
         FROM jobs WHERE project = ? AND {} ORDER BY created_at, id",
        pulse_store::task_source_clause("source")
    ))?;
    let jobs = stmt
        .query_map(params![t.project, exact, like], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut last_at = t.updated_at.clone();
    if last_message_at > last_at {
        last_at = last_message_at.clone();
    }
    let mut sessions = Vec::with_capacity(jobs.len());
    for (job, kind, title, job_status, agent, created_at, finished_at, session) in jobs {
        let mut item = json!({
            "job": job,
            "kind": kind,
            "title": title,
            "job_status": job_status,
            "agent": agent,
            "session": session,
            "status": "",
            "started_at": created_at,
            "updated_at": if finished_at.is_empty() { created_at.clone() } else { finished_at.clone() },
            "ended_at": finished_at,
            "cost": 0.0,
            "steps": 0,
        });
        if !session.is_empty() {
            let row: Option<(String, String, String, String, String, f64, i64)> = conn
                .query_row(
                    "SELECT status, agent, started_at, coalesce(updated_at, started_at), coalesce(ended_at, ''), cost, steps \
                     FROM harness_sessions WHERE id = ?",
                    [&session],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((status, agent, started_at, updated_at, ended_at, cost, steps)) = row {
                let o = item.as_object_mut().expect("object");
                o.insert("status".into(), json!(status));
                if o.get("agent")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .is_empty()
                {
                    o.insert("agent".into(), json!(agent));
                }
                o.insert("started_at".into(), json!(started_at));
                o.insert("updated_at".into(), json!(updated_at));
                o.insert("ended_at".into(), json!(ended_at));
                o.insert("cost".into(), json!(cost));
                o.insert("steps".into(), json!(steps));
            }
        }
        if let Some(u) = item.get("updated_at").and_then(Value::as_str)
            && u > last_at.as_str()
        {
            last_at = u.to_string();
        }
        sessions.push(item);
    }
    Ok(json!({
        "id": t.id,
        "project": t.project,
        "title": t.title,
        "status": t.status,
        "priority": t.priority,
        "lead": t.lead,
        "created_by": t.created_by,
        "created_at": t.created_at,
        "updated_at": t.updated_at,
        "last_at": last_at,
        "messages": messages,
        "last_message_at": last_message_at,
        "last_author": last_author,
        "needs_you": t.status == "review",
        "sessions": sessions,
    }))
}

/// The oldest `updated_at` a finished task may have and still be listed.
fn keep_since(now: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(now)
        .map(|t| {
            (t.with_timezone(&chrono::Utc) - chrono::Duration::days(KEEP_FINISHED_DAYS))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_since_is_a_week_before_now_and_empty_for_a_bad_clock() {
        assert_eq!(keep_since("2026-10-31T13:00:00Z"), "2026-10-24T13:00:00Z");
        assert_eq!(keep_since("not a time"), "");
    }
}
