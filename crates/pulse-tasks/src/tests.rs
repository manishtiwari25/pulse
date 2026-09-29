use super::*;
use pulse_cli::{Buffer, Context, FlagSet};
use rusqlite::params;
use std::path::Path;
use std::time::{Duration, Instant};

fn new_store() -> pulse_store::Store {
    let _guard = pulse_testutil::env_lock();
    unsafe {
        std::env::set_var("PULSE_JOB", "");
    }
    let home = tempfile::tempdir().unwrap().keep();
    let s = pulse_store::Store::open(pulse_store::Options {
        home: home.display().to_string(),
        create: true,
        now: "2026-09-06T21:00:00Z".into(),
        ..pulse_store::Options::default()
    })
    .unwrap();
    s.conn()
        .unwrap()
        .execute(
            "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)",
            params![Path::new(&home).join("demo").display().to_string(), s.now()],
        )
        .unwrap();
    s.conn()
        .unwrap()
        .execute(
            "INSERT INTO agents (name, kind, created_at) VALUES ('demo-lead', 'lead', ?)",
            [s.now()],
        )
        .unwrap();
    s
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
    let c = set_status(&s, &a.id, "cancelled", "human").unwrap();
    assert_eq!(c.status, "cancelled");
    assert!(set_status(&s, &a.id, "done", "human").is_err());
}

#[test]
fn cancel_cancels_queued_jobs_only() {
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
fn work_for_summarizes_task_jobs() {
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
fn messages_since_unread_and_immutable() {
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
fn watch_delivers_events_from_another_connection() {
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
