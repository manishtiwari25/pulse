//! The facts Ask answers from: a compact, read-only view of one project (or
//! of every project) assembled from the store, followed by the posts,
//! records, tasks and jobs whose words match the question. Every section is
//! capped so the whole context stays small enough for a quick model call.

// Each query's row is a tuple read once and printed; naming a struct per
// query would say less than the SELECT next to it does.
#![allow(clippy::type_complexity)]

use anyhow::Result;
use pulse_store::Store;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// One thing the answer may cite.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    /// `post`, `record`, `task` or `job`: what the id names, so a console can link it.
    pub source: String,
    /// The post kind, record kind, task status or job status.
    pub kind: String,
    pub summary: String,
    pub date: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread: String,
}

/// What one question is answered from.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// The facts, as the model sees them.
    pub text: String,
    /// Everything the text names by id, in the order it appears.
    pub items: Vec<Item>,
    /// Where in `items` the question-matched ones start.
    pub matched_from: usize,
    /// One line per section, for an answer without a model.
    pub headlines: Vec<String>,
    /// The project slug, or empty when the context spans every project.
    pub project: String,
}

/// The most characters the facts may take; past it, the question-matched
/// section is cut first.
pub const MAX_CHARS: usize = 18_000;
const MATCHES: usize = 12;
const EXCERPT: usize = 420;

/// Build the context for `project` (`None` means every active project) and
/// the question's words.
pub fn build(s: &Store, project: Option<&str>, question: &str) -> Result<Context> {
    let mut cx = Context {
        project: project.unwrap_or_default().to_string(),
        ..Context::default()
    };
    let now = s.now();
    let _ = writeln!(cx.text, "Now: {now}");
    match project {
        Some(slug) => project_section(s, slug, &mut cx)?,
        None => all_projects_section(s, &mut cx)?,
    }
    let _ = writeln!(cx.text, "\n## Settings");
    let mut settings = Vec::new();
    for name in [
        "stop",
        "owner.presence",
        "landing",
        "evolve.loop",
        "provider",
        "worker_model",
        "light_model",
        "compute.auto_approve",
    ] {
        let v = s.setting(name, "");
        let _ = writeln!(
            cx.text,
            "- {name} = {}",
            if v.is_empty() { "(empty)" } else { &v }
        );
        if matches!(name, "stop" | "owner.presence" | "landing" | "evolve.loop") {
            settings.push(format!(
                "{name} {}",
                if v.is_empty() { "(empty)" } else { &v }
            ));
        }
    }
    cx.headlines
        .push(format!("Settings: {}.", settings.join(", ")));

    // The question-matched section comes last and is the first to be cut.
    let terms = terms(question);
    let budget = MAX_CHARS.saturating_sub(cx.text.chars().count());
    cx.matched_from = cx.items.len();
    if !terms.is_empty() {
        let before = std::mem::take(&mut cx.text);
        matches_section(s, project, &terms, &mut cx)?;
        let matched = std::mem::replace(&mut cx.text, before);
        if matched.chars().count() > budget {
            cx.text.push_str(&clip(&matched, budget.max(200)));
        } else {
            cx.text.push_str(&matched);
        }
    }
    Ok(cx)
}

fn project_section(s: &Store, slug: &str, cx: &mut Context) -> Result<()> {
    let p = s.project(slug)?;
    let _ = writeln!(cx.text, "\n## Project {slug}");
    match &p {
        Some(p) => {
            let _ = writeln!(cx.text, "- name: {}", p.name);
            let _ = writeln!(cx.text, "- status: {}", p.status);
            if !p.remote.is_empty() {
                let _ = writeln!(cx.text, "- remote: {}", p.remote);
            }
            if !p.default_branch.is_empty() {
                let _ = writeln!(cx.text, "- default branch: {}", p.default_branch);
            }
            if p.brain {
                let _ = writeln!(
                    cx.text,
                    "- this is the brain (the repository PULSE itself ships from)"
                );
            }
            if !p.verify_cmd.is_empty() {
                let _ = writeln!(cx.text, "- verify command: {}", clip(&p.verify_cmd, 160));
            }
        }
        None => {
            let _ = writeln!(
                cx.text,
                "- (no such project row; the commons or an unknown slug)"
            );
        }
    }
    if slug != pulse_store::COMMONS_SLUG
        && let Ok(goal) = pulse_goals::get(s, slug)
        && !goal.text.trim().is_empty()
    {
        let _ = writeln!(
            cx.text,
            "\n## Goal ({} by {})",
            goal.source, goal.updated_by
        );
        let _ = writeln!(cx.text, "{}", clip(goal.text.trim(), 900));
    }
    board_section(s, slug, cx)?;
    jobs_section(s, Some(slug), cx)?;
    runs_section(s, Some(slug), cx)?;
    tasks_section(s, Some(slug), cx)?;
    records_section(s, Some(slug), cx)?;
    decisions_section(s, Some(slug), cx)?;
    compute_section(s, Some(slug), cx)?;
    messages_section(s, Some(slug), cx)?;
    events_section(s, Some(slug), cx)?;
    Ok(())
}

fn all_projects_section(s: &Store, cx: &mut Context) -> Result<()> {
    let projects = s.projects(false)?;
    let _ = writeln!(cx.text, "\n## Projects ({})", projects.len());
    cx.headlines.push(format!(
        "Projects: {}.",
        projects
            .iter()
            .map(|p| format!("{} ({})", p.slug, p.status))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for p in &projects {
        let conn = s.conn()?;
        let (queued, running, blocked): (i64, i64, i64) = conn.query_row(
            "SELECT SUM(status='queued'), SUM(status='leased'), SUM(status='blocked') FROM jobs WHERE project = ?",
            [&p.slug],
            |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            },
        )?;
        let open_tasks: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE project = ? AND status IN ('open','planned','in_progress','review')",
            [&p.slug],
            |r| r.get(0),
        )?;
        let _ = writeln!(
            cx.text,
            "- {} ({}): status {}{}; jobs queued {queued}, running {running}, blocked {blocked}; open tasks {open_tasks}",
            p.slug,
            p.name,
            p.status,
            if p.brain { ", brain" } else { "" }
        );
    }
    jobs_section(s, None, cx)?;
    runs_section(s, None, cx)?;
    tasks_section(s, None, cx)?;
    records_section(s, None, cx)?;
    decisions_section(s, None, cx)?;
    compute_section(s, None, cx)?;
    messages_section(s, None, cx)?;
    events_section(s, None, cx)?;
    Ok(())
}

fn board_section(s: &Store, slug: &str, cx: &mut Context) -> Result<()> {
    let t = match pulse_tally::compute_light(s, slug, "", pulse_tally::Thresholds::default()) {
        Ok(t) => t,
        Err(e) => {
            let _ = writeln!(
                cx.text,
                "\n## Board\n- (the board tally is unavailable: {})",
                clip(&e.to_string(), 200)
            );
            return Ok(());
        }
    };
    let open: Vec<_> = t.threads.iter().filter(|th| th.status == "open").collect();
    let awaiting_n = t
        .proposals
        .iter()
        .filter(|p| p.status == "awaiting")
        .count();
    let open_amendments = t.amendments.iter().filter(|a| a.open).count();
    let _ = writeln!(
        cx.text,
        "\n## Board\n- threads: {} open of {}; proposals awaiting the owner: {}; open amendments: {}",
        open.len(),
        t.threads.len(),
        awaiting_n,
        open_amendments
    );
    cx.headlines.push(format!(
        "Board: {} open threads of {}; {awaiting_n} proposal{} waiting for your yes or no; {open_amendments} open amendment{}.",
        open.len(),
        t.threads.len(),
        if awaiting_n == 1 { "" } else { "s" },
        if open_amendments == 1 { "" } else { "s" }
    ));
    let mut recent: Vec<_> = open.clone();
    recent.sort_by(|a, b| b.last.cmp(&a.last));
    if !recent.is_empty() {
        let _ = writeln!(
            cx.text,
            "Open threads, newest first (thread key: last post):"
        );
        for th in recent.iter().take(10) {
            let _ = writeln!(
                cx.text,
                "- {} ({} posts, last {}): {}",
                th.key,
                th.posts,
                day(&th.last),
                clip(&th.last_summary, 160)
            );
        }
    }
    let awaiting: Vec<_> = t
        .proposals
        .iter()
        .filter(|p| p.status == "awaiting")
        .collect();
    if !awaiting.is_empty() {
        let _ = writeln!(cx.text, "Proposals waiting for the owner's yes or no:");
        for p in awaiting.iter().take(8) {
            let _ = writeln!(
                cx.text,
                "- [{}] {} ({}, {}): {}",
                p.id,
                p.key,
                p.agent,
                day(&p.date),
                clip(&p.summary, 200)
            );
            cx.items.push(Item {
                id: p.id.clone(),
                source: "post".into(),
                kind: "proposal".into(),
                summary: p.summary.clone(),
                date: p.date.clone(),
                project: slug.into(),
                thread: p.key.clone(),
            });
        }
    }
    let mut sugg: Vec<_> = t
        .suggestions
        .iter()
        .filter(|x| x.status == "open" || x.status.is_empty())
        .collect();
    sugg.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| b.date.cmp(&a.date)));
    if !sugg.is_empty() {
        let _ = writeln!(cx.text, "Top open suggestions (score, up/down):");
        for x in sugg.iter().take(6) {
            let _ = writeln!(
                cx.text,
                "- [{}] score {} ({}/{}) {}: {}",
                x.id,
                x.score,
                x.up,
                x.down,
                day(&x.date),
                clip(&x.summary, 180)
            );
            cx.items.push(Item {
                id: x.id.clone(),
                source: "post".into(),
                kind: "suggestion".into(),
                summary: x.summary.clone(),
                date: x.date.clone(),
                project: slug.into(),
                thread: x.key.clone(),
            });
        }
    }
    let open_am: Vec<_> = t.amendments.iter().filter(|a| a.open).collect();
    if !open_am.is_empty() {
        let _ = writeln!(cx.text, "Open amendments:");
        for a in open_am.iter().take(4) {
            let _ = writeln!(
                cx.text,
                "- [{}] {} ({} up, {} down, quorum {}): {}",
                a.id,
                a.status,
                a.up,
                a.down,
                a.quorum,
                clip(&a.summary, 180)
            );
            cx.items.push(Item {
                id: a.id.clone(),
                source: "post".into(),
                kind: "amendment".into(),
                summary: a.summary.clone(),
                date: a.date.clone(),
                project: slug.into(),
                thread: a.key.clone(),
            });
        }
    }
    Ok(())
}

fn scope(project: Option<&str>) -> (String, Vec<String>) {
    match project {
        Some(p) => (" AND project = ?".into(), vec![p.to_string()]),
        None => (String::new(), vec![]),
    }
}

fn jobs_section(s: &Store, project: Option<&str>, cx: &mut Context) -> Result<()> {
    let conn = s.conn()?;
    let (filter, args) = scope(project);
    let mut stmt = conn.prepare(&format!(
        "SELECT status, COUNT(*) FROM jobs WHERE 1=1{filter} GROUP BY status ORDER BY status"
    ))?;
    let counts: Vec<(String, i64)> = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let _ = writeln!(cx.text, "\n## Jobs");
    if counts.is_empty() {
        let _ = writeln!(cx.text, "- none yet");
        return Ok(());
    }
    let by_status = counts
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(cx.text, "- by status: {by_status}");
    cx.headlines.push(format!("Jobs by status: {by_status}."));
    for (status, label, limit) in [
        ("leased", "Running now", 6),
        ("blocked", "Blocked", 6),
        ("queued", "Queued, highest priority first", 6),
        ("failed", "Failed most recently", 5),
    ] {
        let order = if status == "queued" {
            "priority DESC, created_at"
        } else {
            "COALESCE(NULLIF(finished_at,''), created_at) DESC"
        };
        let mut stmt = conn.prepare(&format!(
            "SELECT id, project, kind, title, status, created_at, finished_at, lease_agent, substr(result,1,300) FROM jobs WHERE status = ?{filter} ORDER BY {order} LIMIT {limit}"
        ))?;
        let mut params: Vec<String> = vec![status.to_string()];
        params.extend(args.iter().cloned());
        let rows: Vec<(
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        )> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(cx.text, "{label}:");
        for (id, proj, kind, title, st, created, finished, agent, result) in rows {
            let when = if finished.is_empty() {
                &created
            } else {
                &finished
            };
            let who = if agent.is_empty() {
                String::new()
            } else {
                format!(", {agent}")
            };
            let tail = if st == "failed" || st == "blocked" {
                let r = first_line(&result);
                if r.is_empty() {
                    String::new()
                } else {
                    format!(" - {}", clip(&r, 160))
                }
            } else {
                String::new()
            };
            let _ = writeln!(
                cx.text,
                "- [{id}] {kind} {}{who}: {}{tail}",
                day(when),
                clip(&title, 140)
            );
            cx.items.push(Item {
                id,
                source: "job".into(),
                kind: st,
                summary: title,
                date: when.clone(),
                project: proj,
                thread: String::new(),
            });
        }
    }
    Ok(())
}

fn runs_section(s: &Store, project: Option<&str>, cx: &mut Context) -> Result<()> {
    let conn = s.conn()?;
    let (filter, args) = scope(project);
    let mut stmt = conn.prepare(&format!(
        "SELECT id, project, job_id, agent, status, started_at, finished_at, seconds, substr(summary,1,300) FROM runs WHERE 1=1{filter} ORDER BY started_at DESC LIMIT 6"
    ))?;
    let rows: Vec<(
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        Option<f64>,
        String,
    )> = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        return Ok(());
    }
    let _ = writeln!(cx.text, "\n## Latest runs");
    for (id, proj, job, agent, status, started, finished, secs, summary) in rows {
        let dur = secs
            .map(|x| format!(" {}s", x.round() as i64))
            .unwrap_or_default();
        let summ = first_line(&summary);
        let _ = writeln!(
            cx.text,
            "- [{id}] {status}{dur} by {agent} for job {job}, started {}{}{}",
            stamp(&started),
            if finished.is_empty() {
                String::new()
            } else {
                format!(", finished {}", stamp(&finished))
            },
            if summ.is_empty() {
                String::new()
            } else {
                format!(": {}", clip(&summ, 160))
            }
        );
        cx.items.push(Item {
            id,
            source: "run".into(),
            kind: status,
            summary: if summ.is_empty() {
                format!("run of {job}")
            } else {
                summ
            },
            date: started,
            project: proj,
            thread: String::new(),
        });
    }
    Ok(())
}

fn tasks_section(s: &Store, project: Option<&str>, cx: &mut Context) -> Result<()> {
    let conn = s.conn()?;
    let (filter, args) = scope(project);
    let mut stmt = conn.prepare(&format!(
        "SELECT id, project, title, status, priority, updated_at, lead, job_id FROM tasks WHERE status IN ('open','planned','in_progress','review'){filter} ORDER BY updated_at DESC LIMIT 10"
    ))?;
    let rows: Vec<(String, String, String, String, i64, String, String, String)> = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let done: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM tasks WHERE status = 'done'{filter}"),
        rusqlite::params_from_iter(args.iter()),
        |r| r.get(0),
    )?;
    if rows.is_empty() && done == 0 {
        return Ok(());
    }
    let _ = writeln!(cx.text, "\n## Tasks ({} open, {done} done)", rows.len());
    cx.headlines
        .push(format!("Tasks: {} open, {done} done.", rows.len()));
    for (id, proj, title, status, prio, updated, lead, job) in rows {
        let _ = writeln!(
            cx.text,
            "- [{id}] {status} p{prio} {}{}{}: {}",
            day(&updated),
            if lead.is_empty() {
                String::new()
            } else {
                format!(", lead {lead}")
            },
            if job.is_empty() {
                String::new()
            } else {
                format!(", job {job}")
            },
            clip(&title, 140)
        );
        cx.items.push(Item {
            id,
            source: "task".into(),
            kind: status,
            summary: title,
            date: updated,
            project: proj,
            thread: String::new(),
        });
    }
    Ok(())
}

fn records_section(s: &Store, project: Option<&str>, cx: &mut Context) -> Result<()> {
    let conn = s.conn()?;
    let (filter, args) = scope(project);
    let mut stmt = conn.prepare(&format!(
        "SELECT id, project, kind, title, status, COALESCE(NULLIF(date,''), updated_at) FROM records WHERE kind IN ('adr','plan','wu','feature','memory'){filter} ORDER BY updated_at DESC LIMIT 10"
    ))?;
    let rows: Vec<(String, String, String, String, String, String)> = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        return Ok(());
    }
    let _ = writeln!(
        cx.text,
        "\n## Latest records (ADRs, plans, work units, features, memory)"
    );
    for (id, proj, kind, title, status, date) in rows {
        let _ = writeln!(
            cx.text,
            "- [{id}] {kind} {status} {}: {}",
            day(&date),
            clip(&title, 140)
        );
        cx.items.push(Item {
            id,
            source: "record".into(),
            kind,
            summary: title,
            date,
            project: proj,
            thread: String::new(),
        });
    }
    Ok(())
}

fn decisions_section(s: &Store, project: Option<&str>, cx: &mut Context) -> Result<()> {
    let conn = s.conn()?;
    let (filter, args) = scope(project);
    let mut stmt = conn.prepare(&format!(
        "SELECT d.post_id, d.project, d.answer, d.words, d.decided_at, d.decided_by, COALESCE(p.summary,''), COALESCE(p.thread,'') FROM decisions d LEFT JOIN posts p ON p.id = d.post_id WHERE 1=1{} ORDER BY d.decided_at DESC LIMIT 6",
        filter.replace("project", "d.project")
    ))?;
    let rows: Vec<(
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    )> = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        return Ok(());
    }
    let _ = writeln!(cx.text, "\n## Latest decisions on proposals");
    for (id, proj, answer, words, at, by, summary, thread) in rows {
        let _ = writeln!(
            cx.text,
            "- [{id}] {answer} by {by} on {}{}: {}",
            day(&at),
            if words.is_empty() {
                String::new()
            } else {
                format!(" (\"{}\")", clip(&words, 120))
            },
            clip(&summary, 160)
        );
        cx.items.push(Item {
            id,
            source: "post".into(),
            kind: "proposal".into(),
            summary,
            date: at,
            project: proj,
            thread,
        });
    }
    Ok(())
}

fn compute_section(s: &Store, project: Option<&str>, cx: &mut Context) -> Result<()> {
    let conn = s.conn()?;
    let (filter, args) = scope(project);
    let mut stmt = conn.prepare(&format!(
        "SELECT id, project, requested_by, size, hours, status, reason, at FROM compute_requests WHERE status IN ('pending','approved','provisioning','ready'){filter} ORDER BY at DESC LIMIT 6"
    ))?;
    let rows: Vec<(String, String, String, String, i64, String, String, String)> = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        return Ok(());
    }
    let _ = writeln!(cx.text, "\n## Machines (compute requests not yet released)");
    for (id, proj, by, size, hours, status, reason, at) in rows {
        let _ = writeln!(
            cx.text,
            "- {id} {status}: {size} for {hours}h, asked by {by} on {} for {proj}: {}",
            day(&at),
            clip(&reason, 140)
        );
    }
    Ok(())
}

fn messages_section(s: &Store, project: Option<&str>, cx: &mut Context) -> Result<()> {
    let conn = s.conn()?;
    let (filter, args) = scope(project);
    let mut stmt = conn.prepare(&format!(
        "SELECT id, project, COALESCE(task_id,''), author, substr(body,1,300), at FROM messages WHERE 1=1{filter} ORDER BY at DESC LIMIT 5"
    ))?;
    let rows: Vec<(String, String, String, String, String, String)> = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        return Ok(());
    }
    let _ = writeln!(
        cx.text,
        "\n## Latest chat messages between the owner and the leads"
    );
    for (_id, proj, task, author, body, at) in rows {
        let _ = writeln!(
            cx.text,
            "- {} {author} ({proj}{}): {}",
            stamp(&at),
            if task.is_empty() {
                String::new()
            } else {
                format!(", {task}")
            },
            clip(&first_line(&body), 200)
        );
    }
    Ok(())
}

fn events_section(s: &Store, project: Option<&str>, cx: &mut Context) -> Result<()> {
    let conn = s.conn()?;
    let (filter, args) = match project {
        Some(p) => (
            " AND (subject = ? OR subject LIKE ? OR detail LIKE ?)".to_string(),
            vec![p.to_string(), format!("{p}/%"), format!("%{p}%")],
        ),
        None => (String::new(), vec![]),
    };
    let since: String = conn
        .query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', datetime(?, '-24 hours'))",
            [s.now()],
            |r| r.get::<_, Option<String>>(0),
        )?
        .unwrap_or_default();
    let Some(first_seq) = since_seq(&conn, &since)? else {
        return Ok(());
    };
    let mut params: Vec<String> = vec![first_seq.to_string()];
    params.extend(args);
    let mut stmt = conn.prepare(&format!(
        "SELECT action, COUNT(*) FROM events WHERE seq >= ? AND action NOT IN ('harness.step'){filter} GROUP BY action ORDER BY COUNT(*) DESC LIMIT 10"
    ))?;
    let rows: Vec<(String, i64)> = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        return Ok(());
    }
    let _ = writeln!(
        cx.text,
        "\n## Activity in the last 24 hours (event action: count)\n- {}",
        rows.iter()
            .map(|(a, n)| format!("{a}: {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

/// The first event sequence number at or after `since`. `events.at` has no
/// index but `seq` grows with time, so a few indexed seeks find the boundary
/// instead of a scan of every row.
fn since_seq(conn: &rusqlite::Connection, since: &str) -> Result<Option<i64>> {
    let at_or_after = |seq: i64| -> rusqlite::Result<Option<String>> {
        conn.query_row(
            "SELECT at FROM events WHERE seq >= ? ORDER BY seq LIMIT 1",
            [seq],
            |r| r.get(0),
        )
        .optional()
    };
    let Some((mut lo, mut hi)) =
        conn.query_row("SELECT MIN(seq), MAX(seq) FROM events", [], |r| {
            Ok(r.get::<_, Option<i64>>(0)?.zip(r.get::<_, Option<i64>>(1)?))
        })?
    else {
        return Ok(None);
    };
    if at_or_after(hi)?.is_some_and(|at| at.as_str() < since) {
        return Ok(None);
    }
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match at_or_after(mid)? {
            Some(at) if at.as_str() >= since => hi = mid,
            _ => lo = mid + 1,
        }
    }
    Ok(Some(lo))
}

fn matches_section(
    s: &Store,
    project: Option<&str>,
    terms: &[String],
    cx: &mut Context,
) -> Result<()> {
    let conn = s.conn()?;
    let (filter, scope_args) = scope(project);
    let patterns: Vec<String> = terms
        .iter()
        .map(|t| format!("%{}%", like_escape(t)))
        .collect();
    let seen: BTreeSet<String> = cx.items.iter().map(|i| i.id.clone()).collect();

    // Posts: ranked by how many of the question's words they hold, newest first.
    let hits = (1..=patterns.len())
        .map(|i| format!("(CASE WHEN lower(summary) LIKE ?{i} ESCAPE '\\' OR lower(body) LIKE ?{i} ESCAPE '\\' THEN 1 ELSE 0 END)"))
        .collect::<Vec<_>>()
        .join(" + ");
    let scope_sql = filter.replace('?', &format!("?{}", patterns.len() + 1));
    let sql = format!(
        "SELECT id, project, kind, thread, date, summary, substr(body,1,{EXCERPT}), resolution, hits FROM (SELECT id, project, kind, thread, date, summary, body, resolution, {hits} AS hits FROM posts WHERE kind NOT IN ('vote','claim'){scope_sql}) WHERE hits > 0 ORDER BY hits DESC, date DESC LIMIT {MATCHES}"
    );
    let mut params: Vec<String> = patterns.clone();
    params.extend(scope_args.iter().cloned());
    let mut stmt = conn.prepare(&sql)?;
    let posts: Vec<(
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
    )> = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    // Records, tasks and jobs by title.
    let any_title = (1..=patterns.len())
        .map(|i| format!("lower(title) LIKE ?{i} ESCAPE '\\'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut stmt = conn.prepare(&format!(
        "SELECT id, project, kind, title, status, COALESCE(NULLIF(date,''), updated_at) FROM records WHERE ({any_title}){scope_sql} ORDER BY updated_at DESC LIMIT 6"
    ))?;
    let records: Vec<(String, String, String, String, String, String)> = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT id, project, 'task', title, status, updated_at FROM tasks WHERE ({any_title}){scope_sql} ORDER BY updated_at DESC LIMIT 5"
    ))?;
    let tasks: Vec<(String, String, String, String, String, String)> = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT id, project, kind, title, status, COALESCE(NULLIF(finished_at,''), created_at) FROM jobs WHERE ({any_title}){scope_sql} ORDER BY created_at DESC LIMIT 5"
    ))?;
    let jobs: Vec<(String, String, String, String, String, String)> = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    if posts.is_empty() && records.is_empty() && tasks.is_empty() && jobs.is_empty() {
        let _ = writeln!(
            cx.text,
            "\n## Matching the question's words ({})\n- nothing in the store mentions them",
            terms.join(", ")
        );
        return Ok(());
    }
    let _ = writeln!(
        cx.text,
        "\n## Matching the question's words ({})",
        terms.join(", ")
    );
    for (id, proj, kind, thread, date, summary, body, resolution, hits) in posts {
        let _ = writeln!(
            cx.text,
            "- [{id}] {kind} on {proj}/{thread}, {} ({hits} word{} matched{}): {}",
            day(&date),
            if hits == 1 { "" } else { "s" },
            if resolution.is_empty() {
                String::new()
            } else {
                format!(", resolution {resolution}")
            },
            clip(&summary, 200)
        );
        let excerpt = excerpt(&body, terms);
        if !excerpt.is_empty() {
            let _ = writeln!(cx.text, "  > {excerpt}");
        }
        if !seen.contains(&id) {
            cx.items.push(Item {
                id,
                source: "post".into(),
                kind,
                summary,
                date,
                project: proj,
                thread,
            });
        }
    }
    for (source, rows) in [("record", records), ("task", tasks), ("job", jobs)] {
        for (id, proj, kind, title, status, date) in rows {
            let _ = writeln!(
                cx.text,
                "- [{id}] {source} {kind} {status} on {proj}, {}: {}",
                day(&date),
                clip(&title, 160)
            );
            if !seen.contains(&id) && !cx.items.iter().any(|i| i.id == id) {
                cx.items.push(Item {
                    id,
                    source: source.into(),
                    kind: if source == "task" || source == "job" {
                        status
                    } else {
                        kind
                    },
                    summary: title,
                    date,
                    project: proj,
                    thread: String::new(),
                });
            }
        }
    }
    Ok(())
}

/// A short piece of the body around the first matched word, on one line.
fn excerpt(body: &str, terms: &[String]) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return String::new();
    }
    let lower = flat.to_lowercase();
    let at = terms
        .iter()
        .filter_map(|t| lower.find(t.as_str()))
        .min()
        .unwrap_or(0);
    // Start a little before the match, on a character boundary.
    let mut start = at.saturating_sub(80);
    while start > 0 && !flat.is_char_boundary(start) {
        start -= 1;
    }
    let piece: String = flat[start..].chars().take(260).collect();
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.push_str(piece.trim());
    if flat[start..].chars().count() > 260 {
        s.push('…');
    }
    s
}

const STOPWORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "are",
    "but",
    "not",
    "you",
    "all",
    "any",
    "can",
    "had",
    "her",
    "was",
    "one",
    "our",
    "out",
    "has",
    "have",
    "what",
    "whats",
    "when",
    "where",
    "which",
    "who",
    "whom",
    "why",
    "how",
    "does",
    "did",
    "this",
    "that",
    "these",
    "those",
    "with",
    "from",
    "into",
    "about",
    "there",
    "their",
    "they",
    "them",
    "then",
    "than",
    "were",
    "will",
    "would",
    "could",
    "should",
    "been",
    "being",
    "also",
    "just",
    "like",
    "some",
    "such",
    "over",
    "under",
    "after",
    "before",
    "please",
    "tell",
    "show",
    "give",
    "know",
    "need",
    "want",
    "right",
    "now",
    "today",
    "yesterday",
    "last",
    "week",
    "month",
    "year",
    "get",
    "got",
    "its",
    "it's",
    "is",
    "me",
    "my",
    "we",
    "us",
    "do",
    "did",
    "done",
    "doing",
    "anything",
    "something",
    "everything",
    "nothing",
    "still",
    "yet",
    "ever",
    "very",
    "much",
    "many",
    "more",
    "most",
    "less",
    "here",
    "currently",
    "current",
    "recent",
    "recently",
    "latest",
    "new",
    "old",
    "status",
    "state",
    "going",
    "happening",
    "happened",
    "say",
    "said",
    "list",
    "summary",
    "summarize",
    "summarise",
    "explain",
    "describe",
    "whether",
    "while",
    "since",
    "until",
    "between",
    "each",
    "every",
    "other",
    "another",
    "same",
    "own",
    "both",
    "either",
    "neither",
    "because",
    "through",
    "during",
    "without",
    "within",
    "around",
    "again",
    "once",
    "only",
    "really",
    "think",
    "thing",
    "things",
    "stuff",
    "project",
    "projects",
    "pulse",
    "board",
    "ask",
    "question",
];

/// The question's searchable words: lowercase, three characters or more,
/// not a stopword, at most eight, ids kept whole (hyphens and dots stay).
pub fn terms(question: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for raw in question.split(|c: char| {
        !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/' || c == '\'')
    }) {
        let w = raw
            .trim_matches(|c: char| c == '-' || c == '_' || c == '.' || c == '/' || c == '\'')
            .to_lowercase();
        if w.chars().count() < 3 || STOPWORDS.contains(&w.as_str()) || !seen.insert(w.clone()) {
            continue;
        }
        out.push(w);
        if out.len() == 8 {
            break;
        }
    }
    out
}

fn like_escape(t: &str) -> String {
    t.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// The first `n` characters, with an ellipsis when something was cut.
pub fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// `2026-10-12` from an RFC 3339 stamp; the whole string when it is not one.
fn day(stamp: &str) -> &str {
    if stamp.len() >= 10 && stamp.as_bytes()[4] == b'-' {
        &stamp[..10]
    } else {
        stamp
    }
}

/// `2026-10-12 16:45Z` from an RFC 3339 stamp.
fn stamp(stamp: &str) -> String {
    if stamp.len() >= 16 && stamp.as_bytes()[10] == b'T' {
        format!("{} {}Z", &stamp[..10], &stamp[11..16])
    } else {
        stamp.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_drop_noise_and_keep_ids() {
        let t = terms("What is blocked on amistio-template right now? see ADR-024 and TASK-001.");
        assert_eq!(
            t,
            vec!["blocked", "amistio-template", "see", "adr-024", "task-001"]
        );
        assert!(terms("what is the status now?").is_empty());
        assert_eq!(terms("a b c").len(), 0);
        assert_eq!(terms(&"memory ".repeat(20)), vec!["memory"]);
    }

    #[test]
    fn clip_and_excerpt_are_bounded() {
        assert_eq!(clip("  hello ", 10), "hello");
        assert_eq!(clip("héllo wörld", 6), "héllo…");
        let body = format!("{} needle {}", "x".repeat(500), "y".repeat(500));
        let e = excerpt(&body, &["needle".into()]);
        assert!(e.starts_with('…') && e.ends_with('…'), "{e}");
        assert!(e.contains("needle"));
        assert!(e.chars().count() <= 262);
        assert_eq!(excerpt("   ", &["x".into()]), "");
    }

    #[test]
    fn like_escape_quotes_wildcards() {
        assert_eq!(like_escape("a_b%c\\"), "a\\_b\\%c\\\\");
    }

    #[test]
    fn day_and_stamp_shorten_rfc3339() {
        assert_eq!(day("2026-10-12T16:45:22Z"), "2026-10-12");
        assert_eq!(day("soon"), "soon");
        assert_eq!(stamp("2026-10-12T16:45:22Z"), "2026-10-12 16:45Z");
        assert_eq!(stamp("2026-10-12"), "2026-10-12");
    }
}
