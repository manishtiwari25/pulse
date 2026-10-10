//! Forgetting a project: the hard delete behind `pulse remove project SLUG --forget`.
//!
//! `pulse remove project` only deactivates: the registry row and everything the
//! project wrote stay, the hub keeps listing it and the daemon keeps probing its
//! checkout. A project whose checkout is gone for good can be forgotten instead.
//! Its registry row and every row it left - posts, jobs, runs, briefs, decisions,
//! records with their revisions and drafts, harness sessions with their steps,
//! tasks, messages, schedules, compute requests and machines, the evolution
//! tables, hooks and their deliveries, its owner goal, the ledger events about
//! those rows and, when nothing outside the project names it, its lead agent -
//! go in one transaction, children before the rows they refer to, and one
//! `project.forgotten` event records what went. The delete guards on the tables
//! involved are dropped for the transaction and recreated from the definitions
//! `sqlite_master` holds, as `pulse remove duplicate-findings` does for posts;
//! the schema is the same afterwards and a failure anywhere rolls everything
//! back, triggers included.
//!
//! The store refuses while anything is in flight (a running run or harness
//! session, a leased job) and while the project is still active; whether the
//! checkout is really gone is the CLI's call.

use crate::store::{Store, input_err};
use anyhow::{Result, anyhow};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// The cascade, children first: `?1` is the slug, `?2` the lead agent that goes
/// with the project ('' when it stays). Tables with a `project` column go by it;
/// the ones without go by the key that ties them to a row that has one, and the
/// ledger by subject - the project itself, its lead and the ids of its rows.
const CASCADE: &[(&str, &str)] = &[
    (
        "events",
        "subject IN (SELECT ?1 UNION ALL SELECT ?2 WHERE ?2 != '' \
         UNION ALL SELECT id FROM posts WHERE project = ?1 \
         UNION ALL SELECT id FROM jobs WHERE project = ?1 \
         UNION ALL SELECT id FROM runs WHERE project = ?1 \
         UNION ALL SELECT id FROM records WHERE project = ?1 \
         UNION ALL SELECT id FROM harness_sessions WHERE project = ?1 \
         UNION ALL SELECT id FROM tasks WHERE project = ?1 \
         UNION ALL SELECT id FROM messages WHERE project = ?1 \
         UNION ALL SELECT id FROM schedules WHERE project = ?1 \
         UNION ALL SELECT id FROM compute_requests WHERE project = ?1 \
         UNION ALL SELECT id FROM evolution_fixtures WHERE project = ?1 \
         UNION ALL SELECT id FROM evolution_fleets WHERE project = ?1 \
         UNION ALL SELECT id FROM evolution_archive WHERE project = ?1)",
    ),
    (
        "hook_deliveries",
        "project = ?1 OR hook_id IN (SELECT id FROM hooks WHERE project = ?1)",
    ),
    ("hooks", "project = ?1"),
    (
        "briefs",
        "job_id IN (SELECT id FROM jobs WHERE project = ?1) \
         OR run_id IN (SELECT id FROM runs WHERE project = ?1)",
    ),
    (
        "harness_steps",
        "session_id IN (SELECT id FROM harness_sessions WHERE project = ?1)",
    ),
    ("harness_sessions", "project = ?1"),
    ("runs", "project = ?1"),
    ("jobs", "project = ?1"),
    (
        "decisions",
        "project = ?1 OR post_id IN (SELECT id FROM posts WHERE project = ?1)",
    ),
    ("posts", "project = ?1"),
    (
        "record_drafts",
        "record_id IN (SELECT id FROM records WHERE project = ?1)",
    ),
    (
        "record_revisions",
        "record_id IN (SELECT id FROM records WHERE project = ?1)",
    ),
    ("records", "project = ?1"),
    (
        "task_revisions",
        "task_id IN (SELECT id FROM tasks WHERE project = ?1)",
    ),
    ("tasks", "project = ?1"),
    ("messages", "project = ?1"),
    ("schedules", "project = ?1"),
    (
        "compute_machines",
        "request_id IN (SELECT id FROM compute_requests WHERE project = ?1)",
    ),
    ("compute_requests", "project = ?1"),
    (
        "evolution_fixture_keys",
        "fixture_id IN (SELECT id FROM evolution_fixtures WHERE project = ?1)",
    ),
    ("evolution_fixtures", "project = ?1"),
    ("evolution_archive", "project = ?1"),
    ("evolution_baselines", "project = ?1"),
    ("evolution_fleets", "project = ?1"),
    ("evolution_cycle_leases", "project = ?1"),
    ("settings", "name = 'project.goal.' || ?1"),
    (
        "meta",
        "?2 != '' AND key IN ('agent-beat:' || ?2, 'agent-heartbeat:' || ?2)",
    ),
    ("agents", "?2 != '' AND name = ?2"),
    ("projects", "slug = ?1"),
];

/// The project's lead goes with it unless posts, runs or harness sessions of
/// another project still name it; then it stays for `pulse agent prune`.
const LEAD_SQL: &str = "SELECT name FROM agents WHERE name = ?1 || '-lead' AND kind = 'lead' \
     AND NOT EXISTS (SELECT 1 FROM posts WHERE agent = agents.name AND project != ?1) \
     AND NOT EXISTS (SELECT 1 FROM runs WHERE agent = agents.name AND project != ?1) \
     AND NOT EXISTS (SELECT 1 FROM harness_sessions WHERE agent = agents.name AND project != ?1)";

/// Rows one table loses when a project is forgotten.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetRows {
    pub table: String,
    pub rows: i64,
}

/// What forgetting a project deletes (the preview) or deleted (`applied`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetReport {
    pub slug: String,
    pub path: String,
    pub status: String,
    /// The lead agent that goes too; '' when there is none or it stays.
    pub lead: String,
    /// Why forget would refuse right now: running runs and harness sessions,
    /// leased jobs. Empty when nothing is in flight.
    pub in_flight: Vec<String>,
    /// Every table of the cascade in deletion order, zero rows included.
    pub tables: Vec<ForgetRows>,
    pub applied: bool,
}

impl ForgetReport {
    pub fn total(&self) -> i64 {
        self.tables.iter().map(|t| t.rows).sum()
    }
    pub fn rows(&self, table: &str) -> i64 {
        self.tables
            .iter()
            .find(|t| t.table == table)
            .map_or(0, |t| t.rows)
    }
    /// The tables that lose rows, for the event detail and the console.
    pub fn nonzero(&self) -> Vec<&ForgetRows> {
        self.tables.iter().filter(|t| t.rows > 0).collect()
    }
}

fn bind_run(conn: &rusqlite::Connection, sql: &str, slug: &str, lead: &str) -> Result<i64> {
    let mut stmt = conn.prepare(sql)?;
    let n = if stmt.parameter_count() >= 2 {
        stmt.execute(params![slug, lead])?
    } else {
        stmt.execute(params![slug])?
    };
    Ok(n as i64)
}

fn bind_count(conn: &rusqlite::Connection, sql: &str, slug: &str, lead: &str) -> Result<i64> {
    let mut stmt = conn.prepare(sql)?;
    let n = if stmt.parameter_count() >= 2 {
        stmt.query_row(params![slug, lead], |r| r.get(0))?
    } else {
        stmt.query_row(params![slug], |r| r.get(0))?
    };
    Ok(n)
}

fn in_flight(conn: &rusqlite::Connection, slug: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT id FROM runs WHERE project = ? AND status = 'running' ORDER BY started_at, id",
    )?;
    for id in stmt.query_map([slug], |r| r.get::<_, String>(0))? {
        out.push(format!("run {} is running", id?));
    }
    let mut stmt = conn.prepare(
        "SELECT id FROM harness_sessions WHERE project = ? AND status = 'running' ORDER BY started_at, id",
    )?;
    for id in stmt.query_map([slug], |r| r.get::<_, String>(0))? {
        out.push(format!("harness session {} is running", id?));
    }
    let mut stmt = conn.prepare(
        "SELECT id, lease_agent FROM jobs WHERE project = ? AND status = 'leased' ORDER BY created_at, id",
    )?;
    for row in stmt.query_map([slug], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        let (id, agent) = row?;
        out.push(format!("job {id} is leased by {agent}"));
    }
    Ok(out)
}

fn report(conn: &rusqlite::Connection, slug: &str) -> Result<ForgetReport> {
    let Some((path, status)) = conn
        .query_row(
            "SELECT path, status FROM projects WHERE slug = ?",
            [slug],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    else {
        return Err(input_err(format!("No project {slug:?}")));
    };
    let lead: String = conn
        .query_row(LEAD_SQL, [slug], |r| r.get(0))
        .optional()?
        .unwrap_or_default();
    let mut tables = Vec::with_capacity(CASCADE.len());
    for (table, where_sql) in CASCADE {
        let rows = bind_count(
            conn,
            &format!("SELECT COUNT(*) FROM {table} WHERE {where_sql}"),
            slug,
            &lead,
        )?;
        tables.push(ForgetRows {
            table: (*table).to_string(),
            rows,
        });
    }
    Ok(ForgetReport {
        slug: slug.to_string(),
        path,
        status,
        lead,
        in_flight: in_flight(conn, slug)?,
        tables,
        applied: false,
    })
}

fn refusal(r: &ForgetReport) -> Option<anyhow::Error> {
    if r.status == "active" {
        return Some(input_err(format!(
            "Project {} is still active; pulse remove project {} deactivates it first",
            r.slug, r.slug
        )));
    }
    if let Some(first) = r.in_flight.first() {
        return Some(input_err(format!(
            "Project {} has work in flight ({}{}); wait for it to finish",
            r.slug,
            first,
            if r.in_flight.len() > 1 {
                format!(" and {} more", r.in_flight.len() - 1)
            } else {
                String::new()
            }
        )));
    }
    None
}

impl Store {
    /// What `forget_project` would delete, table by table, and why it would
    /// refuse (`in_flight`, an active `status`). Reads only.
    pub fn forget_project_preview(&self, slug: &str) -> Result<ForgetReport> {
        let conn = self.conn()?;
        report(&conn, slug)
    }

    /// Deletes an inactive project and every row it left (see the module doc) in
    /// one transaction and writes a `project.forgotten` event naming the counts.
    /// Refuses while the project is active or has work in flight. The caller
    /// decides whether the checkout is really gone.
    pub fn forget_project(&self, slug: &str, actor: &str) -> Result<ForgetReport> {
        let actor = if actor.is_empty() {
            crate::store::HUMAN_AGENT
        } else {
            actor
        };
        let tx = self.begin()?;
        let mut rep = report(&tx, slug)?;
        if let Some(err) = refusal(&rep) {
            return Err(err);
        }
        let tables: Vec<String> = CASCADE.iter().map(|(t, _)| (*t).to_string()).collect();
        let guards = delete_guards(&tx, &tables)?;
        for (name, _) in &guards {
            tx.execute(&format!("DROP TRIGGER {name}"), [])?;
        }
        let by_agent: Vec<(String, i64)> = tx
            .prepare("SELECT agent, COUNT(*) FROM posts WHERE project = ? GROUP BY agent")?
            .query_map([slug], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let lead = rep.lead.clone();
        for t in &mut rep.tables {
            let (_, where_sql) = CASCADE
                .iter()
                .find(|(name, _)| *name == t.table)
                .ok_or_else(|| anyhow!("forget: no cascade entry for {}", t.table))?;
            t.rows = bind_run(
                &tx,
                &format!("DELETE FROM {} WHERE {where_sql}", t.table),
                slug,
                &lead,
            )?;
        }
        for (agent, n) in by_agent {
            tx.execute(
                "UPDATE agents SET posts = MAX(posts - ?, 0) WHERE name = ?",
                params![n, agent],
            )?;
        }
        for (_, sql) in &guards {
            tx.execute(sql, [])?;
        }
        let restored = count_triggers(
            &tx,
            &guards.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
        )?;
        if restored != guards.len() as i64 {
            anyhow::bail!(
                "forget: {} of {} delete guards came back; nothing changed",
                restored,
                guards.len()
            );
        }
        let rows: serde_json::Map<String, serde_json::Value> = rep
            .nonzero()
            .iter()
            .map(|t| (t.table.clone(), serde_json::Value::from(t.rows)))
            .collect();
        tx.event(
            actor,
            "project.forgotten",
            slug,
            crate::store::Detail::json(serde_json::json!({
                "path": rep.path,
                "lead": rep.lead,
                "rows": rows,
                "total": rep.total(),
            }))?,
        )?;
        tx.commit()?;
        rep.applied = true;
        Ok(rep)
    }
}

/// The `BEFORE DELETE` guards on `tables`, with the SQL that recreates each.
fn delete_guards(conn: &rusqlite::Connection, tables: &[String]) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT name, tbl_name, sql FROM sqlite_master WHERE type = 'trigger' ORDER BY name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (name, table, sql) = row?;
        if !tables.contains(&table) {
            continue;
        }
        let Some(sql) = sql.filter(|s| !s.trim().is_empty()) else {
            anyhow::bail!("forget: trigger {name} has no stored definition");
        };
        if sql.to_uppercase().contains("BEFORE DELETE") {
            out.push((name, sql));
        }
    }
    Ok(out)
}

fn count_triggers(conn: &rusqlite::Connection, names: &[String]) -> Result<i64> {
    let mut n = 0;
    for name in names {
        n += conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?",
            [name],
            |r| r.get::<_, i64>(0),
        )?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Store};

    const NOW: &str = "2026-10-10T04:00:00Z";

    fn open() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: NOW.into(),
            ..Options::default()
        })
        .unwrap();
        (dir, s)
    }

    /// One row in every table of the cascade (two posts, so the agent counter
    /// moves), all tied to `slug`; ids carry the slug so the two projects never
    /// collide.
    fn seed(s: &Store, slug: &str) {
        let conn = s.conn().unwrap();
        let x = |sql: &str, args: &[&dyn rusqlite::ToSql]| {
            conn.execute(sql, args)
                .unwrap_or_else(|e| panic!("{sql}: {e}"));
        };
        let lead = format!("{slug}-lead");
        x(
            "INSERT INTO projects (slug, name, path, status, added_at) VALUES (?, ?, ?, 'inactive', ?)",
            &[&slug, &slug, &format!("/nonexistent/{slug}"), &NOW],
        );
        x(
            "INSERT INTO agents (name, kind, created_at, posts) VALUES (?, 'lead', ?, 1)",
            &[&lead, &NOW],
        );
        x(
            "INSERT INTO meta (key, value) VALUES ('agent-beat:' || ?, ?), ('agent-heartbeat:' || ?, ?)",
            &[&lead, &NOW, &lead, &NOW],
        );
        x(
            "INSERT INTO settings (name, value, updated_at) VALUES ('project.goal.' || ?, '{}', ?)",
            &[&slug, &NOW],
        );
        for (i, agent) in [(1, "shared"), (2, lead.as_str())] {
            x(
                "INSERT INTO posts (id, project, area, thread, kind, agent, date, created_at) VALUES (?, ?, 'runner', 'topic', 'finding', ?, ?, ?)",
                &[&format!("{slug}-post-{i}"), &slug, &agent, &NOW, &NOW],
            );
        }
        x(
            "INSERT INTO decisions (post_id, project, answer, decided_at) VALUES (?, ?, 'yes', ?)",
            &[&format!("{slug}-post-1"), &slug, &NOW],
        );
        x(
            "INSERT INTO jobs (id, project, kind, title, status, created_at) VALUES (?, ?, 'task', 'Tidy', 'done', ?)",
            &[&format!("{slug}-job"), &slug, &NOW],
        );
        x(
            "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES (?, ?, 'shared', ?, ?, 'done')",
            &[&format!("{slug}-run"), &format!("{slug}-job"), &slug, &NOW],
        );
        x(
            "INSERT INTO briefs (run_id, job_id, agent, text, at) VALUES (?, ?, 'shared', 'brief', ?)",
            &[&format!("{slug}-run"), &format!("{slug}-job"), &NOW],
        );
        x(
            "INSERT INTO harness_sessions (id, run_id, job_id, agent, project, status, started_at, updated_at, ended_at) VALUES (?, ?, ?, ?, ?, 'submitted', ?, ?, ?)",
            &[
                &format!("{slug}-hs"),
                &format!("{slug}-run"),
                &format!("{slug}-job"),
                &lead,
                &slug,
                &NOW,
                &NOW,
                &NOW,
            ],
        );
        x(
            "INSERT INTO harness_steps (session_id, n, at, kind, content) VALUES (?, 1, ?, 'assistant', 'hi')",
            &[&format!("{slug}-hs"), &NOW],
        );
        x(
            "INSERT INTO records (id, project, kind, slug, title, created_at, created_by, updated_at, origin) VALUES (?, ?, 'adr', 'first', 'First', ?, 'tester', ?, 'store')",
            &[&format!("ADR-001-{slug}"), &slug, &NOW, &NOW],
        );
        x(
            "INSERT INTO record_revisions (record_id, rev, frontmatter, body, author, at) VALUES (?, 1, '{}', '# First', 'tester', ?)",
            &[&format!("ADR-001-{slug}"), &NOW],
        );
        x(
            "INSERT INTO record_drafts (record_id, base_rev, body, author, at) VALUES (?, 1, 'draft', 'tester', ?)",
            &[&format!("ADR-001-{slug}"), &NOW],
        );
        x(
            "INSERT INTO tasks (id, project, title, created_by, created_at, updated_at) VALUES (?, ?, 'Do it', 'tester', ?, ?)",
            &[&format!("TASK-{slug}"), &slug, &NOW, &NOW],
        );
        x(
            "INSERT INTO task_revisions (task_id, rev, title, author, at) VALUES (?, 1, 'Do it', 'tester', ?)",
            &[&format!("TASK-{slug}"), &NOW],
        );
        x(
            "INSERT INTO messages (id, project, author, body, at) VALUES (?, ?, 'tester', 'hello', ?)",
            &[&format!("MSG-{slug}"), &slug, &NOW],
        );
        x(
            "INSERT INTO schedules (id, project, title, every_minutes, next_at, created_by, created_at, updated_at) VALUES (?, ?, 'Nightly', 60, ?, 'tester', ?, ?)",
            &[&format!("SCHED-{slug}"), &slug, &NOW, &NOW, &NOW],
        );
        x(
            "INSERT INTO compute_requests (id, project, requested_by, size, hours, at, updated_at) VALUES (?, ?, 'tester', 'small', 1, ?, ?)",
            &[&format!("CR-{slug}"), &slug, &NOW, &NOW],
        );
        x(
            "INSERT INTO compute_machines (request_id, provider, name, created_at, released_at) VALUES (?, 'azure', 'vm', ?, ?)",
            &[&format!("CR-{slug}"), &NOW, &NOW],
        );
        x(
            "INSERT INTO evolution_fixtures (id, project, name, request, scorer_cmd, created_by, at) VALUES (?, ?, 'fx', 'do', 'true', 'tester', ?)",
            &[&format!("fx-{slug}"), &slug, &NOW],
        );
        x(
            "INSERT INTO evolution_fixture_keys (fixture_id, path) VALUES (?, 'src/lib.rs')",
            &[&format!("fx-{slug}")],
        );
        x(
            "INSERT INTO evolution_archive (id, project, baseline_score, variant_score, tolerance, kept, at) VALUES (?, ?, 0.5, 0.6, 0.01, 1, ?)",
            &[&format!("exp-{slug}"), &slug, &NOW],
        );
        x(
            "INSERT INTO evolution_baselines (project, \"commit\", fixture_hash, score, fixtures_json, run_id, at) VALUES (?, 'abc', 'h', 0.5, '[]', '', ?)",
            &[&slug, &NOW],
        );
        x(
            "INSERT INTO evolution_fleets (id, project, ref, workers, stripped_json, at, created_by) VALUES (?, ?, 'main', 2, '{}', ?, 'tester')",
            &[&format!("fleet-{slug}"), &slug, &NOW],
        );
        x(
            "INSERT INTO evolution_cycle_leases (project, owner, expires_at) VALUES (?, 'daemon', ?)",
            &[&slug, &NOW],
        );
        x(
            "INSERT INTO hooks (event, kind, target, project, created_by, created_at) VALUES ('post.*', 'script', '/bin/true', ?, 'tester', ?)",
            &[&slug, &NOW],
        );
        let hook_id = conn.last_insert_rowid();
        x(
            "INSERT INTO hook_deliveries (hook_id, event_seq, event_action, project, attempt, status, at) VALUES (?, 1, 'post.add', ?, 1, 'ok', ?)",
            &[&hook_id, &slug, &NOW],
        );
        drop(conn);
        s.event("tester", "project.add", slug, "").unwrap();
        s.event("tester", "agent.join", &lead, "").unwrap();
        for subject in [
            format!("{slug}-post-1"),
            format!("{slug}-job"),
            format!("{slug}-run"),
            format!("ADR-001-{slug}"),
            format!("{slug}-hs"),
            format!("TASK-{slug}"),
            format!("MSG-{slug}"),
            format!("SCHED-{slug}"),
            format!("CR-{slug}"),
            format!("fx-{slug}"),
            format!("fleet-{slug}"),
            format!("exp-{slug}"),
        ] {
            s.event("tester", "x.touched", &subject, "").unwrap();
        }
    }

    fn triggers(s: &Store) -> Vec<String> {
        let conn = s.conn().unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn fk_violations(s: &Store) -> i64 {
        let conn = s.conn().unwrap();
        let mut stmt = conn.prepare("PRAGMA foreign_key_check").unwrap();
        stmt.query_map([], |_| Ok(())).unwrap().count() as i64
    }

    #[test]
    fn forget_project_clears_every_table_and_leaves_the_sibling_alone() {
        let _serial = crate::test_helpers::serial();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let (_dir, s) = open();
        {
            let conn = s.conn().unwrap();
            conn.execute(
                "INSERT INTO agents (name, kind, created_at, posts) VALUES ('shared', 'worker', ?, 2)",
                [NOW],
            )
            .unwrap();
        }
        seed(&s, "gone");
        seed(&s, "kept");
        let before = triggers(&s);
        assert!(before.iter().any(|t| t == "posts_no_delete"));
        let kept_before = s.forget_project_preview("kept").unwrap();

        let pre = s.forget_project_preview("gone").unwrap();
        assert_eq!(pre.status, "inactive");
        assert_eq!(pre.path, "/nonexistent/gone");
        assert_eq!(pre.lead, "gone-lead");
        assert!(pre.in_flight.is_empty());
        assert!(!pre.applied);
        assert_eq!(pre.tables.len(), CASCADE.len());
        for t in &pre.tables {
            let want = match t.table.as_str() {
                "posts" | "meta" => 2,
                // the project, its lead and one per row seeded with an event
                "events" => 14,
                _ => 1,
            };
            assert_eq!(t.rows, want, "preview rows of {}", t.table);
        }
        assert_eq!(pre.total(), pre.tables.iter().map(|t| t.rows).sum::<i64>());
        assert_eq!(pre.rows("posts"), 2);
        assert_eq!(pre.rows("no_such_table"), 0);
        assert_eq!(pre.nonzero().len(), CASCADE.len());

        let rep = s.forget_project("gone", "human").unwrap();
        assert!(rep.applied);
        assert_eq!(rep.tables, pre.tables, "applied counts match the preview");

        let conn = s.conn().unwrap();
        for (table, where_sql) in CASCADE {
            // The ledger keeps exactly one row about the project: the forgotten event.
            let extra = if *table == "events" {
                " AND action != 'project.forgotten'"
            } else {
                ""
            };
            let gone: i64 = bind_count(
                &conn,
                &format!("SELECT COUNT(*) FROM {table} WHERE ({where_sql}){extra}"),
                "gone",
                "gone-lead",
            )
            .unwrap();
            assert_eq!(gone, 0, "{table} still holds rows of the forgotten project");
        }
        let about: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE subject = 'gone'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(about, 1, "only project.forgotten remains");
        drop(conn);
        assert!(s.project("gone").unwrap().is_none());
        assert!(s.agent("gone-lead").unwrap().is_none());
        assert_eq!(s.project("kept").unwrap().unwrap().status, "inactive");
        let kept_after = s.forget_project_preview("kept").unwrap();
        assert_eq!(
            kept_after.tables, kept_before.tables,
            "the sibling lost nothing"
        );
        assert_eq!(kept_after.lead, "kept-lead");
        assert_eq!(
            s.agent("shared").unwrap().unwrap().posts,
            1,
            "one of its two posts went"
        );
        assert_eq!(triggers(&s), before, "every delete guard is back");
        assert_eq!(fk_violations(&s), 0);

        let conn = s.conn().unwrap();
        let detail: String = conn
            .query_row(
                "SELECT detail FROM events WHERE action = 'project.forgotten' AND subject = 'gone'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(v["path"], "/nonexistent/gone");
        assert_eq!(v["lead"], "gone-lead");
        assert_eq!(v["rows"]["posts"], 2);
        assert_eq!(v["rows"]["events"], 14);
        assert_eq!(v["total"], rep.total());
        // The guards hold again: the forgotten event itself cannot be deleted.
        assert!(
            conn.execute("DELETE FROM events WHERE action = 'project.forgotten'", [])
                .is_err()
        );
        assert!(
            conn.execute("DELETE FROM posts WHERE project = 'kept'", [])
                .is_err()
        );
        assert!(
            conn.execute("DELETE FROM records WHERE project = 'kept'", [])
                .is_err()
        );
    }

    #[test]
    fn forget_project_refuses_active_in_flight_and_unknown_projects() {
        let _serial = crate::test_helpers::serial();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let (_dir, s) = open();
        seed(&s, "busy");
        let conn = s.conn().unwrap();
        conn.execute(
            "UPDATE projects SET status = 'active' WHERE slug = 'busy'",
            [],
        )
        .unwrap();
        let err = s.forget_project("busy", "human").unwrap_err().to_string();
        assert!(err.contains("still active"), "{err}");
        conn.execute(
            "UPDATE projects SET status = 'inactive' WHERE slug = 'busy'",
            [],
        )
        .unwrap();

        conn.execute(
            "UPDATE runs SET status = 'running' WHERE id = 'busy-run'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE jobs SET status = 'leased', lease_agent = 'w1' WHERE id = 'busy-job'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE harness_sessions SET status = 'running' WHERE id = 'busy-hs'",
            [],
        )
        .unwrap();
        let pre = s.forget_project_preview("busy").unwrap();
        assert_eq!(
            pre.in_flight,
            vec![
                "run busy-run is running".to_string(),
                "harness session busy-hs is running".to_string(),
                "job busy-job is leased by w1".to_string(),
            ]
        );
        let err = s.forget_project("busy", "human").unwrap_err().to_string();
        assert!(
            err.contains("in flight") && err.contains("and 2 more"),
            "{err}"
        );
        assert!(s.project("busy").unwrap().is_some(), "nothing changed");
        let total = pre.total();
        assert_eq!(s.forget_project_preview("busy").unwrap().total(), total);

        let err = s.forget_project("never", "human").unwrap_err().to_string();
        assert!(err.contains("No project"), "{err}");
        assert!(s.forget_project_preview("never").is_err());
    }

    #[test]
    fn forget_project_keeps_a_lead_that_other_projects_still_name() {
        let _serial = crate::test_helpers::serial();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let (_dir, s) = open();
        seed(&s, "gone");
        seed(&s, "kept");
        let conn = s.conn().unwrap();
        conn.execute(
            "INSERT INTO posts (id, project, area, thread, kind, agent, date, created_at) VALUES ('kept-post-by-gone-lead', 'kept', 'runner', 'topic', 'finding', 'gone-lead', ?, ?)",
            [NOW, NOW],
        )
        .unwrap();
        drop(conn);
        let pre = s.forget_project_preview("gone").unwrap();
        assert_eq!(pre.lead, "", "the lead posted elsewhere, so it stays");
        assert_eq!(pre.rows("agents"), 0);
        assert_eq!(pre.rows("meta"), 0);
        assert_eq!(pre.rows("events"), 13, "the lead's agent.join stays too");
        let rep = s.forget_project("gone", "").unwrap();
        assert_eq!(rep.rows("agents"), 0);
        assert!(s.agent("gone-lead").unwrap().is_some());
        assert_eq!(
            s.agent("gone-lead").unwrap().unwrap().posts,
            0,
            "its post in gone went"
        );
        let conn = s.conn().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE subject = 'gone-lead'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        let actor: String = conn
            .query_row(
                "SELECT actor FROM events WHERE action = 'project.forgotten'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            actor,
            crate::store::HUMAN_AGENT,
            "an empty actor is the human"
        );
    }
}
