//! The story of a task in order: what the owner wrote, what the lead and its
//! workers did step by step, what PULSE landed. The hub's task sheet reads
//! it as a chat-like feed that follows a run while it works.

use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// One line of the feed. `kind` says where it came from; `what` is the finer
/// word a reader groups by. The `id` is stable across fetches so the hub can
/// merge a live refresh into what it already shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ActivityItem {
    pub id: String,
    pub at: String,
    /// `task`, `message`, `job`, `run`, `step`, `quiet`, `land` or `post`.
    pub kind: String,
    /// For a task: created, revised, status, lead, cancelled, followup,
    /// nudged. For a message: sent, or report when a worker's finished job
    /// reported back. For a job: added. For a run: started, ended. For a
    /// step: the harness step kind. For a landing: the word after `land.`.
    /// For a post: its kind.
    pub what: String,
    /// Who did it: the owner, a lead, a worker, the daemon.
    pub who: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub job_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    /// One line: a command's description, a file path, a title.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The longer text: an agent's words, a message, a note, a summary.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// A status word: a run's end state, a task's new status, a post's
    /// resolution, a harness exit.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "is_zero")]
    pub duration_ms: i64,
    /// How many steps a `quiet` line folds; how many commits a run ended with.
    #[serde(skip_serializing_if = "is_zero")]
    pub count: i64,
    /// Answers a message offered the owner.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// The record behind the line: a message id, a post id.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reference: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// A chapter of the feed: one run of one of the task's jobs, with the
/// counts a collapsed chapter shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ActivityRun {
    pub id: String,
    pub job_id: String,
    pub job_title: String,
    pub agent: String,
    pub executor: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub started_at: String,
    pub finished_at: String,
    pub summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    /// Model calls, commands run, files edited or written, steps in all.
    pub calls: i64,
    pub commands: i64,
    pub edits: i64,
    pub steps: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TaskActivity {
    pub task_id: String,
    pub items: Vec<ActivityItem>,
    pub runs: Vec<ActivityRun>,
    /// Lines before the limit cut the oldest away.
    pub total: usize,
    pub has_more: bool,
    /// A run of this task is live right now.
    pub running: bool,
    pub now: String,
}

/// How many lines a fetch returns when the caller names no limit.
pub const ACTIVITY_DEFAULT_LIMIT: usize = 400;

const TEXT_CAP: usize = 2000;
const LINE_CAP: usize = 200;
const COMMAND_CAP: usize = 400;

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Step kinds the feed shows one by one. Everything else folds into a
/// `quiet` line ("read ×3 · grep ×1") so the agent's words and the commands
/// it ran stay readable.
fn loud(kind: &str) -> bool {
    matches!(
        kind,
        "assistant"
            | "command"
            | "edit"
            | "write"
            | "plan"
            | "error"
            | "exit"
            | "subagent"
            | "mcp"
            | "fetch"
            | "search"
            | "skill"
    )
}

/// Tool steps whose outcome arrives as the next observation.
fn tool_step(kind: &str) -> bool {
    matches!(
        kind,
        "command"
            | "read"
            | "grep"
            | "glob"
            | "edit"
            | "write"
            | "process"
            | "mcp"
            | "fetch"
            | "search"
            | "skill"
            | "subagent"
            | "memory"
    )
}

/// Cuts `text` at `cap` characters, on a character boundary; the flag says
/// whether anything was cut.
pub fn clip(text: &str, cap: usize) -> (String, bool) {
    let text = text.trim_end();
    if text.chars().count() <= cap {
        return (text.to_string(), false);
    }
    let mut out: String = text.chars().take(cap).collect();
    out.push('…');
    (out, true)
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

/// Reads one item for a harness step; `outcome` is the observation that
/// followed it, when any.
fn step_item(
    s: &pulse_store::HarnessSession,
    step: &pulse_store::HarnessStep,
    outcome: Option<&pulse_store::HarnessStep>,
) -> ActivityItem {
    let mut item = ActivityItem {
        id: format!("step:{}:{}", s.id, step.n),
        at: step.at.clone(),
        kind: "step".into(),
        what: step.kind.clone(),
        who: s.agent.clone(),
        job_id: s.job_id.clone(),
        run_id: s.run_id.clone(),
        session_id: s.id.clone(),
        ..ActivityItem::default()
    };
    if let Some(o) = outcome {
        item.exit_code = o.exit_code;
        item.duration_ms = o.duration_ms;
    }
    match step.kind.as_str() {
        "assistant" => {
            let (text, cut) = clip(step.content.trim(), TEXT_CAP);
            item.text = text;
            item.truncated = cut;
        }
        "command" => {
            let line = if step.title.trim().is_empty() {
                first_line(&step.command)
            } else {
                step.title.trim()
            };
            item.title = clip(line, LINE_CAP).0;
            let (text, cut) = clip(step.command.trim(), COMMAND_CAP);
            item.text = text;
            item.truncated = cut;
        }
        "edit" | "write" => {
            item.title = clip(first_line(&step.content), LINE_CAP).0;
        }
        "plan" => {
            let (text, cut) = clip(step.content.trim(), TEXT_CAP);
            item.text = text;
            item.truncated = cut;
        }
        "error" => {
            let (text, cut) = clip(step.content.trim(), TEXT_CAP / 2);
            item.text = text;
            item.truncated = cut;
            item.status = "error".into();
        }
        "exit" => {
            item.status = first_line(&step.content).to_string();
            item.title = item.status.clone();
        }
        _ => {
            let line = if step.content.trim().is_empty() {
                first_line(&step.command)
            } else {
                first_line(&step.content)
            };
            item.title = clip(line, LINE_CAP).0;
        }
    }
    item
}

/// What a folded stretch of quiet steps says: kinds in order of first
/// appearance with their counts.
fn quiet_title(kinds: &[String]) -> String {
    let mut order: Vec<&str> = Vec::new();
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for k in kinds {
        let k = k.as_str();
        if !counts.contains_key(k) {
            order.push(k);
        }
        *counts.entry(k).or_insert(0) += 1;
    }
    order
        .iter()
        .map(|k| {
            let n = counts[k];
            if n == 1 {
                (*k).to_string()
            } else {
                format!("{k} ×{n}")
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[derive(Default)]
struct StepCounts {
    calls: i64,
    commands: i64,
    edits: i64,
}

/// Turns one session's steps into feed lines: loud steps one by one with
/// the outcome of their observation attached, quiet stretches folded.
fn fold_steps(
    s: &pulse_store::HarnessSession,
    steps: &[pulse_store::HarnessStep],
) -> (Vec<ActivityItem>, StepCounts) {
    let mut out = Vec::new();
    let mut counts = StepCounts::default();
    let mut quiet: Vec<String> = Vec::new();
    let mut quiet_at = String::new();
    let mut quiet_n = 0;
    let flush = |quiet: &mut Vec<String>,
                 quiet_at: &mut String,
                 quiet_n: i64,
                 out: &mut Vec<ActivityItem>| {
        if quiet.is_empty() {
            return;
        }
        out.push(ActivityItem {
            id: format!("quiet:{}:{}", s.id, quiet_n),
            at: std::mem::take(quiet_at),
            kind: "quiet".into(),
            what: "quiet".into(),
            who: s.agent.clone(),
            job_id: s.job_id.clone(),
            run_id: s.run_id.clone(),
            session_id: s.id.clone(),
            title: quiet_title(quiet),
            count: quiet.len() as i64,
            ..ActivityItem::default()
        });
        quiet.clear();
    };
    let mut i = 0;
    while i < steps.len() {
        let step = &steps[i];
        let outcome = if tool_step(&step.kind) {
            steps
                .get(i + 1)
                .filter(|n| n.kind == "observation" && n.depth == step.depth)
        } else {
            None
        };
        let consumed = if outcome.is_some() { 2 } else { 1 };
        match step.kind.as_str() {
            "assistant" => counts.calls += 1,
            "command" => counts.commands += 1,
            "edit" | "write" => counts.edits += 1,
            _ => {}
        }
        let shown = step.depth == 0
            && loud(&step.kind)
            && !(step.kind == "assistant" && step.content.trim().is_empty());
        if shown {
            flush(&mut quiet, &mut quiet_at, quiet_n, &mut out);
            out.push(step_item(s, step, outcome));
        } else if step.kind != "observation" && step.kind != "assistant" {
            // A tool-only assistant turn says nothing; an observation is the
            // outcome of the step before it.
            if quiet.is_empty() {
                quiet_at = step.at.clone();
                quiet_n = step.n;
            }
            quiet.push(if step.depth > 0 {
                "subagent step".to_string()
            } else {
                step.kind.clone()
            });
        }
        i += consumed;
    }
    flush(&mut quiet, &mut quiet_at, quiet_n, &mut out);
    (out, counts)
}

/// The order of lines that share a second: what opens a stretch first,
/// what closes it last.
fn rank(item: &ActivityItem) -> u8 {
    match (item.kind.as_str(), item.what.as_str()) {
        ("task", "created") | ("job", _) => 0,
        ("run", "started") => 1,
        ("step", _) | ("quiet", _) => 2,
        ("message", _) | ("post", _) => 3,
        ("run", _) => 4,
        _ => 5,
    }
}

struct JobRow {
    id: String,
    title: String,
    status: String,
    created_at: String,
    created_by: String,
    branch: String,
}

struct RunRow {
    id: String,
    job_id: String,
    agent: String,
    executor: String,
    status: String,
    exit_code: Option<i64>,
    started_at: String,
    finished_at: String,
    summary: String,
    branch: String,
    commits: i64,
}

/// Every line of the task's story, oldest first, cut to the newest `limit`
/// lines when `limit` is above zero.
pub fn activity_for(s: &pulse_store::Store, task_id: &str, limit: usize) -> Result<TaskActivity> {
    let (task, revisions) = crate::tasks::get(s, task_id)?;
    let mut items: Vec<ActivityItem> = Vec::new();

    let (brief, cut) = clip(task.brief.trim(), TEXT_CAP);
    items.push(ActivityItem {
        id: "task:created".into(),
        at: task.created_at.clone(),
        kind: "task".into(),
        what: "created".into(),
        who: task.created_by.clone(),
        title: task.title.clone(),
        text: brief,
        truncated: cut,
        ..ActivityItem::default()
    });
    for rev in revisions.iter().filter(|r| r.rev > 1) {
        let (text, cut) = clip(rev.note.trim(), TEXT_CAP);
        items.push(ActivityItem {
            id: format!("rev:{}", rev.rev),
            at: rev.at.clone(),
            kind: "task".into(),
            what: "revised".into(),
            who: rev.author.clone(),
            title: rev.title.clone(),
            text,
            truncated: cut,
            ..ActivityItem::default()
        });
    }

    for m in crate::messages::messages(s, "", task_id, "", "")? {
        let (text, cut) = clip(m.body.trim(), TEXT_CAP * 2);
        // A worker's report on its finished job reads as a report, with the
        // job it is about, not as words somebody typed.
        let what = if m.is_report() { "report" } else { "sent" };
        items.push(ActivityItem {
            id: format!("msg:{}", m.id),
            at: m.at.clone(),
            kind: "message".into(),
            what: what.into(),
            who: m.author.clone(),
            job_id: m.job_id.clone(),
            text,
            truncated: cut,
            choices: m.choices.iter().map(|c| c.label.clone()).collect(),
            reference: m.id.clone(),
            ..ActivityItem::default()
        });
    }

    let conn = s.conn()?;
    let (exact, like) = pulse_store::task_source_params(task_id);
    // A task's jobs carry `task:ID` or `task:ID:label`, plus its planning job.
    let jobs: Vec<JobRow> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT id, title, status, created_at, created_by, branch FROM jobs \
             WHERE project = ? AND ({} OR (? != '' AND id = ?)) ORDER BY created_at, id",
            pulse_store::task_source_clause("source")
        ))?;
        stmt.query_map(
            rusqlite::params![task.project, exact, like, task.job_id, task.job_id],
            |r| {
                Ok(JobRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    status: r.get(2)?,
                    created_at: r.get(3)?,
                    created_by: r.get(4)?,
                    branch: r.get(5)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut branches: HashSet<String> = HashSet::new();
    let mut job_ids: Vec<String> = Vec::new();
    let mut runs: Vec<ActivityRun> = Vec::new();
    let mut running = false;
    for job in jobs {
        let id = job.id.clone();
        let title = job.title.clone();
        if !job.branch.is_empty() {
            branches.insert(job.branch.clone());
        }
        job_ids.push(id.clone());
        items.push(ActivityItem {
            id: format!("job:add:{id}"),
            at: job.created_at,
            kind: "job".into(),
            what: "added".into(),
            who: job.created_by,
            job_id: id.clone(),
            title: title.clone(),
            status: job.status,
            ..ActivityItem::default()
        });
        let run_rows = {
            let mut stmt = conn.prepare(
                "SELECT id, job_id, agent, executor, status, exit_code, started_at, finished_at, summary, branch, commits \
                 FROM runs WHERE job_id = ? ORDER BY started_at, id",
            )?;
            stmt.query_map([&id], |r| {
                Ok(RunRow {
                    id: r.get(0)?,
                    job_id: r.get(1)?,
                    agent: r.get(2)?,
                    executor: r.get(3)?,
                    status: r.get(4)?,
                    exit_code: r.get(5)?,
                    started_at: r.get(6)?,
                    finished_at: r.get(7)?,
                    summary: r.get(8)?,
                    branch: r.get(9)?,
                    commits: r.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut by_run: BTreeMap<String, usize> = BTreeMap::new();
        for row in run_rows {
            if !row.branch.is_empty() {
                branches.insert(row.branch.clone());
            }
            if row.status == "running" {
                running = true;
            }
            items.push(ActivityItem {
                id: format!("run:start:{}", row.id),
                at: row.started_at.clone(),
                kind: "run".into(),
                what: "started".into(),
                who: row.agent.clone(),
                job_id: row.job_id.clone(),
                run_id: row.id.clone(),
                title: title.clone(),
                ..ActivityItem::default()
            });
            if !row.finished_at.is_empty() {
                let (text, cut) = clip(row.summary.trim(), TEXT_CAP);
                items.push(ActivityItem {
                    id: format!("run:end:{}", row.id),
                    at: row.finished_at.clone(),
                    kind: "run".into(),
                    what: "ended".into(),
                    who: row.agent.clone(),
                    job_id: row.job_id.clone(),
                    run_id: row.id.clone(),
                    title: title.clone(),
                    text,
                    truncated: cut,
                    status: row.status.clone(),
                    exit_code: row.exit_code,
                    count: row.commits,
                    ..ActivityItem::default()
                });
            }
            by_run.insert(row.id.clone(), runs.len());
            runs.push(ActivityRun {
                id: row.id,
                job_id: row.job_id,
                job_title: title.clone(),
                agent: row.agent,
                executor: row.executor,
                status: row.status,
                exit_code: row.exit_code,
                started_at: row.started_at,
                finished_at: row.finished_at,
                summary: clip(row.summary.trim(), TEXT_CAP).0,
                ..ActivityRun::default()
            });
        }
        let mut sessions = s.harness_sessions(pulse_store::HarnessSessionFilter {
            job_id: id.clone(),
            limit: 500,
            ..pulse_store::HarnessSessionFilter::default()
        })?;
        sessions.reverse();
        for session in sessions {
            // `after` -1: the first step is n 0.
            let steps = s.harness_steps(&session.id, -1)?;
            let (lines, counts) = fold_steps(&session, &steps);
            items.extend(lines);
            if let Some(&i) = by_run.get(&session.run_id) {
                let run = &mut runs[i];
                run.session_id = session.id.clone();
                run.calls += counts.calls;
                run.commands += counts.commands;
                run.edits += counts.edits;
                run.steps += steps.len() as i64;
            }
        }
    }

    // The daemon's part: landings of the jobs' branches, the task's own
    // turns of status and lead.
    {
        let mut stmt = conn.prepare(
            "SELECT seq, at, actor, action, subject, detail FROM events \
             WHERE action LIKE 'land.%' AND subject = ? ORDER BY seq",
        )?;
        let mut sorted: Vec<&String> = branches.iter().collect();
        sorted.sort();
        for branch in sorted {
            let rows = stmt
                .query_map([branch], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (seq, at, actor, action, subject, detail) in rows {
                let what = action.trim_start_matches("land.").to_string();
                let job = pulse_bus::detail_field(&detail, "job");
                let text = ["note", "reason", "why"]
                    .iter()
                    .map(|k| pulse_bus::detail_field(&detail, k))
                    .find(|v| !v.is_empty())
                    .unwrap_or_default();
                let status = if what == "pushed" {
                    pulse_bus::detail_field(&detail, "to")
                } else {
                    // The landed commit sits under `code_delivery`.
                    serde_json::from_str::<serde_json::Value>(&detail)
                        .ok()
                        .and_then(|v| {
                            v.pointer("/code_delivery/after")
                                .or_else(|| v.get("after"))
                                .and_then(|a| a.as_str())
                                .map(ToString::to_string)
                        })
                        .unwrap_or_default()
                };
                items.push(ActivityItem {
                    id: format!("land:{seq}"),
                    at,
                    kind: "land".into(),
                    what,
                    who: actor,
                    job_id: job,
                    title: subject,
                    text: clip(&text, TEXT_CAP).0,
                    status,
                    ..ActivityItem::default()
                });
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT seq, at, actor, action, detail FROM events \
             WHERE subject = ? AND action LIKE 'task.%' AND action != 'task.created' ORDER BY seq",
        )?;
        let rows = stmt
            .query_map([task_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (seq, at, actor, action, detail) in rows {
            let what = action.trim_start_matches("task.").to_string();
            let field = |k: &str| pulse_bus::detail_field(&detail, k);
            let mut item = ActivityItem {
                id: format!("task:{seq}"),
                at,
                kind: "task".into(),
                what: what.clone(),
                who: actor,
                ..ActivityItem::default()
            };
            match what.as_str() {
                "status" => {
                    item.title = field("from");
                    item.status = field("to");
                    // An owner's reply moves a review back to in progress and
                    // the event names the message; the message is already a
                    // line of its own, so the id would only repeat it.
                    let message = field("message");
                    if !message.starts_with("MSG-") {
                        item.text = clip(&message, TEXT_CAP).0;
                    }
                }
                "lead" => {
                    item.title = field("lead");
                    item.job_id = field("job_id");
                }
                "cancelled" => {
                    item.text = clip(&field("reason"), TEXT_CAP).0;
                    item.status = "cancelled".into();
                }
                "followup" => {
                    item.job_id = field("job");
                    item.title = field("why");
                }
                "nudged" => {
                    item.reference = field("message");
                    item.title = serde_json::from_str::<serde_json::Value>(&detail)
                        .ok()
                        .and_then(|v| v.get("priority").and_then(|p| p.as_i64()))
                        .map(|p| format!("priority {p}"))
                        .unwrap_or_default();
                }
                _ => {}
            }
            items.push(item);
        }
    }

    // Board posts on the task's thread and on each job's thread.
    {
        let mut threads: Vec<(String, String, String)> = vec![(
            "tasks".into(),
            pulse_board::posts::normalize_thread(task_id),
            String::new(),
        )];
        for id in &job_ids {
            threads.push((
                "jobs".into(),
                pulse_board::posts::normalize_thread(id),
                id.clone(),
            ));
        }
        let mut stmt = conn.prepare(
            "SELECT id, kind, agent, summary, resolution, created_at FROM posts \
             WHERE project = ? AND area = ? AND thread = ? ORDER BY id",
        )?;
        for (area, thread, job) in threads {
            let rows = stmt
                .query_map([&task.project, &area, &thread], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (id, kind, agent, summary, resolution, created_at) in rows {
                items.push(ActivityItem {
                    id: format!("post:{id}"),
                    at: created_at,
                    kind: "post".into(),
                    what: kind,
                    who: agent,
                    job_id: job.clone(),
                    text: clip(summary.trim(), TEXT_CAP).0,
                    status: resolution,
                    reference: id,
                    ..ActivityItem::default()
                });
            }
        }
    }
    drop(conn);

    let mut keyed: Vec<(usize, ActivityItem)> = items.into_iter().enumerate().collect();
    keyed.sort_by(|(ia, a), (ib, b)| {
        a.at.cmp(&b.at)
            .then_with(|| rank(a).cmp(&rank(b)))
            .then_with(|| ia.cmp(ib))
    });
    let mut items: Vec<ActivityItem> = keyed.into_iter().map(|(_, i)| i).collect();
    let total = items.len();
    let has_more = limit > 0 && total > limit;
    if has_more {
        items = items.split_off(total - limit);
    }
    Ok(TaskActivity {
        task_id: task.id,
        items,
        runs,
        total,
        has_more,
        running,
        now: s.now(),
    })
}
