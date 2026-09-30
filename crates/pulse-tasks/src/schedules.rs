//! Schedules: a standing job any agent can set for a project - "every day at
//! 09:00, run this brief" - which the daemon queues as an ordinary job each
//! time it is due. The worker that takes the job is the scheduled agent.

use anyhow::Result;
use chrono::{DateTime, Duration, NaiveTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::json;

use crate::tasks::{check_project, clean, input, next_id};

pub const SCHEDULE_STATUSES: &[&str] = &["active", "paused", "ended"];
/// The job kinds a schedule may queue: the ones a worker runs from a brief
/// alone. Experiments, ballots and leads are the planner's.
pub const SCHEDULE_KINDS: &[&str] = &["task", "improve", "clean", "reflect"];
/// The `source` of every job a schedule queues: `schedule:SCHED-001`. One
/// open job per schedule at a time - `add_job` dedupes on it.
pub const SOURCE_PREFIX: &str = "schedule:";
const TIME_LAYOUT: &str = "%Y-%m-%dT%H:%M:%SZ";
const MINUTES_PER_DAY: i64 = 24 * 60;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Schedule {
    pub id: String,
    pub project: String,
    pub title: String,
    pub kind: String,
    pub brief: String,
    pub tier: i32,
    pub priority: i32,
    pub every_minutes: i32,
    pub at_time: String,
    pub status: String,
    pub next_at: String,
    pub last_at: String,
    pub last_job: String,
    pub runs: i32,
    pub max_runs: i32,
    pub note: String,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Schedule {
    /// The cadence as written: `6h`, `1d`, `90m`.
    pub fn every(&self) -> String {
        format_every(i64::from(self.every_minutes))
    }
    pub fn source(&self) -> String {
        format!("{SOURCE_PREFIX}{}", self.id)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScheduleInput {
    pub project: String,
    pub title: String,
    pub kind: String,
    pub brief: String,
    pub tier: i32,
    pub priority: i32,
    /// `30m`, `6h`, `1d`, `2w`, `hourly`, `daily`, `weekly`, or minutes.
    pub every: String,
    /// `HH:MM` UTC; anchors a cadence of whole days.
    pub at: String,
    /// End after this many jobs; 0 keeps going.
    pub times: i32,
    pub author: String,
}

/// What one due schedule did on a daemon tick.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Fired {
    pub schedule: Schedule,
    pub job: String,
    /// False when the job of the previous run was still open, so none was
    /// queued; the schedule moved on to its next time all the same.
    pub queued: bool,
    pub ended: bool,
}

const COLS: &str = "id, project, title, kind, brief, tier, priority, every_minutes, at_time, status, next_at, last_at, last_job, runs, max_runs, note, created_by, created_at, updated_at";

fn scan(row: &rusqlite::Row<'_>) -> rusqlite::Result<Schedule> {
    Ok(Schedule {
        id: row.get(0)?,
        project: row.get(1)?,
        title: row.get(2)?,
        kind: row.get(3)?,
        brief: row.get(4)?,
        tier: row.get(5)?,
        priority: row.get(6)?,
        every_minutes: row.get(7)?,
        at_time: row.get(8)?,
        status: row.get(9)?,
        next_at: row.get(10)?,
        last_at: row.get(11)?,
        last_job: row.get(12)?,
        runs: row.get(13)?,
        max_runs: row.get(14)?,
        note: row.get(15)?,
        created_by: row.get(16)?,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
    })
}

/// Minutes in a cadence: `30m`, `6h`, `1d`, `2w`, `hourly`, `daily`,
/// `weekly`, or a bare number of minutes.
pub fn parse_every(text: &str) -> Result<i64> {
    let text = text.trim().to_lowercase();
    let bad = || {
        input(
            2,
            format!("Not a cadence: {text:?}"),
            "a cadence is a number with a unit",
            "--every 30m | 6h | 1d | 2w (or hourly, daily, weekly)",
        )
    };
    let minutes = match text.as_str() {
        "hourly" => 60,
        "daily" => MINUTES_PER_DAY,
        "weekly" => 7 * MINUTES_PER_DAY,
        "" => return Err(bad()),
        _ => {
            let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
            let unit = text[digits.len()..].trim();
            let n: i64 = digits.parse().map_err(|_| bad())?;
            let per = match unit {
                "" | "m" | "min" | "minute" | "minutes" => 1,
                "h" | "hr" | "hour" | "hours" => 60,
                "d" | "day" | "days" => MINUTES_PER_DAY,
                "w" | "wk" | "week" | "weeks" => 7 * MINUTES_PER_DAY,
                _ => return Err(bad()),
            };
            n.checked_mul(per).ok_or_else(bad)?
        }
    };
    if minutes < 1 {
        return Err(bad());
    }
    Ok(minutes)
}

/// The cadence in its largest whole unit: `2w`, `1d`, `6h`, `90m`.
pub fn format_every(minutes: i64) -> String {
    if minutes > 0 && minutes % (7 * MINUTES_PER_DAY) == 0 {
        format!("{}w", minutes / (7 * MINUTES_PER_DAY))
    } else if minutes > 0 && minutes % MINUTES_PER_DAY == 0 {
        format!("{}d", minutes / MINUTES_PER_DAY)
    } else if minutes > 0 && minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

fn parse_at(text: &str) -> Result<Option<NaiveTime>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    NaiveTime::parse_from_str(text, "%H:%M")
        .map(Some)
        .map_err(|_| {
            input(
                2,
                format!("Not a time of day: {text:?}"),
                "--at takes hours and minutes in UTC",
                "--at 09:00",
            )
        })
}

fn parse_iso(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| {
            input(
                2,
                format!("Not a UTC time like 2026-09-05T08:00:00Z: {value:?}"),
                "times are UTC",
                "pass --now 2026-09-05T08:00:00Z",
            )
        })
}

fn iso(t: DateTime<Utc>) -> String {
    t.format(TIME_LAYOUT).to_string()
}

/// When a new schedule first fires: one cadence from now, or the next `at`
/// time of day when one is given.
pub fn first_due(now: &str, every_minutes: i64, at: Option<NaiveTime>) -> Result<String> {
    let now = parse_iso(now)?;
    Ok(iso(match at {
        Some(at) => {
            let today = now.date_naive().and_time(at).and_utc();
            if today > now {
                today
            } else {
                today + Duration::days(1)
            }
        }
        None => now + Duration::minutes(every_minutes),
    }))
}

/// The time after `next_at` that follows `now`: one cadence on, skipping the
/// ticks a stopped daemon missed instead of queueing one job for each.
pub fn advance(next_at: &str, every_minutes: i64, now: &str) -> Result<String> {
    let now = parse_iso(now)?;
    let step = Duration::minutes(every_minutes.max(1));
    let mut next = parse_iso(next_at)? + step;
    if next <= now {
        let behind = (now - next).num_minutes() / every_minutes.max(1) + 1;
        next += Duration::minutes(behind * every_minutes.max(1));
    }
    Ok(iso(next))
}

fn setting_int(s: &pulse_store::Store, name: &str, default: i64) -> i64 {
    s.setting(name, "").trim().parse().unwrap_or(default)
}

/// Adds a schedule for `input.project`; the first job is due one cadence from
/// now (or at the next `at`).
pub fn add(s: &pulse_store::Store, input: ScheduleInput) -> Result<Schedule> {
    let title = clean(&input.title);
    if title.is_empty() {
        return Err(self::input(
            2,
            "A schedule needs a title",
            "the title names the job it queues",
            "pulse schedule add --project P --title 'what to do' --every 1d --brief '...'",
        ));
    }
    if title.len() > 200 {
        return Err(self::input(
            2,
            format!("title is {} chars; limit 200", title.len()),
            "keep the title short",
            "move detail into the brief (--brief TEXT or --brief-file F)",
        ));
    }
    if input.brief.trim().is_empty() {
        return Err(self::input(
            2,
            "A schedule needs a brief",
            "the worker that takes each job has nothing else to go on",
            "--brief 'what to do, how to know it is done' or --brief-file F",
        ));
    }
    let kind = if input.kind.is_empty() {
        "task".to_string()
    } else {
        input.kind.clone()
    };
    if !SCHEDULE_KINDS.contains(&kind.as_str()) {
        return Err(self::input(
            2,
            format!("A schedule cannot queue {kind:?} jobs"),
            format!("its kinds are {}", SCHEDULE_KINDS.join(", ")),
            "--kind task",
        ));
    }
    if input.tier != 0 && input.tier != 1 {
        return Err(self::input(
            2,
            "The runner takes Tier 0 and Tier 1 jobs only",
            "Tier 2 needs an ADR and the human",
            "--tier 0 or --tier 1",
        ));
    }
    if input.times < 0 {
        return Err(self::input(
            2,
            "--times counts jobs",
            "0 means no end",
            "--times 3",
        ));
    }
    if input.author.is_empty() {
        return Err(self::input(
            2,
            "An author is needed",
            "every schedule records who set it",
            format!(
                "pass --agent NAME or set PULSE_AGENT (the owner is {})",
                pulse_store::HUMAN_AGENT
            ),
        ));
    }
    check_project(s, &input.project)?;
    let every = parse_every(&input.every)?;
    let min = setting_int(s, "schedule.min_minutes", 30).max(1);
    if every < min {
        return Err(self::input(
            2,
            format!(
                "{} is shorter than the {} floor",
                format_every(every),
                format_every(min)
            ),
            "the setting schedule.min_minutes keeps standing jobs from turning into a loop",
            format!("--every {}", format_every(min)),
        ));
    }
    let at = parse_at(&input.at)?;
    if at.is_some() && every % MINUTES_PER_DAY != 0 {
        return Err(self::input(
            2,
            "--at needs a cadence of whole days",
            "a time of day anchors daily and weekly schedules",
            "--every 1d --at 09:00",
        ));
    }
    let cap = setting_int(s, "schedule.max_per_project", 10);
    let now = s.now();
    let tx = s.begin()?;
    if cap > 0 {
        let held: i64 = tx.query_row(
            "SELECT COUNT(*) FROM schedules WHERE project = ? AND status IN ('active', 'paused')",
            [&input.project],
            |r| r.get(0),
        )?;
        if held >= cap {
            return Err(self::input(
                2,
                format!("{} already holds {held} schedules", input.project),
                "the setting schedule.max_per_project caps them",
                format!(
                    "end one first: pulse schedule list --project {}",
                    input.project
                ),
            ));
        }
    }
    let id = next_id(&tx, "schedules", "SCHED-")?;
    let next_at = first_due(&now, every, at)?;
    tx.execute(
        "INSERT INTO schedules (id, project, title, kind, brief, tier, priority, every_minutes, at_time, status, next_at, max_runs, created_by, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, ?, ?)",
        params![
            id,
            input.project,
            title,
            kind,
            input.brief,
            input.tier,
            input.priority,
            every,
            at.map(|t| t.format("%H:%M").to_string()).unwrap_or_default(),
            next_at,
            input.times,
            input.author,
            now,
            now
        ],
    )?;
    tx.event(
        &input.author,
        "schedule.added",
        &id,
        pulse_store::Detail::json(json!({
            "project": input.project,
            "title": title,
            "kind": kind,
            "every": format_every(every),
            "next_at": next_at,
            "times": input.times,
        }))?,
    )?;
    tx.commit()?;
    get(s, &id)
}

pub fn get(s: &pulse_store::Store, id: &str) -> Result<Schedule> {
    let conn = s.conn()?;
    conn.query_row(
        &format!("SELECT {COLS} FROM schedules WHERE id = ?"),
        [id],
        scan,
    )
    .optional()?
    .ok_or_else(|| {
        input(
            2,
            format!("No schedule {id:?}"),
            "the id is not in this store",
            "pulse schedule list",
        )
    })
}

pub fn list(s: &pulse_store::Store, project: &str, status: &str) -> Result<Vec<Schedule>> {
    let mut q = format!("SELECT {COLS} FROM schedules WHERE 1=1");
    let mut args = Vec::new();
    if !project.is_empty() {
        q.push_str(" AND project = ?");
        args.push(project.to_string());
    }
    if !status.is_empty() {
        q.push_str(" AND status = ?");
        args.push(status.to_string());
    }
    q.push_str(
        " ORDER BY CASE status WHEN 'active' THEN 0 WHEN 'paused' THEN 1 ELSE 2 END, next_at, id",
    );
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    Ok(stmt
        .query_map(rusqlite::params_from_iter(refs), scan)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The jobs a schedule queued, newest first.
pub fn jobs(s: &pulse_store::Store, id: &str) -> Result<Vec<crate::tasks::Job>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT id, kind, title, status, lease_agent, branch, priority, created_at, finished_at, result FROM jobs WHERE source = ? ORDER BY created_at DESC")?;
    Ok(stmt
        .query_map([format!("{SOURCE_PREFIX}{id}")], |r| {
            Ok(crate::tasks::Job {
                id: r.get(0)?,
                kind: r.get(1)?,
                title: r.get(2)?,
                status: r.get(3)?,
                agent: r.get(4)?,
                branch: r.get(5)?,
                priority: r.get(6)?,
                created_at: r.get(7)?,
                finished_at: r.get(8)?,
                result: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn owned_by(sched: &Schedule, author: &str) -> Result<()> {
    if author.is_empty() {
        return Err(input(
            2,
            "An author is needed",
            "changes to a schedule record who made them",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    if author != sched.created_by && author != pulse_store::HUMAN_AGENT {
        return Err(input(
            2,
            format!("Schedule {} belongs to {}", sched.id, sched.created_by),
            "only the agent that set a schedule, or the owner, changes it",
            format!(
                "post a question for {} on the board, or ask the owner",
                sched.created_by
            ),
        ));
    }
    Ok(())
}

fn set_status(
    s: &pulse_store::Store,
    id: &str,
    status: &str,
    note: &str,
    author: &str,
    action: &str,
) -> Result<Schedule> {
    let sched = get(s, id)?;
    owned_by(&sched, author)?;
    if sched.status == "ended" {
        return Err(input(
            2,
            format!("Schedule {id} has ended"),
            "an ended schedule stays in the list for its history and never fires again",
            "pulse schedule add ... for a new one",
        ));
    }
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE schedules SET status = ?, note = ?, updated_at = ? WHERE id = ?",
        params![status, clean(note), now, id],
    )?;
    tx.event(
        author,
        action,
        id,
        pulse_store::Detail::json(json!({"project": sched.project, "note": clean(note)}))?,
    )?;
    tx.commit()?;
    get(s, id)
}

/// Pauses a schedule: it keeps its place and fires again after `resume`.
pub fn pause(s: &pulse_store::Store, id: &str, note: &str, author: &str) -> Result<Schedule> {
    set_status(s, id, "paused", note, author, "schedule.paused")
}

/// Resumes a paused schedule; a due time that passed while paused fires on
/// the next tick, once.
pub fn resume(s: &pulse_store::Store, id: &str, author: &str) -> Result<Schedule> {
    set_status(s, id, "active", "", author, "schedule.resumed")
}

/// Ends a schedule for good; its history stays.
pub fn end(s: &pulse_store::Store, id: &str, note: &str, author: &str) -> Result<Schedule> {
    set_status(s, id, "ended", note, author, "schedule.ended")
}

/// Makes the schedule due now; the daemon queues its job on the next tick.
pub fn run_now(s: &pulse_store::Store, id: &str, author: &str) -> Result<Schedule> {
    let sched = get(s, id)?;
    owned_by(&sched, author)?;
    if sched.status != "active" {
        return Err(input(
            2,
            format!("Schedule {id} is {}", sched.status),
            "only an active schedule fires",
            format!("pulse schedule resume {id}"),
        ));
    }
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE schedules SET next_at = ?, updated_at = ? WHERE id = ?",
        params![now, now, id],
    )?;
    tx.event(
        author,
        "schedule.run",
        id,
        pulse_store::Detail::json(json!({"project": sched.project}))?,
    )?;
    tx.commit()?;
    get(s, id)
}

/// The brief a scheduled job carries: the schedule's brief and where it came
/// from, so the worker knows the job recurs and how to change it.
pub fn job_brief(sched: &Schedule, run: i32) -> String {
    let mut brief = sched.brief.trim_end().to_string();
    brief.push_str(&format!(
        "\n\n---\nThis job is run {run} of schedule {} (every {}), set by {}. Do the work above and finish as usual; the schedule queues the next run by itself. `pulse schedule show {}` has its history; only {} or the owner pause or end it.",
        sched.id,
        sched.every(),
        sched.created_by,
        sched.id,
        sched.created_by
    ));
    if sched.max_runs > 0 {
        brief.push_str(&format!(" It ends after {} runs.", sched.max_runs));
    }
    brief
}

/// Queues a job for every active schedule of `projects` whose time has come
/// and moves each on to its next time. One open job per schedule: when the
/// previous run's job is still queued or leased, nothing new is queued and
/// the tick is skipped. Called by the daemon once per loop.
pub fn fire_due(
    s: &pulse_store::Store,
    now: &str,
    projects: &[String],
    actor: &str,
) -> Result<Vec<Fired>> {
    let due = list(s, "", "active")?
        .into_iter()
        .filter(|sc| sc.next_at.as_str() <= now)
        .filter(|sc| projects.is_empty() || projects.contains(&sc.project))
        .collect::<Vec<_>>();
    let mut out = Vec::new();
    for sched in due {
        let run = sched.runs + 1;
        let (job, queued) = pulse_board::add_job(
            s,
            pulse_board::JobInput {
                project: sched.project.clone(),
                kind: sched.kind.clone(),
                title: sched.title.clone(),
                brief: job_brief(&sched, run),
                tier: i64::from(sched.tier),
                priority: i64::from(sched.priority),
                source: sched.source(),
                created_by: sched.created_by.clone(),
                ..Default::default()
            },
        )?;
        let runs = if queued { run } else { sched.runs };
        let ended = sched.max_runs > 0 && runs >= sched.max_runs;
        let next_at = advance(&sched.next_at, i64::from(sched.every_minutes), now)?;
        let note = if ended {
            format!("ended after {runs} runs")
        } else {
            sched.note.clone()
        };
        let tx = s.begin()?;
        tx.execute(
            "UPDATE schedules SET runs = ?, last_at = CASE WHEN ? THEN ? ELSE last_at END,
             last_job = CASE WHEN ? THEN ? ELSE last_job END, next_at = ?,
             status = CASE WHEN ? THEN 'ended' ELSE status END, note = ?, updated_at = ? WHERE id = ?",
            params![runs, queued, now, queued, job.id, next_at, ended, note, now, sched.id],
        )?;
        tx.event(
            actor,
            if queued {
                "schedule.fired"
            } else {
                "schedule.skipped"
            },
            &sched.id,
            pulse_store::Detail::json(json!({
                "project": sched.project,
                "job": job.id,
                "run": runs,
                "next_at": next_at,
                "ended": ended,
            }))?,
        )?;
        if ended {
            tx.event(
                actor,
                "schedule.ended",
                &sched.id,
                pulse_store::Detail::json(json!({"project": sched.project, "note": note}))?,
            )?;
        }
        tx.commit()?;
        out.push(Fired {
            schedule: get(s, &sched.id)?,
            job: job.id,
            queued,
            ended,
        });
    }
    Ok(out)
}
