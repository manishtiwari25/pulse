use crate::tasks::{check_project, get, input};
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Message {
    pub id: String,
    pub project: String,
    pub task_id: String,
    pub author: String,
    pub body: String,
    pub at: String,
    pub read_by: Vec<String>,
}

fn scan_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let task: Option<String> = row.get(2)?;
    let read_by: String = row.get(6)?;
    Ok(Message {
        id: row.get(0)?,
        project: row.get(1)?,
        task_id: task.unwrap_or_default(),
        author: row.get(3)?,
        body: row.get(4)?,
        at: row.get(5)?,
        read_by: serde_json::from_str(&read_by).unwrap_or_default(),
    })
}

pub(crate) fn stamp(iso: &str) -> String {
    iso.replace(['-', ':'], "")
}

pub fn send_message(
    s: &pulse_store::Store,
    project: &str,
    task_id: &str,
    author: &str,
    body: &str,
) -> Result<Message> {
    let body = body.trim();
    if body.is_empty() {
        return Err(input(
            2,
            "An empty message says nothing",
            "the body is required",
            "pulse msg send --project P 'what you want to say'",
        ));
    }
    if author.is_empty() {
        return Err(input(
            2,
            "An author is needed",
            "every message records who wrote it",
            format!(
                "pass --agent NAME or set PULSE_AGENT (the owner is {})",
                pulse_store::HUMAN_AGENT
            ),
        ));
    }
    let mut project = project.to_string();
    if !task_id.is_empty() {
        let (t, _) = get(s, task_id)?;
        if project.is_empty() {
            project = t.project;
        } else if t.project != project {
            return Err(input(
                2,
                format!("{task_id} belongs to {}, not {project}", t.project),
                "a message about a task stays in the task's project",
                format!("drop --project or use --project {}", t.project),
            ));
        }
    }
    check_project(s, &project)?;
    let now = s.now();
    let tx = s.begin()?;
    let base = format!("MSG-{}-", stamp(&now));
    let q = format!(
        "SELECT MAX(CAST(substr(id, {}) AS INTEGER)) FROM messages WHERE id LIKE ?",
        base.len() + 1
    );
    let n: Option<i64> = tx.query_row(&q, [format!("{base}%")], |r| r.get(0))?;
    let id = format!("{}{}", base, n.unwrap_or(0) + 1);
    let task_value: Option<&str> = if task_id.is_empty() {
        None
    } else {
        Some(task_id)
    };
    tx.execute(
        "INSERT INTO messages (id, project, task_id, author, body, at, read_by) VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![id, project, task_value, author, body, now, format!("[\"{author}\"]")],
    )?;
    tx.event(
        author,
        "message.sent",
        &id,
        pulse_store::Detail::json(
            json!({"project": project, "task_id": task_id, "author": author}),
        )?,
    )?;
    tx.commit()?;
    get_message(s, &id)
}

pub fn reply(s: &pulse_store::Store, task_id: &str, author: &str, body: &str) -> Result<Message> {
    send_message(s, "", task_id, author, body)
}

pub fn get_message(s: &pulse_store::Store, id: &str) -> Result<Message> {
    let conn = s.conn()?;
    conn.query_row(
        "SELECT id, project, task_id, author, body, at, read_by FROM messages WHERE id = ?",
        [id],
        scan_message,
    )
    .optional()?
    .ok_or_else(|| {
        input(
            2,
            format!("No message {id:?}"),
            "the id is not in this store",
            "pulse msg list",
        )
    })
}

pub fn messages(
    s: &pulse_store::Store,
    project: &str,
    task_id: &str,
    since_id: &str,
    unread_for: &str,
) -> Result<Vec<Message>> {
    let mut q = "SELECT id, project, task_id, author, body, at, read_by FROM messages WHERE 1=1"
        .to_string();
    let mut args = Vec::new();
    if !project.is_empty() {
        q.push_str(" AND project = ?");
        args.push(project.to_string());
    }
    if !task_id.is_empty() {
        q.push_str(" AND task_id = ?");
        args.push(task_id.to_string());
    }
    if !since_id.is_empty() {
        q.push_str(" AND id > ?");
        args.push(since_id.to_string());
    }
    q.push_str(" ORDER BY id");
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let mut out = Vec::new();
    for row in stmt.query_map(rusqlite::params_from_iter(refs), scan_message)? {
        let m = row?;
        if !unread_for.is_empty() && contains(&m.read_by, unread_for) {
            continue;
        }
        out.push(m);
    }
    Ok(out)
}

pub(crate) fn contains(list: &[String], v: &str) -> bool {
    list.iter().any(|x| x == v)
}

pub fn mark_read(s: &pulse_store::Store, ids: &[String], agent: &str) -> Result<i32> {
    if agent.is_empty() {
        return Err(input(
            2,
            "An agent is needed",
            "read marks are per agent",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    if ids.is_empty() {
        return Ok(0);
    }
    let tx = s.begin()?;
    let mut marked = 0_i32;
    let mut project = String::new();
    for id in ids {
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT project, read_by FROM messages WHERE id = ?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((proj, read_by)) = row else {
            continue;
        };
        let mut list: Vec<String> = serde_json::from_str(&read_by).unwrap_or_default();
        if contains(&list, agent) {
            continue;
        }
        list.push(agent.to_string());
        list.sort();
        let text = pulse_store::gojson::to_string(&list)?;
        tx.execute(
            "UPDATE messages SET read_by = ? WHERE id = ?",
            params![text, id],
        )?;
        marked += 1;
        project = proj;
    }
    if marked > 0 {
        tx.event(
            agent,
            "message.read",
            &ids.join(","),
            pulse_store::Detail::json(json!({"project": project, "count": marked}))?,
        )?;
    }
    tx.commit()?;
    Ok(marked)
}
