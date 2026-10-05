//! Port of internal/evolve/report.go.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub project: String,
    pub days: i64,
    pub since: String,
    pub now: String,
    #[serde(default, deserialize_with = "deserialize_status")]
    pub status: Option<crate::status::LoopStatus>,
    pub posts: Vec<KindCount>,
    pub suggestions: Funnel,
    pub card_jobs: CardJobCount,
    pub jobs: Vec<JobCount>,
    pub research: Research,
    pub landings: Landings,
    pub experiments: Experiments,
    pub owner: OwnerInbox,
    /// The improver's own numbers for the window and the window before it:
    /// the only place the report says whether the system got better at
    /// improving, not only how much it did.
    #[serde(default)]
    pub improver: Improver,
}

fn deserialize_status<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<crate::status::LoopStatus>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(serde_json::Value::Object(ref mut status)) = value else {
        return Ok(None);
    };
    if let Some(serde_json::Value::Object(planning)) = status.get_mut("planning") {
        for key in ["job", "query", "suggestion", "question"] {
            planning
                .entry(key)
                .or_insert_with(|| serde_json::Value::String(String::new()));
        }
    }
    serde_json::from_value(serde_json::Value::Object(status.clone()))
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Research {
    pub jobs: i64,
    pub suggestions: i64,
    pub cards: i64,
    pub votes: i64,
    pub findings: i64,
    pub answers: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct KindCount {
    pub kind: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Funnel {
    pub posted: i64,
    pub voted: i64,
    pub needs_votes: i64,
    pub eligible: i64,
    pub rejected: i64,
    pub carded: i64,
    pub built: i64,
    pub tried: i64,
    pub expired: i64,
    pub closed: i64,
    pub with_card: i64,
    pub in_scope: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CardJobCount {
    pub queued: i64,
    pub carded: i64,
    pub no_card: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct JobCount {
    pub kind: String,
    pub done_changed: i64,
    pub done_unchanged: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub open: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Landings {
    pub landed: i64,
    pub code_files: i64,
    pub docs_only: i64,
    pub pushed: i64,
    pub held: i64,
    pub conflict: i64,
    pub failed_verify: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Experiments {
    pub planned: i64,
    pub kept: i64,
    pub discarded: i64,
    #[serde(rename = "cycles_total")]
    pub cycles: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OwnerInbox {
    pub stop_questions: i64,
    pub questions: i64,
}

/// One window of the improver's productivity: what the loop delivered per
/// worker-hour, how much of it landed first time, how long it took, what it
/// cost the owner, and how often the improver itself changed meanwhile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImproverWindow {
    pub since: String,
    /// Empty for the current window, which runs to the report's `now`.
    pub until: String,
    /// Hours of finished runs that started in the window.
    pub worker_hours: f64,
    pub landed: i64,
    /// Landings plus failed verifies plus conflicts: every time the daemon
    /// tried to land a branch.
    pub attempts: i64,
    pub landings_per_hour: f64,
    /// Landed share of attempts, in percent; 0 when nothing was attempted.
    pub first_pass_pct: f64,
    /// Median minutes from a job's creation to its landing.
    pub lead_minutes_median: f64,
    /// Questions marked for the owner that were posted in the window.
    pub owner_asks: i64,
    /// Landings whose code paths touch the improver (`IMPROVER_PREFIXES`).
    pub improver_landings: i64,
    /// Daemon rebuilds from the brain's commits; 0 for other projects.
    pub self_updates: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Improver {
    pub now: ImproverWindow,
    pub before: ImproverWindow,
}

/// Paths that decide what the agents do and how their work lands: the
/// planner, daemon, landing and harness, the loop, the model providers, the
/// board's rules and the ballot. A landing under one of these changed the
/// improver, so the window after it is the measurement of that change.
pub const IMPROVER_PREFIXES: &[&str] = &[
    "crates/pulse-runner/",
    "crates/pulse-evolve/",
    "crates/pulse-harness/",
    "crates/pulse-provider",
    "crates/pulse-board/",
    "crates/pulse-tally/",
    "crates/pulse-tasks/",
    "crates/pulse-goals",
];

pub fn is_improver_path(path: &str) -> bool {
    IMPROVER_PREFIXES.iter().any(|p| path.starts_with(p))
}

/// Go: func BuildReport(s *store.Store, project string, days int) (*Report, error) {
pub fn build_report(s: &pulse_store::Store, project: &str, days: i64) -> Result<Report> {
    let days = if days <= 0 { 7 } else { days };
    let now = s.now();
    let at = DateTime::parse_from_rfc3339(&now)?;
    let since = (at.with_timezone(&Utc) - Duration::days(days))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let status = crate::status::status(s, project)?;
    let before_since = (at.with_timezone(&Utc) - Duration::days(2 * days))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    Ok(Report {
        project: project.to_string(),
        days,
        since: since.clone(),
        now: now.clone(),
        posts: post_counts(s, project, &since)?,
        suggestions: suggestion_funnel(s, project, &since, &status)?,
        card_jobs: card_job_counts(s, project, &since)?,
        jobs: job_counts(s, project, &since)?,
        research: research_counts(s, project, &since)?,
        landings: landing_counts(s, project, &since)?,
        experiments: experiment_counts(s, project, &since, &status)?,
        owner: owner_inbox(s, project)?,
        improver: Improver {
            now: improver_window(s, project, &since, None)?,
            before: improver_window(s, project, &before_since, Some(&since))?,
        },
        status: Some(status),
    })
}

/// Go: func postCounts(s *store.Store, project, since string) ([]KindCount, error) {
pub(crate) fn post_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<Vec<KindCount>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT kind, COUNT(*) FROM posts WHERE project = ? AND date >= ? GROUP BY kind ORDER BY COUNT(*) DESC, kind")?;
    let rows = stmt.query_map(params![project, since], |r| {
        Ok(KindCount {
            kind: r.get(0)?,
            count: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Go: func suggestionFunnel(s *store.Store, project, since string, status *LoopStatus) (Funnel, error) {
pub(crate) fn suggestion_funnel(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
    status: &crate::status::LoopStatus,
) -> Result<Funnel> {
    let mut f = Funnel::default();
    let result = match pulse_tally::compute(s, project, "", pulse_tally::Thresholds::default()) {
        Ok(r) => r,
        Err(e) if e.downcast_ref::<pulse_tally::ApplyError>().is_some() => return Ok(f),
        Err(e) => return Err(e),
    };
    let posts: BTreeMap<String, pulse_tally::Post> = result
        .posts
        .iter()
        .map(|p| (p.id.clone(), p.clone()))
        .collect();
    for sg in result.suggestions {
        if sg.date.as_str() < since {
            continue;
        }
        f.posted += 1;
        if sg.voters > 0 {
            f.voted += 1;
        }
        match sg.status.as_str() {
            "needs votes" => f.needs_votes += 1,
            "eligible" => f.eligible += 1,
            "rejected" => f.rejected += 1,
            "carded" => f.carded += 1,
            "built" => f.built += 1,
            "tried" => f.tried += 1,
            "expired" => f.expired += 1,
            "closed" => f.closed += 1,
            _ => {}
        }
        if let Some(p) = posts.get(&sg.id) {
            let fields = crate::card::card_fields(&p.body);
            if !fields.get("file").cloned().unwrap_or_default().is_empty()
                && !fields
                    .get("variable")
                    .cloned()
                    .unwrap_or_default()
                    .is_empty()
            {
                f.with_card += 1;
            }
            if crate::card::make_card(s, p, status).is_ok() {
                f.in_scope += 1;
            }
        }
    }
    Ok(f)
}

/// Go: func cardJobCounts(s *store.Store, project, since string) (CardJobCount, error) {
pub(crate) fn card_job_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<CardJobCount> {
    let mut c = CardJobCount::default();
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        r#"SELECT status, result FROM jobs WHERE project = ? AND kind = 'reflect' AND source LIKE 'suggestion:%'
		AND title LIKE ? AND created_at >= ?"#,
    )?;
    let mut rows = stmt.query(params![
        project,
        format!("{}%", crate::card_job::CARD_JOB_TITLE),
        since
    ])?;
    while let Some(row) = rows.next()? {
        let status: String = row.get(0)?;
        let result: String = row.get(1)?;
        if status == "queued" || status == "leased" || status == "blocked" {
            c.queued += 1;
        } else if status == "done" && result.trim().to_ascii_lowercase().starts_with("no card") {
            c.no_card += 1;
        } else if status == "done" {
            c.carded += 1;
        } else {
            c.failed += 1;
        }
    }
    Ok(c)
}

pub(crate) const RESEARCH_JOB_SQL: &str = "(j.kind = 'reflect' AND j.source LIKE 'research:%')";

/// Go: func jobCounts(s *store.Store, project, since string) ([]JobCount, error) {
pub(crate) fn job_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<Vec<JobCount>> {
    let conn = s.conn()?;
    let q = format!(
        r#"SELECT CASE WHEN {RESEARCH_JOB_SQL} THEN 'research' ELSE j.kind END AS kind, j.status,
		(j.branch != '' OR EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.commits > 0)) AS changed
		FROM jobs j WHERE j.project = ? AND j.created_at >= ?"#
    );
    let mut stmt = conn.prepare(&q)?;
    let mut rows = stmt.query(params![project, since])?;
    let mut by_kind: BTreeMap<String, JobCount> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let status: String = row.get(1)?;
        let changed: bool = row.get(2)?;
        let c = by_kind.entry(kind.clone()).or_insert_with(|| JobCount {
            kind,
            ..Default::default()
        });
        match status.as_str() {
            "done" if changed => c.done_changed += 1,
            "done" => c.done_unchanged += 1,
            "failed" => c.failed += 1,
            "cancelled" => c.cancelled += 1,
            _ => c.open += 1,
        }
    }
    Ok(by_kind.into_values().collect())
}

/// Go: func researchCounts(s *store.Store, project, since string) (Research, error) {
pub(crate) fn research_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<Research> {
    let mut out = Research::default();
    let conn = s.conn()?;
    let q = format!(
        "SELECT COUNT(*) FROM jobs j WHERE j.project = ? AND {RESEARCH_JOB_SQL} AND j.created_at >= ?"
    );
    out.jobs = conn.query_row(&q, params![project, since], |r| r.get(0))?;
    let q = format!(
        r#"SELECT p.kind, p.body FROM posts p JOIN runs r ON r.id = p.session JOIN jobs j ON j.id = r.job_id
		WHERE j.project = ? AND {RESEARCH_JOB_SQL} AND p.created_at >= ?"#
    );
    let mut stmt = conn.prepare(&q)?;
    let mut rows = stmt.query(params![project, since])?;
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let body: String = row.get(1)?;
        match kind.as_str() {
            "suggestion" => {
                out.suggestions += 1;
                let fields = crate::card::card_fields(&body);
                if !fields.get("file").cloned().unwrap_or_default().is_empty()
                    && !fields
                        .get("variable")
                        .cloned()
                        .unwrap_or_default()
                        .is_empty()
                {
                    out.cards += 1;
                }
            }
            "vote" => out.votes += 1,
            "finding" => out.findings += 1,
            "answer" => out.answers += 1,
            _ => {}
        }
    }
    Ok(out)
}

/// Go: func landingCounts(s *store.Store, project, since string) (Landings, error) {
pub(crate) fn landing_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<Landings> {
    let mut l = Landings::default();
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        r#"SELECT action, detail FROM events WHERE action IN ('land.landed', 'land.pushed', 'land.held', 'land.conflict', 'land.failed-verify')
		AND at >= ? AND detail LIKE '%"project"%'"#,
    )?;
    let mut rows = stmt.query([since])?;
    while let Some(row) = rows.next()? {
        let action: String = row.get(0)?;
        let detail: String = row.get(1)?;
        let Ok(d) = serde_json::from_str::<serde_json::Value>(&detail) else {
            continue;
        };
        if d.get("project").and_then(|v| v.as_str()) != Some(project) {
            continue;
        }
        match action.as_str() {
            "land.landed" => {
                l.landed += 1;
                if let Some(cd) = d.get("code_delivery") {
                    let code = cd.get("code_files").and_then(|v| v.as_i64()).unwrap_or(0);
                    let docs = cd.get("docs_files").and_then(|v| v.as_i64()).unwrap_or(0);
                    let changed = cd
                        .get("files_changed")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    if code > 0 {
                        l.code_files += 1;
                    } else if docs > 0 && docs == changed {
                        l.docs_only += 1;
                    }
                }
            }
            "land.pushed" => l.pushed += 1,
            "land.held" => l.held += 1,
            "land.conflict" => l.conflict += 1,
            "land.failed-verify" => l.failed_verify += 1,
            _ => {}
        }
    }
    Ok(l)
}

/// Go: func experimentCounts(s *store.Store, project, since string, status *LoopStatus) (Experiments, error) {
pub(crate) fn experiment_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
    status: &crate::status::LoopStatus,
) -> Result<Experiments> {
    let mut e = Experiments {
        cycles: status.cycles,
        ..Default::default()
    };
    let conn = s.conn()?;
    e.planned = conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE project = ? AND kind = 'experiment' AND created_at >= ?",
        params![project, since],
        |r| r.get(0),
    )?;
    for a in crate::archive::list_archive(s, project)? {
        if a.at.as_str() < since || a.hypothesis == crate::api::BASELINE_HYPOTHESIS {
            continue;
        }
        if a.kept {
            e.kept += 1;
        } else {
            e.discarded += 1;
        }
    }
    Ok(e)
}

/// The improver's numbers for `[since, until)`, or from `since` on when
/// `until` is `None` (the current window, like every other count in the
/// report). Landing attempts and lead times come from the landing events,
/// worker hours from the runs that started in the window, owner asks from
/// the board, self-updates from the daemon's own events (brain only).
pub(crate) fn improver_window(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
    until: Option<&str>,
) -> Result<ImproverWindow> {
    improver_window_for(s, Some(project), since, until)
}

pub fn loop_window(
    s: &pulse_store::Store,
    project: Option<&str>,
    since: &str,
    until: Option<&str>,
) -> Result<ImproverWindow> {
    improver_window_for(s, project, since, until)
}

fn improver_window_for(
    s: &pulse_store::Store,
    project: Option<&str>,
    since: &str,
    until: Option<&str>,
) -> Result<ImproverWindow> {
    let mut w = ImproverWindow {
        since: since.to_string(),
        until: until.unwrap_or_default().to_string(),
        ..Default::default()
    };
    // An open window compares against a bound no timestamp reaches.
    let until = until.unwrap_or("9999");
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        r#"SELECT e.action, e.at, e.detail, j.created_at FROM events e
		LEFT JOIN jobs j ON j.id = json_extract(e.detail, '$.job')
		WHERE e.action IN ('land.landed', 'land.conflict', 'land.failed-verify')
		AND (?1 IS NULL OR json_extract(e.detail, '$.project') = ?1)
		AND e.at >= ?2 AND e.at < ?3"#,
    )?;
    let mut leads: Vec<f64> = Vec::new();
    let mut rows = stmt.query(params![project, since, until])?;
    while let Some(row) = rows.next()? {
        let action: String = row.get(0)?;
        let at: String = row.get(1)?;
        let detail: String = row.get(2)?;
        let created: Option<String> = row.get(3)?;
        w.attempts += 1;
        if action != "land.landed" {
            continue;
        }
        w.landed += 1;
        if let (Ok(a), Some(Ok(c))) = (
            DateTime::parse_from_rfc3339(&at),
            created.as_deref().map(DateTime::parse_from_rfc3339),
        ) {
            let minutes = (a - c).num_seconds() as f64 / 60.0;
            if minutes >= 0.0 {
                leads.push(minutes);
            }
        }
        let d: serde_json::Value = serde_json::from_str(&detail).unwrap_or_default();
        let touches_improver = d
            .pointer("/code_delivery/code_paths")
            .and_then(|v| v.as_array())
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(|p| p.as_str())
                    .any(is_improver_path)
            })
            .unwrap_or(false);
        if touches_improver {
            w.improver_landings += 1;
        }
    }
    leads.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    w.lead_minutes_median = median(&leads);
    let seconds: f64 = conn.query_row(
        "SELECT COALESCE(SUM(seconds), 0) FROM runs WHERE (?1 IS NULL OR project = ?1) AND status != 'running' AND started_at >= ?2 AND started_at < ?3",
        params![project, since, until],
        |r| r.get(0),
    )?;
    w.worker_hours = seconds / 3600.0;
    if w.worker_hours > 0.0 {
        w.landings_per_hour = w.landed as f64 / w.worker_hours;
    }
    if w.attempts > 0 {
        w.first_pass_pct = 100.0 * w.landed as f64 / w.attempts as f64;
    }
    let mut asks = conn.prepare(
        "SELECT extra FROM posts WHERE (?1 IS NULL OR project = ?1) AND kind = 'question' AND date >= ?2 AND date < ?3",
    )?;
    let mut rows = asks.query(params![project, since, until])?;
    while let Some(row) = rows.next()? {
        let extra: String = row.get(0)?;
        if pulse_store::post_for_owner(&extra).unwrap_or(false) {
            w.owner_asks += 1;
        }
    }
    if project
        .map(|p| s.project(p).map(|p| p.map(|p| p.brain).unwrap_or(false)))
        .transpose()?
        .unwrap_or(true)
    {
        w.self_updates = conn.query_row(
            "SELECT COUNT(*) FROM events WHERE action = 'daemon.self_update' AND at >= ? AND at < ?",
            params![since, until],
            |r| r.get(0),
        )?;
    }
    Ok(w)
}

fn median(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn improver_line(w: &ImproverWindow) -> String {
    let per_hour = if w.worker_hours > 0.0 {
        format!("{:.2} per worker-hour", w.landings_per_hour)
    } else {
        "no finished runs".to_string()
    };
    let first_pass = if w.attempts > 0 {
        format!(
            "first pass {:.0}% of {} attempt(s)",
            w.first_pass_pct, w.attempts
        )
    } else {
        "no landing attempts".to_string()
    };
    let lead = if w.landed > 0 {
        format!("lead time median {:.0} min", w.lead_minutes_median)
    } else {
        "no lead time".to_string()
    };
    let asks_per_landing = if w.landed > 0 {
        format!("{:.2} per landing", w.owner_asks as f64 / w.landed as f64)
    } else {
        "no landing".to_string()
    };
    format!(
        "{} landed in {:.1} worker-hour(s) = {}; {}; {}; {} owner ask(s) = {}; {} landing(s) changed the improver; {} self-update(s)",
        w.landed,
        w.worker_hours,
        per_hour,
        first_pass,
        lead,
        w.owner_asks,
        asks_per_landing,
        w.improver_landings,
        w.self_updates
    )
}

/// `before -> now` for every number a reader compares, with the direction
/// that counts as better named so the line reads without the table.
fn improver_trend(before: &ImproverWindow, now: &ImproverWindow) -> String {
    if before.landed == 0 && before.attempts == 0 && before.worker_hours <= 0.0 {
        return "no finished runs in the window before; nothing to compare yet".to_string();
    }
    let pct = |b: f64, n: f64| -> String {
        if b <= 0.0 {
            return String::new();
        }
        format!(" ({:+.0}%)", 100.0 * (n - b) / b)
    };
    let mut line = format!(
        "landings per worker-hour {:.2} -> {:.2}{} (higher is better); first pass {:.0}% -> {:.0}%; lead time median {:.0} -> {:.0} min (lower is better); owner asks per landing {:.2} -> {:.2} (lower is better)",
        before.landings_per_hour,
        now.landings_per_hour,
        pct(before.landings_per_hour, now.landings_per_hour),
        before.first_pass_pct,
        now.first_pass_pct,
        before.lead_minutes_median,
        now.lead_minutes_median,
        asks_per_landing(before),
        asks_per_landing(now),
    );
    // A trend against a handful of landings says little; say so on the line.
    if before.landed < THIN_WINDOW_LANDINGS {
        line.push_str(&format!(
            "; only {} landing(s) in the window before - a thin comparison",
            before.landed
        ));
    }
    line
}

/// Below this many landings the window before is too thin to trend against.
pub const THIN_WINDOW_LANDINGS: i64 = 5;

fn asks_per_landing(w: &ImproverWindow) -> f64 {
    if w.landed == 0 {
        return 0.0;
    }
    w.owner_asks as f64 / w.landed as f64
}

/// Go: func ownerInbox(s *store.Store, project string) (OwnerInbox, error) {
pub(crate) fn owner_inbox(s: &pulse_store::Store, project: &str) -> Result<OwnerInbox> {
    let mut o = OwnerInbox::default();
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        r#"SELECT q.summary, q.extra FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND NOT EXISTS (SELECT 1 FROM decisions d WHERE d.post_id = q.id)
		AND NOT EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date)"#,
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let summary: String = row.get(0)?;
        let extra: String = row.get(1)?;
        if !pulse_store::post_for_owner(&extra).unwrap_or(false) {
            continue;
        }
        if summary == crate::cycle::STOP_SUMMARY {
            o.stop_questions += 1;
        } else {
            o.questions += 1;
        }
    }
    Ok(o)
}

/// Go: func reportRows(r *Report) [][]string {
pub(crate) fn report_rows(r: &Report) -> Vec<Vec<String>> {
    let mut posts: Vec<String> = r
        .posts
        .iter()
        .map(|k| format!("{} {}", k.kind, k.count))
        .collect();
    if posts.is_empty() {
        posts.push("none".to_string());
    }
    let f = &r.suggestions;
    let mut rows = vec![
        vec![
            "Window".to_string(),
            format!("last {} day(s), since {}", r.days, r.since),
        ],
        vec!["Posts".to_string(), posts.join(", ")],
        vec![
            "Suggestions posted".to_string(),
            format!(
                "{}; {} voted; {} name one File: and one Variable:, {} of those inside the editable scope",
                f.posted, f.voted, f.with_card, f.in_scope
            ),
        ],
        vec![
            "Suggestions now".to_string(),
            format!(
                "{} need votes, {} eligible, {} rejected, {} carded, {} built, {} tried, {} expired, {} closed",
                f.needs_votes,
                f.eligible,
                f.rejected,
                f.carded,
                f.built,
                f.tried,
                f.expired,
                f.closed
            ),
        ],
        vec![
            "Card jobs".to_string(),
            format!(
                "{} open, {} carded, {} no card, {} failed",
                r.card_jobs.queued, r.card_jobs.carded, r.card_jobs.no_card, r.card_jobs.failed
            ),
        ],
    ];
    if r.jobs.is_empty() {
        rows.push(vec!["Jobs".to_string(), "none".to_string()]);
    }
    for j in &r.jobs {
        rows.push(vec![
            format!("Jobs {}", j.kind),
            format!(
                "{} done with commits, {} done without, {} failed, {} cancelled, {} open",
                j.done_changed, j.done_unchanged, j.failed, j.cancelled, j.open
            ),
        ]);
    }
    rows.push(vec![
        "Research".to_string(),
        format!(
            "{} job(s); left {} suggestion(s) ({} cards), {} vote(s), {} finding(s), {} answer(s)",
            r.research.jobs,
            r.research.suggestions,
            r.research.cards,
            r.research.votes,
            r.research.findings,
            r.research.answers
        ),
    ]);
    let l = &r.landings;
    rows.extend([
        vec![
            "Landings".to_string(),
            format!(
                "{} landed ({} with code files, {} docs only); {} pushed; {} held, {} conflicts, {} failed verify",
                l.landed, l.code_files, l.docs_only, l.pushed, l.held, l.conflict, l.failed_verify
            ),
        ],
        vec![
            "Experiments".to_string(),
            format!(
                "{} planned; {} kept, {} discarded; {} cycles since the loop was enabled",
                r.experiments.planned, r.experiments.kept, r.experiments.discarded, r.experiments.cycles
            ),
        ],
        vec![
            "Owner inbox".to_string(),
            format!(
                "{} undecided stop question(s), {} other open question(s)",
                r.owner.stop_questions, r.owner.questions
            ),
        ],
        vec![
            format!("Improver last {}d", r.days),
            improver_line(&r.improver.now),
        ],
        vec![
            format!("Improver {}d before", r.days),
            improver_line(&r.improver.before),
        ],
        vec![
            "Improver trend".to_string(),
            improver_trend(&r.improver.before, &r.improver.now),
        ],
    ]);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::join_test_agent;
    use crate::test_helpers_e2::*;

    #[test]
    fn build_report_counts_what_the_loop_and_the_operating_layer_did() {
        let _serial = crate::test_helpers::serial();
        let _guard = e2_guard();
        let s = cycle_ready();
        join_test_agent(&s, "card-writer");
        let cardless = cardless_suggestion(&s, "shape", "Shorten the daily brief", &["up", "up"]);
        let carded = voted_suggestion(&s, "card", "product", "value.txt", 1);
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: "orphan".into(),
                kind: "finding".into(),
                agent: "proposer".into(),
                summary: "A finding is not a suggestion".into(),
                ..Default::default()
            },
        )
        .unwrap();
        report_job(
            &s,
            "reflect",
            &(crate::card_job::CARD_JOB_TITLE.to_string() + &cardless.id),
            &format!("suggestion:{}", cardless.id),
            "done",
            0,
        );
        report_job(
            &s,
            "improve",
            "Improve: changed",
            &format!("suggestion:{}", carded.id),
            "done",
            2,
        );
        report_job(&s, "improve", "Improve: unchanged", "board", "done", 0);
        report_job(&s, "lead", "Lead: failed", "board", "failed", 0);
        report_job(
            &s,
            "experiment",
            "Experiment: open",
            &format!("suggestion:{}", carded.id),
            "",
            0,
        );
        for detail in [
            serde_json::json!({"project": "demo", "code_delivery": {"code_files": 2, "docs_files": 1, "files_changed": 3}}),
            serde_json::json!({"project": "demo", "code_delivery": {"code_files": 0, "docs_files": 1, "files_changed": 1}}),
            serde_json::json!({"project": "other", "code_delivery": {"code_files": 5}}),
        ] {
            s.event("daemon", "land.landed", "job", detail).unwrap();
        }
        s.event(
            "daemon",
            "land.held",
            "job",
            serde_json::json!({"project": "demo", "reason": "main moved"}),
        )
        .unwrap();
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: "loop-gate".into(),
                kind: "question".into(),
                agent: "proposer".into(),
                for_owner: true,
                summary: crate::cycle::STOP_SUMMARY.into(),
                body: "a stop rule fired".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let decided = pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: "decided".into(),
                kind: "question".into(),
                agent: "proposer".into(),
                for_owner: true,
                summary: "Which scorer?".into(),
                body: "answered by pulse decide".into(),
                ..Default::default()
            },
        )
        .unwrap();
        pulse_board::decide(&s, &decided.id, "yes", "keep the scorer", "human").unwrap();
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: "open".into(),
                kind: "question".into(),
                agent: "proposer".into(),
                for_owner: true,
                summary: "Allow product scope?".into(),
                body: "still waiting".into(),
                ..Default::default()
            },
        )
        .unwrap();
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: "agents".into(),
                kind: "question".into(),
                agent: "proposer".into(),
                summary: "An agent question is not owner inbox".into(),
                body: "peer question".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let r = build_report(&s, "demo", 0).unwrap();
        assert!(r.days == 7 && r.since < r.now && r.status.as_ref().unwrap().project == "demo");
        assert_eq!(r.suggestions.posted, 2);
        assert_eq!(r.suggestions.voted, 2);
        assert_eq!(r.suggestions.with_card, 1);
        assert_eq!(r.suggestions.in_scope, 1);
        assert_eq!(r.suggestions.carded, 1);
        assert_eq!(r.card_jobs.no_card, 1);
        assert_eq!(r.card_jobs.carded, 0);
        assert_eq!(r.card_jobs.queued, 0);
        let got: std::collections::BTreeMap<_, _> =
            r.jobs.iter().map(|j| (j.kind.as_str(), j)).collect();
        assert_eq!(got["improve"].done_changed, 1);
        assert_eq!(got["improve"].done_unchanged, 1);
        assert_eq!(got["lead"].failed, 1);
        assert_eq!(got["experiment"].open, 1);
        assert_eq!(got["reflect"].done_unchanged, 1);
        assert_eq!(r.landings.landed, 2);
        assert_eq!(r.landings.code_files, 1);
        assert_eq!(r.landings.docs_only, 1);
        assert_eq!(r.landings.held, 1);
        assert_eq!(r.experiments.planned, 1);
        assert_eq!(r.owner.stop_questions, 1);
        assert_eq!(r.owner.questions, 1);
        let kinds: std::collections::BTreeMap<_, _> =
            r.posts.iter().map(|k| (k.kind.as_str(), k.count)).collect();
        assert_eq!(kinds["suggestion"], 2);
        assert_eq!(kinds["finding"], 1);
        assert_eq!(kinds["question"], 4);
        let text = report_rows(&r)
            .into_iter()
            .map(|row| row.join(" | "))
            .collect::<Vec<_>>()
            .join("\n");
        for want in [
            "Suggestions posted",
            "Card jobs",
            "improve",
            "Landings",
            "Owner inbox",
        ] {
            assert!(text.contains(want), "{text}");
        }
        let raw = serde_json::to_string(&r).unwrap();
        let back: Report = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.suggestions, r.suggestions);
        assert_eq!(back.landings, r.landings);
        assert_eq!(back.owner, r.owner);
    }
    #[test]
    fn improver_lines_read_plainly_when_a_window_is_empty() {
        let empty = ImproverWindow::default();
        assert_eq!(
            improver_line(&empty),
            "0 landed in 0.0 worker-hour(s) = no finished runs; no landing attempts; no lead time; 0 owner ask(s) = no landing; 0 landing(s) changed the improver; 0 self-update(s)"
        );
        let now = ImproverWindow {
            landed: 2,
            attempts: 2,
            worker_hours: 1.0,
            landings_per_hour: 2.0,
            first_pass_pct: 100.0,
            ..Default::default()
        };
        assert_eq!(
            improver_trend(&empty, &now),
            "no finished runs in the window before; nothing to compare yet"
        );
        // Runs without a landing still make a window worth comparing.
        let thin = ImproverWindow {
            worker_hours: 0.5,
            attempts: 1,
            ..Default::default()
        };
        let line = improver_trend(&thin, &now);
        assert!(
            line.starts_with("landings per worker-hour 0.00 -> 2.00 (higher is better)"),
            "{line}"
        );
        assert!(
            line.ends_with("only 0 landing(s) in the window before - a thin comparison"),
            "{line}"
        );
    }
    #[test]
    fn improver_window_measures_the_loop_against_the_window_before() {
        let _serial = crate::test_helpers::serial();
        let _guard = e2_guard();
        let s = cycle_ready();
        join_test_agent(&s, "card-writer");
        let now = DateTime::parse_from_rfc3339(&s.now())
            .unwrap()
            .with_timezone(&Utc);
        let iso = |d: DateTime<Utc>| d.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let conn = s.conn().unwrap();
        // Three jobs: two land this week (one touching the improver, one not),
        // one landed last week; one branch fails verify this week.
        let mut job_ids = Vec::new();
        for (i, created) in [
            now - Duration::hours(3),
            now - Duration::hours(1),
            now - Duration::days(8),
        ]
        .iter()
        .enumerate()
        {
            let job = report_job(&s, "improve", &format!("Improve {i}"), "board", "done", 1);
            conn.execute(
                "UPDATE jobs SET created_at = ? WHERE id = ?",
                params![iso(*created), job.id],
            )
            .unwrap();
            job_ids.push(job.id);
        }
        let event = |action: &str, at: DateTime<Utc>, detail: serde_json::Value| {
            conn.execute(
                "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'daemon', ?, 'job', ?)",
                params![iso(at), action, detail.to_string()],
            )
            .unwrap();
        };
        event(
            "land.landed",
            now - Duration::hours(2),
            serde_json::json!({"project": "demo", "job": job_ids[0], "code_delivery": {"code_files": 1, "code_paths": ["crates/pulse-runner/src/plan.rs"]}}),
        );
        event(
            "land.landed",
            now - Duration::minutes(30),
            serde_json::json!({"project": "demo", "job": job_ids[1], "code_delivery": {"code_files": 1, "code_paths": ["web/hub/src/app.tsx"]}}),
        );
        event(
            "land.failed-verify",
            now - Duration::minutes(20),
            serde_json::json!({"project": "demo", "job": job_ids[1]}),
        );
        event(
            "land.landed",
            now - Duration::days(7) - Duration::hours(2),
            serde_json::json!({"project": "demo", "job": job_ids[2], "code_delivery": {"code_files": 1, "code_paths": ["README.md"]}}),
        );
        event(
            "land.landed",
            now - Duration::hours(2),
            serde_json::json!({"project": "other", "job": job_ids[0], "code_delivery": {"code_files": 1, "code_paths": ["crates/pulse-runner/src/plan.rs"]}}),
        );
        for (job, started, seconds) in [
            (&job_ids[0], now - Duration::hours(3), 5400.0),
            (&job_ids[1], now - Duration::hours(1), 1800.0),
            (&job_ids[2], now - Duration::days(8), 3600.0),
        ] {
            // report_job already wrote a run per job at "now" without
            // seconds; move it into place so each window has known hours.
            conn.execute(
                "UPDATE runs SET started_at = ?, finished_at = ?, seconds = ? WHERE id = ?",
                params![
                    iso(started),
                    iso(started + Duration::seconds(seconds as i64)),
                    seconds,
                    format!("run-{job}")
                ],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, seconds, commits) VALUES ('run-live', ?, 'card-writer', 'demo', ?, '', 'running', 999999, 0)",
            params![job_ids[1], iso(now - Duration::minutes(5))],
        )
        .unwrap();
        drop(conn);
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "runner".into(),
                thread: "recipe".into(),
                kind: "question".into(),
                agent: "proposer".into(),
                for_owner: true,
                summary: "The recipe does not parse".into(),
                body: "pulse env import".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let r = build_report(&s, "demo", 7).unwrap();
        let now_w = &r.improver.now;
        assert_eq!((now_w.landed, now_w.attempts), (2, 3));
        assert!((now_w.worker_hours - 2.0).abs() < 1e-9, "{now_w:?}");
        assert!((now_w.landings_per_hour - 1.0).abs() < 1e-9, "{now_w:?}");
        assert!(
            (now_w.first_pass_pct - 200.0 / 3.0).abs() < 1e-6,
            "{now_w:?}"
        );
        // Lead times 60 and 30 minutes: the median of two is their mean.
        assert!((now_w.lead_minutes_median - 45.0).abs() < 1e-6, "{now_w:?}");
        assert_eq!(now_w.owner_asks, 1);
        assert_eq!(now_w.improver_landings, 1);
        assert_eq!(now_w.self_updates, 0, "demo is not the brain");
        let before = &r.improver.before;
        assert_eq!((before.landed, before.attempts), (1, 1));
        assert!((before.worker_hours - 1.0).abs() < 1e-9, "{before:?}");
        assert!(
            (before.lead_minutes_median - 1320.0).abs() < 1e-6,
            "{before:?}"
        );
        assert_eq!(before.improver_landings, 0);
        let text = report_rows(&r)
            .into_iter()
            .map(|row| row.join(" | "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("Improver last 7d | 2 landed in 2.0 worker-hour(s) = 1.00 per worker-hour; first pass 67% of 3 attempt(s); lead time median 45 min; 1 owner ask(s) = 0.50 per landing; 1 landing(s) changed the improver; 0 self-update(s)"),
            "{text}"
        );
        assert!(
            text.contains("Improver trend | landings per worker-hour 1.00 -> 1.00 (+0%) (higher is better); first pass 100% -> 67%; lead time median 1320 -> 45 min (lower is better); owner asks per landing 0.00 -> 0.50 (lower is better); only 1 landing(s) in the window before - a thin comparison"),
            "{text}"
        );
        let raw = serde_json::to_string(&r).unwrap();
        let back: Report = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.improver, r.improver);
        // A report written before this section existed still loads.
        let old: Report = serde_json::from_str(&raw.replace(
            &format!(
                ",\"improver\":{}",
                serde_json::to_string(&r.improver).unwrap()
            ),
            "",
        ))
        .unwrap();
        assert_eq!(old.improver, Improver::default());
        assert!(is_improver_path("crates/pulse-provider-copilot/src/lib.rs"));
        assert!(!is_improver_path("crates/pulse-hub/src/board.rs"));
    }
    #[test]
    fn build_report_window_excludes_older_work() {
        let _serial = crate::test_helpers::serial();
        let _guard = e2_guard();
        let s = cycle_ready();
        join_test_agent(&s, "card-writer");
        report_job(&s, "improve", "Improve: old", "board", "done", 1);
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: "open".into(),
                kind: "question".into(),
                agent: "proposer".into(),
                for_owner: true,
                summary: "Allow product scope?".into(),
                body: "still waiting".into(),
                ..Default::default()
            },
        )
        .unwrap();
        s.conn()
            .unwrap()
            .execute("UPDATE jobs SET created_at = '2020-01-01T00:00:00Z'", [])
            .unwrap();
        let r = build_report(&s, "demo", 3).unwrap();
        assert_eq!(r.days, 3);
        assert!(r.jobs.is_empty(), "{:?}", r.jobs);
        assert_eq!(r.suggestions.posted, 0);
        assert_eq!(r.landings.landed, 0);
        assert_eq!(r.experiments.cycles, r.status.as_ref().unwrap().cycles);
        assert_eq!(r.owner.questions, 1);
    }
    #[test]
    fn build_report_counts_what_research_left_behind() {
        let _serial = crate::test_helpers::serial();
        let _guard = e2_guard();
        let s = cycle_ready();
        join_test_agent(&s, "card-writer");
        let research = report_job(
            &s,
            "reflect",
            "Research (checks): the project's own checks of demo",
            "research:demo:20260906T100000Z",
            "done",
            1,
        );
        let daily = report_job(
            &s,
            "reflect",
            "Daily reflect: find and fix one provable Tier 0 issue",
            "reflect:demo:2026-09-06",
            "done",
            0,
        );
        s.conn().unwrap().execute(
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, commits) VALUES (?, ?, 'card-writer', 'demo', ?, ?, 'done', 0)",
            rusqlite::params![format!("run-{}", daily.id), daily.id, s.now(), s.now()],
        ).unwrap();
        let post = |kind: &str,
                    session: &str,
                    summary: &str,
                    body: &str,
                    extra: Option<(&str, &str, &str)>| {
            let mut input = pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: "research".into(),
                kind: kind.into(),
                agent: "card-writer".into(),
                session: session.into(),
                summary: summary.into(),
                body: body.into(),
                ..Default::default()
            };
            if let Some((on, vote, thread)) = extra {
                input.on = on.into();
                input.vote = vote.into();
                input.thread = thread.into();
            }
            pulse_board::create_post(&s, input).unwrap()
        };
        post(
            "suggestion",
            &format!("run-{}", research.id),
            "Make the flaky test deterministic",
            "Class: operating-layer\nFile: internal/x/y_test.go\nVariable: fix the clock\nHypothesis: go test passes 10 times\nTier: 0\nSignal: internal/x/y_test.go",
            None,
        );
        post(
            "suggestion",
            &format!("run-{}", research.id),
            "A one-liner without a card",
            "",
            None,
        );
        post(
            "finding",
            &format!("run-{}", research.id),
            "The lint step is not in CI",
            "",
            None,
        );
        post(
            "answer",
            &format!("run-{}", research.id),
            "The scorer lives in score.go",
            "",
            None,
        );
        let target = cardless_suggestion(&s, "shape", "Shorten the daily brief", &["up"]);
        post(
            "vote",
            &format!("run-{}", research.id),
            "up",
            "",
            Some((&target.id, "up", &target.thread)),
        );
        post(
            "finding",
            &format!("run-{}", daily.id),
            "Daily reflect finding",
            "",
            None,
        );
        post("suggestion", "", "Interactive suggestion", "", None);
        let r = build_report(&s, "demo", 0).unwrap();
        assert_eq!(
            r.research,
            Research {
                jobs: 1,
                suggestions: 2,
                cards: 1,
                votes: 1,
                findings: 1,
                answers: 1
            }
        );
        let got: std::collections::BTreeMap<_, _> =
            r.jobs.iter().map(|j| (j.kind.as_str(), j)).collect();
        assert_eq!(got["research"].done_changed, 1);
        assert_eq!(got["reflect"].done_unchanged, 1);
        assert_eq!(got["reflect"].done_changed, 0);
        let text = report_rows(&r)
            .into_iter()
            .map(|row| row.join(" | "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Research | 1 job(s); left 2 suggestion(s) (1 cards), 1 vote(s), 1 finding(s), 1 answer(s)"), "{text}");
        assert!(
            text.contains("Jobs research | 1 done with commits"),
            "{text}"
        );
    }
}
