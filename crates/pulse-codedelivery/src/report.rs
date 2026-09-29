use crate::{
    EVIDENCE_UNAVAILABLE, REASON_EVIDENCE_TOO_LARGE, REASON_NO_TRUSTWORTHY_EVIDENCE,
    decode_evidence, decode_work_snapshot, public_reason, safe_code_paths_for_report,
};
use anyhow::{Result, anyhow};
use pulse_store::{CancellationToken, ctx_error};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub project: Option<String>,
    pub limit: i32,
    pub scanned: i32,
    pub has_more: bool,
    pub next_before: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub code_deliveries: i32,
    pub code_files: i32,
    pub docs_only: i32,
    pub non_code: i32,
    pub unknown: i32,
    pub duplicates: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRef {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRef {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRef {
    pub name: String,
    pub runner: String,
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Votes {
    pub up: i32,
    pub down: i32,
    pub score: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionRef {
    pub id: String,
    pub summary: String,
    pub area: String,
    pub thread: String,
    pub votes: Option<Votes>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub event_seq: i64,
    pub project: String,
    pub at: String,
    pub commit: String,
    pub code_files: i32,
    pub files_changed: i32,
    pub code_paths: Vec<String>,
    pub paths_truncated: bool,
    pub evidence: String,
    pub job: Option<JobRef>,
    pub tasks: Vec<TaskRef>,
    pub worker: Option<WorkerRef>,
    pub suggestion: Option<SuggestionRef>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unavailable {
    pub event_seq: i64,
    pub project: String,
    pub at: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub scope: Scope,
    pub summary: Summary,
    pub deliveries: Vec<Delivery>,
    pub unavailable: Vec<Unavailable>,
}

#[derive(Debug, Clone)]
struct EventRow {
    seq: i64,
    at: String,
    detail: String,
}

#[derive(Debug, Clone)]
struct SuggestionPost {
    id: String,
    project: String,
    area: String,
    thread: String,
    summary: String,
}

/// Go: Recent(ctx, s, project, limit, before) - bounded to two seconds and
/// abandoned with ctx.Err() as soon as the caller's context is done.
pub fn recent(
    ctx: &CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    limit: i32,
    before: i64,
) -> Result<Report> {
    let ctx = &ctx.with_timeout(Duration::from_secs(2));
    let limit = if limit <= 0 { 100 } else { limit };
    if limit > 200 {
        return Err(anyhow!("code-delivery limit exceeds the supported range"));
    }
    let before = if before <= 0 { i64::MAX } else { before };
    let scope = Scope {
        project: if project.is_empty() {
            None
        } else {
            Some(project.to_string())
        },
        limit,
        ..Scope::default()
    };
    let mut report = Report {
        scope,
        deliveries: Vec::new(),
        unavailable: Vec::new(),
        ..Report::default()
    };
    let (events, has_more, next_before) = landing_events(ctx, s, project, limit, before)?;
    report.scope.scanned = events.len() as i32;
    report.scope.has_more = has_more;
    if next_before > 0 {
        report.scope.next_before = Some(next_before);
    }
    let mut seen_commits = BTreeMap::new();
    let mut suggestions = BTreeMap::new();
    for ev in events {
        add_event(ctx, s, &mut report, ev, &mut seen_commits, &mut suggestions)?;
    }
    if let Some(err) = ctx.err() {
        return Err(anyhow::Error::new(err));
    }
    Ok(report)
}

fn landing_events(
    ctx: &CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    limit: i32,
    before: i64,
) -> Result<(Vec<EventRow>, bool, i64)> {
    const MINIMUM_EVENT_SCAN: i64 = 5000;
    let mut scan_limit = i64::from(limit) * 100;
    if scan_limit < MINIMUM_EVENT_SCAN {
        scan_limit = MINIMUM_EVENT_SCAN;
    }
    let conn = s.conn_ctx(ctx)?;
    let max_seq: i64 = conn
        .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))
        .map_err(|e| ctx_error(ctx, e.into()))?;
    let mut high = before - 1;
    if high > max_seq {
        high = max_seq;
    }
    if high <= 0 {
        return Ok((Vec::new(), false, 0));
    }
    let low = (high - scan_limit + 1).max(1);
    let mut stmt = conn
        .prepare(
            "SELECT seq, at, action, detail FROM events WHERE seq BETWEEN ? AND ? ORDER BY seq DESC",
        )
        .map_err(|e| ctx_error(ctx, e.into()))?;
    let mut rows = stmt
        .query(rusqlite::params![low, high])
        .map_err(|e| ctx_error(ctx, e.into()))?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().map_err(|e| ctx_error(ctx, e.into()))? {
        let action: String = row.get(2)?;
        if action != "land.landed" {
            continue;
        }
        let ev = EventRow {
            seq: row.get(0)?,
            at: row.get(1)?,
            detail: row.get(3)?,
        };
        if !project.is_empty() {
            let (detail, ok) = decode_detail(&ev.detail);
            if !ok || string_field(&detail, "project") != project {
                continue;
            }
        }
        events.push(ev);
        if events.len() > limit as usize {
            break;
        }
    }
    let has_more = events.len() > limit as usize || low > 1;
    let mut next_before = 0;
    if events.len() > limit as usize {
        events.truncate(limit as usize);
        next_before = events.last().map_or(0, |ev| ev.seq);
    } else if low > 1 {
        next_before = low;
    }
    Ok((events, has_more, next_before))
}

fn add_event(
    ctx: &CancellationToken,
    s: &pulse_store::Store,
    report: &mut Report,
    ev: EventRow,
    seen_commits: &mut BTreeMap<String, bool>,
    suggestions: &mut BTreeMap<String, Option<SuggestionRef>>,
) -> Result<()> {
    let (detail, ok) = decode_detail(&ev.detail);
    let project = string_field(&detail, "project");
    if !ok {
        add_unavailable(report, &ev, &project, REASON_NO_TRUSTWORTHY_EVIDENCE);
        return Ok(());
    }
    let (evidence, evidence_ok) = decode_evidence(detail.get("code_delivery"));
    if !evidence_ok || evidence.evidence == EVIDENCE_UNAVAILABLE {
        let reason = if evidence.reason.is_empty() {
            REASON_NO_TRUSTWORTHY_EVIDENCE
        } else {
            &evidence.reason
        };
        add_unavailable(report, &ev, &project, reason);
        return Ok(());
    }
    if evidence.changes_truncated {
        add_unavailable(report, &ev, &project, REASON_EVIDENCE_TOO_LARGE);
        return Ok(());
    }
    let mut commit = string_field(&detail, "sha");
    if commit.is_empty() {
        commit = evidence.after.clone();
    }
    if !commit.is_empty() {
        let key = format!("{project}\0{commit}");
        if seen_commits.get(&key).copied().unwrap_or(false) {
            report.summary.duplicates += 1;
            return Ok(());
        }
        seen_commits.insert(key, true);
    }
    if evidence.code_files == 0 {
        if evidence.docs_files > 0 && evidence.non_code_files == 0 {
            report.summary.docs_only += 1;
        } else {
            report.summary.non_code += 1;
        }
        return Ok(());
    }
    let work = decode_work_snapshot(detail.get("work"));
    let mut delivery = Delivery {
        event_seq: ev.seq,
        project: project.clone(),
        at: ev.at.clone(),
        commit,
        code_files: evidence.code_files,
        files_changed: evidence.files_changed,
        code_paths: safe_code_paths_for_report(&evidence.code_paths),
        paths_truncated: evidence.paths_truncated,
        evidence: evidence.evidence,
        tasks: Vec::new(),
        ..Delivery::default()
    };
    if let Some(job) = &work.job
        && !job.id.is_empty()
    {
        delivery.job = Some(JobRef {
            id: job.id.clone(),
            title: job.title.clone(),
            kind: job.kind.clone(),
            status: job.status.clone(),
        });
    }
    if let Some(worker) = &work.worker
        && !worker.name.is_empty()
    {
        delivery.worker = Some(WorkerRef {
            name: worker.name.clone(),
            runner: worker.runner.clone(),
            model: worker.model.clone(),
        });
    }
    let mut source = String::new();
    let mut job_id = String::new();
    if let Some(job) = &work.job {
        source.clone_from(&job.source);
        job_id.clone_from(&job.id);
    }
    delivery.tasks = delivery_tasks(ctx, s, &project, &job_id, &source)?;
    let key = format!("{project}\0{source}");
    if !suggestions.contains_key(&key) {
        suggestions.insert(key.clone(), delivery_suggestion(ctx, s, &project, &source)?);
    }
    delivery.suggestion = suggestions.get(&key).cloned().unwrap_or(None);
    report.deliveries.push(delivery);
    report.summary.code_deliveries += 1;
    report.summary.code_files += evidence.code_files;
    Ok(())
}

fn add_unavailable(report: &mut Report, ev: &EventRow, project: &str, reason: &str) {
    report.summary.unknown += 1;
    report.unavailable.push(Unavailable {
        event_seq: ev.seq,
        project: project.to_string(),
        at: ev.at.clone(),
        reason: public_reason(reason),
    });
}

fn delivery_tasks(
    ctx: &CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    job_id: &str,
    source: &str,
) -> Result<Vec<TaskRef>> {
    let conn = s.conn_ctx(ctx)?;
    let mut refs = Vec::new();
    let mut seen = BTreeMap::new();
    if let Some(id) = source.strip_prefix("task:")
        && !id.is_empty()
    {
        let mut stmt = conn
            .prepare("SELECT id, title FROM tasks WHERE project = ? AND id = ? ORDER BY id")
            .map_err(|e| ctx_error(ctx, e.into()))?;
        let mut rows = stmt
            .query(rusqlite::params![project, id])
            .map_err(|e| ctx_error(ctx, e.into()))?;
        add_task_rows(ctx, &mut rows, &mut refs, &mut seen)?;
    }
    if !job_id.is_empty() {
        let mut stmt = conn
            .prepare("SELECT id, title FROM tasks WHERE project = ? AND job_id = ? ORDER BY id")
            .map_err(|e| ctx_error(ctx, e.into()))?;
        let mut rows = stmt
            .query(rusqlite::params![project, job_id])
            .map_err(|e| ctx_error(ctx, e.into()))?;
        add_task_rows(ctx, &mut rows, &mut refs, &mut seen)?;
    }
    Ok(refs)
}

fn add_task_rows(
    ctx: &CancellationToken,
    rows: &mut rusqlite::Rows<'_>,
    refs: &mut Vec<TaskRef>,
    seen: &mut BTreeMap<String, bool>,
) -> Result<()> {
    while let Some(row) = rows.next().map_err(|e| ctx_error(ctx, e.into()))? {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        if !seen.get(&id).copied().unwrap_or(false) {
            seen.insert(id.clone(), true);
            refs.push(TaskRef { id, title });
        }
    }
    Ok(())
}

fn delivery_suggestion(
    ctx: &CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    source: &str,
) -> Result<Option<SuggestionRef>> {
    let Some(id) = source.strip_prefix("suggestion:") else {
        return Ok(None);
    };
    if id.is_empty() {
        return Ok(None);
    }
    let Some(post) = load_suggestion(ctx, s, project, id)? else {
        return Ok(None);
    };
    let votes = suggestion_votes(ctx, s, &post)?;
    Ok(Some(SuggestionRef {
        id: post.id,
        summary: post.summary,
        area: post.area,
        thread: post.thread,
        votes,
    }))
}

fn load_suggestion(
    ctx: &CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    id: &str,
) -> Result<Option<SuggestionPost>> {
    let conn = s.conn_ctx(ctx)?;
    conn.query_row(
        "SELECT id, project, area, thread, summary FROM posts\n\t\tWHERE project = ? AND id = ? AND kind = 'suggestion'",
        rusqlite::params![project, id],
        |r| {
            Ok(SuggestionPost {
                id: r.get(0)?,
                project: r.get(1)?,
                area: r.get(2)?,
                thread: r.get(3)?,
                summary: r.get(4)?,
            })
        },
    )
    .optional()
    .map_err(|e| ctx_error(ctx, e.into()))
}

fn suggestion_votes(
    ctx: &CancellationToken,
    s: &pulse_store::Store,
    post: &SuggestionPost,
) -> Result<Option<Votes>> {
    if let Some(err) = ctx.err() {
        return Err(anyhow::Error::new(err));
    }
    let (up, down, found) =
        pulse_tally::suggestion_vote_counts_context(ctx, s, &post.project, &post.id)?;
    if !found {
        return Ok(None);
    }
    Ok(Some(Votes {
        up,
        down,
        score: up - down,
    }))
}

fn decode_detail(text: &str) -> (BTreeMap<String, Value>, bool) {
    match serde_json::from_str::<BTreeMap<String, Value>>(text) {
        Ok(detail) => (detail, true),
        Err(_) => (BTreeMap::new(), false),
    }
}

fn string_field(row: &BTreeMap<String, Value>, key: &str) -> String {
    let Some(value) = row.get(key) else {
        return String::new();
    };
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => pulse_store::gojson::sprint(other),
    }
}
