use crate::tasks::{check_project, get, input};
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The most answers one message offers; a longer list is a form, not a
/// choice.
pub const MAX_CHOICES: usize = 6;
/// The longest label a choice carries, in characters.
pub const MAX_CHOICE_LABEL: usize = 80;
/// What picking a choice does: `reply` sends the label back as the owner's
/// words, `done` closes the task with them, `amend` opens the brief for a
/// change. Anything else is refused when the message is sent.
pub const CHOICE_ACTIONS: &[&str] = &["reply", "done", "amend"];

/// One answer the owner can pick instead of writing. The hub shows it as a
/// button under the message; the CLI prints it as a numbered line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub label: String,
    /// `reply` (the default), `done`, or `amend`.
    #[serde(default = "default_action")]
    pub action: String,
}

fn default_action() -> String {
    "reply".into()
}

impl Choice {
    pub fn reply(label: impl Into<String>) -> Self {
        Choice {
            label: label.into(),
            action: "reply".into(),
        }
    }
    pub fn with_action(label: impl Into<String>, action: impl Into<String>) -> Self {
        Choice {
            label: label.into(),
            action: action.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Message {
    pub id: String,
    pub project: String,
    pub task_id: String,
    pub author: String,
    pub body: String,
    pub at: String,
    pub read_by: Vec<String>,
    /// Answers the owner can pick; empty for a plain message.
    pub choices: Vec<Choice>,
}

const MESSAGE_COLUMNS: &str = "id, project, task_id, author, body, at, read_by, choices";

fn scan_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let task: Option<String> = row.get(2)?;
    let read_by: String = row.get(6)?;
    let choices: String = row.get(7)?;
    Ok(Message {
        id: row.get(0)?,
        project: row.get(1)?,
        task_id: task.unwrap_or_default(),
        author: row.get(3)?,
        body: row.get(4)?,
        at: row.get(5)?,
        read_by: serde_json::from_str(&read_by).unwrap_or_default(),
        choices: parse_choices(&choices),
    })
}

/// Reads the stored `choices` column; anything unreadable counts as none.
pub fn parse_choices(text: &str) -> Vec<Choice> {
    serde_json::from_str(text).unwrap_or_default()
}

/// Turns `--choice` values into choices: `label` or `label=action`, where the
/// action is one of `reply`, `done`, `amend`. A label alone is a reply.
pub fn choices_from_flags(values: &[String]) -> Result<Vec<Choice>> {
    let mut out = Vec::new();
    for v in values {
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        let (label, action) = match v.rsplit_once('=') {
            Some((l, a)) if CHOICE_ACTIONS.contains(&a.trim()) => (l.trim(), a.trim()),
            _ => (v, "reply"),
        };
        out.push(Choice::with_action(label, action));
    }
    check_choices(&out)?;
    Ok(out)
}

/// Refuses choices the hub could not show: too many, an empty or overlong
/// label, a repeated label, an unknown action.
pub fn check_choices(choices: &[Choice]) -> Result<()> {
    if choices.len() > MAX_CHOICES {
        return Err(input(
            2,
            format!("{} choices is too many", choices.len()),
            format!("a message offers at most {MAX_CHOICES}; more is a form, not a choice"),
            "keep the two to four answers the owner is most likely to pick",
        ));
    }
    let mut seen = Vec::new();
    for c in choices {
        let label = c.label.trim();
        if label.is_empty() {
            return Err(input(
                2,
                "A choice needs a label",
                "the owner picks the label; an empty one shows nothing",
                "--choice 'Try again another way'",
            ));
        }
        if label.chars().count() > MAX_CHOICE_LABEL {
            return Err(input(
                2,
                format!("Choice {label:?} is too long"),
                format!("a label fits a button: at most {MAX_CHOICE_LABEL} characters"),
                "put the detail in the message body and keep the label short",
            ));
        }
        if !CHOICE_ACTIONS.contains(&c.action.as_str()) {
            return Err(input(
                2,
                format!("Unknown choice action {:?}", c.action),
                format!("actions are {}", CHOICE_ACTIONS.join(", ")),
                format!("--choice '{label}=reply'"),
            ));
        }
        let key = label.to_lowercase();
        if seen.contains(&key) {
            return Err(input(
                2,
                format!("Choice {label:?} is offered twice"),
                "each answer appears once",
                "drop the repeated --choice",
            ));
        }
        seen.push(key);
    }
    Ok(())
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
    send_message_with_choices(s, project, task_id, author, body, &[])
}

/// Sends a message that may offer the owner answers to pick. When the owner
/// writes on a task that waits in `review`, the answer ends the review: the
/// task goes back to `in_progress` and the lead carries on with it.
pub fn send_message_with_choices(
    s: &pulse_store::Store,
    project: &str,
    task_id: &str,
    author: &str,
    body: &str,
    choices: &[Choice],
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
    check_choices(choices)?;
    if !choices.is_empty() && task_id.is_empty() {
        return Err(input(
            2,
            "Choices belong to a task",
            "picking one answers the task's lead",
            "add --task TASK-001",
        ));
    }
    let mut project = project.to_string();
    let mut task = None;
    if !task_id.is_empty() {
        let (t, _) = get(s, task_id)?;
        if project.is_empty() {
            project = t.project.clone();
        } else if t.project != project {
            return Err(input(
                2,
                format!("{task_id} belongs to {}, not {project}", t.project),
                "a message about a task stays in the task's project",
                format!("drop --project or use --project {}", t.project),
            ));
        }
        task = Some(t);
    }
    check_project(s, &project)?;
    let choices = choices
        .iter()
        .map(|c| Choice::with_action(c.label.trim(), c.action.as_str()))
        .collect::<Vec<_>>();
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
        "INSERT INTO messages (id, project, task_id, author, body, at, read_by, choices) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            id,
            project,
            task_value,
            author,
            body,
            now,
            format!("[\"{author}\"]"),
            pulse_store::gojson::to_string(&choices)?
        ],
    )?;
    tx.event(
        author,
        "message.sent",
        &id,
        pulse_store::Detail::json(
            json!({"project": project, "task_id": task_id, "author": author}),
        )?,
    )?;
    // The owner's words end a review: the lead asked, the owner answered, the
    // task is in progress again until the lead finishes or asks once more.
    if let Some(t) = &task
        && author == pulse_store::HUMAN_AGENT
        && t.status == "review"
    {
        tx.execute(
            "UPDATE tasks SET status = 'in_progress', updated_at = ? WHERE id = ?",
            params![now, t.id],
        )?;
        tx.event(
            author,
            "task.status",
            &t.id,
            pulse_store::Detail::json(json!({
                "project": project,
                "from": "review",
                "to": "in_progress",
                "message": id,
            }))?,
        )?;
    }
    tx.commit()?;
    get_message(s, &id)
}

pub fn reply(s: &pulse_store::Store, task_id: &str, author: &str, body: &str) -> Result<Message> {
    send_message(s, "", task_id, author, body)
}

pub fn reply_with_choices(
    s: &pulse_store::Store,
    task_id: &str,
    author: &str,
    body: &str,
    choices: &[Choice],
) -> Result<Message> {
    send_message_with_choices(s, "", task_id, author, body, choices)
}

pub fn get_message(s: &pulse_store::Store, id: &str) -> Result<Message> {
    let conn = s.conn()?;
    conn.query_row(
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?"),
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
    let mut q = format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE 1=1");
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
