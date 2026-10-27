use super::*;
use pulse_cli::{Buffer, Context, FlagSet};
use rusqlite::params;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct TestStore {
    _home: TempDir,
    store: pulse_store::Store,
}
impl std::ops::Deref for TestStore {
    type Target = pulse_store::Store;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

fn new_store() -> TestStore {
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
            params![
                Path::new(home.path()).join("demo").display().to_string(),
                s.now()
            ],
        )
        .unwrap();
    s.conn()
        .unwrap()
        .execute(
            "INSERT INTO agents (name, kind, created_at) VALUES ('demo-lead', 'lead', ?)",
            [s.now()],
        )
        .unwrap();
    TestStore {
        _home: home,
        store: s,
    }
}

fn events(s: &pulse_store::Store, action: &str) -> Vec<pulse_bus::Event> {
    pulse_bus::since(
        s,
        0,
        pulse_bus::Filter {
            actions: vec![action.to_string()],
            ..pulse_bus::Filter::default()
        },
        0,
    )
    .unwrap()
}

#[test]
fn new_get_list_and_ids() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    assert!(new(&s, "nope", "x", "", 0, "human").is_err());
    assert!(new(&s, "demo", "   ", "", 0, "human").is_err());
    let a = new(
        &s,
        "demo",
        "  Ship   the thing ",
        "do it well\n",
        3,
        "human",
    )
    .unwrap();
    let b = new(&s, pulse_store::COMMONS_SLUG, "Second", "", 0, "demo-lead").unwrap();
    assert_eq!((a.id.as_str(), b.id.as_str()), ("TASK-001", "TASK-002"));
    assert_eq!(a.title, "Ship the thing");
    assert_eq!(a.brief, "do it well\n");
    assert_eq!(a.status, "open");
    assert_eq!(a.priority, 3);
    assert_eq!(a.current_rev, 1);
    assert_eq!(a.created_by, "human");
    let (got, revs) = get(&s, "TASK-001").unwrap();
    assert_eq!(got.id, a.id);
    assert_eq!(revs.len(), 1);
    assert_eq!(revs[0].brief, "do it well\n");
    let all = list(&s, "", "").unwrap();
    let demo = list(&s, "demo", "open").unwrap();
    let none = list(&s, "demo", "done").unwrap();
    assert_eq!(
        (all.len(), demo.len(), none.len(), all[0].id.as_str()),
        (2, 1, 0, "TASK-002")
    );
    let evs = events(&s, "task.created");
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0].subject, "TASK-001");
    assert!(evs[0].detail.contains(r#""project": "demo""#));
    assert!(get(&s, "TASK-999").is_err());
}

#[test]
fn amend_creates_revisions_and_refuses_finished() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let a = new(&s, "demo", "Title", "v1", 0, "human").unwrap();
    s.conn().unwrap().execute("INSERT INTO jobs (id, project, kind, title, brief, source, created_at) VALUES ('J-1', 'demo', 'task', 'Title', 'old brief', 'task:TASK-001', ?)", [s.now()]).unwrap();
    assert!(amend(&s, &a.id, "", "", "", "human").is_err());
    let b = amend(&s, &a.id, "", "v2", "more detail", "human").unwrap();
    assert_eq!(
        (b.current_rev, b.brief.as_str(), b.title.as_str()),
        (2, "v2", "Title")
    );
    let c = amend(&s, &a.id, "New title", "", "", "demo-lead").unwrap();
    assert_eq!(
        (c.current_rev, c.title.as_str(), c.brief.as_str()),
        (3, "New title", "v2")
    );
    let (_, revs) = get(&s, &a.id).unwrap();
    assert_eq!(revs.len(), 3);
    assert_eq!(revs[1].note, "more detail");
    assert_eq!(revs[2].author, "demo-lead");
    assert_eq!(revs[0].brief, "v1");
    let evs = events(&s, "task.amended");
    assert_eq!(evs.len(), 2);
    assert!(evs[0].detail.contains(r#""rev": 2"#));
    let brief: String = s
        .conn()
        .unwrap()
        .query_row("SELECT brief FROM jobs WHERE id = 'J-1'", [], |r| r.get(0))
        .unwrap();
    assert!(brief.contains("task amended: rev 2"));
    assert!(brief.contains("rev 3"));
    assert!(
        s.conn()
            .unwrap()
            .execute(
                "UPDATE task_revisions SET brief = 'hacked' WHERE task_id = ? AND rev = 1",
                [&a.id]
            )
            .is_err()
    );
    assert!(
        s.conn()
            .unwrap()
            .execute("DELETE FROM task_revisions WHERE task_id = ?", [&a.id])
            .is_err()
    );
    assert!(
        s.conn()
            .unwrap()
            .execute("DELETE FROM tasks WHERE id = ?", [&a.id])
            .is_err()
    );
    for st in ["planned", "in_progress", "review", "done"] {
        set_status(&s, &a.id, st, "demo-lead").unwrap();
    }
    assert!(amend(&s, &a.id, "", "v3", "", "human").is_err());
    assert!(cancel(&s, &a.id, "x", "human").is_err());
}

#[test]
fn state_machine() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let a = new(&s, "demo", "T", "", 0, "human").unwrap();
    assert!(set_status(&s, &a.id, "bogus", "x").is_err());
    assert!(set_status(&s, &a.id, "open", "x").is_err());
    let b = set_status(&s, &a.id, "in_progress", "demo-lead").unwrap();
    assert_eq!(b.status, "in_progress");
    assert!(set_status(&s, &a.id, "planned", "demo-lead").is_err());
    let evs = events(&s, "task.status");
    assert_eq!(evs.len(), 1);
    assert!(evs[0].detail.contains(r#""from": "open""#));
    assert!(evs[0].detail.contains(r#""to": "in_progress""#));
    // The one step back: review goes to in_progress when the work goes on.
    let r = set_status(&s, &a.id, "review", "demo-lead").unwrap();
    assert_eq!(r.status, "review");
    assert!(set_status(&s, &a.id, "planned", "demo-lead").is_err());
    let back = set_status(&s, &a.id, "in_progress", "demo-lead").unwrap();
    assert_eq!(back.status, "in_progress");
    assert!(set_status(&s, &a.id, "review", "demo-lead").is_ok());
    let c = set_status(&s, &a.id, "cancelled", "human").unwrap();
    assert_eq!(c.status, "cancelled");
    assert!(set_status(&s, &a.id, "done", "human").is_err());
    assert!(set_status(&s, &a.id, "in_progress", "human").is_err());
}

#[test]
fn owner_answer_ends_review_and_choices_are_checked() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let a = new(&s, "demo", "T", "", 0, "human").unwrap();
    for st in ["planned", "in_progress", "review"] {
        set_status(&s, &a.id, st, "demo-lead").unwrap();
    }
    // The lead asks with choices; the message keeps them in order.
    let ask = reply_with_choices(
        &s,
        &a.id,
        "demo-lead",
        "Two steps failed. How do we go on?",
        &[
            Choice::reply("Try again another way"),
            Choice::with_action("Good enough, close it", "done"),
            Choice::with_action("Change what I asked", "amend"),
        ],
    )
    .unwrap();
    assert_eq!(ask.choices.len(), 3);
    assert_eq!(
        (
            ask.choices[1].label.as_str(),
            ask.choices[1].action.as_str()
        ),
        ("Good enough, close it", "done")
    );
    assert_eq!(get_message(&s, &ask.id).unwrap().choices, ask.choices);
    assert_eq!(get(&s, &a.id).unwrap().0.status, "review");
    // An agent's message changes nothing; the owner's answer ends the review.
    reply(&s, &a.id, "demo-lead", "still waiting").unwrap();
    assert_eq!(get(&s, &a.id).unwrap().0.status, "review");
    let answer = reply(&s, &a.id, "human", "Try again another way").unwrap();
    assert!(answer.choices.is_empty());
    assert_eq!(get(&s, &a.id).unwrap().0.status, "in_progress");
    let evs = events(&s, "task.status");
    let last = evs.last().unwrap();
    assert!(last.detail.contains(r#""from": "review""#));
    assert!(last.detail.contains(r#""to": "in_progress""#));
    assert!(
        last.detail
            .contains(&format!(r#""message": "{}""#, answer.id))
    );
    // The owner writing on a task in progress leaves its status alone.
    reply(&s, &a.id, "human", "and hurry").unwrap();
    assert_eq!(get(&s, &a.id).unwrap().0.status, "in_progress");
    assert_eq!(events(&s, "task.status").len(), evs.len());
    // Choices the hub could not show are refused.
    assert!(check_choices(&[Choice::reply("")]).is_err());
    assert!(check_choices(&[Choice::reply("a".repeat(MAX_CHOICE_LABEL + 1))]).is_err());
    assert!(check_choices(&[Choice::with_action("x", "explode")]).is_err());
    assert!(check_choices(&[Choice::reply("Yes"), Choice::reply("yes")]).is_err());
    let many = (0..=MAX_CHOICES)
        .map(|i| Choice::reply(format!("c{i}")))
        .collect::<Vec<_>>();
    assert!(check_choices(&many).is_err());
    assert!(
        send_message_with_choices(&s, "demo", "", "demo-lead", "x", &[Choice::reply("y")]).is_err()
    );
    // --choice values: a bare label replies; label=action names the action.
    let parsed = choices_from_flags(&[
        "Ship it".into(),
        "Close it=done".into(),
        "a=b".into(),
        "  ".into(),
    ])
    .unwrap();
    assert_eq!(
        parsed
            .iter()
            .map(|c| format!("{}:{}", c.label, c.action))
            .collect::<Vec<_>>(),
        vec!["Ship it:reply", "Close it:done", "a=b:reply"]
    );
}

#[test]
fn cancel_cancels_queued_jobs_only() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let a = new(&s, "demo", "T", "", 0, "human").unwrap();
    for (id, status) in [("J-q", "queued"), ("J-l", "leased"), ("J-d", "done")] {
        s.conn().unwrap().execute("INSERT INTO jobs (id, project, kind, title, source, status, created_at) VALUES (?, 'demo', 'task', 'T', 'task:TASK-001', ?, ?)", params![id, status, s.now()]).unwrap();
    }
    set_lead(&s, &a.id, "demo-lead", "J-q", "runner").unwrap();
    let c = cancel(&s, &a.id, "changed my mind", "human").unwrap();
    assert_eq!(
        (c.status.as_str(), c.lead.as_str(), c.job_id.as_str()),
        ("cancelled", "demo-lead", "J-q")
    );
    let conn = s.conn().unwrap();
    let q: String = conn
        .query_row("SELECT status FROM jobs WHERE id = 'J-q'", [], |r| r.get(0))
        .unwrap();
    let l: String = conn
        .query_row("SELECT status FROM jobs WHERE id = 'J-l'", [], |r| r.get(0))
        .unwrap();
    let d: String = conn
        .query_row("SELECT status FROM jobs WHERE id = 'J-d'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        (q.as_str(), l.as_str(), d.as_str()),
        ("cancelled", "leased", "done")
    );
    let evs = events(&s, "task.cancelled");
    assert_eq!(evs.len(), 1);
    assert!(evs[0].detail.contains(r#""cancelled_jobs": 1"#));
    assert!(evs[0].detail.contains(r#""leased_jobs": 1"#));
    assert!(evs[0].detail.contains(r#""lead": "demo-lead""#));
    assert_eq!(jobs(&s, &a.id).unwrap().len(), 3);
}

#[test]
fn labelled_task_sources_count_as_the_tasks_jobs() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let a = new(&s, "demo", "T", "", 0, "human").unwrap();
    let other = new(&s, "demo", "U", "", 0, "human").unwrap();
    assert_eq!((a.id.as_str(), other.id.as_str()), ("TASK-001", "TASK-002"));
    for (id, source) in [
        ("J-plain", "task:TASK-001"),
        ("J-ui", "task:TASK-001:hub-ui"),
        ("J-skills", "task:TASK-001:skills"),
        ("J-other", "task:TASK-002"),
        ("J-prefix", "task:TASK-0010"),
    ] {
        s.conn().unwrap().execute("INSERT INTO jobs (id, project, kind, title, source, status, created_at) VALUES (?, 'demo', 'task', 'T', ?, 'queued', ?)", params![id, source, s.now()]).unwrap();
    }
    let mut mine = jobs(&s, &a.id)
        .unwrap()
        .into_iter()
        .map(|j| j.id)
        .collect::<Vec<_>>();
    mine.sort();
    assert_eq!(mine, vec!["J-plain", "J-skills", "J-ui"]);
    let c = cancel(&s, &a.id, "changed my mind", "human").unwrap();
    assert_eq!(c.status, "cancelled");
    let conn = s.conn().unwrap();
    let status = |id: &str| -> String {
        conn.query_row("SELECT status FROM jobs WHERE id = ?", [id], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(
        (
            status("J-plain"),
            status("J-ui"),
            status("J-skills"),
            status("J-other"),
            status("J-prefix")
        ),
        (
            "cancelled".into(),
            "cancelled".into(),
            "cancelled".into(),
            "queued".into(),
            "queued".into()
        )
    );
}

#[test]
fn work_for_summarizes_task_jobs() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let mut task = new(&s, "demo", "Owner task", "brief", 0, "human").unwrap();
    let empty = new(&s, "demo", "Empty task", "", 0, "human").unwrap();
    let jobs_rows = [
        (
            "JOB-plan",
            "demo",
            "plan",
            "Plan task",
            "lead:demo:20260906T210000Z",
            "leased",
            "demo-lead",
            "",
            "2026-09-06T21:01:00Z",
        ),
        (
            "JOB-queued",
            "demo",
            "improve",
            "Queued work",
            "task:TASK-001",
            "queued",
            "",
            "",
            "2026-09-06T21:02:00Z",
        ),
        (
            "JOB-blocked",
            "demo",
            "improve",
            "Blocked work",
            "task:TASK-001",
            "blocked",
            "",
            "",
            "2026-09-06T21:03:00Z",
        ),
        (
            "JOB-run",
            "demo",
            "improve",
            "Running work",
            "task:TASK-001",
            "leased",
            "cinder",
            "2026-09-06T22:00:00Z",
            "2026-09-06T21:04:00Z",
        ),
        (
            "JOB-expired",
            "demo",
            "improve",
            "Expired lease",
            "task:TASK-001",
            "leased",
            "ember",
            "2026-09-06T20:00:00Z",
            "2026-09-06T21:05:00Z",
        ),
        (
            "JOB-done",
            "demo",
            "improve",
            "Done work",
            "task:TASK-001",
            "done",
            "",
            "",
            "2026-09-06T21:06:00Z",
        ),
        (
            "JOB-failed",
            "demo",
            "improve",
            "Failed work",
            "task:TASK-001",
            "failed",
            "",
            "",
            "2026-09-06T21:07:00Z",
        ),
        (
            "JOB-cancelled",
            "demo",
            "improve",
            "Cancelled work",
            "task:TASK-001",
            "cancelled",
            "",
            "",
            "2026-09-06T21:08:00Z",
        ),
        (
            "JOB-other",
            "other",
            "improve",
            "Wrong project",
            "task:TASK-001",
            "failed",
            "",
            "",
            "2026-09-06T21:09:00Z",
        ),
    ];
    for row in jobs_rows {
        s.conn().unwrap().execute("INSERT INTO jobs (id, project, kind, title, source, status, lease_agent, lease_until, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)", params![row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8]).unwrap();
    }
    s.conn()
        .unwrap()
        .execute(
            "INSERT INTO events(at, actor, action, subject, detail) VALUES
        ('2026-09-06T21:00:30Z', 'demo-lead', 'job.lease', 'JOB-plan', '{}'),
        ('2026-09-06T21:10:00Z', 'cinder', 'job.lease', 'JOB-run', '{}'),
        ('2026-09-06T21:11:00Z', 'ember', 'job.lease', 'JOB-expired', '{}')",
            [],
        )
        .unwrap();
    s.conn().unwrap().execute("INSERT INTO runs (id, job_id, agent, project, started_at, status, mode, executor) VALUES ('RUN-1', 'JOB-run', 'cinder', 'demo', '2026-09-06T21:12:00Z', 'running', 'real', 'remote')", []).unwrap();
    task = set_lead(&s, &task.id, "demo-lead", "JOB-plan", "runner").unwrap();
    let work = work_for(&s, &[task.clone(), empty.clone()]).unwrap();
    let got = &work[&task.id];
    assert_eq!(
        (
            got.total,
            got.queued,
            got.blocked,
            got.running,
            got.done,
            got.failed
        ),
        (8, 1, 1, 3, 1, 1)
    );
    assert_eq!(got.working.len(), 2);
    assert_eq!(
        got.working[0],
        Worker {
            job: "JOB-plan".into(),
            kind: "plan".into(),
            title: "Plan task".into(),
            agent: "demo-lead".into(),
            since: "2026-09-06T21:00:30Z".into(),
            executor: String::new()
        }
    );
    assert_eq!(
        got.working[1],
        Worker {
            job: "JOB-run".into(),
            kind: "improve".into(),
            title: "Running work".into(),
            agent: "cinder".into(),
            since: "2026-09-06T21:12:00Z".into(),
            executor: "remote".into()
        }
    );
    let empty_work = &work[&empty.id];
    assert_eq!(empty_work.total, 0);
    assert!(empty_work.working.is_empty());
    assert!(
        serde_json::to_string(empty_work)
            .unwrap()
            .contains(r#""working":[]"#)
    );
}

#[test]
fn lead_work_for_tells_the_owner_where_the_lead_is() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let insert = |id: &str,
                  source: &str,
                  status: &str,
                  agent: &str,
                  lease_until: &str,
                  created: &str,
                  finished: &str| {
        s.conn().unwrap().execute("INSERT INTO jobs (id, project, kind, title, source, status, lease_agent, lease_until, created_at, finished_at) VALUES (?, 'demo', 'lead', ?, ?, ?, ?, ?, ?, ?)", params![id, format!("Lead: {id}"), source, status, agent, lease_until, created, finished]).unwrap();
    };
    let lease = |id: &str, agent: &str, at: &str| {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO events(at, actor, action, subject, detail) VALUES (?, ?, 'job.lease', ?, '{}')",
                params![at, agent, id],
            )
            .unwrap();
    };
    // Nothing of the lead's yet: idle, never sat down.
    assert_eq!(
        lead_work_for(&s, "demo").unwrap(),
        LeadWork {
            state: "idle".into(),
            ..LeadWork::default()
        }
    );
    // A role lead's sitting is not the owner's.
    insert(
        "JOB-role",
        "lead-role:demo:ops:20260906T200000Z",
        "leased",
        "demo-ops",
        "2026-09-06T23:00:00Z",
        "2026-09-06T20:00:00Z",
        "",
    );
    lease("JOB-role", "demo-ops", "2026-09-06T20:00:30Z");
    assert_eq!(lead_work_for(&s, "demo").unwrap().state, "idle");
    // The lead's earlier sitting finished: it read the chat when it began.
    insert(
        "JOB-earlier",
        "lead:demo:20260906T190000Z",
        "done",
        "",
        "",
        "2026-09-06T19:00:00Z",
        "2026-09-06T19:20:00Z",
    );
    lease("JOB-earlier", "demo-lead", "2026-09-06T19:05:00Z");
    let idle = lead_work_for(&s, "demo").unwrap();
    assert_eq!(
        (
            idle.state.as_str(),
            idle.read_at.as_str(),
            idle.job.as_str()
        ),
        ("idle", "2026-09-06T19:05:00Z", "")
    );
    // The owner wrote: the lead's answer waits for a slot.
    insert(
        "JOB-next",
        "lead:demo:20260906T210000Z",
        "queued",
        "",
        "",
        "2026-09-06T21:00:00Z",
        "",
    );
    let queued = lead_work_for(&s, "demo").unwrap();
    assert_eq!(
        queued,
        LeadWork {
            state: "queued".into(),
            job: "JOB-next".into(),
            title: "Lead: JOB-next".into(),
            agent: String::new(),
            since: "2026-09-06T21:00:00Z".into(),
            read_at: "2026-09-06T19:05:00Z".into(),
        }
    );
    // Leased and running: reading since the run began.
    s.conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET status = 'leased', lease_agent = 'demo-lead', lease_until = '2026-09-06T23:00:00Z' WHERE id = 'JOB-next'",
            [],
        )
        .unwrap();
    lease("JOB-next", "demo-lead", "2026-09-06T21:01:00Z");
    let leased = lead_work_for(&s, "demo").unwrap();
    assert_eq!(
        (
            leased.state.as_str(),
            leased.agent.as_str(),
            leased.since.as_str()
        ),
        ("running", "demo-lead", "2026-09-06T21:01:00Z")
    );
    s.conn().unwrap().execute("INSERT INTO runs (id, job_id, agent, project, started_at, status, mode, executor) VALUES ('RUN-lead', 'JOB-next', 'demo-lead', 'demo', '2026-09-06T21:01:30Z', 'running', 'real', 'local')", []).unwrap();
    assert_eq!(
        lead_work_for(&s, "demo").unwrap().since,
        "2026-09-06T21:01:30Z"
    );
    // An expired lease is no sitting: the job waits again.
    s.conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET lease_until = '2026-09-06T20:00:00Z' WHERE id = 'JOB-next'",
            [],
        )
        .unwrap();
    assert_eq!(lead_work_for(&s, "demo").unwrap().state, "queued");
    // Finished: idle again, and the chat was read when this sitting began.
    s.conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET status = 'done', finished_at = '2026-09-06T21:10:00Z' WHERE id = 'JOB-next'",
            [],
        )
        .unwrap();
    let after = lead_work_for(&s, "demo").unwrap();
    assert_eq!(
        (after.state.as_str(), after.read_at.as_str()),
        ("idle", "2026-09-06T21:01:00Z")
    );
    assert_eq!(lead_work_for(&s, "other").unwrap().state, "idle");
}

#[test]
fn messages_since_unread_and_immutable() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let a = new(&s, "demo", "T", "", 0, "human").unwrap();
    assert!(send_message(&s, "demo", "", "human", "   ").is_err());
    let m1 = send_message(&s, "demo", "", "human", "hello lead").unwrap();
    let m2 = reply(&s, &a.id, "demo-lead", "on it").unwrap();
    let m3 = send_message(&s, "demo", &a.id, "human", "thanks").unwrap();
    assert_eq!(
        (m1.id.as_str(), m2.id.as_str(), m3.id.as_str()),
        (
            "MSG-20260906T210000Z-1",
            "MSG-20260906T210000Z-2",
            "MSG-20260906T210000Z-3"
        )
    );
    assert!(m1.task_id.is_empty());
    assert_eq!(m2.task_id, a.id);
    assert_eq!(m2.project, "demo");
    assert!(send_message(&s, pulse_store::COMMONS_SLUG, &a.id, "human", "x").is_err());
    let all = messages(&s, "demo", "", "", "").unwrap();
    let on_task = messages(&s, "", &a.id, "", "").unwrap();
    let since = messages(&s, "demo", "", &m1.id, "").unwrap();
    let unread = messages(&s, "demo", "", "", "demo-lead").unwrap();
    assert_eq!(
        (all.len(), on_task.len(), since.len(), unread.len()),
        (3, 2, 2, 2)
    );
    assert_eq!(since[0].id, m2.id);
    assert_eq!(
        mark_read(
            &s,
            &[m1.id.clone(), m3.id.clone(), "MSG-nope".into()],
            "demo-lead"
        )
        .unwrap(),
        2
    );
    assert_eq!(
        mark_read(&s, std::slice::from_ref(&m1.id), "demo-lead").unwrap(),
        0
    );
    assert!(
        messages(&s, "demo", "", "", "demo-lead")
            .unwrap()
            .is_empty()
    );
    let got = get_message(&s, &m1.id).unwrap();
    assert_eq!(got.read_by.join(","), "demo-lead,human");
    let evs = events(&s, "message.sent");
    assert_eq!(evs.len(), 3);
    assert_eq!(
        evs[1].detail,
        r#"{"author": "demo-lead", "project": "demo", "task_id": "TASK-001"}"#
    );
    assert!(
        s.conn()
            .unwrap()
            .execute("UPDATE messages SET body = 'x' WHERE id = ?", [&m1.id])
            .is_err()
    );
    assert!(
        s.conn()
            .unwrap()
            .execute("DELETE FROM messages WHERE id = ?", [&m1.id])
            .is_err()
    );
}

#[test]
fn compute_post_summary_limit() {
    let _serial = crate::test_helpers::serial();
    const PREFIX: &str = "compute request CR-001: Standard_B2ms for 1h - ";
    for length in [
        pulse_board::MAX_SUMMARY_CHARS,
        pulse_board::MAX_SUMMARY_CHARS + 1,
    ] {
        let s = new_store();
        let reason = "x".repeat(length - PREFIX.len());
        let request =
            request_compute(&s, "demo", "demo-lead", "Standard_B2ms", 1, &reason).unwrap();
        assert!(!request.proposal_post.is_empty());
        let (summary, body): (String, String) = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT summary, body FROM posts WHERE id = ?",
                [&request.proposal_post],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut want = format!("{PREFIX}{reason}");
        if length > pulse_board::MAX_SUMMARY_CHARS {
            want.truncate(pulse_board::MAX_SUMMARY_CHARS - 3);
            want.push_str("...");
        }
        assert_eq!(summary, want);
        assert!(summary.len() <= pulse_board::MAX_SUMMARY_CHARS);
        assert!(body.contains(&format!("Reason: {reason}")));
    }
}

#[test]
fn compute_auto_approve_under_caps_else_proposal() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    assert!(request_compute(&s, "demo", "demo-lead", "", 0, "x").is_err());
    let r1 = request_compute(
        &s,
        "demo",
        "demo-lead",
        "Standard_B2ms",
        4,
        "run the  parity suite",
    )
    .unwrap();
    assert_eq!((r1.id.as_str(), r1.status.as_str()), ("CR-001", "pending"));
    assert!(!r1.proposal_post.is_empty());
    let (kind, area, thread, summary, project): (String, String, String, String, String) = s
        .conn()
        .unwrap()
        .query_row(
            "SELECT kind, area, thread, summary, project FROM posts WHERE id = ?",
            [&r1.proposal_post],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        (
            kind.as_str(),
            area.as_str(),
            thread.as_str(),
            project.as_str(),
            summary.as_str()
        ),
        (
            "proposal",
            "runner",
            "compute",
            "demo",
            "compute request CR-001: Standard_B2ms for 4h - run the parity suite"
        )
    );
    assert!(r1.proposal_post.starts_with("20260906T210000Z-demo-lead"));
    let posts: i64 = s
        .conn()
        .unwrap()
        .query_row(
            "SELECT posts FROM agents WHERE name = 'demo-lead'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(posts, 1);
    let r2 = request_compute(&s, "demo", "demo-lead", "Standard_B2ms", 1, "again").unwrap();
    assert_eq!(r2.proposal_post, format!("{}-2", r1.proposal_post));
    for (k, v) in [
        ("compute.auto_approve", "1"),
        ("compute.max_vms", "1"),
        ("compute.max_hours_per_day", "8"),
    ] {
        s.set_setting("human", k, v).unwrap();
    }
    let r3 = request_compute(&s, "demo", "demo-lead", "", 2, "under caps").unwrap();
    assert_eq!(
        (
            r3.status.as_str(),
            r3.decided_by.as_str(),
            r3.size.as_str(),
            r3.proposal_post.as_str()
        ),
        ("approved", "pulse", "Standard_B2ms", "")
    );
    let caps = get_caps(&s).unwrap();
    assert_eq!(
        (
            caps.vms_now,
            caps.hours_today,
            caps.auto_approve,
            caps.max_vms
        ),
        (1, 7, true, 1)
    );
    let r4 = request_compute(&s, "demo", "demo-lead", "", 1, "over the vm cap").unwrap();
    assert_eq!(r4.status, "pending");
    s.set_setting("human", "compute.max_vms", "5").unwrap();
    let r5 = request_compute(&s, "demo", "demo-lead", "", 3, "over the hours cap").unwrap();
    assert_eq!(r5.status, "pending");
    assert!(approve(&s, &r3.id, "", "human").is_err());
    let ok = approve(&s, &r1.id, "go ahead", "").unwrap();
    assert_eq!(
        (
            ok.status.as_str(),
            ok.decided_by.as_str(),
            ok.words.as_str()
        ),
        ("approved", "human", "go ahead")
    );
    let no = deny(&s, &r2.id, "not now", "human").unwrap();
    assert_eq!(no.status, "denied");
    let (answer, result): (String, String) = s
        .conn()
        .unwrap()
        .query_row(
            "SELECT answer, result_post FROM decisions WHERE post_id = ?",
            [&r1.proposal_post],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(answer, "yes");
    assert!(!result.is_empty());
    let (resolution, rsum): (String, String) = s
        .conn()
        .unwrap()
        .query_row(
            "SELECT resolution, summary FROM posts WHERE id = ?",
            [&result],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(resolution, "accepted");
    assert!(rsum.starts_with("accepted: compute request CR-001"));
    assert_eq!(events(&s, "compute.approved").len(), 2);
    assert_eq!(events(&s, "compute.denied").len(), 1);
    assert_eq!(events(&s, "decide").len(), 2);
    let rd = set_compute_status(&s, &r1.id, "ready", "hand-1", "runner").unwrap();
    assert_eq!(
        (rd.status.as_str(), rd.executor.as_str()),
        ("ready", "hand-1")
    );
    assert!(
        s.conn()
            .unwrap()
            .execute("DELETE FROM compute_requests WHERE id = ?", [&r1.id])
            .is_err()
    );
    assert_eq!(list_compute(&s, "demo", "pending").unwrap().len(), 2);
}

fn ctx_for(s: &pulse_store::Store, agent: &str, project: &str, json: bool) -> (Context, Buffer) {
    let out = Buffer::new();
    let err = out.clone();
    (
        Context {
            store: Some(s.clone()),
            home: s.home.clone(),
            out: Box::new(out.clone()),
            err: Box::new(err),
            json,
            quiet: false,
            color: false,
            agent: agent.into(),
            project: project.into(),
            session: String::new(),
            now: String::new(),
            args: Vec::new(),
        },
        out,
    )
}

fn run_cmd(c: &mut Context, name: &str, args: &[&str]) -> anyhow::Result<()> {
    register();
    let cmd = pulse_cli::lookup(name).unwrap();
    let mut fs = FlagSet::new(name);
    if let Some(flags) = cmd.flags {
        flags(&mut fs);
    }
    let args = args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    let positional = pulse_cli::flags::parse_interleaved(&mut fs, &args)?;
    (cmd.run)(c, &fs, &positional)
}

#[test]
fn commands_end_to_end() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let brief_dir = tempfile::tempdir().unwrap();
    let brief_file = brief_dir.path().join("brief.md");
    std::fs::write(&brief_file, "# Brief\n\nDo the thing.\n").unwrap();
    let (mut c, out) = ctx_for(&s, "", "demo", false);
    run_cmd(
        &mut c,
        "task",
        &[
            "new",
            "--title",
            "Ship it",
            "--brief-file",
            brief_file.to_str().unwrap(),
            "--priority",
            "2",
        ],
    )
    .unwrap();
    assert!(out.string().contains("TASK-001 created in demo: Ship it"));
    let (mut c, out) = ctx_for(&s, "demo-lead", "", true);
    run_cmd(&mut c, "task", &["show", "TASK-001"]).unwrap();
    let show: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(show["task"]["created_by"], "human");
    assert_eq!(show["task"]["brief"], "# Brief\n\nDo the thing.\n");
    assert_eq!(show["revisions"].as_array().unwrap().len(), 1);
    assert!(show["messages"].is_array());
    assert!(show["jobs"].is_array());
    let (mut c, out) = ctx_for(&s, "demo-lead", "", false);
    run_cmd(&mut c, "task", &["reply", "TASK-001", "starting", "now"]).unwrap();
    run_cmd(&mut c, "task", &["status", "TASK-001", "in_progress"]).unwrap();
    run_cmd(
        &mut c,
        "task",
        &[
            "amend",
            "TASK-001",
            "--note",
            "clarified",
            "--brief",
            "Do it faster.",
        ],
    )
    .unwrap();
    run_cmd(&mut c, "task", &["list"]).unwrap();
    let text = out.string();
    assert!(text.contains("MSG-"));
    assert!(text.contains("TASK-001 is in_progress"));
    assert!(text.contains("amended: rev 2"));
    assert!(text.contains("in_progress"));
    let (mut c, _) = ctx_for(&s, "", "demo", false);
    run_cmd(
        &mut c,
        "msg",
        &["send", "--task", "TASK-001", "how", "is", "it", "going?"],
    )
    .unwrap();
    let (mut c, out) = ctx_for(&s, "demo-lead", "demo", true);
    run_cmd(&mut c, "msg", &["list", "--unread"]).unwrap();
    let list: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    let arr = list["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["author"], "human");
    assert_eq!(arr[0]["body"], "how is it going?");
    let msg_id = arr[0]["id"].as_str().unwrap();
    let (mut c, out) = ctx_for(&s, "demo-lead", "", false);
    run_cmd(&mut c, "msg", &["read", msg_id]).unwrap();
    assert!(out.string().contains("1 marked read for demo-lead"));
    // The lead asks with choices; the terminal lists them, the owner's pick
    // (a plain reply) ends the review.
    let (mut c, out) = ctx_for(&s, "demo-lead", "", false);
    run_cmd(&mut c, "task", &["status", "TASK-001", "review"]).unwrap();
    run_cmd(
        &mut c,
        "task",
        &[
            "reply",
            "TASK-001",
            "The check fails twice. How do we go on?",
            "--choice",
            "Try again another way",
            "--choice",
            "Good enough, close it=done",
        ],
    )
    .unwrap();
    let text = out.string();
    assert!(text.contains("[1] Try again another way\n"), "{text}");
    assert!(
        text.contains("[2] Good enough, close it  (done)\n"),
        "{text}"
    );
    let (mut c, _) = ctx_for(&s, "demo-lead", "demo", false);
    let err = run_cmd(&mut c, "msg", &["send", "no task", "--choice", "x"]).unwrap_err();
    assert!(err.to_string().contains("belong to a task"), "{err}");
    let (mut c, out) = ctx_for(&s, "", "", true);
    run_cmd(&mut c, "task", &["show", "TASK-001"]).unwrap();
    let show: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(show["task"]["status"], "review");
    let msgs = show["messages"].as_array().unwrap();
    let asked = msgs.last().unwrap();
    assert_eq!(asked["choices"][1]["action"], "done");
    let (mut c, _) = ctx_for(&s, "", "", false);
    run_cmd(
        &mut c,
        "task",
        &["reply", "TASK-001", "Try again another way"],
    )
    .unwrap();
    let (mut c, out) = ctx_for(&s, "", "", true);
    run_cmd(&mut c, "task", &["show", "TASK-001"]).unwrap();
    let show: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(show["task"]["status"], "in_progress");
    let (mut c, out) = ctx_for(&s, "demo-lead", "demo", false);
    run_cmd(
        &mut c,
        "compute",
        &[
            "request",
            "--size",
            "Standard_D4s",
            "--hours",
            "3",
            "--reason",
            "parity runs",
        ],
    )
    .unwrap();
    assert!(out.string().contains("CR-001 pending"));
    let (mut c, out) = ctx_for(&s, "", "", false);
    run_cmd(
        &mut c,
        "compute",
        &["approve", "CR-001", "--words", "yes please"],
    )
    .unwrap();
    assert!(out.string().contains("CR-001 approved by human"));
    let (mut c, out) = ctx_for(&s, "", "", true);
    run_cmd(&mut c, "compute", &["list"]).unwrap();
    let cl: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(cl["requests"].as_array().unwrap().len(), 1);
    assert_eq!(cl["caps"]["vms_now"], 1);
    let (mut c, _) = ctx_for(&s, "", "no-such-project", false);
    let err = run_cmd(&mut c, "compute", &["list"]).unwrap_err();
    assert!(
        err.to_string().contains("No project"),
        "unexpected error: {err}"
    );
    let (mut c, _) = ctx_for(&s, "", "", false);
    run_cmd(
        &mut c,
        "task",
        &["cancel", "TASK-001", "--reason", "done differently"],
    )
    .unwrap();
    assert!(run_cmd(&mut c, "task", &["bogus"]).is_err());
    assert!(run_cmd(&mut c, "task", &["show", "TASK-404"]).is_err());
    let (mut c, out) = ctx_for(&s, "", "", false);
    run_cmd(&mut c, "task", &["show", "TASK-001"]).unwrap();
    for want in [
        "TASK-001  cancelled  Ship it",
        "REVISIONS",
        "MESSAGES",
        "starting now",
        "JOBS",
        "Do it faster.",
    ] {
        assert!(
            out.string().contains(want),
            "missing {want}:\n{}",
            out.string()
        );
    }
}

#[test]
fn task_list_rejects_unknown_project() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let (mut c, _) = ctx_for(&s, "", "demmo", false);
    let err = run_cmd(&mut c, "task", &["list"]).unwrap_err();
    assert!(
        err.to_string().contains("No project \"demmo\""),
        "unexpected error: {err}"
    );
    let (mut c, out) = ctx_for(&s, "", "demo", false);
    run_cmd(&mut c, "task", &["list"]).unwrap();
    assert!(out.string().contains("No tasks."));
}

#[test]
fn watch_delivers_events_from_another_connection() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let other = pulse_store::Store::open(pulse_store::Options {
        home: s.home.clone(),
        now: "2026-09-06T21:05:00Z".into(),
        ..pulse_store::Options::default()
    })
    .unwrap();
    let (mut c, out) = ctx_for(&s, "", "demo", false);
    let cancel = pulse_bus::CancellationToken::new();
    let done_cancel = cancel.clone();
    let handle = std::thread::spawn(move || {
        watch(
            done_cancel,
            &mut c,
            -1,
            pulse_bus::Filter {
                project: "demo".into(),
                ..pulse_bus::Filter::default()
            },
            Duration::from_millis(10),
            false,
        )
    });
    std::thread::sleep(Duration::from_millis(60));
    new(&other, "demo", "From elsewhere", "", 0, "human").unwrap();
    new(
        &other,
        pulse_store::COMMONS_SLUG,
        "Not demo",
        "",
        0,
        "human",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !out.string().contains("TASK-001") {
        std::thread::sleep(Duration::from_millis(20));
    }
    cancel.cancel();
    handle.join().unwrap().unwrap();
    let text = out.string();
    assert!(text.contains("2026-09-06T21:05:00Z human task.created TASK-001"));
    assert!(!text.contains("TASK-002"));

    let (mut c, out) = ctx_for(&s, "", "", true);
    let cancel = pulse_bus::CancellationToken::new();
    let done_cancel = cancel.clone();
    let handle = std::thread::spawn(move || {
        watch(
            done_cancel,
            &mut c,
            0,
            pulse_bus::Filter {
                actions: vec!["task.created".into()],
                ..pulse_bus::Filter::default()
            },
            Duration::from_millis(10),
            false,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && out.string().lines().count() < 2 {
        std::thread::sleep(Duration::from_millis(20));
    }
    cancel.cancel();
    handle.join().unwrap().unwrap();
    let lines = out
        .string()
        .trim()
        .lines()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    let e: pulse_bus::Event = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(e.subject, "TASK-002");
    assert_eq!(e.action, "task.created");
}

#[test]
fn compute_auto_approve_held_without_login_source() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    for (k, v) in [
        ("compute.auto_approve", "1"),
        ("compute.max_vms", "2"),
        ("compute.max_hours_per_day", "8"),
    ] {
        s.set_setting("human", k, v).unwrap();
    }
    *LOGIN_SOURCE_CHECK.write().unwrap() = Some(|_| false);
    let r = request_compute(&s, "demo", "demo-lead", "", 1, "no login anywhere").unwrap();
    assert_eq!(r.status, "pending");
    assert!(!r.proposal_post.is_empty());
    let body: String = s
        .conn()
        .unwrap()
        .query_row(
            "SELECT body FROM posts WHERE id = ?",
            [&r.proposal_post],
            |r| r.get(0),
        )
        .unwrap();
    assert!(body.contains(NO_LOGIN_SOURCE));
    assert!(body.contains("pulse executor token --with-token"));
    assert!(!get_caps(&s).unwrap().login_source);
    *LOGIN_SOURCE_CHECK.write().unwrap() = Some(|_| true);
    let r2 = request_compute(&s, "demo", "demo-lead", "", 1, "with a source").unwrap();
    assert_eq!(r2.status, "approved");
    *LOGIN_SOURCE_CHECK.write().unwrap() = None;
}

fn sched_input(every: &str) -> schedules::ScheduleInput {
    schedules::ScheduleInput {
        project: "demo".into(),
        title: "Audit the dependencies".into(),
        kind: "task".into(),
        brief: "Run the audit; fix or file what it names.".into(),
        tier: 1,
        every: every.into(),
        author: "demo-lead".into(),
        ..Default::default()
    }
}

#[test]
fn schedule_cadence_parsing_and_arithmetic() {
    for (text, minutes) in [
        ("30m", 30),
        ("6h", 360),
        ("1d", 1440),
        ("2w", 20160),
        ("hourly", 60),
        ("daily", 1440),
        ("weekly", 10080),
        ("90", 90),
        (" 2 hours ", 120),
    ] {
        assert_eq!(schedules::parse_every(text).unwrap(), minutes, "{text}");
    }
    for text in ["", "0m", "soon", "3x", "-5m"] {
        assert!(schedules::parse_every(text).is_err(), "{text:?} parsed");
    }
    assert_eq!(schedules::format_every(20160), "2w");
    assert_eq!(schedules::format_every(2880), "2d");
    assert_eq!(schedules::format_every(360), "6h");
    assert_eq!(schedules::format_every(90), "90m");

    let now = "2026-09-06T21:00:00Z";
    assert_eq!(
        schedules::first_due(now, 360, None).unwrap(),
        "2026-09-07T03:00:00Z"
    );
    let nine = chrono::NaiveTime::from_hms_opt(9, 0, 0);
    assert_eq!(
        schedules::first_due(now, 1440, nine).unwrap(),
        "2026-09-07T09:00:00Z",
        "the next 09:00 is tomorrow"
    );
    let late = chrono::NaiveTime::from_hms_opt(23, 30, 0);
    assert_eq!(
        schedules::first_due(now, 1440, late).unwrap(),
        "2026-09-06T23:30:00Z",
        "23:30 is still ahead today"
    );
    assert_eq!(
        schedules::advance("2026-09-07T03:00:00Z", 360, "2026-09-07T03:04:00Z").unwrap(),
        "2026-09-07T09:00:00Z"
    );
    assert_eq!(
        schedules::advance("2026-09-07T03:00:00Z", 360, "2026-09-08T10:00:00Z").unwrap(),
        "2026-09-08T15:00:00Z",
        "a day of missed ticks collapses to the next one after now"
    );
}

#[test]
fn schedule_add_validates_and_lists() {
    let s = new_store();
    let sc = schedules::add(&s, sched_input("6h")).unwrap();
    assert_eq!(sc.id, "SCHED-001");
    assert_eq!(sc.status, "active");
    assert_eq!(sc.every(), "6h");
    assert_eq!(sc.next_at, "2026-09-07T03:00:00Z");
    assert_eq!(sc.source(), "schedule:SCHED-001");
    assert_eq!(events(&s, "schedule.added").len(), 1);

    let daily = schedules::add(
        &s,
        schedules::ScheduleInput {
            at: "9:00".into(),
            times: 3,
            ..sched_input("daily")
        },
    )
    .unwrap();
    assert_eq!(daily.id, "SCHED-002");
    assert_eq!(
        daily.at_time, "09:00",
        "the time of day is stored normalised"
    );
    assert_eq!(daily.next_at, "2026-09-07T09:00:00Z");
    assert_eq!(daily.max_runs, 3);

    let refused = |input: schedules::ScheduleInput, needle: &str| {
        let err = schedules::add(&s, input).expect_err(needle).to_string();
        assert!(err.contains(needle), "wanted {needle:?} in {err}");
    };
    refused(sched_input("10m"), "shorter than the 30m floor");
    refused(sched_input("soon"), "Not a cadence");
    refused(
        schedules::ScheduleInput {
            at: "09:00".into(),
            ..sched_input("6h")
        },
        "whole days",
    );
    refused(
        schedules::ScheduleInput {
            at: "noon".into(),
            ..sched_input("1d")
        },
        "Not a time of day",
    );
    refused(
        schedules::ScheduleInput {
            kind: "experiment".into(),
            ..sched_input("1d")
        },
        "cannot queue",
    );
    refused(
        schedules::ScheduleInput {
            brief: "  ".into(),
            ..sched_input("1d")
        },
        "needs a brief",
    );
    refused(
        schedules::ScheduleInput {
            title: String::new(),
            ..sched_input("1d")
        },
        "needs a title",
    );
    refused(
        schedules::ScheduleInput {
            tier: 2,
            ..sched_input("1d")
        },
        "Tier 0 and Tier 1",
    );
    refused(
        schedules::ScheduleInput {
            author: String::new(),
            ..sched_input("1d")
        },
        "author is needed",
    );
    refused(
        schedules::ScheduleInput {
            project: "nope".into(),
            ..sched_input("1d")
        },
        "nope",
    );

    s.set_setting("human", "schedule.max_per_project", "2")
        .unwrap();
    refused(sched_input("1d"), "already holds 2 schedules");

    let all = schedules::list(&s, "", "").unwrap();
    assert_eq!(
        all.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
        ["SCHED-001", "SCHED-002"],
        "active first, then by next time"
    );
    assert_eq!(schedules::list(&s, "demo", "paused").unwrap().len(), 0);
    assert_eq!(schedules::list(&s, "other", "").unwrap().len(), 0);
    assert!(schedules::get(&s, "SCHED-009").is_err());
}

#[test]
fn schedule_fires_once_per_open_job_and_ends_after_its_runs() {
    let s = new_store();
    let sc = schedules::add(
        &s,
        schedules::ScheduleInput {
            times: 2,
            ..sched_input("1h")
        },
    )
    .unwrap();
    let demo = vec!["demo".to_string()];
    assert!(
        schedules::fire_due(&s, "2026-09-06T21:30:00Z", &demo, "daemon")
            .unwrap()
            .is_empty(),
        "fired early"
    );
    assert!(
        schedules::fire_due(&s, "2026-09-06T22:00:00Z", &["other".to_string()], "daemon")
            .unwrap()
            .is_empty(),
        "fired for a project the daemon does not tend"
    );

    let fired = schedules::fire_due(&s, "2026-09-06T22:00:00Z", &demo, "daemon").unwrap();
    assert_eq!(fired.len(), 1);
    assert!(fired[0].queued && !fired[0].ended);
    assert_eq!(fired[0].schedule.runs, 1);
    assert_eq!(fired[0].schedule.next_at, "2026-09-06T23:00:00Z");
    assert_eq!(fired[0].schedule.last_at, "2026-09-06T22:00:00Z");
    assert_eq!(fired[0].schedule.last_job, fired[0].job);
    let job = pulse_board::job_by_id(&s, &fired[0].job).unwrap().unwrap();
    assert_eq!(job.source, "schedule:SCHED-001");
    assert_eq!(job.created_by, "demo-lead");
    assert_eq!(job.kind, "task");
    assert_eq!(job.title, "Audit the dependencies");
    assert!(
        job.brief
            .contains("run 1 of schedule SCHED-001 (every 1h), set by demo-lead")
            && job.brief.contains("It ends after 2 runs."),
        "brief = {}",
        job.brief
    );
    assert_eq!(schedules::jobs(&s, &sc.id).unwrap().len(), 1);

    // Still open at the next time: skipped, moved on, runs unchanged.
    let fired = schedules::fire_due(&s, "2026-09-06T23:00:00Z", &demo, "daemon").unwrap();
    assert_eq!(fired.len(), 1);
    assert!(!fired[0].queued);
    assert_eq!(fired[0].job, job.id);
    assert_eq!(fired[0].schedule.runs, 1);
    assert_eq!(fired[0].schedule.next_at, "2026-09-07T00:00:00Z");
    assert_eq!(fired[0].schedule.last_at, "2026-09-06T22:00:00Z");
    assert_eq!(events(&s, "schedule.skipped").len(), 1);
    assert_eq!(schedules::jobs(&s, &sc.id).unwrap().len(), 1);

    s.conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?",
            params!["2026-09-06T23:30:00Z", job.id],
        )
        .unwrap();
    let fired = schedules::fire_due(&s, "2026-09-07T00:00:00Z", &demo, "daemon").unwrap();
    assert_eq!(fired.len(), 1);
    assert!(fired[0].queued && fired[0].ended);
    assert_eq!(fired[0].schedule.status, "ended");
    assert_eq!(fired[0].schedule.runs, 2);
    assert_eq!(fired[0].schedule.note, "ended after 2 runs");
    assert_eq!(events(&s, "schedule.fired").len(), 2);
    assert_eq!(events(&s, "schedule.ended").len(), 1);
    assert_eq!(schedules::jobs(&s, &sc.id).unwrap().len(), 2);
    assert!(
        schedules::fire_due(&s, "2026-09-07T05:00:00Z", &demo, "daemon")
            .unwrap()
            .is_empty(),
        "an ended schedule fired"
    );
    let err = schedules::resume(&s, &sc.id, "demo-lead")
        .expect_err("resumed an ended schedule")
        .to_string();
    assert!(err.contains("has ended"), "{err}");
}

#[test]
fn schedule_owner_controls_pause_resume_end_and_run() {
    let s = new_store();
    let sc = schedules::add(&s, sched_input("1d")).unwrap();
    let err = schedules::pause(&s, &sc.id, "", "someone-else")
        .expect_err("another agent paused it")
        .to_string();
    assert!(err.contains("belongs to demo-lead"), "{err}");
    let err = schedules::end(&s, &sc.id, "", "")
        .expect_err("anonymous end")
        .to_string();
    assert!(err.contains("author is needed"), "{err}");

    let paused = schedules::pause(&s, &sc.id, "holiday", pulse_store::HUMAN_AGENT).unwrap();
    assert_eq!(paused.status, "paused");
    assert_eq!(paused.note, "holiday");
    let demo = vec!["demo".to_string()];
    assert!(
        schedules::fire_due(&s, "2026-09-09T00:00:00Z", &demo, "daemon")
            .unwrap()
            .is_empty(),
        "a paused schedule fired"
    );
    let err = schedules::run_now(&s, &sc.id, "demo-lead")
        .expect_err("ran a paused schedule")
        .to_string();
    assert!(err.contains("is paused"), "{err}");

    let resumed = schedules::resume(&s, &sc.id, "demo-lead").unwrap();
    assert_eq!(resumed.status, "active");
    assert_eq!(resumed.note, "");
    let fired = schedules::fire_due(&s, "2026-09-09T00:00:00Z", &demo, "daemon").unwrap();
    assert_eq!(fired.len(), 1, "the missed time fires once after resume");
    assert_eq!(fired[0].schedule.next_at, "2026-09-09T21:00:00Z");

    let ran = schedules::run_now(&s, &sc.id, "demo-lead").unwrap();
    assert_eq!(ran.next_at, s.now(), "run makes it due now");
    assert_eq!(events(&s, "schedule.run").len(), 1);

    let ended = schedules::end(&s, &sc.id, "done with it", "demo-lead").unwrap();
    assert_eq!(ended.status, "ended");
    assert_eq!(ended.note, "done with it");
    assert_eq!(events(&s, "schedule.ended").len(), 1);
    assert!(
        s.conn()
            .unwrap()
            .execute("DELETE FROM schedules WHERE id = ?", params![sc.id])
            .is_err(),
        "schedules are never deleted"
    );
}

#[test]
fn schedule_repeated_pause_or_resume_is_a_no_op() {
    let s = new_store();
    let sched = schedules::add(&s, sched_input("1d")).unwrap();
    schedules::pause(&s, &sched.id, "first reason", "demo-lead").unwrap();
    let again = schedules::pause(&s, &sched.id, "second reason", "demo-lead").unwrap();
    assert_eq!(again.status, "paused");
    assert_eq!(
        again.note, "first reason",
        "a repeated pause keeps the original note"
    );
    assert_eq!(events(&s, "schedule.paused").len(), 1);
    schedules::resume(&s, &sched.id, "demo-lead").unwrap();
    schedules::resume(&s, &sched.id, "demo-lead").unwrap();
    assert_eq!(events(&s, "schedule.resumed").len(), 1);
}

#[test]
fn schedule_commands_end_to_end() {
    let s = new_store();
    let (mut c, out) = ctx_for(&s, "demo-lead", "demo", false);
    run_cmd(
        &mut c,
        "schedule",
        &[
            "add",
            "--title",
            "Audit the dependencies",
            "--every",
            "daily",
            "--at",
            "09:00",
            "--brief",
            "Run the audit.",
            "--times",
            "5",
        ],
    )
    .unwrap();
    assert!(
        out.string().contains(
            "SCHED-001 set in demo: Audit the dependencies every 1d, first at 2026-09-07T09:00:00Z"
        ),
        "{}",
        out.string()
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["list"]).unwrap();
    let text = out.string();
    assert!(
        text.contains("SCHED-001") && text.contains("0/5") && text.contains("demo-lead"),
        "{text}"
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["show", "SCHED-001"]).unwrap();
    let text = out.string();
    assert!(
        text.contains("every 1d at 09:00 UTC, 0 of 5 runs")
            && text.contains("Run the audit.")
            && text.contains("JOBS"),
        "{text}"
    );
    out.clear();
    run_cmd(
        &mut c,
        "schedule",
        &["pause", "SCHED-001", "--note", "later"],
    )
    .unwrap();
    assert!(
        out.string().contains("SCHED-001 paused"),
        "{}",
        out.string()
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["resume", "SCHED-001"]).unwrap();
    assert!(
        out.string().contains("SCHED-001 active, next at"),
        "{}",
        out.string()
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["run", "SCHED-001"]).unwrap();
    assert!(
        out.string().contains("SCHED-001 due now"),
        "{}",
        out.string()
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["end", "SCHED-001"]).unwrap();
    assert!(out.string().contains("SCHED-001 ended"), "{}", out.string());
    assert!(
        run_cmd(&mut c, "schedule", &["show"]).is_err(),
        "show without an id"
    );
    assert!(run_cmd(&mut c, "schedule", &["bogus"]).is_err());
    assert!(
        run_cmd(
            &mut c,
            "schedule",
            &["add", "--title", "x", "--every", "5m", "--brief", "b"]
        )
        .is_err(),
        "a 5m cadence passed the floor"
    );

    let (mut c, out) = ctx_for(&s, "demo-lead", "demo", true);
    run_cmd(&mut c, "schedule", &["list", "--status", "ended"]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(v["schedules"][0]["id"], "SCHED-001");
    assert_eq!(v["schedules"][0]["status"], "ended");
    out.clear();
    run_cmd(&mut c, "schedule", &["show", "SCHED-001"]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(v["schedule"]["every_minutes"], 1440);
    assert_eq!(v["jobs"].as_array().unwrap().len(), 0);
}

#[test]
fn schedule_list_rejects_unknown_project() {
    let s = new_store();
    let (mut c, out) = ctx_for(&s, "demo-lead", "nope", false);
    let err = run_cmd(&mut c, "schedule", &["list"]).unwrap_err();
    let text = format!("{err}");
    assert!(text.contains("No project \"nope\""), "{text}");
    assert!(!out.string().contains("No schedules"), "{}", out.string());
    out.clear();
    // The default (all projects) listing still works.
    let (mut c, out) = ctx_for(&s, "demo-lead", "", false);
    run_cmd(&mut c, "schedule", &["list"]).unwrap();
    assert!(out.string().contains("No schedules"), "{}", out.string());
}

/// A git repo for project `demo` whose `main` holds `scripts/check.sh` and
/// `scripts/watch.sh`, with the project row pointed at it.
fn script_repo(s: &pulse_store::Store) -> String {
    let repo = Path::new(&s.home).join("demo-repo");
    std::fs::create_dir_all(repo.join("scripts")).unwrap();
    std::fs::write(repo.join("scripts/check.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::write(repo.join("scripts/watch.sh"), "#!/bin/sh\necho hi\n").unwrap();
    let dir = repo.display().to_string();
    let git = |args: &[&str]| {
        let mut full = vec!["-C", dir.as_str()];
        full.extend_from_slice(args);
        pulse_testutil::git(&full).unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "scripts"]);
    // One file only in the working tree, never committed: not on main.
    std::fs::write(repo.join("scripts/draft.sh"), "#!/bin/sh\n").unwrap();
    s.conn()
        .unwrap()
        .execute(
            "UPDATE projects SET path = ?, default_branch = 'main' WHERE slug = 'demo'",
            [&dir],
        )
        .unwrap();
    dir
}

fn detail(e: &pulse_bus::Event) -> serde_json::Value {
    serde_json::from_str(&e.detail).unwrap()
}

fn script_input(kind: &str, script: &str) -> schedules::ScheduleInput {
    schedules::ScheduleInput {
        project: "demo".into(),
        title: format!("{kind} {script}"),
        kind: kind.into(),
        every: "1d".into(),
        author: "demo-lead".into(),
        script: script.into(),
        ..Default::default()
    }
}

#[test]
fn schedule_script_path_validation() {
    for (path, want) in [
        ("scripts/check.sh", "scripts/check.sh"),
        ("./scripts/check.sh", "scripts/check.sh"),
        ("check.sh", "check.sh"),
        (" scripts/a b.sh ", "scripts/a b.sh"),
    ] {
        assert_eq!(
            schedules::validate_script_path(path).unwrap(),
            want,
            "{path:?}"
        );
    }
    for (path, why) in [
        ("", "needs a script"),
        ("   ", "needs a script"),
        ("/usr/bin/true", "not absolute"),
        ("../other/check.sh", "`..`"),
        ("scripts/../../x.sh", "`..`"),
        ("scripts\\check.sh", "forward slashes"),
        ("./", "names no file"),
    ] {
        let err = schedules::validate_script_path(path).unwrap_err();
        let text = format!("{err}");
        assert!(text.contains(why), "{path:?}: {text}");
    }
}

#[test]
fn schedule_script_kinds_need_a_script_on_the_default_branch() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let repo = script_repo(&s);
    // A job kind takes no script and no job kind.
    let mut input = sched_input("1d");
    input.script = "scripts/check.sh".into();
    let err = format!("{}", schedules::add(&s, input).unwrap_err());
    assert!(err.contains("--script is for a check or a watch"), "{err}");
    let mut input = sched_input("1d");
    input.job_kind = "improve".into();
    let err = format!("{}", schedules::add(&s, input).unwrap_err());
    assert!(
        err.contains("--job-kind is for a check or a watch"),
        "{err}"
    );
    // A script kind needs one.
    let err = format!(
        "{}",
        schedules::add(&s, script_input("check", "")).unwrap_err()
    );
    assert!(err.contains("needs a script"), "{err}");
    let err = format!(
        "{}",
        schedules::add(&s, script_input("watch", "/abs/watch.sh")).unwrap_err()
    );
    assert!(err.contains("not absolute"), "{err}");
    // On the default branch, not merely in the working tree.
    let err = format!(
        "{}",
        schedules::add(&s, script_input("check", "scripts/draft.sh")).unwrap_err()
    );
    assert!(
        err.contains("No file \"scripts/draft.sh\" on main of demo"),
        "{err}"
    );
    let err = format!(
        "{}",
        schedules::add(&s, script_input("check", "scripts")).unwrap_err()
    );
    assert!(err.contains("No file \"scripts\" on main"), "{err}");
    // The job kind of a script is task or improve.
    let mut input = script_input("check", "scripts/check.sh");
    input.job_kind = "clean".into();
    let err = format!("{}", schedules::add(&s, input).unwrap_err());
    assert!(err.contains("A check cannot queue \"clean\" jobs"), "{err}");
    // Brief optional; job_kind defaults to task.
    let (sc, warning) =
        schedules::add_checked(&s, script_input("check", "./scripts/check.sh")).unwrap();
    assert_eq!(warning, "");
    assert_eq!(
        (
            sc.kind.as_str(),
            sc.script.as_str(),
            sc.job_kind.as_str(),
            sc.state.as_str(),
            sc.last_rc,
            sc.fails
        ),
        ("check", "scripts/check.sh", "task", "", None, 0)
    );
    assert!(sc.is_script());
    let mut input = script_input("watch", "scripts/watch.sh");
    input.job_kind = "improve".into();
    input.brief = "Triage the issues it lists.".into();
    let sc = schedules::add(&s, input).unwrap();
    assert_eq!(
        (sc.kind.as_str(), sc.job_kind.as_str()),
        ("watch", "improve")
    );
    let added = events(&s, "schedule.added");
    assert_eq!(added.len(), 2);
    assert_eq!(detail(&added[1])["script"], "scripts/watch.sh");
    assert_eq!(detail(&added[1])["job_kind"], "improve");
    // The job kinds record their own kind as the job kind.
    let sc = schedules::add(&s, sched_input("1d")).unwrap();
    assert_eq!((sc.kind.as_str(), sc.job_kind.as_str()), ("task", "task"));
    // A repo out of reach: accepted with a warning.
    s.conn()
        .unwrap()
        .execute(
            "UPDATE projects SET path = ? WHERE slug = 'demo'",
            [format!("{repo}-gone")],
        )
        .unwrap();
    let (sc, warning) =
        schedules::add_checked(&s, script_input("check", "scripts/nowhere.sh")).unwrap();
    assert_eq!(sc.script, "scripts/nowhere.sh");
    assert!(
        warning.contains("could not look for scripts/nowhere.sh on main"),
        "{warning}"
    );
    // The commons has no repo to run a script in.
    let mut input = script_input("check", "scripts/check.sh");
    input.project = pulse_store::COMMONS_SLUG.into();
    let err = format!("{}", schedules::add(&s, input).unwrap_err());
    assert!(err.contains("needs a repo"), "{err}");
}

#[test]
fn schedule_scripts_are_not_fired_as_jobs() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    script_repo(&s);
    let check = schedules::add(&s, script_input("check", "scripts/check.sh")).unwrap();
    let job = schedules::add(&s, sched_input("1d")).unwrap();
    let later = "2026-09-08T10:00:00Z";
    let fired = schedules::fire_due(&s, later, &[], "daemon").unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].schedule.id, job.id);
    let due = schedules::due_scripts(&s, later, &[]).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, check.id);
    assert!(
        schedules::due_scripts(&s, later, &["other".to_string()])
            .unwrap()
            .is_empty()
    );
    assert!(
        schedules::due_scripts(&s, "2026-09-07T10:00:00Z", &[])
            .unwrap()
            .is_empty(),
        "not due yet"
    );
    // The longest overdue first.
    let watch = schedules::add(&s, script_input("watch", "scripts/watch.sh")).unwrap();
    s.conn()
        .unwrap()
        .execute(
            "UPDATE schedules SET next_at = '2026-09-01T00:00:00Z' WHERE id = ?",
            [&watch.id],
        )
        .unwrap();
    let due = schedules::due_scripts(&s, later, &[]).unwrap();
    assert_eq!(
        due.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        [watch.id.as_str(), check.id.as_str()]
    );
}

#[test]
fn schedule_verdicts_and_briefs() {
    let sched = schedules::Schedule {
        id: "SCHED-007".into(),
        project: "demo".into(),
        title: "Store tests".into(),
        kind: "check".into(),
        brief: "Keep the store green.".into(),
        script: "scripts/check.sh".into(),
        every_minutes: 1440,
        created_by: "demo-lead".into(),
        ..Default::default()
    };
    let v = schedules::verdict(&sched, 0, "");
    assert_eq!(
        (v.state.as_str(), v.fails, v.transition, v.job_why.as_str()),
        ("green", 0, false, ""),
        "a first green run is not a transition"
    );
    let v = schedules::verdict(&sched, 1, "");
    assert_eq!(
        (v.state.as_str(), v.fails, v.transition, v.job_why.as_str()),
        ("red", 1, true, "red")
    );
    let red = schedules::Schedule {
        state: "red".into(),
        fails: 1,
        ..sched.clone()
    };
    let v = schedules::verdict(&red, 2, "");
    assert_eq!(
        (v.state.as_str(), v.fails, v.transition, v.job_why.as_str()),
        ("red", 2, false, "red"),
        "every red run wants the job; the dedupe keeps one"
    );
    let v = schedules::verdict(&red, 0, "");
    assert_eq!(
        (v.state.as_str(), v.fails, v.transition, v.job_why.as_str()),
        ("green", 0, true, "")
    );
    let watch = schedules::Schedule {
        kind: "watch".into(),
        script: "scripts/watch.sh".into(),
        brief: String::new(),
        ..sched.clone()
    };
    let v = schedules::verdict(&watch, 0, "  \n");
    assert_eq!((v.state.as_str(), v.job_why.as_str()), ("ok", ""));
    let v = schedules::verdict(&watch, 0, "issue #12 opened\n");
    assert_eq!((v.state.as_str(), v.job_why.as_str()), ("ok", "trigger"));
    let v = schedules::verdict(&watch, 2, "");
    assert_eq!(
        (v.state.as_str(), v.fails, v.transition, v.job_why.as_str()),
        ("broken", 1, true, "")
    );
    let failing = schedules::Schedule {
        state: "broken".into(),
        fails: 2,
        ..watch.clone()
    };
    let v = schedules::verdict(&failing, 2, "");
    assert_eq!(
        (v.state.as_str(), v.fails, v.transition, v.job_why.as_str()),
        ("broken", 3, false, "fix")
    );
    let v = schedules::verdict(&failing, 0, "");
    assert_eq!(
        (v.state.as_str(), v.fails, v.transition, v.job_why.as_str()),
        ("ok", 0, true, "")
    );

    let tree = schedules::Tree {
        branch: "main".into(),
        sha: "0123456789abcdef0123".into(),
    };
    let brief = schedules::check_job_brief(
        &sched,
        3,
        1,
        "2026-09-07T09:00:00Z",
        &tree,
        "test store::x ... FAILED\n",
    );
    assert!(brief.starts_with("Keep the store green.\n\nThe check `scripts/check.sh` failed (exit 1) at 2026-09-07T09:00:00Z on main@0123456789ab. Output tail:\n\n    test store::x ... FAILED\n\nMake it pass on the default branch. If the check itself is wrong, fix the script, or end the schedule with `pulse schedule end SCHED-007 --note WHY` and say why."), "{brief}");
    assert!(
        brief.contains("run 3 of schedule SCHED-007 (a check of `scripts/check.sh`, every 1d), set by demo-lead")
            && brief.contains("pulse schedule log SCHED-007"),
        "{brief}"
    );
    let brief = schedules::check_job_brief(&sched, 1, -1, "t", &tree, "");
    assert!(
        brief.contains("failed (timed out and was killed) at t")
            && brief.contains("    (no output)"),
        "{brief}"
    );
    let brief = schedules::watch_job_brief(
        &watch,
        1,
        "2026-09-07T09:00:00Z",
        "#12 Crash on start\n#13 Typo\n",
    );
    assert!(
        brief.starts_with("The watcher `scripts/watch.sh` reported at 2026-09-07T09:00:00Z:\n\n    #12 Crash on start\n    #13 Typo\n\nHandle what it reports; when done, the next run of the watcher will see your changes."),
        "{brief}"
    );
    let brief = schedules::watch_fix_brief(&watch, 4, 2, 3, "t", &tree, "gh: not found");
    assert!(
        brief.contains("The watcher `scripts/watch.sh` itself is broken: it failed (exit 2) 3 runs in a row, last at t on main@0123456789ab.")
            && brief.contains("    gh: not found")
            && brief.contains("pulse schedule end SCHED-007 --note WHY"),
        "{brief}"
    );
    assert_eq!(
        schedules::script_job_title(&sched, "red"),
        "Make check `scripts/check.sh` pass: Store tests"
    );
    assert_eq!(
        schedules::script_job_title(&watch, "fix"),
        "Fix watcher `scripts/watch.sh`: Store tests"
    );
    assert_eq!(
        schedules::script_job_title(&watch, "trigger"),
        "Store tests"
    );
}

#[test]
fn schedule_script_runs_are_recorded() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    script_repo(&s);
    let mut input = script_input("check", "scripts/check.sh");
    input.times = 2;
    let sc = schedules::add(&s, input).unwrap();
    let t1 = "2026-09-08T10:00:00Z";
    let run = schedules::ScriptRun {
        rc: Some(1),
        seconds: 3.21,
        output: "boom\n".into(),
        verdict: schedules::verdict(&sc, 1, ""),
        job: "job-1".into(),
        queued: true,
        post: "POST-1".into(),
        ..Default::default()
    };
    let rec = schedules::record_script_run(&s, &sc, t1, "daemon", &run).unwrap();
    assert_eq!(rec.previous, "");
    assert!(!rec.ended);
    let got = &rec.schedule;
    assert_eq!(
        (
            got.state.as_str(),
            got.state_since.as_str(),
            got.last_rc,
            got.last_output.as_str(),
            got.fails,
            got.runs,
            got.last_at.as_str(),
            got.last_job.as_str()
        ),
        ("red", t1, Some(1), "boom\n", 1, 1, t1, "job-1")
    );
    assert_eq!(
        got.next_at, "2026-09-08T21:00:00Z",
        "one cadence on from its due time"
    );
    let ran = events(&s, "schedule.ran");
    assert_eq!(ran.len(), 1);
    let d = detail(&ran[0]);
    assert_eq!(d["rc"], 1);
    assert_eq!(d["seconds"], 3.2);
    assert_eq!(d["state"], "red");
    assert_eq!(d["job"], "job-1");
    let red = events(&s, "schedule.red");
    assert_eq!(red.len(), 1);
    assert_eq!(detail(&red[0])["post"], "POST-1");
    assert_eq!(events(&s, "schedule.fired").len(), 1);
    assert_eq!(events(&s, "schedule.green").len(), 0);
    // A second red run: no transition; the dedupe found the job open.
    let t2 = "2026-09-09T10:00:00Z";
    let run = schedules::ScriptRun {
        rc: Some(1),
        seconds: 1.0,
        output: "boom again\n".into(),
        verdict: schedules::verdict(got, 1, ""),
        job: "job-1".into(),
        queued: false,
        ..Default::default()
    };
    let rec = schedules::record_script_run(&s, got, t2, "daemon", &run).unwrap();
    assert_eq!(rec.previous, "red");
    assert!(rec.ended, "--times 2 counts script runs");
    let got = &rec.schedule;
    assert_eq!(
        (
            got.state.as_str(),
            got.state_since.as_str(),
            got.fails,
            got.runs,
            got.status.as_str(),
            got.note.as_str()
        ),
        ("red", t1, 2, 2, "ended", "ended after 2 runs")
    );
    assert_eq!(events(&s, "schedule.red").len(), 1);
    assert_eq!(events(&s, "schedule.skipped").len(), 1);
    assert_eq!(events(&s, "schedule.ended").len(), 1);
    // A run that could not start: counted, nothing else changes.
    let other = schedules::add(&s, script_input("watch", "scripts/watch.sh")).unwrap();
    let run = schedules::ScriptRun {
        rc: None,
        output: "no worktree: git failed".into(),
        ..Default::default()
    };
    let rec = schedules::record_script_run(&s, &other, t2, "daemon", &run).unwrap();
    assert_eq!(
        (
            rec.schedule.state.as_str(),
            rec.schedule.last_rc,
            rec.schedule.runs,
            rec.schedule.last_output.as_str()
        ),
        ("", None, 1, "no worktree: git failed")
    );
    assert_eq!(events(&s, "schedule.ran").len(), 3);
    // The output tail is capped.
    let long = "x".repeat(schedules::OUTPUT_TAIL + 100);
    let run = schedules::ScriptRun {
        rc: Some(0),
        output: format!("{long}END"),
        verdict: schedules::verdict(&rec.schedule, 0, ""),
        ..Default::default()
    };
    let rec = schedules::record_script_run(&s, &rec.schedule, t2, "daemon", &run).unwrap();
    assert_eq!(rec.schedule.last_output.len(), schedules::OUTPUT_TAIL);
    assert!(rec.schedule.last_output.ends_with("END"));
    assert_eq!(rec.schedule.state, "ok");
}

#[test]
fn schedule_script_commands_end_to_end() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    script_repo(&s);
    let (mut c, out) = ctx_for(&s, "demo-lead", "demo", false);
    let err = run_cmd(
        &mut c,
        "schedule",
        &[
            "add",
            "--title",
            "Store tests",
            "--every",
            "1d",
            "--kind",
            "check",
        ],
    )
    .unwrap_err();
    assert!(format!("{err}").contains("needs a script"), "{err}");
    let err = run_cmd(
        &mut c,
        "schedule",
        &[
            "add",
            "--title",
            "x",
            "--every",
            "1d",
            "--script",
            "scripts/check.sh",
            "--brief",
            "b",
        ],
    )
    .unwrap_err();
    assert!(
        format!("{err}").contains("--script is for a check or a watch"),
        "{err}"
    );
    run_cmd(
        &mut c,
        "schedule",
        &[
            "add",
            "--title",
            "Store tests",
            "--every",
            "1d",
            "--at",
            "03:00",
            "--kind",
            "check",
            "--script",
            "scripts/check.sh",
            "--job-kind",
            "improve",
        ],
    )
    .unwrap();
    assert!(
        out.string().contains(
            "SCHED-001 set in demo: check `scripts/check.sh` every 1d, first run at 2026-09-07T03:00:00Z"
        ),
        "{}",
        out.string()
    );
    out.clear();
    run_cmd(
        &mut c,
        "schedule",
        &[
            "add",
            "--title",
            "New issues",
            "--every",
            "6h",
            "--kind",
            "watch",
            "--script",
            "scripts/watch.sh",
            "--brief",
            "Triage each issue.",
        ],
    )
    .unwrap();
    out.clear();
    run_cmd(&mut c, "schedule", &["list"]).unwrap();
    let text = out.string();
    assert!(text.contains("KIND") && text.contains("STATE"), "{text}");
    let line = text.lines().find(|l| l.starts_with("SCHED-001")).unwrap();
    assert!(
        line.contains("check") && line.contains("Store tests (scripts/check.sh)"),
        "{line}"
    );
    assert_eq!(
        line.split_whitespace().nth(4),
        Some("-"),
        "no run yet: {line}"
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["show", "SCHED-001"]).unwrap();
    let text = out.string();
    assert!(
        text.contains(
            "check `scripts/check.sh`, queues improve jobs at tier 1, priority 0; set by demo-lead"
        ) && text.contains("state: not run yet")
            && text.contains("JOBS"),
        "{text}"
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["log", "SCHED-001"]).unwrap();
    assert!(
        out.string()
            .contains("SCHED-001 has not run `scripts/check.sh` yet"),
        "{}",
        out.string()
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["run", "SCHED-001"]).unwrap();
    assert!(
        out.string()
            .contains("SCHED-001 due now; the daemon runs `scripts/check.sh` on the next tick"),
        "{}",
        out.string()
    );
    out.clear();
    // After a run.
    let sc = schedules::get(&s, "SCHED-001").unwrap();
    let t = "2026-09-07T03:00:05Z";
    let run = schedules::ScriptRun {
        rc: Some(101),
        seconds: 12.0,
        output: "line 1\nline 2\ntest x FAILED\n".into(),
        verdict: schedules::verdict(&sc, 101, ""),
        job: "job-abc".into(),
        queued: true,
        ..Default::default()
    };
    schedules::record_script_run(&s, &sc, t, "daemon", &run).unwrap();
    run_cmd(&mut c, "schedule", &["list"]).unwrap();
    let line = out
        .string()
        .lines()
        .find(|l| l.starts_with("SCHED-001"))
        .unwrap()
        .to_string();
    assert_eq!(line.split_whitespace().nth(4), Some("red"), "{line}");
    out.clear();
    run_cmd(&mut c, "schedule", &["show", "SCHED-001"]).unwrap();
    let text = out.string();
    assert!(
        text.contains("state: red since 2026-09-07T03:00:05Z; last exit 101, 1 failure in a row")
            && text.contains("last run at 2026-09-07T03:00:05Z (last job job-abc)")
            && text.contains("LAST OUTPUT")
            && text.contains("test x FAILED"),
        "{text}"
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["log", "SCHED-001"]).unwrap();
    let text = out.string();
    assert!(
        text.contains("SCHED-001 `scripts/check.sh` at 2026-09-07T03:00:05Z: red (exit 101)")
            && text.contains("line 1\nline 2\ntest x FAILED"),
        "{text}"
    );
    out.clear();
    // A job schedule has no log.
    run_cmd(
        &mut c,
        "schedule",
        &["add", "--title", "Audit", "--every", "1d", "--brief", "b"],
    )
    .unwrap();
    let err = run_cmd(&mut c, "schedule", &["log", "SCHED-003"]).unwrap_err();
    assert!(
        format!("{err}").contains("SCHED-003 is a task schedule, not a check or watch"),
        "{err}"
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["list"]).unwrap();
    let line = out
        .string()
        .lines()
        .find(|l| l.starts_with("SCHED-003"))
        .unwrap()
        .to_string();
    assert!(line.contains(" task ") && line.contains("Audit"), "{line}");

    let (mut c, out) = ctx_for(&s, "demo-lead", "demo", true);
    run_cmd(&mut c, "schedule", &["show", "SCHED-001"]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(v["schedule"]["kind"], "check");
    assert_eq!(v["schedule"]["script"], "scripts/check.sh");
    assert_eq!(v["schedule"]["job_kind"], "improve");
    assert_eq!(v["schedule"]["state"], "red");
    assert_eq!(v["schedule"]["state_since"], t);
    assert_eq!(v["schedule"]["last_rc"], 101);
    assert_eq!(v["schedule"]["fails"], 1);
    assert_eq!(
        v["schedule"]["last_output"],
        "line 1\nline 2\ntest x FAILED\n"
    );
    out.clear();
    run_cmd(&mut c, "schedule", &["list"]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    let watch = v["schedules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sc| sc["id"] == "SCHED-002")
        .unwrap();
    assert_eq!(watch["kind"], "watch");
    assert_eq!(watch["script"], "scripts/watch.sh");
    assert_eq!(watch["job_kind"], "task");
    assert!(watch["last_rc"].is_null());
    out.clear();
    run_cmd(&mut c, "schedule", &["log", "SCHED-001"]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(
        v["schedule"]["last_output"],
        "line 1\nline 2\ntest x FAILED\n"
    );
}

#[test]
fn schedule_add_rejects_a_cadence_the_row_cannot_hold() {
    let s = new_store();
    let err = schedules::add(&s, sched_input("3000000000m")).unwrap_err();
    assert!(err.to_string().contains("cadence"), "{err}");
    assert_eq!(schedules::list(&s, "demo", "").unwrap().len(), 0);
}

fn suggest(s: &pulse_store::Store, title: &str, by: &str) -> suggestions::Suggestion {
    suggestions::suggest(s, "demo", title, "Because the README promises it.", 1, by).unwrap()
}

fn job_for(s: &pulse_store::Store, g: &suggestions::Suggestion) -> String {
    let id = format!("job-review-{}-{}", g.id, g.reviews + 1);
    s.conn()
        .unwrap()
        .execute(
            "INSERT INTO jobs (id, project, kind, title, brief, source, status, created_at, created_by) VALUES (?, 'demo', 'reflect', 'Review', '', ?, 'queued', ?, 'daemon')",
            params![id, g.source(), s.now()],
        )
        .unwrap();
    id
}

fn job_status(s: &pulse_store::Store, id: &str) -> String {
    s.conn()
        .unwrap()
        .query_row("SELECT status FROM jobs WHERE id = ?", [id], |r| r.get(0))
        .unwrap()
}

#[test]
fn suggestions_are_made_reviewed_and_become_tasks() {
    let s = new_store();
    let g = suggest(&s, "Add a smoke test for the installer", "scout");
    assert_eq!(g.id, "SUG-001");
    assert_eq!(g.status, "suggested");
    assert_eq!(g.suggested_by, "scout");
    assert_eq!(events(&s, "suggestion.new").len(), 1);

    // Nobody reviews their own; the daemon's job marks it reviewing.
    let v = suggestions::Verdict {
        verdict: "accept".into(),
        reason: "fits the goal".into(),
        ..Default::default()
    };
    let err = suggestions::review(&s, "SUG-001", &v, "scout").unwrap_err();
    assert!(err.to_string().contains("own suggestion"), "{err}");
    let job = job_for(&s, &g);
    let g = suggestions::reviewing(&s, "SUG-001", &job, "daemon").unwrap();
    assert_eq!(g.status, "reviewing");
    assert_eq!(g.job_id, job);

    // Another agent accepts with a sharper title: the task exists, the
    // queued review job is cancelled, the suggestion points at the task.
    let v = suggestions::Verdict {
        verdict: "accept".into(),
        reason: "fits the goal; the README promises it".into(),
        title: "Smoke-test the installer on a clean macOS".into(),
        priority: Some(3),
        ..Default::default()
    };
    let (g, task) = suggestions::review(&s, "SUG-001", &v, "demo-lead").unwrap();
    let task = task.unwrap();
    assert_eq!(g.status, "accepted");
    assert_eq!(g.reviewer, "demo-lead");
    assert_eq!(g.task_id, task.id);
    assert_eq!(task.id, "TASK-001");
    assert_eq!(task.title, "Smoke-test the installer on a clean macOS");
    assert_eq!(task.priority, 3);
    assert_eq!(task.created_by, "scout");
    assert!(task.brief.contains("Because the README promises it."));
    assert!(
        task.brief
            .contains("Suggested by scout as SUG-001; accepted by demo-lead: fits the goal")
    );
    assert_eq!(job_status(&s, &job), "cancelled");
    assert_eq!(events(&s, "suggestion.review").len(), 1);

    // A closed suggestion does not change.
    let err = suggestions::review(&s, "SUG-001", &v, "demo-lead").unwrap_err();
    assert!(err.to_string().contains("already accepted"), "{err}");
    let err = suggestions::withdraw(&s, "SUG-001", "", "scout").unwrap_err();
    assert!(err.to_string().contains("already accepted"), "{err}");
}

#[test]
fn suggestions_refuse_duplicates_and_cap_an_agent() {
    let s = new_store();
    suggest(&s, "Add a smoke test", "scout");
    let err =
        suggestions::suggest(&s, "demo", "add a SMOKE   test", "why", 0, "other").unwrap_err();
    assert!(
        err.to_string().contains("Already suggested as SUG-001"),
        "{err}"
    );
    new(&s, "demo", "Ship the docs", "", 0, "human").unwrap();
    let err = suggestions::suggest(&s, "demo", "ship the docs", "why", 0, "scout").unwrap_err();
    assert!(
        err.to_string().contains("Already a task: TASK-001"),
        "{err}"
    );
    let err = suggestions::suggest(&s, "demo", "No brief", "", 0, "scout").unwrap_err();
    assert!(err.to_string().contains("needs a brief"), "{err}");
    // The owner may suggest without a brief and without a cap.
    for i in 0..suggestions::OPEN_PER_AGENT + 1 {
        suggestions::suggest(&s, "demo", &format!("Owner idea {i}"), "", 0, "human").unwrap();
    }
    for i in 1..suggestions::OPEN_PER_AGENT {
        suggest(&s, &format!("Scout idea {i}"), "scout");
    }
    let err = suggestions::suggest(&s, "demo", "One too many", "why", 0, "scout").unwrap_err();
    assert!(err.to_string().contains("waiting for a review"), "{err}");
    // Withdrawing frees a slot.
    let g = suggestions::withdraw(&s, "SUG-001", "superseded", "scout").unwrap();
    assert_eq!(g.status, "withdrawn");
    assert_eq!(g.verdict, "superseded");
    suggest(&s, "One too many", "scout");
    let open = suggestions::list(&s, "demo", "open").unwrap();
    assert!(open.iter().all(|g| g.status == "suggested"));
    assert_eq!(suggestions::list(&s, "demo", "withdrawn").unwrap().len(), 1);
    let err = suggestions::list(&s, "demo", "bogus").unwrap_err();
    assert!(err.to_string().contains("Unknown status"), "{err}");
}

#[test]
fn suggestions_decline_hand_to_owner_and_wear_out() {
    let s = new_store();
    let g = suggest(&s, "Rewrite the storage layer", "scout");
    let v = suggestions::Verdict {
        verdict: "owner".into(),
        reason: "Tier 2: it rewrites a shipped module".into(),
        ..Default::default()
    };
    // The owner cannot hand a decision to the owner.
    let err = suggestions::review(&s, "SUG-001", &v, "human").unwrap_err();
    assert!(err.to_string().contains("You are the owner"), "{err}");
    let (g1, task) = suggestions::review(&s, &g.id, &v, "reviewer").unwrap();
    assert!(task.is_none());
    assert_eq!(g1.status, "owner");
    assert_eq!(g1.verdict, "Tier 2: it rewrites a shipped module");
    // Agents leave it alone now; the owner rules.
    let d = suggestions::Verdict {
        verdict: "decline".into(),
        reason: "not now".into(),
        ..Default::default()
    };
    let err = suggestions::review(&s, "SUG-001", &d, "another").unwrap_err();
    assert!(err.to_string().contains("waits for the owner"), "{err}");
    let (g2, _) = suggestions::review(&s, "SUG-001", &d, "human").unwrap();
    assert_eq!(g2.status, "declined");
    assert_eq!(g2.reviewer, "human");
    // A reviewer needs a reason; the owner does not.
    let g = suggest(&s, "Second idea", "scout");
    let bare = suggestions::Verdict {
        verdict: "decline".into(),
        ..Default::default()
    };
    let err = suggestions::review(&s, &g.id, &bare, "reviewer").unwrap_err();
    assert!(err.to_string().contains("needs a reason"), "{err}");
    suggestions::review(&s, &g.id, &bare, "human").unwrap();
    // Three review jobs ending without a verdict send it to the owner.
    let g = suggest(&s, "Third idea", "scout");
    for n in 1..=suggestions::MAX_REVIEWS {
        let g = suggestions::get(&s, &g.id).unwrap();
        let job = job_for(&s, &g);
        suggestions::reviewing(&s, &g.id, &job, "daemon").unwrap();
        let g = suggestions::review_ended(&s, &g.id, "daemon").unwrap();
        assert_eq!(g.reviews, n);
        if n < suggestions::MAX_REVIEWS {
            assert_eq!(g.status, "suggested");
        } else {
            assert_eq!(g.status, "owner");
            assert!(g.verdict.contains("without a verdict"));
        }
    }
    let err = suggestions::review_ended(&s, &g.id, "daemon").unwrap_err();
    assert!(err.to_string().contains("is owner"), "{err}");
    assert_eq!(events(&s, "suggestion.unreviewed").len(), 3);
}

#[test]
fn suggestion_commands_end_to_end() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let (mut c, out) = ctx_for(&s, "scout", "demo", false);
    set_stdin_for_tests(Some(b"The installer has no smoke test.\n".to_vec()));
    run_cmd(
        &mut c,
        "task",
        &[
            "suggest",
            "--title",
            "Smoke-test the installer",
            "--priority",
            "2",
        ],
    )
    .unwrap();
    set_stdin_for_tests(None);
    assert!(
        out.string()
            .contains("SUG-001 suggested in demo: Smoke-test the installer"),
        "{}",
        out.string()
    );
    out.clear();
    let err = run_cmd(
        &mut c,
        "task",
        &["review", "SUG-001", "accept", "--reason", "mine"],
    )
    .unwrap_err();
    assert!(err.to_string().contains("own suggestion"), "{err}");
    let (mut c, out) = ctx_for(&s, "demo-lead", "", true);
    run_cmd(&mut c, "task", &["show", "SUG-001"]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(v["suggestion"]["brief"], "The installer has no smoke test.");
    assert_eq!(v["suggestion"]["status"], "suggested");
    out.clear();
    run_cmd(
        &mut c,
        "task",
        &[
            "review",
            "SUG-001",
            "accept",
            "--reason",
            "fits the goal",
            "--priority",
            "4",
        ],
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(v["suggestion"]["status"], "accepted");
    assert_eq!(v["task"]["id"], "TASK-001");
    assert_eq!(v["task"]["priority"], 4);
    out.clear();
    run_cmd(&mut c, "task", &["suggestions", "--status", "accepted"]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.string()).unwrap();
    assert_eq!(v["suggestions"].as_array().unwrap().len(), 1);
    assert_eq!(v["suggestions"][0]["task_id"], "TASK-001");
    let (mut c, out) = ctx_for(&s, "scout", "demo", false);
    run_cmd(
        &mut c,
        "task",
        &["suggest", "--title", "Second", "--brief", "why"],
    )
    .unwrap();
    out.clear();
    run_cmd(
        &mut c,
        "task",
        &["withdraw", "SUG-002", "--reason", "done elsewhere"],
    )
    .unwrap();
    assert!(out.string().contains("SUG-002 withdrawn"));
    out.clear();
    run_cmd(&mut c, "task", &["suggestions"]).unwrap();
    assert!(out.string().contains("SUG-002"));
    assert!(out.string().contains("withdrawn"));
    let err = run_cmd(&mut c, "task", &["review", "SUG-002"]).unwrap_err();
    assert!(err.to_string().contains("Missing argument"), "{err}");
}

fn activity_step(
    session: &str,
    n: i64,
    at: &str,
    kind: &str,
    content: &str,
) -> pulse_store::HarnessStep {
    pulse_store::HarnessStep {
        session_id: session.into(),
        n,
        at: at.into(),
        kind: kind.into(),
        content: content.into(),
        ..pulse_store::HarnessStep::default()
    }
}

#[test]
fn activity_tells_the_task_story_in_order() {
    let _serial = crate::test_helpers::serial();
    let s = new_store();
    let task = new(&s, "demo", "Ship it", "the brief", 0, "human").unwrap();
    let id = task.id.clone();
    // 21:00 created; 21:01 the lead's planning job; 21:02 a worker job.
    s.conn().unwrap().execute(
        "INSERT INTO jobs (id, project, kind, title, source, status, created_at, created_by, branch) VALUES \
         ('JOB-plan', 'demo', 'task', 'Plan the work', ?, 'done', '2026-09-06T21:01:00Z', 'daemon', ''), \
         ('JOB-work', 'demo', 'task', 'Do the work', ?, 'running', '2026-09-06T21:02:00Z', 'demo-lead', 'pulse/cinder/JOB-work')",
        params![format!("task:{id}"), format!("task:{id}:work")],
    ).unwrap();
    set_lead(&s, &id, "demo-lead", "JOB-plan", "runner").unwrap();
    set_status(&s, &id, "in_progress", "demo-lead").unwrap();
    s.conn().unwrap().execute(
        "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, exit_code, summary, branch, commits, mode, executor) VALUES \
         ('RUN-1', 'JOB-work', 'cinder', 'demo', '2026-09-06T21:03:00Z', '2026-09-06T21:10:00Z', 'done', 0, 'did the work', 'pulse/cinder/JOB-work', 2, 'real', 'local'), \
         ('RUN-2', 'JOB-work', 'cinder', 'demo', '2026-09-06T21:12:00Z', '', 'running', NULL, '', '', 0, 'real', 'local')",
        [],
    ).unwrap();
    s.begin_harness_session(pulse_store::HarnessSession {
        id: "HS-1".into(),
        run_id: "RUN-1".into(),
        job_id: "JOB-work".into(),
        agent: "cinder".into(),
        project: "demo".into(),
        started_at: "2026-09-06T21:03:00Z".into(),
        ..pulse_store::HarnessSession::default()
    })
    .unwrap();
    let long = "x".repeat(2500);
    for step in [
        activity_step("HS-1", 0, "2026-09-06T21:03:01Z", "note", "read memory"),
        activity_step("HS-1", 1, "2026-09-06T21:03:02Z", "note", "MCP connected"),
        activity_step("HS-1", 2, "2026-09-06T21:03:05Z", "assistant", &long),
        activity_step("HS-1", 3, "2026-09-06T21:03:06Z", "read", "src/lib.rs:1-40"),
        activity_step("HS-1", 4, "2026-09-06T21:03:06Z", "observation", "fn main"),
        activity_step("HS-1", 5, "2026-09-06T21:03:07Z", "grep", "fn main"),
        activity_step(
            "HS-1",
            6,
            "2026-09-06T21:03:07Z",
            "observation",
            "src/lib.rs:3",
        ),
        activity_step("HS-1", 7, "2026-09-06T21:03:08Z", "assistant", ""),
        pulse_store::HarnessStep {
            command: "cargo test -q".into(),
            title: "Run the tests".into(),
            ..activity_step("HS-1", 8, "2026-09-06T21:03:09Z", "command", "")
        },
        pulse_store::HarnessStep {
            exit_code: Some(101),
            duration_ms: 4200,
            ..activity_step(
                "HS-1",
                9,
                "2026-09-06T21:03:13Z",
                "observation",
                "test failed",
            )
        },
        activity_step(
            "HS-1",
            10,
            "2026-09-06T21:03:20Z",
            "edit",
            "src/lib.rs\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE",
        ),
        activity_step("HS-1", 11, "2026-09-06T21:03:20Z", "observation", "edited"),
        activity_step("HS-1", 12, "2026-09-06T21:09:59Z", "exit", "submitted"),
    ] {
        s.append_harness_step(step).unwrap();
    }
    s.finish_harness_session("HS-1", "ended", "submitted", "", "", 0.0, "")
        .unwrap();
    send_message(&s, "demo", &id, "human", "How is it going?").unwrap();
    pulse_bus::emit(
        &s,
        "daemon",
        "land.landed",
        "pulse/cinder/JOB-work",
        r#"{"before": "aaa", "code_delivery": {"after": "bbb"}}"#,
    )
    .unwrap();
    pulse_bus::emit(
        &s,
        "daemon",
        "land.held",
        "pulse/other/JOB-else",
        r#"{"job": "JOB-else", "reason": "not ours"}"#,
    )
    .unwrap();
    s.conn().unwrap().execute(
        "INSERT INTO posts (id, project, area, thread, kind, agent, date, summary, resolution, created_at) VALUES \
         ('20260906T211100Z-cinder', 'demo', 'jobs', 'job-work', 'result', 'cinder', '2026-09-06', 'done: the work', 'proceeded', '2026-09-06T21:11:00Z'), \
         ('20260906T211200Z-other', 'demo', 'jobs', 'job-other', 'result', 'other', '2026-09-06', 'not this task', '', '2026-09-06T21:12:00Z')",
        [],
    ).unwrap();

    let a = activity_for(&s, &id, 0).unwrap();
    assert_eq!(a.task_id, id);
    assert!(a.running);
    assert!(!a.has_more);
    assert_eq!(a.total, a.items.len());
    let ids: Vec<&str> = a.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            // Everything the test wrote through the API happened "now", 21:00:
            // the task, the lead and status events, the message, the landing.
            "task:created",
            "msg:MSG-20260906T210000Z-1",
            "land:6",
            "task:3",
            "task:4",
            "job:add:JOB-plan",
            "job:add:JOB-work",
            "run:start:RUN-1",
            "quiet:HS-1:0",
            "step:HS-1:2",
            "quiet:HS-1:3",
            "step:HS-1:8",
            "step:HS-1:10",
            "step:HS-1:12",
            "run:end:RUN-1",
            "post:20260906T211100Z-cinder",
            "run:start:RUN-2",
        ],
        "{:#?}",
        a.items
    );
    let by_id = |want: &str| a.items.iter().find(|i| i.id == want).unwrap();
    let created = by_id("task:created");
    assert_eq!(
        (
            created.kind.as_str(),
            created.what.as_str(),
            created.who.as_str()
        ),
        ("task", "created", "human")
    );
    assert_eq!(
        (created.title.as_str(), created.text.as_str()),
        ("Ship it", "the brief")
    );
    let lead = a.items.iter().find(|i| i.what == "lead").unwrap();
    assert_eq!(
        (lead.title.as_str(), lead.job_id.as_str()),
        ("demo-lead", "JOB-plan")
    );
    let status = a.items.iter().find(|i| i.what == "status").unwrap();
    assert_eq!(
        (status.title.as_str(), status.status.as_str()),
        ("open", "in_progress")
    );
    let quiet = by_id("quiet:HS-1:0");
    assert_eq!((quiet.title.as_str(), quiet.count), ("note ×2", 2));
    assert_eq!(by_id("quiet:HS-1:3").title, "read · grep");
    let words = by_id("step:HS-1:2");
    assert!(words.truncated);
    assert_eq!(words.text.chars().count(), 2001);
    assert_eq!(words.who, "cinder");
    let cmd = by_id("step:HS-1:8");
    assert_eq!(
        (cmd.title.as_str(), cmd.text.as_str()),
        ("Run the tests", "cargo test -q")
    );
    assert_eq!((cmd.exit_code, cmd.duration_ms), (Some(101), 4200));
    assert_eq!(by_id("step:HS-1:10").title, "src/lib.rs");
    assert_eq!(by_id("step:HS-1:12").status, "submitted");
    let end = by_id("run:end:RUN-1");
    assert_eq!(
        (
            end.status.as_str(),
            end.exit_code,
            end.count,
            end.text.as_str()
        ),
        ("done", Some(0), 2, "did the work")
    );
    let landed = by_id("land:6");
    assert_eq!(
        (
            landed.what.as_str(),
            landed.title.as_str(),
            landed.status.as_str()
        ),
        ("landed", "pulse/cinder/JOB-work", "bbb")
    );
    let post = by_id("post:20260906T211100Z-cinder");
    assert_eq!(
        (
            post.what.as_str(),
            post.who.as_str(),
            post.job_id.as_str(),
            post.status.as_str()
        ),
        ("result", "cinder", "JOB-work", "proceeded")
    );
    let msg = by_id("msg:MSG-20260906T210000Z-1");
    assert_eq!(
        (msg.who.as_str(), msg.text.as_str()),
        ("human", "How is it going?")
    );

    assert_eq!(a.runs.len(), 2);
    let r1 = &a.runs[0];
    assert_eq!(
        (
            r1.id.as_str(),
            r1.job_title.as_str(),
            r1.session_id.as_str()
        ),
        ("RUN-1", "Do the work", "HS-1")
    );
    assert_eq!((r1.calls, r1.commands, r1.edits, r1.steps), (2, 1, 1, 13));
    assert_eq!(
        (
            a.runs[1].id.as_str(),
            a.runs[1].status.as_str(),
            a.runs[1].session_id.as_str()
        ),
        ("RUN-2", "running", "")
    );

    // The limit keeps the newest lines and says what it cut.
    let cut = activity_for(&s, &id, 3).unwrap();
    assert!(cut.has_more);
    assert_eq!(cut.total, 17);
    assert_eq!(
        cut.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        vec![
            "run:end:RUN-1",
            "post:20260906T211100Z-cinder",
            "run:start:RUN-2"
        ]
    );
    assert!(activity_for(&s, "TASK-999", 0).is_err());
}

#[test]
fn activity_clip_cuts_on_character_boundaries() {
    assert_eq!(clip("héllo wörld", 5), ("héllo…".to_string(), true));
    assert_eq!(clip("short  \n", 50), ("short".to_string(), false));
    assert_eq!(clip("", 0), (String::new(), false));
}
