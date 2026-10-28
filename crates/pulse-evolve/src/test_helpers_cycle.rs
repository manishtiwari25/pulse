#![allow(dead_code)]

use crate::api::{ArchiveRow, Scores};
use crate::test_helpers::*;
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

pub(crate) static CYCLE_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));
static FAKE_EXECUTABLE: std::sync::LazyLock<std::sync::Mutex<Option<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

pub(crate) fn fake_executable_hook() -> std::io::Result<String> {
    Ok(FAKE_EXECUTABLE
        .lock()
        .expect("fake executable lock")
        .clone()
        .unwrap_or_default())
}

pub(crate) fn set_fake_executable(path: Option<String>) {
    *FAKE_EXECUTABLE.lock().expect("fake executable lock") = path;
}

pub(crate) fn cycle_guard() -> std::sync::MutexGuard<'static, ()> {
    CYCLE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn set_env(name: &str, value: &str) {
    unsafe { std::env::set_var(name, value) };
}

pub(crate) fn unset_env(name: &str) {
    unsafe { std::env::remove_var(name) };
}

pub(crate) fn disable_project_hook() -> Option<pulse_board::ProjectCreatedHook> {
    pulse_board::set_on_project_created(None)
}

pub(crate) fn put_record(s: &pulse_store::Store, rec: pulse_store::Record, body: &str) {
    s.put_record(rec, BTreeMap::new(), body, "human", "fixture")
        .expect("put record");
}

pub(crate) fn governance(s: &pulse_store::Store, project: &str) {
    let constitution = "# Constitution\n\n## Editable scope\n| File class | Paths | Stage | Autonomy |\n| --- | --- | --- | --- |\n| product | `value.txt`, `other.txt` | 4 | standing |\n| operating | `README.md` | 2 | standing |\n\n## Immutable paths\n| Path | Why |\n| --- | --- |\n| `protected.txt` | owner-owned |\n";
    for (slug, body) in [
        ("constitution", constitution),
        ("evaluators", "Product scope: allowed\n"),
    ] {
        put_record(
            s,
            pulse_store::Record {
                id: format!("EVO-{slug}-{project}"),
                project: project.to_string(),
                kind: "evolution".to_string(),
                slug: slug.to_string(),
                ..Default::default()
            },
            body,
        );
    }
}

pub(crate) fn baseline(s: &pulse_store::Store, project: &str) {
    crate::api::archive_row(
        s,
        ArchiveRow {
            project: project.to_string(),
            baseline: Some(Scores {
                total: 0.1,
                ..Default::default()
            }),
            variant: Some(Scores {
                total: 0.2,
                ..Default::default()
            }),
            kept: true,
            tolerance: 0.02,
            ..Default::default()
        },
    )
    .expect("archive baseline");
}

pub(crate) fn cycle_ready() -> pulse_store::Store {
    let s = test_store();
    let _prev = disable_project_hook();
    test_project(&s, "demo");
    governance(&s, "demo");
    fixture(&s, "demo", "F01", "printf 1", 1.0);
    baseline(&s, "demo");
    private_set(&s);
    for name in [
        "tester",
        "proposer",
        "voter-one",
        "voter-two",
        "voter-three",
    ] {
        join_test_agent(&s, name);
    }
    s.set_setting("human", "evolve.loop", "1")
        .expect("enable loop");
    s
}

pub(crate) fn voted_suggestion(
    s: &pulse_store::Store,
    thread: &str,
    class: &str,
    file: &str,
    votes: usize,
) -> pulse_board::Post {
    let p = pulse_board::create_post(
        s,
        pulse_board::PostInput {
            project: "demo".into(),
            area: "evolution".into(),
            thread: thread.into(),
            kind: "suggestion".into(),
            agent: "proposer".into(),
            summary: format!("Improve one {class} variable"),
            files: file.into(),
            body: format!(
                "Class: {class}\nFile: {file}\nVariable: one constant\nHypothesis: one constant improves the weighted score\nTier: 1\n"
            ),
            ..Default::default()
        },
    )
    .expect("create suggestion");
    for agent in ["voter-one", "voter-two", "voter-three"].iter().take(votes) {
        pulse_board::create_post(
            s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: thread.into(),
                kind: "vote".into(),
                agent: (*agent).into(),
                summary: "independent evidence".into(),
                on: p.id.clone(),
                vote: "up".into(),
                ..Default::default()
            },
        )
        .expect("create vote");
    }
    p
}

pub(crate) fn cardless_suggestion(
    s: &pulse_store::Store,
    thread: &str,
    summary: &str,
    votes: &[&str],
) -> pulse_board::Post {
    let p = pulse_board::create_post(
        s,
        pulse_board::PostInput {
            project: "demo".into(),
            area: "evolution".into(),
            thread: thread.into(),
            kind: "suggestion".into(),
            agent: "proposer".into(),
            summary: summary.into(),
            body: format!(
                "One-variable improvement seen by the worker in run `run-1` of job `job-1` (Daily reflect), not made there.\n\n{summary}\n"
            ),
            ..Default::default()
        },
    )
    .expect("create cardless suggestion");
    for vote in votes {
        pulse_board::create_post(
            s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: thread.into(),
                kind: "vote".into(),
                agent: "voter-one".into(),
                summary: (*vote).into(),
                on: p.id.clone(),
                vote: (*vote).into(),
                ..Default::default()
            },
        )
        .expect("create cardless vote");
    }
    p
}

pub(crate) fn fake_archive(s: &pulse_store::Store, job_id: &str) -> Result<()> {
    fake_archive_outcome(s, job_id, true)
}

pub(crate) fn fake_archive_outcome(s: &pulse_store::Store, job_id: &str, kept: bool) -> Result<()> {
    let mut job = pulse_board::job_by_id(s, job_id)?;
    let start = std::time::Instant::now();
    while job.is_none() && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
        job = pulse_board::job_by_id(s, job_id)?;
    }
    let job = job.ok_or_else(|| {
        let jobs = s
            .conn()
            .ok()
            .and_then(|conn| {
                let mut stmt = conn
                    .prepare("SELECT id || ':' || kind || ':' || status FROM jobs ORDER BY id")
                    .ok()?;
                Some(
                    stmt.query_map([], |r| r.get::<_, String>(0))
                        .ok()?
                        .filter_map(Result::ok)
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        anyhow!("fake runner could not read its queued job {job_id}; jobs={jobs:?}")
    })?;
    if job.kind != "experiment"
        || !job.brief.contains("Hypothesis:")
        || !job.brief.contains("Suggestion:")
        || job.branch != format!("evolve/{}", job.source.trim_start_matches("suggestion:"))
    {
        return Err(anyhow!("experiment job lacks the runner contract"));
    }
    let run_id = format!("run-{}", job.id);
    s.conn()?.execute(
        "INSERT INTO runs (id, job_id, agent, project, started_at) VALUES (?, ?, ?, ?, ?)",
        rusqlite::params![run_id, job.id, "tester", job.project, s.now()],
    )?;
    let variant = if kept { 0.4 } else { 0.1 };
    crate::api::archive_row(
        s,
        ArchiveRow {
            project: job.project.clone(),
            suggestion_post: job.source.trim_start_matches("suggestion:").to_string(),
            hypothesis: "one constant improves the weighted score".into(),
            branch: format!("evolve/{}", job.source.trim_start_matches("suggestion:")),
            baseline: Some(Scores {
                total: 0.2,
                ..Default::default()
            }),
            variant: Some(Scores {
                total: variant,
                ..Default::default()
            }),
            kept,
            tolerance: 0.02,
            run_id,
            ..Default::default()
        },
    )?;
    Ok(())
}

pub(crate) fn fake_finalize_archive(
    s: &pulse_store::Store,
    job_id: &str,
    action: &str,
    proof: Option<&crate::archive::Archived>,
) -> Result<()> {
    let current = crate::fleet::archived_for_job(s, "demo", job_id)?
        .ok_or_else(|| anyhow!("fake finalization needs a measured current run"))?;
    let tx = s.begin()?;
    tx.execute(
        "UPDATE runs SET status = 'done' WHERE id = ?",
        rusqlite::params![current.run_id],
    )?;
    tx.execute(
        "UPDATE jobs SET status = 'done', branch = '', result = 'fake controller finished', finished_at = ? WHERE id = ?",
        rusqlite::params![s.now(), job_id],
    )?;
    if let Some(proof) = proof {
        tx.event(
            "tester",
            action,
            &proof.branch,
            serde_json::json!({
                "project": proof.project,
                "job": job_id,
                "archive": proof.id,
                "run_id": proof.run_id,
            }),
        )?;
    }
    tx.commit()
}

pub(crate) fn fake_landing(s: &pulse_store::Store, job_id: &str, hold: Duration) -> Result<()> {
    fake_archive(s, job_id)?;
    thread::sleep(hold);
    pulse_board::finish_job(s, job_id, "tester", "done", "measured and kept", "", None)?;
    let archive = crate::fleet::archived_for_job(s, "demo", job_id)?
        .ok_or_else(|| anyhow!("missing archive"))?;
    fake_finalize_archive(s, job_id, "land.landed", Some(&archive))
}

pub(crate) fn report_job(
    s: &pulse_store::Store,
    kind: &str,
    title: &str,
    source: &str,
    status: &str,
    commits: i64,
) -> pulse_board::Job {
    let (job, created) = pulse_board::add_job(
        s,
        pulse_board::JobInput {
            project: "demo".into(),
            kind: kind.into(),
            title: title.into(),
            source: source.into(),
            created_by: "daemon".into(),
            tier: 1,
            ..Default::default()
        },
    )
    .expect("add report job");
    assert!(created, "report job should be created");
    if status.is_empty() {
        return job;
    }
    s.conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET status = 'leased', lease_agent = 'card-writer' WHERE id = ?",
            [&job.id],
        )
        .unwrap();
    if commits > 0 {
        s.conn().unwrap().execute(
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, commits) VALUES (?, ?, 'card-writer', 'demo', ?, ?, 'done', ?)",
            rusqlite::params![format!("run-{}", job.id), job.id, s.now(), s.now(), commits],
        ).unwrap();
    }
    let mut result = "SUMMARY: done";
    if title.starts_with(crate::card_job::CARD_JOB_TITLE) && status == "done" && commits == 0 {
        result = "no card: already implemented";
    }
    pulse_board::finish_job(s, &job.id, "card-writer", status, result, "", None)
        .expect("finish report job")
}

pub(crate) fn score_repo() -> (String, String) {
    let repo = crate::test_helpers::temp_path();
    test_git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    put_file(
        repo.join("README.md"),
        "# demo\n\nWe recieve requests.\n",
        0o644,
    );
    put_file(
        repo.join("AGENTS.md"),
        "# Agents\n\nWarn before running unsandboxed.\n",
        0o644,
    );
    put_file(repo.join("main.go"), "package main\n", 0o644);
    put_file(repo.join("tests/main_test.go"), "package main\n", 0o644);
    test_git(&repo, &["add", "."]);
    test_git(&repo, &["commit", "--quiet", "-m", "start"]);
    let commit = test_git(&repo, &["rev-parse", "HEAD"]);
    (repo.display().to_string(), commit)
}

/// Scores every starter fixture against the commit. Each scorer is read-only on
/// its own checkout (one `git -C DIR` per scorer, PULSE_HOME shared read-only), so
/// they run on scoped threads: serially this helper was the slowest test in the
/// suite (~100s of git subprocesses).
pub(crate) fn all_scores(repo: &str, commit: &str) -> BTreeMap<String, i64> {
    let cancel = pulse_bus::CancellationToken::new();
    let names = crate::score::starter_names();
    let results: Vec<(String, i64)> = std::thread::scope(|scope| {
        let handles: Vec<_> = names
            .iter()
            .map(|name| {
                let cancel = &cancel;
                scope.spawn(move || {
                    let (score, reason) =
                        crate::score::score_tree(cancel, repo, name, commit).expect("score tree");
                    assert_eq!(score == 1, reason.is_empty(), "{name}: {score} {reason}");
                    (name.clone(), score)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    results.into_iter().collect()
}

pub(crate) fn only(names: &[&str]) -> BTreeMap<String, i64> {
    let mut scores = BTreeMap::new();
    for name in crate::score::starter_names() {
        scores.insert(name, 0);
    }
    for name in names {
        scores.insert((*name).to_string(), 1);
    }
    scores
}

pub(crate) fn run_cycle_async(
    s: pulse_store::Store,
    cancel: pulse_bus::CancellationToken,
    once: bool,
    json: bool,
    interval: Duration,
) -> (
    pulse_cli::Buffer,
    Receiver<Result<()>>,
    std::thread::JoinHandle<()>,
) {
    let out = pulse_cli::Buffer::new();
    let writer = out.clone();
    let (tx, rx) = channel();
    let handle = thread::spawn(move || {
        let res = crate::cycle::cycle(
            &cancel,
            &s,
            "demo",
            crate::cycle::CycleOptions {
                once,
                agent: "tester".into(),
                out: Some(Box::new(writer)),
                json,
                interval,
                ..Default::default()
            },
        );
        let _ = tx.send(res);
    });
    (out, rx, handle)
}

pub(crate) fn chdir_guard(path: &str) -> PathBuf {
    let old = std::env::current_dir().expect("current dir");
    std::env::set_current_dir(path).expect("chdir");
    old
}

pub(crate) fn restore_chdir(old: PathBuf) {
    std::env::set_current_dir(old).expect("restore cwd");
}

pub(crate) fn path_exists(path: impl AsRef<Path>) -> bool {
    fs::symlink_metadata(path).is_ok()
}
