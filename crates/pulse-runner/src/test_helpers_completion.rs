#![cfg_attr(
    not(test),
    allow(dead_code, reason = "shared test helpers; not every one has a caller")
)]
#![cfg_attr(test, allow(dead_code))]

use crate::brief::{BriefOpts, write_brief};
use crate::jobs::add_job;
use crate::lead::{LEAD_KIND, lead_name};
use crate::run::{RunOpts, work_once};
use crate::test_helpers::*;
use crate::util::{Row, shell_quote};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub(crate) struct CompletionAgents(pub(crate) BTreeMap<String, String>);

pub(crate) struct CompletionSuite {
    pub(crate) project: Row,
    pub(crate) agents: CompletionAgents,
    snap: CompletionSnapshot,
}

pub(crate) struct CompletionSnapshot {
    home: String,
    file: PathBuf,
    s: pulse_store::Store,
    _guard: crate::test_helpers::TestStore,
}

pub(crate) fn must_write_brief(opts: BriefOpts) -> String {
    write_brief(opts).expect("write brief")
}

pub(crate) fn completion_fixture(s: &pulse_store::Store) -> (Row, CompletionAgents) {
    let (project, _) = new_bare_repo(s);
    let worker = "aurora";
    let lead = lead_name("proj".into());
    join(s, worker);
    crate::board::join_agent(
        s,
        lead.clone(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        LEAD_KIND.into(),
        String::new(),
    )
    .expect("join lead");
    let mut agents = BTreeMap::new();
    for kind in ["task", "answer", "ballot", "experiment", LEAD_KIND] {
        agents.insert(kind.to_string(), worker.to_string());
    }
    agents.insert(LEAD_KIND.to_string(), lead);
    (project, CompletionAgents(agents))
}

pub(crate) fn new_completion_job(kind: &str) -> (crate::test_helpers::TestStore, Row, Row, String) {
    let s = new_store();
    let (project, agents) = completion_fixture(&s);
    let job = completion_job(&s, kind);
    let agent = agents
        .0
        .get(kind)
        .cloned()
        .unwrap_or_else(|| "aurora".into());
    (s, job, project, agent)
}

pub(crate) fn new_completion_suite() -> CompletionSuite {
    let s = new_store();
    let (project, agents) = completion_fixture(&s);
    let snap = snapshot_completion_store(s);
    CompletionSuite {
        project,
        agents,
        snap,
    }
}

impl CompletionSuite {
    pub(crate) fn job(&mut self, kind: &str) -> (pulse_store::Store, Row, Row, String) {
        let s = self.snap.reset();
        let job = completion_job(&s, kind);
        let agent = self
            .agents
            .0
            .get(kind)
            .cloned()
            .unwrap_or_else(|| "aurora".into());
        (s, job, self.project.clone(), agent)
    }
}

pub(crate) fn completion_job(s: &pulse_store::Store, kind: &str) -> Row {
    add_job(
        s,
        "proj".into(),
        kind.into(),
        "Completion regression".into(),
        "Report the outcome.".into(),
        0,
        0,
        String::new(),
        crate::util::HUMAN_AGENT.into(),
        false,
    )
    .expect("add completion job")
    .0
}

pub(crate) fn snapshot_completion_store(
    guard: crate::test_helpers::TestStore,
) -> CompletionSnapshot {
    let s = (*guard).clone();
    let file = PathBuf::from(&s.home).join("completion-snapshot.sqlite");
    let conn = s.conn().expect("conn");
    conn.execute("VACUUM INTO ?", [&file.display().to_string()])
        .expect("vacuum snapshot");
    drop(conn);
    CompletionSnapshot {
        home: s.home.clone(),
        file,
        s,
        _guard: guard,
    }
}

impl CompletionSnapshot {
    pub(crate) fn reset(&mut self) -> pulse_store::Store {
        self.s.close().expect("close store");
        let path = PathBuf::from(&self.home).join(pulse_store::STORE_NAME);
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
        let data = std::fs::read(&self.file).expect("read snapshot");
        std::fs::write(&path, data).expect("restore snapshot");
        let s = pulse_store::Store::open(pulse_store::Options {
            home: self.home.clone(),
            now: TEST_NOW.into(),
            ..Default::default()
        })
        .expect("open restored store");
        self.s = s.clone();
        s
    }
}

pub(crate) fn run_completion(s: &pulse_store::Store, job: Row, agent: &str, command: &str) -> Row {
    let (r, reason) = work_once(
        s,
        agent,
        &job.str("project"),
        &job.str("id"),
        TEST_NOW,
        RunOpts {
            worker_cmd: command.into(),
            ..Default::default()
        },
    )
    .expect("work once");
    assert!(!r.0.is_empty(), "work returned no run: {reason}");
    r
}

pub(crate) fn completion_worker_command(
    status: &str,
    result: &str,
    output: &str,
    exit: i32,
    direct_answer: &str,
) -> (String, Vec<pulse_testutil::SetenvGuard>) {
    // The helper process inherits these; they revert when the guards drop.
    let env = vec![
        pulse_testutil::setenv("PULSE_COMPLETION_HELPER", "1"),
        pulse_testutil::setenv("PULSE_COMPLETION_STATUS", status),
        pulse_testutil::setenv("PULSE_COMPLETION_RESULT", result),
        pulse_testutil::setenv("PULSE_COMPLETION_OUTPUT", output),
        pulse_testutil::setenv("PULSE_COMPLETION_EXIT", exit.to_string()),
        pulse_testutil::setenv("PULSE_COMPLETION_ANSWER", direct_answer),
    ];
    (
        format!(
            "{} --exact test_helpers_completion::tests::completion_worker_process --nocapture",
            shell_quote(&build_pulse())
        ),
        env,
    )
}

pub(crate) fn lightweight_completion_worker_command(output: &str, exit: i32) -> String {
    format!(
        "sh -c {}",
        shell_quote(&format!("printf %s {}; exit {}", shell_quote(output), exit))
    )
}

pub(crate) fn plan_handoff(s: &pulse_store::Store, now: &str, o: crate::board::PostOpts) -> Row {
    crate::board::create_post(s, now.into(), o).expect("create handoff")
}

pub(crate) fn suppress_planner_reflect(s: &pulse_store::Store) {
    add_job(
        s,
        "proj".into(),
        "reflect".into(),
        "Daily reflect".into(),
        "already done".into(),
        0,
        1,
        format!("reflect:proj:{}", &TEST_NOW[..10]),
        "daemon".into(),
        true,
    )
    .expect("add reflect");
    crate::util::exec1(
        &s.conn().expect("conn"),
        "UPDATE jobs SET status = 'done', finished_at = ? WHERE project = 'proj' AND kind = 'reflect'",
        &[&TEST_NOW],
    );
}

pub(crate) fn jobs_by_source(s: &pulse_store::Store, source: &str) -> Row {
    let j = crate::util::must_one(
        &s.conn().expect("conn"),
        "SELECT * FROM jobs WHERE source = ?",
        &[&source],
    );
    assert!(!j.0.is_empty(), "no job with source {source}");
    j
}

#[must_use = "the gate reopens when the guard drops"]
pub(crate) fn with_closed_login_gate() -> crate::login_gate::LoginGateGuard {
    crate::login_gate::swap_login_gate(Box::new(|_| {
        "workers on this machine wait for a Gate Fake login: sign in with GitHub".into()
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn completion_question(
        s: &pulse_store::Store,
        job: &Row,
        mut opts: crate::board::PostOpts,
    ) -> Row {
        if opts.kind.is_empty() {
            opts.kind = "question".into();
        }
        if opts.project.is_empty() {
            opts.project = job.str("project");
        }
        if opts.area.is_empty() {
            opts.area = "parser".into();
        }
        if opts.thread.is_empty() {
            opts.thread = "empty-input".into();
        }
        if opts.summary.is_empty() {
            opts.summary = "Why is empty input ignored?".into();
        }
        if opts.agent.is_none() {
            opts.agent = crate::board::actor_agent(s, "requester".into());
        }
        if opts.date.is_empty() {
            opts.date = crate::util::plus_hours(TEST_NOW, -1.0);
        }
        let q = crate::board::create_post(s, TEST_NOW.into(), opts).expect("create question");
        crate::util::exec1(
            &s.conn().expect("conn"),
            "UPDATE jobs SET source = ? WHERE id = ?",
            &[&format!("question:{}", q.str("id")), &job.str("id")],
        );
        q
    }

    #[test]
    pub(crate) fn completion_worker_process() {
        let _serial = crate::test_helpers::serial();
        if std::env::var("PULSE_COMPLETION_HELPER").as_deref() != Ok("1")
            || std::env::var("PULSE_JOB").unwrap_or_default().is_empty()
        {
            return;
        }
        let s = pulse_store::Store::open(pulse_store::Options {
            home: std::env::var("PULSE_HOME").unwrap_or_default(),
            now: TEST_NOW.into(),
            ..Default::default()
        })
        .expect("open store");
        let conn = s.conn().expect("conn");
        let job = crate::jobs::job_row(&conn, std::env::var("PULSE_JOB").unwrap_or_default());
        let agent = std::env::var("PULSE_AGENT").unwrap_or_default();
        if let Ok(mode) = std::env::var("PULSE_COMPLETION_ANSWER")
            && !mode.is_empty()
        {
            let source = job.str("source");
            let id = source
                .strip_prefix("question:")
                .expect("fixture answer has no question source");
            let q = crate::board::post_row(&conn, id.into());
            let on = if mode == "thread" { "" } else { id };
            crate::board::create_post(
                &s,
                TEST_NOW.into(),
                crate::board::PostOpts {
                    kind: "answer".into(),
                    project: q.str("project"),
                    area: q.str("area"),
                    thread: q.str("thread"),
                    summary: "The worker already answered.".into(),
                    body: "Direct worker answer.\n".into(),
                    on: on.into(),
                    agent: crate::board::actor_agent(&s, agent.clone()),
                    session: std::env::var("PULSE_SESSION").unwrap_or_default(),
                    ..Default::default()
                },
            )
            .expect("create direct answer");
        }
        if let Ok(status) = std::env::var("PULSE_COMPLETION_STATUS")
            && !status.is_empty()
        {
            let generation = job.int("generation");
            crate::jobs::finish_job(
                &s,
                job.str("id"),
                agent.clone(),
                status,
                std::env::var("PULSE_COMPLETION_RESULT").unwrap_or_default(),
                format!("pulse/{}/{}", agent, job.str("id")),
                generation,
            )
            .expect("finish job");
        }
        print!(
            "{}",
            std::env::var("PULSE_COMPLETION_OUTPUT").unwrap_or_default()
        );
        let exit = std::env::var("PULSE_COMPLETION_EXIT")
            .unwrap_or_else(|_| "0".into())
            .parse()
            .expect("exit int");
        s.close().expect("close store");
        std::process::exit(exit);
    }

    #[test]
    fn completion_answer_delivered_to_source() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let mut suite = new_completion_suite();
        for transport in ["stdout", "finish-summary", "finish-raw"] {
            let (s, job, _, agent) = suite.job("answer");
            let q = completion_question(&s, &job, crate::board::PostOpts::default());
            let other = completion_question(
                &s,
                &job,
                crate::board::PostOpts {
                    date: crate::util::plus_minutes(TEST_NOW, 1.0),
                    summary: "A later question still needs an answer.".into(),
                    ..Default::default()
                },
            );
            crate::util::exec1(
                &s.conn().expect("conn"),
                "UPDATE jobs SET source = ? WHERE id = ?",
                &[&format!("question:{}", q.str("id")), &job.str("id")],
            );
            let text = format!(
                "Empty input is ignored by the parser guard. {}",
                "The fixture documents this behavior. ".repeat(8)
            )
            .trim()
            .to_string();
            let (status, result, output) = if transport == "stdout" {
                (String::new(), String::new(), format!("SUMMARY: {text}\n"))
            } else if transport == "finish-summary" {
                ("done".into(), format!("SUMMARY: {text}"), String::new())
            } else {
                ("done".into(), text.clone(), String::new())
            };
            let r = run_completion(
                &s,
                job.clone(),
                &agent,
                &completion_worker_command(&status, &result, &output, 0, "").0,
            );
            assert_eq!(r.str("status"), "done", "answer run: {r:?}");
            let answers = crate::util::must_rows(
                &s.conn().expect("conn"),
                "SELECT * FROM posts WHERE kind = 'answer'",
                &[],
            );
            assert_eq!(
                answers.len(),
                1,
                "successful answer produced {} answer post(s), want one",
                answers.len()
            );
            let a = &answers[0];
            assert!(
                a.str("project") == "proj"
                    && a.str("area") == q.str("area")
                    && a.str("thread") == q.str("thread")
                    && a.str("on_post") == q.str("id")
                    && a.str("agent") == agent
                    && a.str("session") == r.str("id")
                    && a.str("summary") == crate::util::trunc(&text, 230)
                    && a.str("body").trim() == text
                    && a.str("resolution").is_empty(),
                "answer metadata/content mismatch: {a:?}"
            );
            assert_eq!(
                crate::board::post_row(&s.conn().expect("conn"), q.str("id")).str("resolution"),
                "",
                "answer mutated the source question"
            );
            let open = crate::plan::open_questions(
                &s,
                "proj".into(),
                crate::util::plus_minutes(TEST_NOW, 2.0),
            );
            assert!(
                open.len() == 1 && open[0].str("id") == other.str("id"),
                "source answer closed an unrelated question: {open:?}"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result' AND area = 'jobs' AND thread = ?",
                    &[&crate::util::slugify(&job.str("id"), 40)],
                ),
                1,
                "job result post count mismatch"
            );
            crate::util::exec1(
                &s.conn().expect("conn"),
                "UPDATE jobs SET status = 'queued' WHERE id = ?",
                &[&job.str("id")],
            );
            run_completion(
                &s,
                job.clone(),
                &agent,
                &completion_worker_command("", "", "SUMMARY: repeated delivery\n", 0, "").0,
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM posts WHERE kind = 'answer'",
                    &[]
                ),
                1,
                "repeated completion duplicated answer"
            );
        }
    }

    #[test]
    fn completion_answer_guards() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let mut suite = new_completion_suite();
        for name in [
            "owner flag",
            "owner path",
            "owner path signal",
            "daemon question",
            "loop stop",
            "hard stop",
            "tier two",
            "closed thread",
            "jobs area",
            "missing source",
            "empty source",
            "malformed source",
            "wrong prefix",
            "wrong kind",
            "cross project",
            "failed",
            "cancelled",
            "cancelled finish",
            "question output",
            "question finish",
            "hard stop result",
            "empty summary",
            "non-answer job",
        ] {
            let kind = if name == "non-answer job" {
                "task"
            } else {
                "answer"
            };
            let (s, job, _, agent) = suite.job(kind);
            let mut opts = crate::board::PostOpts::default();
            match name {
                "owner flag" => opts.for_owner = true,
                "owner path" => opts.body = "Should docs/evolution/STOP be removed?".into(),
                "owner path signal" => opts.body = "Signal: docs/evolution/constitution.md".into(),
                "daemon question" => opts.agent = crate::board::actor_agent(&s, "daemon".into()),
                "loop stop" => opts.summary = pulse_evolve::STOP_SUMMARY.into(),
                "hard stop" => opts.body = "Should the worker cross this hard stop?".into(),
                "tier two" => opts.summary = "Should ADR-021 be changed?".into(),
                "closed thread" => opts.resolution = "answered".into(),
                "jobs area" => opts.area = "jobs".into(),
                "wrong kind" => opts.kind = "finding".into(),
                "cross project" => {
                    crate::board::add_project(
                        &s,
                        template_repo(),
                        "Other".into(),
                        String::new(),
                        false,
                        String::new(),
                        crate::util::HUMAN_AGENT.into(),
                        "other".into(),
                    )
                    .expect("add other project");
                    opts.project = "other".into();
                }
                _ => {}
            }
            let q = completion_question(&s, &job, opts);
            let mut source = format!("question:{}", q.str("id"));
            match name {
                "missing source" => source = "question:missing".into(),
                "empty source" => source = "question:".into(),
                "malformed source" => source.push_str(":extra"),
                "wrong prefix" => source = format!("suggestion:{}", q.str("id")),
                _ => {}
            }
            crate::util::exec1(
                &s.conn().expect("conn"),
                "UPDATE jobs SET source = ? WHERE id = ?",
                &[&source, &job.str("id")],
            );
            let (mut status, mut result, mut output, mut exit) =
                ("", "", "SUMMARY: investigated the parser\n", 0);
            match name {
                "failed" => exit = 1,
                "cancelled" => exit = 5,
                "cancelled finish" => {
                    status = "cancelled";
                    result = "SUMMARY: stopped";
                }
                "question output" => {
                    output = "SUMMARY: investigated the parser\nQUESTION: Need an owner decision first.\n";
                }
                "question finish" => {
                    status = "done";
                    result = "SUMMARY: investigated\nQUESTION: Need an owner decision first.";
                }
                "hard stop result" => {
                    output = "SUMMARY: stopped at a hard stop; owner disposition required\n"
                }
                "empty summary" => output = "FINDING: no conclusion yet\n",
                _ => {}
            }
            let (command, _env) = if status.is_empty() && result.is_empty() {
                (
                    lightweight_completion_worker_command(output, exit),
                    Vec::new(),
                )
            } else {
                completion_worker_command(status, result, output, exit, "")
            };
            run_completion(&s, job, &agent, &command);
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM posts WHERE kind = 'answer'",
                    &[]
                ),
                0,
                "guarded completion emitted answer(s) for {name}"
            );
        }
    }

    #[test]
    fn completion_answer_preserves_existing_answer() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let mut suite = new_completion_suite();
        for mode in [
            "linked",
            "thread",
            "same-time-thread",
            "worker-linked",
            "worker-thread",
        ] {
            let (s, job, _, agent) = suite.job("answer");
            let q = completion_question(&s, &job, crate::board::PostOpts::default());
            let mut direct = "";
            if let Some(rest) = mode.strip_prefix("worker-") {
                direct = rest;
            } else {
                let on = if mode == "linked" {
                    q.str("id")
                } else {
                    String::new()
                };
                let at = if mode == "same-time-thread" {
                    q.str("date")
                } else {
                    TEST_NOW.into()
                };
                crate::board::create_post(
                    &s,
                    at,
                    crate::board::PostOpts {
                        kind: "answer".into(),
                        project: "proj".into(),
                        area: q.str("area"),
                        thread: q.str("thread"),
                        summary: "An independent answer.".into(),
                        body: "Keep the independent answer.\n".into(),
                        on,
                        agent: crate::board::actor_agent(&s, "other-worker".into()),
                        ..Default::default()
                    },
                )
                .expect("existing answer");
            }
            run_completion(
                &s,
                job,
                &agent,
                &completion_worker_command("", "", "SUMMARY: investigated the parser\n", 0, direct)
                    .0,
            );
            let answers = crate::util::must_rows(
                &s.conn().expect("conn"),
                "SELECT * FROM posts WHERE kind = 'answer'",
                &[],
            );
            assert_eq!(answers.len(), 1, "existing answer duplicated for {mode}");
            assert!(
                !answers[0].str("body").contains("investigated the parser"),
                "runner replaced a direct or independent answer for {mode}"
            );
        }
    }

    #[test]
    fn completion_answer_after_older_same_timestamp_reply() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, job, _, agent) = new_completion_job("answer");
        crate::board::create_post(
            &s,
            TEST_NOW.into(),
            crate::board::PostOpts {
                kind: "answer".into(),
                project: "proj".into(),
                area: "parser".into(),
                thread: "empty-input".into(),
                summary: "An answer to an earlier question.".into(),
                agent: crate::board::actor_agent(&s, "other-worker".into()),
                date: crate::util::plus_hours(TEST_NOW, -1.0),
                ..Default::default()
            },
        )
        .expect("older answer");
        let q = completion_question(&s, &job, crate::board::PostOpts::default());
        run_completion(
            &s,
            job,
            &agent,
            &completion_worker_command(
                "",
                "",
                "SUMMARY: The new question has a different answer.\n",
                0,
                "",
            )
            .0,
        );
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'answer' AND on_post = ?",
            &[&q.str("id")],
        );
        if n != 1 {
            panic!(
                "an older answer with the same timestamp suppressed the new answer: {n} linked answers"
            );
        }
    }

    #[test]
    fn completion_question_transports_deduplicated() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, job, _, agent) = new_completion_job("answer");
        completion_question(&s, &job, crate::board::PostOpts::default());
        let output = "SUMMARY: Investigated the parser.\nQUESTION: Need an owner decision first.\n";
        run_completion(
            &s,
            job.clone(),
            &agent,
            &completion_worker_command("done", output, output, 0, "").0,
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question' AND area = 'jobs' AND thread = ?",
                &[&crate::util::slugify(&job.str("id"), 40)],
            ),
            1,
            "the same question from stdout and the job result was not deduplicated"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'answer'",
                &[]
            ),
            0,
            "a still-blocked answer emitted answers"
        );
    }

    #[test]
    fn completion_dry_run_keeps_planned_experiment_branch() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, job, _, agent) = new_completion_job("experiment");
        crate::util::exec1(
            &s.conn().expect("conn"),
            "UPDATE jobs SET branch = 'evolve/completion-fixture' WHERE id = ?",
            &[&job.str("id")],
        );
        let (r, reason) = crate::run::work_once(
            &s,
            &agent,
            "proj",
            &job.str("id"),
            TEST_NOW,
            RunOpts {
                dry_run: true,
                ..Default::default()
            },
        )
        .expect("dry run");
        assert!(!r.0.is_empty(), "dry run: {r:?} {reason:?}");
        assert_eq!(r.str("status"), "dry-run", "dry run: {r:?} {reason:?}");
        let j = crate::jobs::job_row(&s.conn().expect("conn"), job.str("id"));
        assert_eq!(
            j.str("status"),
            "queued",
            "dry run lost the planned experiment branch: {j:?}"
        );
        assert_eq!(
            j.str("branch"),
            "evolve/completion-fixture",
            "dry run lost the planned experiment branch: {j:?}"
        );
        assert!(
            crate::land::pending_landings(&s.conn().expect("conn"), "proj").is_empty(),
            "dry run made the planned experiment branch landable: {j:?}"
        );
    }

    #[test]
    fn completion_rejects_unreported_success() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let mut suite = new_completion_suite();
        for (name, output) in [
            ("empty", ""),
            ("unstructured", "Starting work; everything looks fine.\n"),
            ("empty label", "SUMMARY:   \n"),
        ] {
            let (s, job, _, agent) = suite.job("task");
            let (command, _env) = completion_worker_command("", "", output, 0, "");
            let r = run_completion(&s, job.clone(), &agent, &command);
            assert_eq!(
                r.str("status"),
                "failed",
                "exit-zero output {name:?}: summary={:?}",
                r.str("summary")
            );
            assert!(
                r.str("summary").contains("worker wrote no report"),
                "exit-zero output {name:?}: summary={:?}",
                r.str("summary")
            );
            let j = crate::jobs::job_row(&s.conn().expect("conn"), job.str("id"));
            assert_eq!(j.str("status"), "failed", "unreported job {name}: {j:?}");
            assert!(
                j.str("branch").is_empty(),
                "unreported job {name}: branch={:?}",
                j.str("branch")
            );
        }
    }

    #[test]
    fn completion_summary_only_kinds() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let mut suite = new_completion_suite();
        for kind in ["answer", "ballot", LEAD_KIND, "task"] {
            let (s, job, _, agent) = suite.job(kind);
            let (command, _env) = completion_worker_command(
                "",
                "",
                "SUMMARY: investigated without changing code\n",
                0,
                "",
            );
            let r = run_completion(&s, job, &agent, &command);
            assert_eq!(
                r.str("status"),
                "done",
                "summary-only {kind}: commits={} branch={:?}",
                r.int("commits"),
                r.str("branch")
            );
            assert_eq!(r.int("commits"), 0, "summary-only {kind}: {r:?}");
            assert!(r.str("branch").is_empty(), "summary-only {kind}: {r:?}");
        }
    }

    #[test]
    fn completion_worker_finish_authoritative() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let mut suite = new_completion_suite();
        for (name, kind, status, result, output, want, exit) in [
            (
                "lead without artifacts",
                LEAD_KIND,
                "done",
                "SUMMARY: reviewed the queue",
                "",
                "done",
                0,
            ),
            (
                "explicit finish without text",
                "task",
                "done",
                "",
                "",
                "done",
                0,
            ),
            (
                "raw finish result",
                "task",
                "done",
                "reviewed the queue",
                "Worker exited normally.\n",
                "done",
                0,
            ),
            (
                "worker failure",
                "task",
                "failed",
                "SUMMARY: cannot finish",
                "",
                "failed",
                0,
            ),
            (
                "empty worker failure",
                "task",
                "failed",
                "",
                "",
                "failed",
                1,
            ),
            (
                "nonzero exit overrides done",
                "task",
                "done",
                "SUMMARY: claimed success",
                "",
                "failed",
                1,
            ),
            (
                "cancel overrides done",
                "task",
                "done",
                "SUMMARY: claimed success",
                "",
                "failed",
                5,
            ),
            (
                "cancelled finish",
                "task",
                "cancelled",
                "SUMMARY: stopped",
                "",
                "failed",
                0,
            ),
        ] {
            let (s, job, _, agent) = suite.job(kind);
            let (command, _env) = completion_worker_command(status, result, output, exit, "");
            let r = run_completion(&s, job.clone(), &agent, &command);
            let j = crate::jobs::job_row(&s.conn().expect("conn"), job.str("id"));
            assert_eq!(
                r.str("status"),
                want,
                "{name}: summary={:?}",
                r.str("summary")
            );
            assert_eq!(j.str("status"), want, "{name}: job={j:?}");
            assert!(r.str("branch").is_empty(), "{name}: run={r:?}");
            assert!(j.str("branch").is_empty(), "{name}: job={j:?}");
            assert!(
                crate::land::pending_landings(&s.conn().expect("conn"), "proj").is_empty(),
                "{name}: zero-artifact completion became landable"
            );
        }
    }
}
