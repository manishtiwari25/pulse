//! Goal review: how far a project's recent work has moved toward its goal,
//! read from the store alone - tasks, landings, failed jobs, board posts -
//! and set beside what the project's lead last said about it. The review
//! also carries the lead's proposed goal, when one waits, so the owner can
//! take it with one yes (`pulse decide`) or dismiss it.
//!
//! Four signals, weighted to 100: delivery (work finished and landed),
//! reliability (landings that passed their checks), direction (recent work
//! that speaks to the goal's own words) and momentum (this window against
//! the one before). A signal with nothing to measure is left out and the
//! score is taken over the rest; under half the weight measurable the
//! verdict is `quiet`, not a number.

use crate::Goal;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Board area the lead's readings and proposals live on.
pub const AREA: &str = "goal";
/// Thread of the lead's readings (`finding` posts with a `Score:` line).
pub const REVIEW_THREAD: &str = "review";
/// Thread of proposed goals (`opinion` posts whose body starts with
/// [`AMEND_MARKER`]).
pub const AMEND_THREAD: &str = "amend";
/// First line of a proposal body. It is a decision marker: a yes on the
/// post sets the goal and queues no worker job (pulse-board).
pub const AMEND_MARKER: &str = "goal: amend";
/// Setting: days between lead readings before one is due again.
pub const REVIEW_DAYS_SETTING: &str = "goal.review_days";
pub const DEFAULT_REVIEW_DAYS: i64 = 7;
/// Event written when a decided proposal is applied (or could not be).
pub const AMEND_ACTION: &str = "goal.amend";

pub const ON_TRACK: &str = "on-track";
pub const SLIPPING: &str = "slipping";
pub const STALLED: &str = "stalled";
pub const QUIET: &str = "quiet";

const WEIGHT_DELIVERY: i32 = 30;
const WEIGHT_RELIABILITY: i32 = 25;
const WEIGHT_DIRECTION: i32 = 25;
const WEIGHT_MOMENTUM: i32 = 20;
/// Items of recent work the direction signal needs before it counts.
const DIRECTION_MIN_ITEMS: i64 = 3;
/// Days an owner goal may stand unchanged before the review says so.
const STALE_DAYS: i64 = 90;
/// Words below which a goal reads as a slogan.
const VAGUE_WORDS: usize = 8;
/// Share of the lead's score in the headline when a fresh reading exists.
const LEAD_SHARE: f64 = 0.4;
/// Most advice lines a review carries.
const MAX_ADVICE: usize = 5;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    pub name: String,
    pub label: String,
    pub weight: i32,
    pub earned: i32,
    pub measured: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advice {
    pub kind: String,
    pub text: String,
    /// What the owner can do, as a short imperative; empty when nothing is
    /// asked of them.
    pub action: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadReading {
    pub post: String,
    pub agent: String,
    pub date: String,
    pub score: Option<i32>,
    pub verdict: String,
    pub summary: String,
    pub body: String,
    pub fresh: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub post: String,
    pub agent: String,
    pub date: String,
    pub text: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub tasks_open: i64,
    pub tasks_waiting: i64,
    pub tasks_done: i64,
    pub landed: i64,
    pub failed: i64,
    pub items: i64,
    pub matched: i64,
    pub recent: i64,
    pub previous: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub project: String,
    pub goal: Goal,
    /// Headline 0..100; meaningful when `scored` is true.
    pub score: i32,
    pub scored: bool,
    /// The store's own score, before the lead's reading is blended in.
    pub measured_score: i32,
    /// Weight (of 100) the store could measure.
    pub measurable: i32,
    pub verdict: String,
    pub window_days: i64,
    pub review_days: i64,
    pub review_due: bool,
    pub signals: Vec<Signal>,
    pub advice: Vec<Advice>,
    pub lead: Option<LeadReading>,
    pub proposal: Option<Proposal>,
    pub counts: Counts,
    pub reviewed_at: String,
}

static SCORE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?im)^\s*score:\s*(\d{1,3})\b").unwrap());
static AMEND_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t]*goal:[ \t]*amend[ \t]*$").unwrap());
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{N}][\p{L}\p{N}'-]*").unwrap());

const STOPWORDS: &[&str] = &[
    "about",
    "after",
    "again",
    "against",
    "also",
    "always",
    "among",
    "another",
    "around",
    "because",
    "before",
    "being",
    "between",
    "both",
    "change",
    "changes",
    "could",
    "does",
    "doing",
    "done",
    "each",
    "every",
    "existing",
    "from",
    "goal",
    "have",
    "having",
    "here",
    "into",
    "its",
    "just",
    "keep",
    "keeping",
    "make",
    "makes",
    "making",
    "more",
    "most",
    "much",
    "must",
    "never",
    "only",
    "other",
    "over",
    "people",
    "project",
    "projects",
    "pulse",
    "same",
    "should",
    "since",
    "some",
    "such",
    "than",
    "that",
    "their",
    "them",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "through",
    "under",
    "until",
    "very",
    "well",
    "were",
    "what",
    "when",
    "where",
    "which",
    "while",
    "with",
    "within",
    "without",
    "work",
    "would",
    "your",
    "stay",
    "within",
    "describes",
    "itself",
    "serve",
    "names",
    "prioritize",
    "connect",
    "concrete",
    "measure",
    "progress",
    "checks",
    "useful",
    "users",
    "user",
    "outcome",
    "scope",
    "tasks",
    "assigned",
    "accepted",
    "decisions",
    "gates",
    "invent",
    "domain",
    "requirements",
    "bypass",
    "hard",
    "stops",
    "approvals",
    "constraints",
    "registered",
    "verification",
    "command",
    "where",
    "applicable",
    "real",
    "defects",
    "correctness",
    "reliability",
    "experience",
    "repeated",
    "operating",
    "layer",
    "churn",
    "workflow",
    "owners",
    "agents",
    "deliver",
    "evidence",
    "backed",
    "improvements",
    "safely",
    "reliably",
];

/// The goal's own words worth looking for in recent work: four letters or
/// more, not a stopword, lowercase, deduplicated, in order of appearance.
pub fn terms(goal: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in WORD.find_iter(goal) {
        let w = m.as_str().trim_matches(['\'', '-']).to_lowercase();
        if w.chars().count() < 4
            || STOPWORDS.contains(&w.as_str())
            || w.chars().all(|c| c.is_numeric())
        {
            continue;
        }
        if !out.contains(&w) {
            out.push(w);
        }
        if out.len() >= 48 {
            break;
        }
    }
    out
}

/// Whether `text` speaks to any of the goal's terms. A term matches a word
/// of the text when the two share their first six letters, so `landing`
/// finds `landings` and `landed`.
pub fn mentions(terms: &[String], text: &str) -> bool {
    if terms.is_empty() {
        return false;
    }
    let stems: Vec<String> = terms.iter().map(|t| stem(t)).collect();
    WORD.find_iter(text)
        .map(|m| stem(&m.as_str().to_lowercase()))
        .any(|w| stems.contains(&w))
}

fn stem(w: &str) -> String {
    w.chars().take(6).collect()
}

/// Whether a board post body is a goal proposal (first line
/// [`AMEND_MARKER`]).
pub fn is_proposal(body: &str) -> bool {
    body.lines().next().is_some_and(|l| AMEND_LINE.is_match(l))
}

/// The proposed goal text and the reason, from a proposal body: everything
/// after the marker line up to a `Why:` paragraph is the goal; that
/// paragraph is the reason. None when the text does not pass the goal rules.
pub fn parse_proposal(body: &str) -> Option<(String, String)> {
    if !is_proposal(body) {
        return None;
    }
    let rest: Vec<&str> = body.lines().skip(1).collect();
    let mut text = String::new();
    let mut why = String::new();
    let mut in_why = false;
    for line in rest {
        let t = line.trim();
        if !in_why && t.to_lowercase().starts_with("why:") {
            in_why = true;
            why.push_str(t[4..].trim());
            continue;
        }
        if in_why {
            if !why.is_empty() {
                why.push(' ');
            }
            why.push_str(t);
        } else {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(t);
        }
    }
    let text = crate::normalize(&text).ok()?;
    Some((text, why.trim().to_string()))
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn stamp(d: DateTime<Utc>) -> String {
    d.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn count(conn: &Connection, sql: &str, p: &[&dyn rusqlite::ToSql]) -> i64 {
    conn.query_row(sql, p, |r| r.get::<_, i64>(0)).unwrap_or(0)
}

fn share(weight: i32, num: i64, den: i64) -> i32 {
    if den <= 0 {
        return 0;
    }
    ((weight as f64) * (num as f64) / (den as f64))
        .round()
        .clamp(0.0, weight as f64) as i32
}

fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Reviews `slug`'s goal against the store. Read-only.
pub fn review(s: &pulse_store::Store, slug: &str) -> Result<Review> {
    let goal = crate::get(s, slug)?;
    let conn = s.conn()?;
    let now_text = s.now();
    let now = parse_time(&now_text).unwrap_or_else(Utc::now);
    let review_days = {
        let v = s.setting_int(REVIEW_DAYS_SETTING);
        if v <= 0 { DEFAULT_REVIEW_DAYS } else { v }
    };
    let window_days = (review_days * 2).max(7);
    let since = stamp(now - Duration::days(window_days));
    let before = stamp(now - Duration::days(window_days * 2));
    let goal_terms = terms(&goal.text);

    // Tasks.
    let tasks_open = count(
        &conn,
        "SELECT COUNT(*) FROM tasks WHERE project = ? AND status IN ('open','planned','in_progress','review')",
        params![slug],
    );
    let tasks_waiting = count(
        &conn,
        "SELECT COUNT(*) FROM tasks WHERE project = ? AND status IN ('open','planned','in_progress','review') AND created_at < ?",
        params![slug, since],
    );
    let tasks_done = count(
        &conn,
        "SELECT COUNT(*) FROM tasks WHERE project = ? AND status = 'done' AND updated_at >= ?",
        params![slug, since],
    );
    // Landings and failures.
    let landed = count(
        &conn,
        "SELECT COUNT(*) FROM events WHERE action IN ('land.landed','land.merged') AND at >= ? AND json_valid(detail) AND json_extract(detail, '$.project') = ?",
        params![since, slug],
    );
    let failed_landings = count(
        &conn,
        "SELECT COUNT(*) FROM events WHERE action IN ('land.failed-verify','land.base-red') AND at >= ? AND json_valid(detail) AND json_extract(detail, '$.project') = ?",
        params![since, slug],
    );
    let failed_jobs = count(
        &conn,
        "SELECT COUNT(*) FROM jobs WHERE project = ? AND status = 'failed' AND finished_at >= ?",
        params![slug, since],
    );
    let failed = failed_landings + failed_jobs;

    // Direction: recent work items and how many speak to the goal.
    let mut items: Vec<String> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT title FROM tasks WHERE project = ? AND updated_at >= ? ORDER BY updated_at DESC LIMIT 200",
        )?;
        items.extend(
            st.query_map(params![slug, since], |r| r.get::<_, String>(0))?
                .flatten(),
        );
        let mut st = conn.prepare(
            "SELECT title || ' ' || substr(brief, 1, 400) FROM jobs WHERE project = ? AND created_at >= ? AND kind NOT IN ('lead','answer','ballot','reflect','research') ORDER BY created_at DESC LIMIT 200",
        )?;
        items.extend(
            st.query_map(params![slug, since], |r| r.get::<_, String>(0))?
                .flatten(),
        );
        let mut st = conn.prepare(
            "SELECT summary || ' ' || substr(body, 1, 400) FROM posts WHERE project = ? AND date >= ? AND kind IN ('result','finding','suggestion','handoff') AND area <> ? ORDER BY date DESC LIMIT 200",
        )?;
        items.extend(
            st.query_map(params![slug, since, AREA], |r| r.get::<_, String>(0))?
                .flatten(),
        );
    }
    let matched = items.iter().filter(|t| mentions(&goal_terms, t)).count() as i64;
    let item_count = items.len() as i64;

    // Momentum: this window against the one before.
    let activity = |from: &str, to: &str| -> i64 {
        count(
            &conn,
            "SELECT COUNT(*) FROM tasks WHERE project = ? AND updated_at >= ? AND updated_at < ?",
            params![slug, from, to],
        ) + count(
            &conn,
            "SELECT COUNT(*) FROM jobs WHERE project = ? AND created_at >= ? AND created_at < ? AND kind NOT IN ('lead','answer','ballot','reflect')",
            params![slug, from, to],
        ) + count(
            &conn,
            "SELECT COUNT(*) FROM events WHERE action IN ('land.landed','land.merged') AND at >= ? AND at < ? AND json_valid(detail) AND json_extract(detail, '$.project') = ?",
            params![from, to, slug],
        ) + count(
            &conn,
            "SELECT COUNT(*) FROM posts WHERE project = ? AND date >= ? AND date < ? AND kind IN ('result','finding','suggestion','handoff','question','answer')",
            params![slug, from, to],
        )
    };
    let far = stamp(now + Duration::days(1));
    let recent = activity(&since, &far);
    let previous = activity(&before, &since);

    // Signals.
    let delivered = tasks_done + landed;
    let delivery_den = delivered + failed_jobs + tasks_waiting;
    let delivery = Signal {
        name: "delivery".into(),
        label: "Delivery".into(),
        weight: WEIGHT_DELIVERY,
        earned: share(WEIGHT_DELIVERY, delivered, delivery_den),
        measured: delivery_den > 0,
        detail: if delivery_den > 0 {
            format!(
                "{} and {} in {window_days} days; {} failed, {} waiting since before the window",
                plural(tasks_done, "task done", "tasks done"),
                plural(landed, "landing", "landings"),
                plural(failed_jobs, "job", "jobs"),
                plural(tasks_waiting, "task", "tasks")
            )
        } else {
            format!("nothing finished, failed or waiting in {window_days} days")
        },
    };
    let rel_den = landed + failed;
    let reliability = Signal {
        name: "reliability".into(),
        label: "Reliability".into(),
        weight: WEIGHT_RELIABILITY,
        earned: share(WEIGHT_RELIABILITY, landed, rel_den),
        measured: rel_den > 0,
        detail: if rel_den > 0 {
            format!(
                "{landed} of {rel_den} attempts landed clean; {} failed verification or found a red base, {} failed",
                plural(failed_landings, "landing", "landings"),
                plural(failed_jobs, "job", "jobs")
            )
        } else {
            format!("no landing or failed job in {window_days} days")
        },
    };
    let direction = Signal {
        name: "direction".into(),
        label: "Direction".into(),
        weight: WEIGHT_DIRECTION,
        earned: share(WEIGHT_DIRECTION, matched, item_count),
        measured: item_count >= DIRECTION_MIN_ITEMS && !goal_terms.is_empty(),
        detail: if goal_terms.is_empty() {
            "the goal has no words to look for".into()
        } else if item_count >= DIRECTION_MIN_ITEMS {
            format!(
                "{matched} of {} recent tasks, jobs and posts speak to the goal's words",
                plural(item_count, "item", "items")
            )
        } else {
            format!("only {item_count} recent items; {DIRECTION_MIN_ITEMS} are needed")
        },
    };
    let momentum_ratio = if previous == 0 {
        if recent > 0 { 1.0 } else { 0.0 }
    } else {
        ((recent as f64) / (previous as f64)).min(1.0)
    };
    let momentum = Signal {
        name: "momentum".into(),
        label: "Momentum".into(),
        weight: WEIGHT_MOMENTUM,
        earned: ((WEIGHT_MOMENTUM as f64) * momentum_ratio).round() as i32,
        measured: recent + previous > 0,
        detail: if recent + previous > 0 {
            format!(
                "{recent} moves in the last {window_days} days against {previous} in the {window_days} before"
            )
        } else {
            format!("no moves in {} days", window_days * 2)
        },
    };
    let signals = vec![delivery, reliability, direction, momentum];
    let measurable: i32 = signals
        .iter()
        .filter(|x| x.measured)
        .map(|x| x.weight)
        .sum();
    let earned: i32 = signals
        .iter()
        .filter(|x| x.measured)
        .map(|x| x.earned)
        .sum();
    let measured_score = if measurable > 0 {
        ((100.0 * earned as f64) / (measurable as f64)).round() as i32
    } else {
        0
    };

    // The lead's reading and proposal.
    let lead = lead_reading(&conn, slug, &since);
    let proposal = open_proposal(&conn, slug);
    let lead_fresh_score = lead.as_ref().filter(|l| l.fresh).and_then(|l| l.score);
    let review_due = !lead.as_ref().is_some_and(|l| {
        parse_time(&l.date).is_some_and(|d| d >= now - Duration::days(review_days))
    });

    let (score, scored) = match (measurable >= 50, lead_fresh_score) {
        (true, Some(ls)) => (
            ((1.0 - LEAD_SHARE) * measured_score as f64 + LEAD_SHARE * ls as f64).round() as i32,
            true,
        ),
        (true, None) => (measured_score, true),
        (false, Some(ls)) => (ls, true),
        (false, None) => (measured_score, false),
    };
    let verdict = if !scored {
        QUIET
    } else if score >= 65 {
        ON_TRACK
    } else if score >= 40 {
        SLIPPING
    } else {
        STALLED
    }
    .to_string();

    let counts = Counts {
        tasks_open,
        tasks_waiting,
        tasks_done,
        landed,
        failed,
        items: item_count,
        matched,
        recent,
        previous,
    };
    let mut r = Review {
        project: slug.to_string(),
        goal,
        score,
        scored,
        measured_score,
        measurable,
        verdict,
        window_days,
        review_days,
        review_due,
        signals,
        advice: Vec::new(),
        lead,
        proposal,
        counts,
        reviewed_at: now_text,
    };
    r.advice = advise(&r, now);
    Ok(r)
}

fn lead_reading(conn: &Connection, slug: &str, since: &str) -> Option<LeadReading> {
    // The reading is the newest finding on the thread that carries a
    // `Score:` line; other findings there (a session's observation, a
    // worker's note) are discussion, not the lead's reading.
    let mut st = conn
        .prepare(
            "SELECT id, agent, date, summary, body FROM posts WHERE project = ? AND area = ? AND thread = ? AND kind = 'finding' ORDER BY date DESC, id DESC LIMIT 20",
        )
        .ok()?;
    let rows: Vec<(String, String, String, String, String)> = st
        .query_map(params![slug, AREA, REVIEW_THREAD], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .ok()?
        .flatten()
        .collect();
    let (post, agent, date, summary, body, score) =
        rows.into_iter()
            .find_map(|(post, agent, date, summary, body)| {
                let score = SCORE_LINE
                    .captures(&body)
                    .and_then(|c| c[1].parse::<i32>().ok())
                    .filter(|n| (0..=100).contains(n))?;
                Some((post, agent, date, summary, body, score))
            })?;
    let verdict = summary
        .split(':')
        .next()
        .map(|v| v.trim().to_lowercase())
        .filter(|v| [ON_TRACK, SLIPPING, STALLED, QUIET].contains(&v.as_str()))
        .unwrap_or_default();
    Some(LeadReading {
        fresh: date.as_str() >= since,
        post,
        agent,
        date,
        score: Some(score),
        verdict,
        summary,
        body,
    })
}

fn open_proposal(conn: &Connection, slug: &str) -> Option<Proposal> {
    let mut st = conn
        .prepare(
            "SELECT p.id, p.agent, p.date, p.body FROM posts p WHERE p.project = ? AND p.area = ? AND p.thread = ? AND p.kind = 'opinion' AND p.resolution = '' AND NOT EXISTS (SELECT 1 FROM decisions d WHERE d.post_id = p.id) ORDER BY p.date DESC, p.id DESC LIMIT 10",
        )
        .ok()?;
    let rows = st
        .query_map(params![slug, AREA, AMEND_THREAD], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .ok()?;
    for (post, agent, date, body) in rows.flatten() {
        if let Some((text, why)) = parse_proposal(&body) {
            return Some(Proposal {
                post,
                agent,
                date,
                text,
                why,
            });
        }
    }
    None
}

fn advise(r: &Review, now: DateTime<Utc>) -> Vec<Advice> {
    let mut out = Vec::new();
    let mut push = |kind: &str, text: String, action: &str| {
        out.push(Advice {
            kind: kind.into(),
            text,
            action: action.into(),
        })
    };
    if let Some(p) = &r.proposal {
        push(
            "lead-proposes",
            format!(
                "{} proposes a new goal. Take it with one yes, or dismiss it and say why.",
                p.agent
            ),
            "Read the proposal",
        );
    }
    if r.goal.source == crate::GENERATED {
        push(
            "set-your-own",
            "PULSE wrote this goal from the project. Set your own and the score measures what you actually want."
                .into(),
            "Edit goal",
        );
    }
    if !r.scored {
        push(
            "quiet",
            format!(
                "Too little happened in the last {} days to score the goal ({} of 100 measurable). Add a task, or wait for the next landing.",
                r.window_days, r.measurable
            ),
            "Add a task",
        );
    } else if r.verdict == STALLED {
        push(
            "stalled",
            format!(
                "Work toward the goal has stalled: {} waiting since before the window, {} in {} days. Ask the lead for a plan or cut the backlog.",
                plural(r.counts.tasks_waiting, "task", "tasks"),
                plural(r.counts.landed, "landing", "landings"),
                r.window_days
            ),
            "Message the lead",
        );
    } else if r.verdict == SLIPPING {
        push(
            "slipping",
            "The goal is slipping: less lands than is started. Finish or drop what waits before adding more.".into(),
            "",
        );
    }
    if let Some(rel) = r.signals.iter().find(|x| x.name == "reliability")
        && rel.measured
        && rel.earned * 2 < rel.weight
    {
        push(
            "unreliable",
            format!(
                "Most attempts fail before they land ({}). Make the checks green before new work.",
                rel.detail
            ),
            "",
        );
    }
    if let Some(dir) = r.signals.iter().find(|x| x.name == "direction")
        && dir.measured
        && dir.earned * 5 < dir.weight * 2
    {
        push(
            "drift",
            format!(
                "Recent work rarely speaks to the goal ({} of {} items). Either the goal is out of date - amend it - or the tasks need redirecting.",
                r.counts.matched, r.counts.items
            ),
            "Edit goal",
        );
    }
    if r.goal.source == crate::OWNER
        && let Some(changed) = parse_time(&r.goal.updated_at)
        && now - changed > Duration::days(STALE_DAYS)
    {
        push(
            "stale",
            format!(
                "The goal has stood unchanged for {} days. Re-read it; amend it if the project has moved on.",
                (now - changed).num_days()
            ),
            "Edit goal",
        );
    }
    let words = r.goal.text.split_whitespace().count();
    if words < VAGUE_WORDS {
        push(
            "vague",
            format!(
                "The goal is {} words. One that names an outcome and how you would see the progress scores better.",
                words
            ),
            "Edit goal",
        );
    }
    if r.scored && r.score >= 90 && r.counts.tasks_open == 0 && r.counts.landed > 0 {
        push(
            "achieved",
            "The goal looks reached: nothing open and everything landed. Raise the bar - set the next goal.".into(),
            "Edit goal",
        );
    }
    if r.review_due {
        push(
            "review-due",
            format!(
                "No lead review in the last {} days; the lead reads the goal at its next sitting.",
                r.review_days
            ),
            "",
        );
    }
    out.truncate(MAX_ADVICE);
    out
}

/// The paragraph a lead's brief carries so its sitting reviews the goal:
/// the store's reading, what the owner is told, and how to post a reading or
/// a proposal. Empty when the review is not due and no proposal waits.
pub fn lead_brief(r: &Review) -> String {
    use std::fmt::Write as _;
    let mut b = String::new();
    let _ = writeln!(b, "## Goal review\n");
    let _ = writeln!(
        b,
        "The project's goal ({}): {}\n",
        if r.goal.source == crate::OWNER {
            "set by the owner"
        } else {
            "written by PULSE, the owner has not set one"
        },
        r.goal.text.trim()
    );
    if r.scored {
        let _ = writeln!(
            b,
            "The store scores it {} of 100 ({}) over the last {} days:",
            r.measured_score, r.verdict, r.window_days
        );
    } else {
        let _ = writeln!(
            b,
            "The store cannot score it yet ({} of 100 measurable over the last {} days):",
            r.measurable, r.window_days
        );
    }
    for s in &r.signals {
        let _ = writeln!(
            b,
            "- {}: {} ({})",
            s.label,
            if s.measured {
                format!("{} of {}", s.earned, s.weight)
            } else {
                "not measured".into()
            },
            s.detail
        );
    }
    if let Some(l) = &r.lead {
        let _ = writeln!(
            b,
            "\nYour last reading ({}, post {}): {}{}",
            l.date,
            l.post,
            l.summary,
            l.score.map(|n| format!(" - score {n}")).unwrap_or_default()
        );
    }
    if let Some(p) = &r.proposal {
        let _ = writeln!(
            b,
            "\nA proposed goal waits for the owner (post {}): {}",
            p.post,
            crate::normalize(&p.text).unwrap_or_default()
        );
    }
    let _ = write!(
        b,
        "\nWhat to do: {}read the goal against what actually landed and post one reading: `pulse post finding --project {} --area {AREA} --thread {REVIEW_THREAD} --summary \"<{ON_TRACK}|{SLIPPING}|{STALLED}>: <one line>\"` with a body that starts `Score: NN` (0-100, your own judgement of how far the goal is reached), then the evidence (landings, tasks, what the users got), then what should happen next. When the goal itself is wrong - reached, stale, or pointing away from what the work shows - post one proposal{}: `pulse post opinion --project {} --area {AREA} --thread {AMEND_THREAD} --summary \"proposed goal: <gist>\"` whose body's first line is `{AMEND_MARKER}`, then the new goal in full (1-4000 characters, no private paths), then a `Why:` paragraph. The owner takes it with one yes or dismisses it; never set the goal yourself.\n",
        if r.review_due {
            "a review is due: "
        } else {
            ""
        },
        r.project,
        if r.proposal.is_some() {
            " only if it differs from the one waiting"
        } else {
            ""
        },
        r.project
    );
    b
}

/// Applies a decided goal proposal: a yes from the owner on a post whose
/// body is a proposal sets the project's goal to the proposed text. Returns
/// the new goal, or None when the post is not a proposal or the answer is
/// no. A proposal that fails the goal rules is recorded as not applied.
pub fn apply_decision(
    s: &pulse_store::Store,
    post_id: &str,
    project: &str,
    body: &str,
    answer: &str,
    decided_by: &str,
) -> Result<Option<Goal>> {
    if answer != "yes" || !is_proposal(body) {
        return Ok(None);
    }
    let Some((text, _why)) = parse_proposal(body) else {
        s.event(
            decided_by,
            AMEND_ACTION,
            project,
            pulse_store::Detail::json(serde_json::json!({
                "project": project, "post": post_id, "applied": false,
                "why": "the proposed text does not pass the goal rules"
            }))?,
        )?;
        return Ok(None);
    };
    match crate::set(s, project, &text, decided_by) {
        Ok(goal) => {
            s.event(
                decided_by,
                AMEND_ACTION,
                project,
                pulse_store::Detail::json(serde_json::json!({
                    "project": project, "post": post_id, "applied": true
                }))?,
            )?;
            Ok(Some(goal))
        }
        Err(e) => {
            s.event(
                decided_by,
                AMEND_ACTION,
                project,
                pulse_store::Detail::json(serde_json::json!({
                    "project": project, "post": post_id, "applied": false,
                    "why": e.to_string()
                }))?,
            )?;
            Ok(None)
        }
    }
}

/// One screen of the review for the terminal.
pub fn render(r: &Review) -> String {
    use std::fmt::Write as _;
    let mut b = String::new();
    let _ = writeln!(b, "Goal review: {}", r.project);
    let _ = writeln!(b, "  goal ({}): {}", r.goal.source, r.goal.text.trim());
    if r.scored {
        let _ = writeln!(
            b,
            "  score: {} of 100 - {} (store {}, {} of 100 measurable{})",
            r.score,
            r.verdict,
            r.measured_score,
            r.measurable,
            r.lead
                .as_ref()
                .filter(|l| l.fresh)
                .and_then(|l| l.score)
                .map(|n| format!(", lead {n}"))
                .unwrap_or_default()
        );
    } else {
        let _ = writeln!(
            b,
            "  score: not yet - {} ({} of 100 measurable over {} days)",
            r.verdict, r.measurable, r.window_days
        );
    }
    for s in &r.signals {
        let _ = writeln!(
            b,
            "  {:<12} {:>9}  {}",
            s.label,
            if s.measured {
                format!("{}/{}", s.earned, s.weight)
            } else {
                "-".to_string()
            },
            s.detail
        );
    }
    if let Some(l) = &r.lead {
        let _ = writeln!(
            b,
            "  lead: {} ({}, {}){}",
            l.summary,
            l.agent,
            l.date,
            l.score.map(|n| format!(" score {n}")).unwrap_or_default()
        );
    } else {
        let _ = writeln!(b, "  lead: no reading yet");
    }
    if let Some(p) = &r.proposal {
        let _ = writeln!(
            b,
            "  proposed goal ({}, {}): {}\n    take it: pulse decide {} yes    dismiss: pulse decide {} no",
            p.agent, p.post, p.text, p.post, p.post
        );
    }
    if !r.advice.is_empty() {
        let _ = writeln!(b, "  advice:");
        for a in &r.advice {
            let _ = writeln!(b, "  - {}", a.text);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_store::{Options, Store};
    use std::path::Path;

    const NOW: &str = "2026-10-20T12:00:00Z";

    fn set_env(key: &str, value: &str) {
        unsafe { std::env::set_var(key, value) }
    }

    fn store() -> (pulse_testutil::EnvGuard, tempfile::TempDir, Store) {
        let guard = pulse_testutil::env_lock();
        let home = tempfile::tempdir().unwrap();
        for key in ["PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT"] {
            set_env(key, "");
        }
        set_env("PULSE_HOME", home.path().to_str().unwrap());
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            now: NOW.to_string(),
            ..Options::default()
        })
        .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, brain, verify_cmd, added_at) VALUES ('shop', 'Shop', ?, 0, 'npm test', '2026-01-01T00:00:00Z')",
                params![Path::new(&s.home).join("missing").display().to_string()],
            )
            .unwrap();
        (guard, home, s)
    }

    fn task(s: &Store, id: &str, title: &str, status: &str, created: &str, updated: &str) {
        s.conn().unwrap().execute(
            "INSERT INTO tasks (id, project, title, status, created_by, created_at, updated_at) VALUES (?, 'shop', ?, ?, 'human', ?, ?)",
            params![id, title, status, created, updated],
        ).unwrap();
    }

    fn landing(s: &Store, at: &str, action: &str) {
        s.conn().unwrap().execute(
            "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'pulse', ?, 'evolve/x', ?)",
            params![at, action, r#"{"project":"shop","job":"J-1"}"#],
        ).unwrap();
    }

    fn job(s: &Store, id: &str, title: &str, status: &str, at: &str) {
        s.conn().unwrap().execute(
            "INSERT INTO jobs (id, project, kind, title, status, created_at, finished_at) VALUES (?, 'shop', 'task', ?, ?, ?, ?)",
            params![id, title, status, at, at],
        ).unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn post(
        s: &Store,
        id: &str,
        area: &str,
        thread: &str,
        kind: &str,
        date: &str,
        summary: &str,
        body: &str,
    ) {
        s.conn().unwrap().execute(
            "INSERT INTO posts (id, project, area, thread, kind, agent, date, summary, body, created_at) VALUES (?, 'shop', ?, ?, ?, 'shop-lead', ?, ?, ?, ?)",
            params![id, area, thread, kind, date, summary, body, date],
        ).unwrap();
    }

    #[test]
    fn terms_and_mentions_follow_the_goal_words() {
        let t = terms("Make checkout faster for shoppers; cut the cart abandonment rate.");
        assert_eq!(
            t,
            vec![
                "checkout",
                "faster",
                "shoppers",
                "cart",
                "abandonment",
                "rate"
            ]
        );
        assert!(mentions(&t, "Speed up the Checkout page"));
        assert!(mentions(&t, "carts abandoned less"));
        assert!(!mentions(&t, "bump the dependency versions"));
        assert!(terms("").is_empty());
    }

    #[test]
    fn proposal_parsing_takes_the_marker_text_and_why() {
        assert!(is_proposal("goal: amend\nMake it fast."));
        assert!(is_proposal("  goal:  amend  \nx"));
        assert!(!is_proposal("goal: amendments\nx"));
        assert!(!is_proposal("Make it fast.\ngoal: amend"));
        let (text, why) = parse_proposal(
            "goal: amend\nMake checkout finish in two taps for returning shoppers,\nmeasured by the checkout funnel.\nWhy: the old goal is reached.\nShoppers asked for it.",
        )
        .unwrap();
        assert_eq!(
            text,
            "Make checkout finish in two taps for returning shoppers, measured by the checkout funnel."
        );
        assert_eq!(why, "the old goal is reached. Shoppers asked for it.");
        assert!(
            parse_proposal("goal: amend\n").is_none(),
            "empty text fails the rules"
        );
        assert!(
            parse_proposal("goal: amend\nSee ~/.ssh/secret").is_none(),
            "private path fails"
        );
    }

    #[test]
    fn quiet_project_has_no_score_and_says_what_to_do() {
        let (_g, _h, s) = store();
        let r = review(&s, "shop").unwrap();
        assert!(!r.scored);
        assert_eq!(r.verdict, QUIET);
        assert_eq!(r.measurable, 0);
        assert!(r.signals.iter().all(|x| !x.measured));
        assert!(r.advice.iter().any(|a| a.kind == "quiet"), "{:?}", r.advice);
        assert!(r.advice.iter().any(|a| a.kind == "set-your-own"));
        assert!(r.review_due);
        assert_eq!(r.window_days, 14);
        assert!(render(&r).contains("not yet"));
    }

    #[test]
    fn delivered_work_toward_the_goal_scores_on_track() {
        let (_g, _h, s) = store();
        crate::set(
            &s,
            "shop",
            "Make checkout faster for shoppers and cut cart abandonment.",
            "human",
        )
        .unwrap();
        task(
            &s,
            "TASK-001",
            "Speed up the checkout page",
            "done",
            "2026-10-10T00:00:00Z",
            "2026-10-15T00:00:00Z",
        );
        task(
            &s,
            "TASK-002",
            "Trim the cart summary query",
            "done",
            "2026-10-12T00:00:00Z",
            "2026-10-18T00:00:00Z",
        );
        task(
            &s,
            "TASK-003",
            "Checkout: fewer fields",
            "planned",
            "2026-10-19T00:00:00Z",
            "2026-10-19T00:00:00Z",
        );
        landing(&s, "2026-10-15T01:00:00Z", "land.landed");
        landing(&s, "2026-10-18T01:00:00Z", "land.landed");
        landing(&s, "2026-09-25T01:00:00Z", "land.landed");
        job(
            &s,
            "J-1",
            "Speed up checkout",
            "done",
            "2026-10-15T00:00:00Z",
        );
        job(
            &s,
            "J-2",
            "Cart summary query",
            "done",
            "2026-10-18T00:00:00Z",
        );
        let r = review(&s, "shop").unwrap();
        assert!(r.scored, "{r:?}");
        assert_eq!(r.measurable, 100);
        let by = |n: &str| r.signals.iter().find(|x| x.name == n).unwrap().clone();
        assert_eq!(by("delivery").earned, 30, "{:?}", by("delivery"));
        assert_eq!(by("reliability").earned, 25);
        assert_eq!(by("direction").earned, 25, "{:?}", by("direction"));
        assert_eq!(by("momentum").earned, 20, "{:?}", by("momentum"));
        assert_eq!(r.score, 100);
        assert_eq!(r.verdict, ON_TRACK);
        assert_eq!(r.counts.landed, 2);
        assert!(!r.advice.iter().any(|a| a.kind == "set-your-own"));
        assert!(r.advice.iter().any(|a| a.kind == "review-due"));
        assert!(
            !r.advice.iter().any(|a| a.kind == "achieved"),
            "a task is still open"
        );
    }

    #[test]
    fn failures_and_drift_lower_the_score_with_advice() {
        let (_g, _h, s) = store();
        crate::set(
            &s,
            "shop",
            "Make checkout faster for shoppers and cut cart abandonment.",
            "human",
        )
        .unwrap();
        task(
            &s,
            "TASK-001",
            "Rename internal modules",
            "open",
            "2026-09-01T00:00:00Z",
            "2026-09-01T00:00:00Z",
        );
        task(
            &s,
            "TASK-002",
            "Upgrade the linter",
            "open",
            "2026-09-02T00:00:00Z",
            "2026-09-02T00:00:00Z",
        );
        job(
            &s,
            "J-1",
            "Rename modules",
            "failed",
            "2026-10-15T00:00:00Z",
        );
        job(
            &s,
            "J-2",
            "Linter upgrade",
            "failed",
            "2026-10-16T00:00:00Z",
        );
        job(&s, "J-3", "Theme tweaks", "failed", "2026-10-17T00:00:00Z");
        landing(&s, "2026-10-18T01:00:00Z", "land.failed-verify");
        landing(&s, "2026-10-19T01:00:00Z", "land.landed");
        let r = review(&s, "shop").unwrap();
        assert!(r.scored);
        let by = |n: &str| r.signals.iter().find(|x| x.name == n).unwrap().clone();
        assert_eq!(by("delivery").earned, 5, "{:?}", by("delivery")); // 1 of 6
        assert_eq!(by("reliability").earned, 5, "{:?}", by("reliability")); // 1 of 5
        assert!(by("direction").measured);
        assert_eq!(by("direction").earned, 0);
        assert_eq!(r.verdict, STALLED, "{r:?}");
        let kinds: Vec<&str> = r.advice.iter().map(|a| a.kind.as_str()).collect();
        assert!(kinds.contains(&"stalled"), "{kinds:?}");
        assert!(kinds.contains(&"unreliable"), "{kinds:?}");
        assert!(kinds.contains(&"drift"), "{kinds:?}");
        assert!(r.advice.len() <= MAX_ADVICE);
    }

    #[test]
    fn lead_reading_blends_in_and_proposal_waits_for_the_owner() {
        let (_g, _h, s) = store();
        crate::set(
            &s,
            "shop",
            "Make checkout faster for shoppers and cut cart abandonment.",
            "human",
        )
        .unwrap();
        for i in 0..4 {
            task(
                &s,
                &format!("TASK-00{i}"),
                "Checkout speed",
                "done",
                "2026-10-10T00:00:00Z",
                "2026-10-15T00:00:00Z",
            );
        }
        landing(&s, "2026-10-15T01:00:00Z", "land.landed");
        post(
            &s,
            "P-1",
            AREA,
            REVIEW_THREAD,
            "finding",
            "2026-10-19T00:00:00Z",
            "slipping: checkout is faster, abandonment unmeasured",
            "Score: 50\nThe funnel has no numbers yet.",
        );
        post(
            &s,
            "P-0",
            AREA,
            REVIEW_THREAD,
            "finding",
            "2026-10-01T00:00:00Z",
            "stalled: nothing yet",
            "Score: 10",
        );
        // A newer finding on the thread without a Score line is discussion,
        // not the lead's reading.
        post(
            &s,
            "P-3",
            AREA,
            REVIEW_THREAD,
            "finding",
            "2026-10-19T00:05:00Z",
            "the funnel metric lives in analytics, not the store",
            "Observed while wiring the dashboard; no score here.",
        );
        post(
            &s,
            "P-2",
            AREA,
            AMEND_THREAD,
            "opinion",
            "2026-10-19T00:10:00Z",
            "proposed goal: measure abandonment",
            "goal: amend\nMeasure cart abandonment weekly and cut it by a fifth before the year ends.\nWhy: speed is done; the number is missing.",
        );
        let r = review(&s, "shop").unwrap();
        let lead = r.lead.clone().unwrap();
        assert_eq!(lead.post, "P-1");
        assert_eq!(lead.score, Some(50));
        assert_eq!(lead.verdict, SLIPPING);
        assert!(lead.fresh);
        assert!(!r.review_due);
        assert_eq!(r.measured_score, 100);
        assert_eq!(r.score, 80, "0.6 * 100 + 0.4 * 50");
        let p = r.proposal.clone().unwrap();
        assert_eq!(p.post, "P-2");
        assert!(p.text.starts_with("Measure cart abandonment weekly"));
        assert_eq!(p.why, "speed is done; the number is missing.");
        assert_eq!(r.advice[0].kind, "lead-proposes");
        let brief = lead_brief(&r);
        assert!(brief.contains("## Goal review"));
        assert!(brief.contains("Your last reading"));
        assert!(brief.contains("A proposed goal waits"));
        assert!(brief.contains("Score: NN"));
        assert!(render(&r).contains("pulse decide P-2 yes"));

        // A yes applies the proposal; the goal changes and an event says so.
        let applied = apply_decision(
            &s,
            "P-2",
            "shop",
            &s.conn()
                .unwrap()
                .query_row("SELECT body FROM posts WHERE id = 'P-2'", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            "yes",
            "human",
        )
        .unwrap();
        assert!(applied.is_some());
        let goal = crate::get(&s, "shop").unwrap();
        assert!(goal.text.starts_with("Measure cart abandonment weekly"));
        assert_eq!(goal.source, crate::OWNER);
        let n: i64 = s.conn().unwrap().query_row("SELECT COUNT(*) FROM events WHERE action = ? AND json_extract(detail, '$.applied') = 1", [AMEND_ACTION], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // A no, or a post that is not a proposal, changes nothing.
        assert!(
            apply_decision(&s, "P-1", "shop", "Score: 50", "yes", "human")
                .unwrap()
                .is_none()
        );
        assert!(
            apply_decision(&s, "P-2", "shop", "goal: amend\nOther.", "no", "human")
                .unwrap()
                .is_none()
        );
        // Once decided, the proposal no longer waits.
        s.conn().unwrap().execute("INSERT INTO decisions (post_id, project, answer, decided_at) VALUES ('P-2', 'shop', 'yes', ?)", [NOW]).unwrap();
        let r = review(&s, "shop").unwrap();
        assert!(r.proposal.is_none());
    }

    #[test]
    fn a_proposal_with_a_private_path_is_not_applied() {
        let (_g, _h, s) = store();
        let before = crate::get(&s, "shop").unwrap();
        let r = apply_decision(
            &s,
            "P-9",
            "shop",
            "goal: amend\nShip ~/.ssh/keys to everyone.",
            "yes",
            "human",
        )
        .unwrap();
        assert!(r.is_none());
        assert_eq!(crate::get(&s, "shop").unwrap(), before);
        let why: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT json_extract(detail, '$.why') FROM events WHERE action = ?",
                [AMEND_ACTION],
                |r| r.get(0),
            )
            .unwrap();
        assert!(why.contains("goal rules"), "{why}");
    }

    #[test]
    fn stale_vague_and_achieved_advice() {
        let (_g, _h, s) = store();
        crate::set(&s, "shop", "Faster checkout.", "human").unwrap();
        s.conn().unwrap().execute("UPDATE settings SET updated_at = '2026-01-01T00:00:00Z' WHERE name = 'project.goal.shop'", []).unwrap();
        for i in 0..3 {
            task(
                &s,
                &format!("TASK-00{i}"),
                "Checkout speed",
                "done",
                "2026-10-10T00:00:00Z",
                "2026-10-15T00:00:00Z",
            );
        }
        landing(&s, "2026-10-15T01:00:00Z", "land.landed");
        let r = review(&s, "shop").unwrap();
        let kinds: Vec<&str> = r.advice.iter().map(|a| a.kind.as_str()).collect();
        assert!(kinds.contains(&"stale"), "{kinds:?}");
        assert!(kinds.contains(&"vague"), "{kinds:?}");
        assert!(kinds.contains(&"achieved"), "{kinds:?} {r:?}");
        assert_eq!(r.goal.updated_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn review_days_setting_sets_the_window_and_due_date() {
        let (_g, _h, s) = store();
        s.conn().unwrap().execute("INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, '3', ?, 'human')", params![REVIEW_DAYS_SETTING, NOW]).unwrap();
        let r = review(&s, "shop").unwrap();
        assert_eq!(r.review_days, 3);
        assert_eq!(r.window_days, 7, "never under a week");
        post(
            &s,
            "P-1",
            AREA,
            REVIEW_THREAD,
            "finding",
            "2026-10-18T00:00:00Z",
            "on-track: fine",
            "Score: 70",
        );
        assert!(!review(&s, "shop").unwrap().review_due);
        post(
            &s,
            "P-0",
            AREA,
            REVIEW_THREAD,
            "finding",
            "2026-10-19T00:00:00Z",
            "on-track: fine",
            "Score: 70",
        );
        let r = review(&s, "shop").unwrap();
        assert_eq!(r.lead.unwrap().post, "P-0", "the latest reading counts");
        s.conn()
            .unwrap()
            .execute(
                "UPDATE settings SET value = '0' WHERE name = ?",
                [REVIEW_DAYS_SETTING],
            )
            .unwrap();
        assert_eq!(
            review(&s, "shop").unwrap().review_days,
            DEFAULT_REVIEW_DAYS,
            "0 falls back to the default"
        );
    }
}
