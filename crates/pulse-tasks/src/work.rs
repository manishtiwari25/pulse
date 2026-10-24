use crate::tasks::Task;
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Worker {
    pub job: String,
    pub kind: String,
    pub title: String,
    pub agent: String,
    pub since: String,
    pub executor: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Work {
    #[serde(rename = "jobs_total")]
    pub total: i32,
    #[serde(rename = "jobs_queued")]
    pub queued: i32,
    #[serde(rename = "jobs_blocked")]
    pub blocked: i32,
    #[serde(rename = "jobs_running")]
    pub running: i32,
    #[serde(rename = "jobs_done")]
    pub done: i32,
    #[serde(rename = "jobs_failed")]
    pub failed: i32,
    pub working: Vec<Worker>,
}

#[derive(Debug, Clone)]
struct WorkJob {
    id: String,
    project: String,
    kind: String,
    title: String,
    status: String,
    source: String,
    agent: String,
    lease_until: String,
    since: String,
}

#[derive(Debug, Clone)]
struct RunningRun {
    started_at: String,
    executor: String,
}

pub fn work_for(s: &pulse_store::Store, ts: &[Task]) -> Result<BTreeMap<String, Work>> {
    let mut out = BTreeMap::new();
    if ts.is_empty() {
        return Ok(out);
    }
    let mut source_tasks: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut job_tasks: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut source_args = Vec::new();
    let mut job_args = Vec::new();
    let mut seen_sources = BTreeSet::new();
    let mut seen_jobs = BTreeSet::new();
    for (i, t) in ts.iter().enumerate() {
        out.insert(
            t.id.clone(),
            Work {
                working: Vec::new(),
                ..Work::default()
            },
        );
        if !t.id.is_empty() {
            let source = format!("task:{}", t.id);
            source_tasks
                .entry(work_key(&t.project, &source))
                .or_default()
                .push(i);
            if seen_sources.insert(source.clone()) {
                source_args.push(source);
            }
        }
        if !t.job_id.is_empty() {
            job_tasks
                .entry(work_key(&t.project, &t.job_id))
                .or_default()
                .push(i);
            if seen_jobs.insert(t.job_id.clone()) {
                job_args.push(t.job_id.clone());
            }
        }
    }
    if source_args.is_empty() && job_args.is_empty() {
        return Ok(out);
    }
    let (query, args) = work_jobs_query(&source_args, &job_args);
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&query)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let mut jobs = Vec::new();
    let mut active_job_ids = BTreeSet::new();
    let now = s.now();
    for row in stmt.query_map(rusqlite::params_from_iter(refs), |r| {
        Ok(WorkJob {
            id: r.get(0)?,
            project: r.get(1)?,
            kind: r.get(2)?,
            title: r.get(3)?,
            status: r.get(4)?,
            source: r.get(5)?,
            agent: r.get(6)?,
            lease_until: r.get(7)?,
            since: r.get(8)?,
        })
    })? {
        let j = row?;
        if work_task_indexes(&source_tasks, &job_tasks, &j).is_empty() {
            continue;
        }
        if work_is_active(&j, &now) {
            active_job_ids.insert(j.id.clone());
        }
        jobs.push(j);
    }
    let runs = work_running_runs(s, &active_job_ids)?;
    for j in jobs {
        for idx in work_task_indexes(&source_tasks, &job_tasks, &j) {
            let t = &ts[idx];
            let mut w = out.remove(&t.id).unwrap_or_default();
            w.total += 1;
            match j.status.as_str() {
                "queued" => w.queued += 1,
                "blocked" => w.blocked += 1,
                "leased" => w.running += 1,
                "done" => w.done += 1,
                "failed" => w.failed += 1,
                _ => {}
            }
            if work_is_active(&j, &now) {
                let mut worker = Worker {
                    job: j.id.clone(),
                    kind: j.kind.clone(),
                    title: j.title.clone(),
                    agent: j.agent.clone(),
                    since: j.since.clone(),
                    executor: String::new(),
                };
                if let Some(run) = runs.get(&j.id) {
                    worker.since = run.started_at.clone();
                    worker.executor = run.executor.clone();
                }
                w.working.push(worker);
            }
            out.insert(t.id.clone(), w);
        }
    }
    for w in out.values_mut() {
        w.working
            .sort_by(|a, b| a.since.cmp(&b.since).then_with(|| a.job.cmp(&b.job)));
    }
    Ok(out)
}

fn work_jobs_query(source_args: &[String], job_args: &[String]) -> (String, Vec<String>) {
    let mut clauses = Vec::new();
    let mut args = Vec::new();
    if !source_args.is_empty() {
        clauses.push(format!(
            "source IN ({})",
            work_placeholders(source_args.len())
        ));
        args.extend_from_slice(source_args);
    }
    if !job_args.is_empty() {
        clauses.push(format!("id IN ({})", work_placeholders(job_args.len())));
        args.extend_from_slice(job_args);
    }
    (
        format!(
            "SELECT id, project, kind, title, status, source, lease_agent, lease_until,\n\t\tCOALESCE((SELECT MAX(at) FROM events WHERE action = 'job.lease' AND subject = jobs.id AND actor = jobs.lease_agent), created_at)\n\t\tFROM jobs WHERE {} ORDER BY created_at, id",
            clauses.join(" OR ")
        ),
        args,
    )
}

fn work_running_runs(
    s: &pulse_store::Store,
    job_ids: &BTreeSet<String>,
) -> Result<BTreeMap<String, RunningRun>> {
    let mut out = BTreeMap::new();
    if job_ids.is_empty() {
        return Ok(out);
    }
    let args = job_ids.iter().cloned().collect::<Vec<_>>();
    let q = format!(
        "SELECT job_id, started_at, executor FROM runs\n\t\tWHERE status = 'running' AND mode != 'dry-run' AND job_id IN ({})\n\t\tORDER BY started_at, rowid",
        work_placeholders(args.len())
    );
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    for row in stmt.query_map(rusqlite::params_from_iter(refs), |r| {
        Ok((
            r.get::<_, String>(0)?,
            RunningRun {
                started_at: r.get(1)?,
                executor: r.get(2)?,
            },
        ))
    })? {
        let (job_id, run) = row?;
        out.entry(job_id).or_insert(run);
    }
    Ok(out)
}

fn work_task_indexes(
    source_tasks: &BTreeMap<String, Vec<usize>>,
    job_tasks: &BTreeMap<String, Vec<usize>>,
    j: &WorkJob,
) -> Vec<usize> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    if let Some(v) = source_tasks.get(&work_key(&j.project, &j.source)) {
        for idx in v {
            seen.insert(*idx);
            out.push(*idx);
        }
    }
    if let Some(v) = job_tasks.get(&work_key(&j.project, &j.id)) {
        for idx in v {
            if seen.insert(*idx) {
                out.push(*idx);
            }
        }
    }
    out
}

fn work_is_active(j: &WorkJob, now: &str) -> bool {
    j.status == "leased"
        && !j.agent.is_empty()
        && (j.lease_until.is_empty() || j.lease_until.as_str() > now)
}

fn work_key(project: &str, value: &str) -> String {
    format!("{project}\0{value}")
}

fn work_placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}
