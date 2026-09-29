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
    pub status: Option<crate::status::LoopStatus>,
    pub posts: Vec<KindCount>,
    pub suggestions: Funnel,
    pub card_jobs: CardJobCount,
    pub jobs: Vec<JobCount>,
    pub research: Research,
    pub landings: Landings,
    pub experiments: Experiments,
    pub owner: OwnerInbox,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Research {
    pub jobs: i64,
    pub suggestions: i64,
    pub cards: i64,
    pub votes: i64,
    pub findings: i64,
    pub answers: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KindCount {
    pub kind: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardJobCount {
    pub queued: i64,
    pub carded: i64,
    pub no_card: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobCount {
    pub kind: String,
    pub done_changed: i64,
    pub done_unchanged: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub open: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Landings {
    pub landed: i64,
    pub code_files: i64,
    pub docs_only: i64,
    pub pushed: i64,
    pub held: i64,
    pub conflict: i64,
    pub failed_verify: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Experiments {
    pub planned: i64,
    pub kept: i64,
    pub discarded: i64,
    #[serde(rename = "cycles_total")]
    pub cycles: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OwnerInbox {
    pub stop_questions: i64,
    pub questions: i64,
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
    Ok(Report {
        project: project.to_string(),
        days,
        since: since.clone(),
        now,
        posts: post_counts(s, project, &since)?,
        suggestions: suggestion_funnel(s, project, &since, &status)?,
        card_jobs: card_job_counts(s, project, &since)?,
        jobs: job_counts(s, project, &since)?,
        research: research_counts(s, project, &since)?,
        landings: landing_counts(s, project, &since)?,
        experiments: experiment_counts(s, project, &since, &status)?,
        owner: owner_inbox(s, project)?,
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
    ]);
    rows
}

#[cfg(test)]
mod tests {
    #[test]
    fn build_report_counts_what_the_loop_and_the_operating_layer_did() {}
    #[test]
    fn build_report_window_excludes_older_work() {}
    #[test]
    fn build_report_counts_what_research_left_behind() {}
}
