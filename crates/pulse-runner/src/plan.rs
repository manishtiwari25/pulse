//! Skeleton port of internal/runner/plan.go.

use crate::util::{
    Querier, Row, day_number, event_at, must_one, must_rows, plus_hours, setting_int, stamp_of,
    trunc,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// Go: func ballotHours(s *store.Store) int
pub(crate) fn ballot_hours(s: &pulse_store::Store) -> i32 {
    let h = setting_int(s, "lead.ballot_hours");
    if h >= 1 { h } else { 1 }
}

/// Go: func recordPlanDeferred(s *store.Store, slug, now, actor, suggestionID string)
pub(crate) fn record_plan_deferred(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    actor: String,
    suggestion_id: String,
) {
    if plan_deferred_event_exists(s, slug.clone(), suggestion_id.clone()) {
        return;
    }
    if let Ok(conn) = s.conn() {
        event_at(
            &conn,
            &now,
            &actor,
            "plan.deferred",
            &suggestion_id,
            json!({"project": slug, "reason": "left to the evolution loop"}),
        );
    }
}

/// Go: func planDeferredEventExists(s *store.Store, slug, suggestionID string) bool
pub(crate) fn plan_deferred_event_exists(
    s: &pulse_store::Store,
    slug: String,
    suggestion_id: String,
) -> bool {
    let Ok(conn) = s.conn() else { return false };
    must_one(&conn, "SELECT COUNT(*) AS n FROM events WHERE action = 'plan.deferred' AND subject = ?\n\t\tAND json_valid(detail) AND json_extract(detail, '$.project') = ?", &[&suggestion_id, &slug]).int("n") != 0
}

/// Go: func lastEventSeq(s *store.Store) int
pub(crate) fn last_event_seq(s: &pulse_store::Store) -> i32 {
    let Ok(conn) = s.conn() else { return 0 };
    must_one(&conn, "SELECT COALESCE(MAX(seq), 0) AS n FROM events", &[]).int("n")
}

/// Go: func planDeferredLinesSince(s *store.Store, seq int) []string
pub(crate) fn plan_deferred_lines_since(s: &pulse_store::Store, seq: i32) -> Vec<String> {
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    must_rows(&conn, "SELECT subject, json_extract(detail, '$.project') AS project FROM events\n\t\tWHERE seq > ? AND action = 'plan.deferred' AND json_valid(detail) ORDER BY seq", &[&seq])
        .into_iter()
        .map(|r| format!("plan {}: left suggestion {} to the evolution loop", r.str("project"), r.str("subject")))
        .collect()
}

/// Go: func threadStatuses(s *store.Store, rows []row, now string) map[string]string
pub(crate) fn thread_statuses(
    s: &pulse_store::Store,
    rows: Vec<Row>,
    now: String,
) -> BTreeMap<String, String> {
    let stale_days = setting_int(s, "stale_days");
    let today = day_number(&now);
    let mut last = BTreeMap::new();
    let mut closed = BTreeMap::new();
    for r in rows {
        let key = format!("{}/{}", r.str("area"), r.str("thread"));
        if !last.contains_key(&key) || r.str("date") >= *last.get(&key).unwrap_or(&String::new()) {
            last.insert(key.clone(), r.str("date"));
        }
        if !r.str("resolution").is_empty() {
            closed.insert(key, true);
        }
    }
    last.into_iter()
        .map(|(key, d)| {
            let status = if closed.get(&key).copied().unwrap_or(false) {
                "closed"
            } else if today - day_number(&d) > stale_days {
                "stale"
            } else {
                "open"
            };
            (key, status.to_string())
        })
        .collect()
}

/// Go: func projectPosts(q querier, slug string) []row
pub(crate) fn project_posts(q: &dyn Querier, slug: String) -> Vec<Row> {
    must_rows(
        q,
        "SELECT rowid AS post_order, * FROM posts WHERE project = ? ORDER BY date, rowid",
        &[&slug],
    )
    .into_iter()
    .map(crate::board::parse_post_row)
    .collect()
}

/// Go: func openQuestions(s *store.Store, slug, now string) []row
pub(crate) fn open_questions(s: &pulse_store::Store, slug: String, now: String) -> Vec<Row> {
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    let rows = project_posts(&conn, slug);
    let status = thread_statuses(s, rows.clone(), now);
    let mut answered = BTreeMap::new();
    let mut answered_at = BTreeMap::new();
    let mut thread_answers: BTreeMap<String, Row> = BTreeMap::new();
    for r in &rows {
        if r.str("kind") != "answer" {
            continue;
        }
        let key = format!("{}/{}", r.str("area"), r.str("thread"));
        answered_at.insert(key.clone(), r.str("date"));
        if !r.str("on_post").is_empty() {
            answered.insert(r.str("on_post"), true);
        } else {
            thread_answers.insert(key, r.clone());
        }
    }
    let mut out = Vec::new();
    for r in rows {
        if r.str("kind") != "question" || r.str("area") == "jobs" {
            continue;
        }
        let key = format!("{}/{}", r.str("area"), r.str("thread"));
        if status.get(&key).is_some_and(|s| s == "closed")
            || answered.get(&r.str("id")).copied().unwrap_or(false)
            || answered_at.get(&key).is_some_and(|d| d > &r.str("date"))
        {
            continue;
        }
        if let Some(a) = thread_answers.get(&key)
            && a.str("date") == r.str("date")
            && a.int("post_order") > r.int("post_order")
        {
            continue;
        }
        out.push(r);
    }
    out
}

/// Go: func ballotCap(s *store.Store) int
pub(crate) fn ballot_cap(s: &pulse_store::Store) -> i32 {
    pulse_tally::ballot_cap(s) as i32
}

/// Go: func ballotDue(s *store.Store, slug, now string, suggestions []tally.Suggestion) []tally.Suggestion
pub(crate) fn ballot_due(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    suggestions: Vec<pulse_tally::Suggestion>,
) -> Vec<pulse_tally::Suggestion> {
    let since = plus_hours(&now, -(ballot_hours(s) as f64));
    let prefix = format!("ballot:{slug}:");
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    let start = (prefix.len() + 1) as i32;
    let last = must_one(&conn, "SELECT MAX(substr(source, ?)) AS m FROM jobs WHERE project = ? AND kind = 'ballot' AND source LIKE ?", &[&start, &slug, &format!("{prefix}%")]).str("m");
    if !last.is_empty() && last > stamp_of(&since) {
        return Vec::new();
    }
    let out: Vec<_> = suggestions
        .into_iter()
        .filter(|sg| {
            sg.status == "needs votes"
                && sg.date <= since
                && can_any_agent_vote_on_suggestion(s, sg.clone())
        })
        .collect();
    pulse_tally::limit_ballot_suggestions(&out, setting_int(s, "quorum"), ballot_cap(s) as usize)
}

/// Go: func canAnyAgentVoteOnSuggestion(s *store.Store, sg tally.Suggestion) bool
pub(crate) fn can_any_agent_vote_on_suggestion(
    s: &pulse_store::Store,
    sg: pulse_tally::Suggestion,
) -> bool {
    let Ok(conn) = s.conn() else { return false };
    !must_one(&conn, "SELECT 1 AS x FROM agents a\n\t\tWHERE a.status = 'active' AND a.kind != 'human' AND a.name != ?\n\t\tAND NOT EXISTS (SELECT 1 FROM posts v WHERE v.kind = 'vote' AND v.agent = a.name AND v.on_post = ?)\n\t\tLIMIT 1", &[&sg.agent, &sg.id]).0.is_empty()
}

/// Go: func isOwnerAddressedPost(post row) bool
pub(crate) fn is_owner_addressed_post(post: Row) -> bool {
    post.get("for_owner")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Go: func answerable(q row, plannerActor string) bool
pub(crate) fn answerable(q: Row, planner_actor: String) -> bool {
    if is_owner_addressed_post(q.clone()) || owner_targeted(q.clone()) {
        return false;
    }
    let agent = q.str("agent");
    if agent == planner_actor || agent == "daemon" {
        return false;
    }
    q.str("summary") != pulse_evolve::STOP_SUMMARY
}

/// Go: func tallyPost(s *store.Store, id string) *tally.Post
pub(crate) fn tally_post(s: &pulse_store::Store, id: String) -> Option<pulse_tally::Post> {
    let Ok(conn) = s.conn() else { return None };
    let r = crate::board::post_row(&conn, id);
    if r.0.is_empty() {
        return None;
    }
    Some(pulse_tally::Post {
        id: r.str("id"),
        project: r.str("project"),
        area: r.str("area"),
        thread: r.str("thread"),
        kind: r.str("kind"),
        agent: r.str("agent"),
        date: r.str("date"),
        summary: r.str("summary"),
        body: r.str("body"),
        for_owner: is_owner_addressed_post(r),
        ..Default::default()
    })
}

/// Go: func ownerTargeted(post row) bool
pub(crate) fn owner_targeted(post: Row) -> bool {
    pulse_tally::owner_targeted(&post.str("summary"), &post.str("body"))
}

/// Go: func planJobs(s *store.Store, slug, now, createdBy string) ([]row, error)
pub(crate) fn plan_jobs(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    created_by: String,
) -> anyhow::Result<Vec<Row>> {
    let conn = s.conn()?;
    let waiting = must_one(&conn, "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND kind != 'experiment' AND status IN ('queued', 'leased')", &[&slug]).int("n");
    if waiting >= crate::jobs::parallel_per_project(s) {
        return Ok(Vec::new());
    }
    let project = crate::board::project_by_slug(&conn, slug.clone());
    let (job, is_new) = crate::baseline::plan_baseline(
        s,
        project.clone(),
        slug.clone(),
        now.clone(),
        created_by.clone(),
    )?;
    if is_new {
        return Ok(vec![job]);
    }
    let mut created = Vec::new();
    if let Ok((Some(job), true, _)) =
        pulse_evolve::plan_draft_job(s, &slug, &created_by, &now, false)
    {
        created.push(crate::jobs::job_row(&conn, job.id));
    }
    let stuck = stuck_threads(s, slug.clone(), now.clone(), created_by.clone());
    if let Ok(handoffs) = pulse_tally::open_handoffs_for(s, &slug) {
        let tried: BTreeMap<_, _> = must_rows(
            &conn,
            "SELECT source FROM jobs WHERE source LIKE 'handoff:%'",
            &[],
        )
        .into_iter()
        .map(|r| (r.str("source"), true))
        .collect();
        for p in handoffs {
            if tried.contains_key(&format!("handoff:{}", p.id))
                || stuck
                    .get(&format!("{}/{}", p.area, p.thread))
                    .copied()
                    .unwrap_or(false)
            {
                continue;
            }
            let post = crate::board::post_row(&conn, p.id.clone());
            if post.0.is_empty() || is_owner_addressed_post(post.clone()) || owner_targeted(post) {
                continue;
            }
            let brief = format!(
                "A worker handed off thread {}/{} and nobody claimed it since (handoff {} by {}, {}).\n\n{}\n\nRead the handoff (`pulse show {}`) and the thread it names (`pulse board --project {}`), post a claim on the thread, then continue exactly the work the handoff states - one variable at a time, proven by the project's own checks. When the handoff says the next step needs the owner, post a question on the thread (a `QUESTION:` line) and stop. End with `SUMMARY:` saying what moved.",
                p.area, p.thread, p.id, p.agent, p.date, p.body, p.id, slug
            );
            let (job, is_new) = crate::jobs::add_job(
                s,
                slug.clone(),
                "improve".into(),
                format!("Continue handoff {}: {}", p.id, trunc(&p.summary, 120)),
                brief,
                1,
                8,
                format!("handoff:{}", p.id),
                created_by.clone(),
                true,
            )?;
            if is_new {
                created.push(job);
            }
            break;
        }
    }
    let suggestions = pulse_tally::compute_light(
        s,
        &slug,
        "",
        pulse_tally::Thresholds {
            now: now.clone(),
            ..Default::default()
        },
    )
    .map(|r| r.suggestions)
    .unwrap_or_default();
    if created.is_empty() {
        let cards = ballot_due(s, slug.clone(), now.clone(), suggestions);
        if !cards.is_empty() {
            let listed = cards
                .iter()
                .map(|c| {
                    format!(
                        "- `{}` {} ({} of {} voters): {}",
                        c.id,
                        c.key,
                        c.voters,
                        setting_int(s, "quorum"),
                        trunc(&c.summary, 160)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let brief = format!(
                "{} suggestion(s) on this project's board have waited more than {} hours without quorum deciding, and nothing else is queued. Read each listed card in full (its body is in the Ballot section; check the path or post it names in the clone when it names one) and cast one `VOTE: <id> up|down <reason>` line per card in your report: up when the change is one measurable product defect, correctness, reliability or user-experience improvement, down when it is a guess, a duplicate (vote on the older same File+Variable), owner-only, already done, or only adds/fixes a test/lint/format unless it unblocks landing or fixes a flake that failed a job this week. Never vote on your own cards. A card leaves the ballot once quorum decides it. Change no files and make no commits; a ballot job lands nothing. Write `SUMMARY:` with how many votes you cast and the one pattern you saw across the cards.\n\nWaiting below quorum:\n{}",
                cards.len(),
                ballot_hours(s),
                listed
            );
            let (job, is_new) = crate::jobs::add_job(
                s,
                slug.clone(),
                "ballot".into(),
                format!(
                    "Vote the ballot: {} card(s) below quorum for {}",
                    cards.len(),
                    slug
                ),
                brief,
                0,
                3,
                format!("ballot:{}:{}", slug, stamp_of(&now)),
                created_by.clone(),
                true,
            )?;
            if is_new {
                created.push(job);
            }
        }
    }
    if created.is_empty() {
        let (job, is_new) = crate::research::plan_research(s, slug, now, created_by)?;
        if is_new {
            created.push(job);
        }
    }
    Ok(created)
}

/// Go: func stuckThreads(s *store.Store, slug, now, actor string) map[string]bool
pub(crate) fn stuck_threads(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    actor: String,
) -> BTreeMap<String, bool> {
    let limit = setting_int(s, "lead.stuck_thread_jobs");
    let mut stuck = BTreeMap::new();
    if limit <= 0 {
        return stuck;
    }
    let Ok(conn) = s.conn() else { return stuck };
    let mut reset_at = BTreeMap::new();
    for r in must_rows(
        &conn,
        "SELECT area || '/' || thread AS key, MAX(date) AS d FROM posts WHERE project = ? AND kind = 'answer' AND agent IN (?, ?) GROUP BY area, thread",
        &[
            &slug,
            &crate::lead::lead_name(slug.clone()),
            &pulse_store::HUMAN_AGENT,
        ],
    ) {
        reset_at.insert(r.str("key"), r.str("d"));
    }
    #[derive(Default)]
    struct Episode {
        ids: Vec<String>,
        notes: Vec<String>,
        closed: bool,
    }
    let mut episodes: BTreeMap<String, Episode> = BTreeMap::new();
    for r in must_rows(
        &conn,
        "SELECT j.id, j.status, j.result, j.finished_at, (j.branch != '' OR EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.project = j.project AND r.agent = j.lease_agent AND r.status = 'running')) AS committed, p.area || '/' || p.thread AS key FROM jobs j JOIN posts p ON p.id = substr(j.source, instr(j.source, ':') + 1) WHERE j.project = ? AND j.status IN ('done', 'failed') AND j.kind != 'reflect' AND (j.source LIKE 'suggestion:%' OR j.source LIKE 'handoff:%' OR j.source LIKE 'question:%') ORDER BY j.finished_at DESC, j.created_at DESC",
        &[&slug],
    ) {
        let key = r.str("key");
        let ep = episodes.entry(key.clone()).or_default();
        if ep.closed {
            continue;
        }
        let reset = reset_at.get(&key).cloned().unwrap_or_default();
        if r.int("committed") != 0 || r.str("finished_at") <= reset {
            ep.closed = true;
            if r.str("finished_at") > reset {
                reset_at.insert(key, r.str("finished_at"));
            }
            continue;
        }
        ep.ids.push(r.str("id"));
        ep.notes.push(format!(
            "- {} ({}): {}",
            r.str("id"),
            r.str("status"),
            trunc(&r.str("result"), 120)
        ));
    }
    for (key, ep) in episodes {
        if ep.ids.len() < limit as usize {
            continue;
        }
        stuck.insert(key.clone(), true);
        let Some((area, thread)) = key.split_once('/') else {
            continue;
        };
        let reset = reset_at.get(&key).cloned().unwrap_or_default();
        let like = format!("{STUCK_THREAD_SUMMARY}%");
        if !must_one(&conn, "SELECT 1 AS x FROM posts WHERE project = ? AND area = ? AND thread = ? AND kind = 'question' AND summary LIKE ? AND date > ?", &[&slug, &area, &thread, &like, &reset]).0.is_empty() {
            continue;
        }
        let body = format!(
            "The planner seeded {} finished worker jobs from thread {} (its suggestions, handoffs and questions) and none committed: each run read, claimed, asked or handed off, and the next one repeated it. The planner seeds no more jobs from this thread (lead.stuck_thread_jobs = {}) until a job from it commits or the lead or the owner answers on it.\n\n@{}: decide it. Queue the one job with the decision written into its brief (`pulse job add --project {} --kind improve --source <suggestion:|handoff:|question:><post-id> --title \"...\" --brief \"...\" [--tier 0|1]`), answer on the thread with what the next worker must do differently, close the thread with an answer saying why it stops, or hand this question to the owner when only the owner can decide.\n\nJobs that changed nothing:\n{}\n",
            ep.ids.len(),
            key,
            limit,
            crate::lead::lead_name(slug.clone()),
            slug,
            ep.notes.join("\n")
        );
        let agent = crate::board::actor_agent(s, actor.clone());
        if let Err(e) = crate::board::create_post(
            s,
            now.clone(),
            crate::board::PostOpts {
                kind: "question".into(),
                project: slug.clone(),
                area: area.into(),
                thread: thread.into(),
                for_owner: true,
                agent,
                summary: format!(
                    "{STUCK_THREAD_SUMMARY} {} worker jobs in a row changed no code; lead decision needed",
                    ep.ids.len()
                ),
                body,
                ..Default::default()
            },
        ) {
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "plan.stuck.error",
                &key,
                serde_json::Value::String(e.to_string()),
            );
        }
    }
    stuck
}

/// Go: func suggestionQualityScore(body string) int
pub(crate) fn suggestion_quality_score(body: String) -> i32 {
    let b = body.to_lowercase();
    let mut score = 50;
    if FILE_LIKE_RE.is_match(&b) {
        score += 25;
    }
    if SYMBOL_LIKE_RE.is_match(&b) {
        score += 15;
    }
    if b.contains("signal:") || b.contains("hypothesis:") || b.contains("variable:") {
        score += 10;
    }
    for c in [
        "gofmt",
        "format code",
        "formatting",
        "whitespace",
        "trailing space",
        "lint only",
        "linting only",
        "increase coverage",
        "more tests",
        "add tests",
        "test coverage",
        "operating layer",
        "runner harness",
        "ci pipeline",
    ] {
        if b.contains(c) {
            score -= 20;
        }
    }
    if score < 50 && !b.contains('.') && !b.contains('`') {
        score -= 15;
    }
    score.clamp(0, 100)
}

pub(crate) const STUCK_THREAD_SUMMARY: &str = "Stuck thread:";
static FILE_LIKE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\b[a-zA-Z0-9_./-]+\.[a-zA-Z]{2,8}\b").unwrap());
static SYMBOL_LIKE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"`[^`]+`|'[a-zA-Z0-9_]+'|\b[a-zA-Z0-9_]+\(\)").unwrap());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_quality_score_prefers_concrete_signals() {
        let cases = [
            (
                "One variable: README.md opening paragraph. Signal: visitors bounce. Hypothesis: a shorter lead raises retention.",
                85,
            ),
            ("Add `ParseConfig()` error handling in cmd/start.go.", 90),
            ("gofmt everything and fix trailing whitespace", 0),
            ("Increase test coverage", 15),
            ("Make it better", 50),
        ];
        for (body, want) in cases {
            assert_eq!(suggestion_quality_score(body.into()), want, "{body}");
        }
    }
}
