//! Role leads: besides the one lead every project has, the owner can seat a
//! lead per concern - RSI, guide docs, security, monitoring, tests,
//! dependencies, releases, performance, accessibility, issue triage,
//! tidiness, developer experience, or a word of their own - each with a
//! standing brief and a cadence. The daemon queues a
//! lead job for a role lead when it is due; the job runs as the role's own
//! agent (`<slug>-lead-<role>`), so its posts, runs and memory carry its
//! name. The daemon also looks at each project for a concern nobody owns
//! and asks the owner, once, whether to seat a lead for it.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::json;

use crate::schedules::{advance, format_every, parse_every};
use crate::tasks::{check_project, clean, input, next_id};

pub const LEAD_STATUSES: &[&str] = &["active", "paused", "ended"];
/// The `source` of every job a role lead's sitting queues:
/// `lead-role:<slug>:<role>:<stamp>`. The prefix is what tells a role lead's
/// job from the project lead's.
pub const SOURCE_PREFIX: &str = "lead-role:";
/// The board thread the daemon asks the owner on about a missing lead:
/// `lead/<role>`.
pub const BOARD_AREA: &str = "lead";
/// The shortest cadence a role lead may sit at.
pub const MIN_EVERY_MINUTES: i64 = 60;

/// A concern pulse knows how to brief a lead on. Custom roles need a title
/// and a brief from whoever seats them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Role {
    pub name: &'static str,
    pub title: &'static str,
    /// What it owns, for the suggestion and the list.
    pub about: &'static str,
    pub every: &'static str,
}

pub const BUILTIN_ROLES: &[Role] = &[
    Role {
        name: "rsi",
        title: "RSI lead",
        about: "keeps the self-improvement loop measured and moving: the evolution plan, the ballot, the experiment lane and the archive",
        every: "1d",
    },
    Role {
        name: "docs",
        title: "Guide docs lead",
        about: "keeps the guides and README true to what the code does, writing the pages a change left behind",
        every: "1d",
    },
    Role {
        name: "security",
        title: "Security lead",
        about: "watches dependencies, secrets, permissions and the attack surface, and queues the fixes",
        every: "1w",
    },
    Role {
        name: "monitor",
        title: "Monitoring lead",
        about: "keeps the checks and watchers that tell when the product is broken, and reads what they say",
        every: "6h",
    },
    Role {
        name: "tests",
        title: "Test lead",
        about: "keeps the tests worth trusting: what is untested, what is flaky, what is slow, and that the check proves what a change claims",
        every: "1d",
    },
    Role {
        name: "deps",
        title: "Dependencies lead",
        about: "keeps dependencies current and the upgrade path short: the lockfile, major versions, deprecations, the toolchain pin",
        every: "1w",
    },
    Role {
        name: "release",
        title: "Release lead",
        about: "prepares each release: the changelog, the version, the notes a user reads, and what must be true before shipping",
        every: "1w",
    },
    Role {
        name: "perf",
        title: "Performance lead",
        about: "measures what users wait for - startup, the hot paths, the size of what ships - keeps the benchmarks honest and catches regressions",
        every: "1w",
    },
    Role {
        name: "a11y",
        title: "Accessibility lead",
        about: "uses the product the way every user does - keyboard, screen reader, zoom, a phone - and fixes what gets in the way",
        every: "1w",
    },
    Role {
        name: "triage",
        title: "Issue triage lead",
        about: "reads the repository's issues and pull requests, reproduces the bugs, drafts the answers and turns the rest into jobs",
        every: "1d",
    },
    Role {
        name: "tidy",
        title: "Tidy-up lead",
        about: "keeps the code easy to change: lint findings, dead code, duplicates, TODOs that aged, files that grew past their name",
        every: "1w",
    },
    Role {
        name: "dx",
        title: "Developer experience lead",
        about: "keeps the path from clone to green short: setup, the env recipe, build and test time, the tooling every worker and newcomer touches",
        every: "1w",
    },
];

pub fn builtin(role: &str) -> Option<&'static Role> {
    BUILTIN_ROLES.iter().find(|r| r.name == role)
}

/// The agent a role lead runs as.
pub fn agent_name(project: &str, role: &str) -> String {
    format!("{project}-lead-{role}")
}

/// `(project, role)` from a role lead's agent name, if it is one.
pub fn split_agent(agent: &str) -> Option<(&str, &str)> {
    let (project, role) = agent.rsplit_once("-lead-")?;
    if project.is_empty() || role.is_empty() {
        return None;
    }
    Some((project, role))
}

/// `(project, role)` from a job source `lead-role:<slug>:<role>:<stamp>`.
pub fn split_source(source: &str) -> Option<(&str, &str)> {
    let rest = source.strip_prefix(SOURCE_PREFIX)?;
    let mut parts = rest.splitn(3, ':');
    let project = parts.next()?;
    let role = parts.next()?;
    if project.is_empty() || role.is_empty() {
        return None;
    }
    Some((project, role))
}

/// A role word: lowercase letters, digits and hyphens, 2 to 24 characters,
/// starting with a letter.
pub fn check_role(role: &str) -> Result<String> {
    let role = role.trim().to_lowercase();
    let ok = role.len() >= 2
        && role.len() <= 24
        && role.starts_with(|c: char| c.is_ascii_lowercase())
        && role
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !role.contains("--")
        && !role.ends_with('-')
        // `<slug>-lead` is the project lead's name; a role containing the
        // word would let two agents share one name.
        && !role.split('-').any(|part| part == "lead");
    if !ok {
        return Err(input(
            2,
            format!("Not a role: {role:?}"),
            "a role is a short lowercase word: letters, digits and hyphens",
            format!(
                "--role {} or a word of your own, like content",
                BUILTIN_ROLES
                    .iter()
                    .map(|r| r.name)
                    .collect::<Vec<_>>()
                    .join("|")
            ),
        ));
    }
    Ok(role)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Lead {
    pub id: String,
    pub project: String,
    pub role: String,
    pub agent: String,
    pub title: String,
    /// The standing brief; empty means the built-in brief of a built-in role.
    pub brief: String,
    pub every_minutes: i32,
    pub status: String,
    pub next_at: String,
    pub last_at: String,
    pub last_job: String,
    pub runs: i32,
    pub note: String,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Lead {
    pub fn every(&self) -> String {
        format_every(i64::from(self.every_minutes))
    }
    /// The source of the job a sitting at `stamp` queues.
    pub fn source(&self, stamp: &str) -> String {
        format!("{SOURCE_PREFIX}{}:{}:{stamp}", self.project, self.role)
    }
    /// The prefix every job of this lead shares.
    pub fn source_prefix(&self) -> String {
        format!("{SOURCE_PREFIX}{}:{}:", self.project, self.role)
    }
}

#[derive(Debug, Clone, Default)]
pub struct LeadInput {
    pub project: String,
    pub role: String,
    pub title: String,
    pub brief: String,
    pub every: String,
    pub author: String,
}

const COLS: &str = "id, project, role, agent, title, brief, every_minutes, status, next_at, last_at, last_job, runs, note, created_by, created_at, updated_at";

fn scan(row: &rusqlite::Row<'_>) -> rusqlite::Result<Lead> {
    Ok(Lead {
        id: row.get(0)?,
        project: row.get(1)?,
        role: row.get(2)?,
        agent: row.get(3)?,
        title: row.get(4)?,
        brief: row.get(5)?,
        every_minutes: row.get(6)?,
        status: row.get(7)?,
        next_at: row.get(8)?,
        last_at: row.get(9)?,
        last_job: row.get(10)?,
        runs: row.get(11)?,
        note: row.get(12)?,
        created_by: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

/// Seats a role lead. A built-in role brings its title, cadence and brief;
/// a custom role needs a title and a brief. One live lead per role and
/// project.
pub fn add(s: &pulse_store::Store, input: LeadInput) -> Result<Lead> {
    let role = check_role(&input.role)?;
    let known = builtin(&role);
    let title = {
        let t = clean(&input.title);
        if !t.is_empty() {
            t
        } else if let Some(r) = known {
            r.title.to_string()
        } else {
            return Err(self::input(
                2,
                format!("A {role} lead needs a title"),
                "pulse knows no such role, so it cannot name what the lead owns",
                format!(
                    "--title 'Content lead' --brief '...' (built-in roles need neither: {})",
                    builtin_names()
                ),
            ));
        }
    };
    if title.len() > 120 {
        return Err(self::input(
            2,
            format!("title is {} chars; limit 120", title.len()),
            "the title names what the lead owns",
            "move detail into the brief (--brief TEXT or --brief-file F)",
        ));
    }
    let brief = input.brief.trim().to_string();
    if brief.is_empty() && known.is_none() {
        return Err(self::input(
            2,
            format!("A {role} lead needs a brief"),
            "the brief is the standing instruction every sitting starts from",
            "--brief 'what it owns, what to check, what counts as done' or --brief-file F",
        ));
    }
    let every_text = if input.every.trim().is_empty() {
        known.map(|r| r.every).unwrap_or("1d").to_string()
    } else {
        input.every.clone()
    };
    let every = parse_every(&every_text)?;
    if every < MIN_EVERY_MINUTES {
        return Err(self::input(
            2,
            format!(
                "{} is shorter than the {} floor",
                format_every(every),
                format_every(MIN_EVERY_MINUTES)
            ),
            "a lead that sits more often than hourly turns into a loop",
            format!("--every {}", format_every(MIN_EVERY_MINUTES)),
        ));
    }
    if input.author.is_empty() {
        return Err(self::input(
            2,
            "An author is needed",
            "every lead records who seated it",
            format!(
                "pass --agent NAME or set PULSE_AGENT (the owner is {})",
                pulse_store::HUMAN_AGENT
            ),
        ));
    }
    check_project(s, &input.project)?;
    let now = s.now();
    let tx = s.begin()?;
    let live: Option<String> = tx
        .query_row(
            "SELECT id FROM leads WHERE project = ? AND role = ? AND status != 'ended'",
            params![input.project, role],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = live {
        return Err(self::input(
            2,
            format!("{} already has a {role} lead: {id}", input.project),
            "one lead owns a role at a time",
            format!("pulse lead show {id}, or pulse lead end {id} first"),
        ));
    }
    let id = next_id(&tx, "leads", "LEAD-")?;
    let agent = agent_name(&input.project, &role);
    // The seat is due at once: the owner seats a lead because the concern
    // wants attention now, so the daemon queues the first sitting on its next
    // tick and the cadence counts from there.
    let next_at = now.clone();
    tx.execute(
        "INSERT INTO leads (id, project, role, agent, title, brief, every_minutes, status, next_at, created_by, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, ?)",
        params![
            id,
            input.project,
            role,
            agent,
            title,
            brief,
            every,
            next_at,
            input.author,
            now,
            now
        ],
    )?;
    tx.event(
        &input.author,
        "lead.seated",
        &id,
        pulse_store::Detail::json(json!({
            "project": input.project,
            "role": role,
            "agent": agent,
            "title": title,
            "every": format_every(every),
            "next_at": next_at,
        }))?,
    )?;
    tx.commit()?;
    get(s, &id)
}

pub fn builtin_names() -> String {
    BUILTIN_ROLES
        .iter()
        .map(|r| r.name)
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn get(s: &pulse_store::Store, id: &str) -> Result<Lead> {
    let conn = s.conn()?;
    conn.query_row(
        &format!("SELECT {COLS} FROM leads WHERE id = ?"),
        [id],
        scan,
    )
    .optional()?
    .ok_or_else(|| {
        input(
            2,
            format!("No lead {id:?}"),
            "the id is not in this store",
            "pulse lead list",
        )
    })
}

/// The live (active or paused) lead for a role in a project, if one is
/// seated.
pub fn for_role(s: &pulse_store::Store, project: &str, role: &str) -> Result<Option<Lead>> {
    let conn = s.conn()?;
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLS} FROM leads WHERE project = ? AND role = ? AND status != 'ended' ORDER BY created_at DESC LIMIT 1"
            ),
            params![project, role],
            scan,
        )
        .optional()?)
}

/// The live lead that runs as `agent`, if any.
pub fn for_agent(s: &pulse_store::Store, agent: &str) -> Result<Option<Lead>> {
    let conn = s.conn()?;
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLS} FROM leads WHERE agent = ? AND status != 'ended' ORDER BY created_at DESC LIMIT 1"
            ),
            [agent],
            scan,
        )
        .optional()?)
}

pub fn list(s: &pulse_store::Store, project: &str, status: &str) -> Result<Vec<Lead>> {
    let mut q = format!("SELECT {COLS} FROM leads WHERE 1=1");
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
        " ORDER BY CASE status WHEN 'active' THEN 0 WHEN 'paused' THEN 1 ELSE 2 END, project, role, id",
    );
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    Ok(stmt
        .query_map(rusqlite::params_from_iter(refs), scan)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The jobs a lead's sittings queued, newest first.
pub fn jobs(s: &pulse_store::Store, lead: &Lead) -> Result<Vec<crate::tasks::Job>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT id, kind, title, status, lease_agent, branch, priority, created_at, finished_at, result FROM jobs WHERE source LIKE ? ORDER BY created_at DESC")?;
    Ok(stmt
        .query_map([format!("{}%", lead.source_prefix())], |r| {
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

fn owned_by(lead: &Lead, author: &str) -> Result<()> {
    if author.is_empty() {
        return Err(input(
            2,
            "An author is needed",
            "changes to a lead record who made them",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    if author != lead.created_by && author != pulse_store::HUMAN_AGENT {
        return Err(input(
            2,
            format!("Lead {} was seated by {}", lead.id, lead.created_by),
            "only who seated a lead, or the owner, changes it",
            format!(
                "post a question for {} on the board, or ask the owner",
                lead.created_by
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
) -> Result<Lead> {
    let lead = get(s, id)?;
    owned_by(&lead, author)?;
    if lead.status == "ended" {
        return Err(input(
            2,
            format!("Lead {id} has ended"),
            "an ended lead stays in the list for its history and never sits again",
            format!(
                "pulse lead add --project {} --role {} for a new one",
                lead.project, lead.role
            ),
        ));
    }
    if lead.status == status {
        return Ok(lead);
    }
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE leads SET status = ?, note = ?, updated_at = ? WHERE id = ?",
        params![status, clean(note), now, id],
    )?;
    tx.event(
        author,
        action,
        id,
        pulse_store::Detail::json(json!({
            "project": lead.project,
            "role": lead.role,
            "agent": lead.agent,
            "note": clean(note),
        }))?,
    )?;
    tx.commit()?;
    get(s, id)
}

/// Pauses a lead: it keeps its seat and sits again after `resume`.
pub fn pause(s: &pulse_store::Store, id: &str, note: &str, author: &str) -> Result<Lead> {
    set_status(s, id, "paused", note, author, "lead.paused")
}

pub fn resume(s: &pulse_store::Store, id: &str, author: &str) -> Result<Lead> {
    set_status(s, id, "active", "", author, "lead.resumed")
}

/// Ends a lead for good; its history and its agent's posts stay. Sittings
/// still waiting in the queue are cancelled; one that is running finishes.
pub fn end(s: &pulse_store::Store, id: &str, note: &str, author: &str) -> Result<Lead> {
    let lead = set_status(s, id, "ended", note, author, "lead.ended")?;
    let queued: Vec<String> = {
        let conn = s.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id FROM jobs WHERE kind = 'lead' AND source LIKE ? AND status IN ('queued', 'blocked') ORDER BY id",
        )?;
        stmt.query_map([format!("{}%", lead.source_prefix())], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
    };
    for job in queued {
        let _ = pulse_board::finish_job(
            s,
            &job,
            author,
            "cancelled",
            &format!("lead {} ended", lead.id),
            "",
            None,
        );
    }
    Ok(lead)
}

/// Makes the lead due now; the daemon queues its sitting on the next tick.
pub fn run_now(s: &pulse_store::Store, id: &str, author: &str) -> Result<Lead> {
    let lead = get(s, id)?;
    owned_by(&lead, author)?;
    if lead.status != "active" {
        return Err(input(
            2,
            format!("Lead {id} is {}", lead.status),
            "only an active lead sits",
            format!("pulse lead resume {id}"),
        ));
    }
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE leads SET next_at = ?, updated_at = ? WHERE id = ?",
        params![now, now, id],
    )?;
    tx.event(
        author,
        "lead.run",
        id,
        pulse_store::Detail::json(json!({"project": lead.project, "role": lead.role}))?,
    )?;
    tx.commit()?;
    get(s, id)
}

/// The active leads whose `next_at` has passed.
pub fn due(s: &pulse_store::Store, project: &str, now: &str) -> Result<Vec<Lead>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM leads WHERE project = ? AND status = 'active' AND next_at <= ? ORDER BY next_at, id"
    ))?;
    Ok(stmt
        .query_map(params![project, now], scan)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Records that a sitting was queued as `job` and moves `next_at` one
/// cadence past `now`.
pub fn fired(s: &pulse_store::Store, id: &str, job: &str, now: &str) -> Result<Lead> {
    let lead = get(s, id)?;
    let next = advance(&lead.next_at, i64::from(lead.every_minutes), now)?;
    let conn = s.conn()?;
    conn.execute(
        "UPDATE leads SET next_at = ?, last_at = ?, last_job = ?, runs = runs + 1, updated_at = ? WHERE id = ?",
        params![next, now, job, now, id],
    )?;
    get(s, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestStore {
        _home: tempfile::TempDir,
        store: pulse_store::Store,
    }

    impl std::ops::Deref for TestStore {
        type Target = pulse_store::Store;
        fn deref(&self) -> &pulse_store::Store {
            &self.store
        }
    }

    fn store() -> TestStore {
        // A worker's PULSE_JOB makes `worker_guard` refuse to create a store;
        // clear it as crate::tests::new_store does so the order tests run in
        // (or the verify hook's environment) cannot fail this one.
        let _guard = pulse_testutil::env_lock();
        unsafe {
            std::env::set_var("PULSE_JOB", "");
        }
        let home = tempfile::tempdir().unwrap();
        let s = pulse_store::Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)",
                params![home.path().join("demo").display().to_string(), s.now()],
            )
            .unwrap();
        TestStore {
            _home: home,
            store: s,
        }
    }

    fn seat(s: &pulse_store::Store, role: &str) -> Lead {
        add(
            s,
            LeadInput {
                project: "demo".into(),
                role: role.into(),
                author: pulse_store::HUMAN_AGENT.into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn builtin_role_brings_title_and_cadence() {
        let s = store();
        let l = seat(&s, "rsi");
        assert_eq!(l.id, "LEAD-001");
        assert_eq!(l.agent, "demo-lead-rsi");
        assert_eq!(l.title, "RSI lead");
        assert_eq!(l.every(), "1d");
        assert_eq!(l.status, "active");
        assert!(
            l.brief.is_empty(),
            "built-in brief is looked up at run time"
        );
        let m = seat(&s, "monitor");
        assert_eq!(m.every(), "6h");
    }

    #[test]
    fn custom_role_needs_title_and_brief() {
        let s = store();
        let err = add(
            &s,
            LeadInput {
                project: "demo".into(),
                role: "content".into(),
                author: "owner".into(),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("needs a title"), "{err}");
        let l = add(
            &s,
            LeadInput {
                project: "demo".into(),
                role: "Content".into(),
                title: "Content lead".into(),
                brief: "Write one post a week.".into(),
                every: "1w".into(),
                author: "owner".into(),
            },
        )
        .unwrap();
        assert_eq!(l.role, "content");
        assert_eq!(l.every(), "1w");
        for bad in ["", "x", "-rsi", "two words", "a--b", "UPPER-", "trailing-"] {
            assert!(check_role(bad).is_err(), "{bad:?}");
        }
        assert_eq!(check_role(" Docs ").unwrap(), "docs");
    }

    #[test]
    fn one_live_lead_per_role() {
        let s = store();
        let first = seat(&s, "docs");
        let err = add(
            &s,
            LeadInput {
                project: "demo".into(),
                role: "docs".into(),
                author: "owner".into(),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("already has a docs lead: LEAD-001"), "{err}");
        end(&s, &first.id, "done", pulse_store::HUMAN_AGENT).unwrap();
        let again = seat(&s, "docs");
        assert_eq!(again.id, "LEAD-002");
        assert_eq!(
            for_role(&s, "demo", "docs").unwrap().unwrap().id,
            "LEAD-002"
        );
        assert_eq!(
            for_agent(&s, "demo-lead-docs").unwrap().unwrap().id,
            "LEAD-002"
        );
        assert!(for_agent(&s, "demo-lead").unwrap().is_none());
    }

    #[test]
    fn cadence_floor_and_status_changes() {
        let s = store();
        let err = add(
            &s,
            LeadInput {
                project: "demo".into(),
                role: "monitor".into(),
                every: "30m".into(),
                author: "owner".into(),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("shorter than the 1h floor"), "{err}");
        let l = seat(&s, "security");
        let p = pause(&s, &l.id, "holiday", pulse_store::HUMAN_AGENT).unwrap();
        assert_eq!((p.status.as_str(), p.note.as_str()), ("paused", "holiday"));
        assert!(
            pause(&s, &l.id, "x", "stranger").is_err(),
            "only the seater or the owner"
        );
        let r = resume(&s, &l.id, pulse_store::HUMAN_AGENT).unwrap();
        assert_eq!(r.status, "active");
        let e = end(&s, &l.id, "", pulse_store::HUMAN_AGENT).unwrap();
        assert_eq!(e.status, "ended");
        assert!(resume(&s, &l.id, pulse_store::HUMAN_AGENT).is_err());
        assert_eq!(list(&s, "demo", "ended").unwrap().len(), 1);
    }

    #[test]
    fn due_and_fired_advance_the_clock() {
        let s = store();
        let l = seat(&s, "rsi");
        // A fresh seat is due at once: its first sitting is the next tick's.
        let d = due(&s, "demo", &s.now()).unwrap();
        assert_eq!(d.len(), 1);
        let after = fired(&s, &l.id, "JOB-0", &s.now()).unwrap();
        assert!(after.next_at > s.now());
        assert!(due(&s, "demo", &s.now()).unwrap().is_empty());
        let l = run_now(&s, &l.id, pulse_store::HUMAN_AGENT).unwrap();
        let d = due(&s, "demo", &s.now()).unwrap();
        assert_eq!(d.len(), 1);
        let after = fired(&s, &l.id, "JOB-1", &s.now()).unwrap();
        assert_eq!(after.runs, 2);
        assert_eq!(after.last_job, "JOB-1");
        assert!(after.next_at > s.now());
        assert!(due(&s, "demo", &s.now()).unwrap().is_empty());
        assert_eq!(
            l.source("20260101T000000Z"),
            "lead-role:demo:rsi:20260101T000000Z"
        );
        assert_eq!(split_source("lead-role:demo:rsi:x"), Some(("demo", "rsi")));
        assert_eq!(split_source("lead-idle:demo:x"), None);
        assert_eq!(split_agent("demo-lead-rsi"), Some(("demo", "rsi")));
        assert_eq!(split_agent("demo-lead"), None);
    }
}
