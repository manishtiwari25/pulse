#![allow(dead_code)]

use crate::api::{ArchiveRow, Scores};
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

pub(crate) static MEASURE_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static TEST_EXECUTABLE: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

pub(crate) fn measure_test_lock() -> MutexGuard<'static, ()> {
    MEASURE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn disable_project_hook() {
    pulse_board::set_on_project_created(None);
}

pub(crate) fn put_record(s: &pulse_store::Store, rec: pulse_store::Record, body: &str) {
    s.put_record(rec, BTreeMap::new(), body, "human", "fixture")
        .unwrap();
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
                project: project.into(),
                kind: "evolution".into(),
                slug: slug.into(),
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
            project: project.into(),
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
    .unwrap();
}

pub(crate) fn cycle_ready() -> pulse_store::Store {
    let s = crate::test_helpers::test_store();
    disable_project_hook();
    crate::test_helpers::test_project(&s, "demo");
    governance(&s, "demo");
    crate::test_helpers::fixture(&s, "demo", "F01", "printf 1", 1.0);
    baseline(&s, "demo");
    crate::test_helpers::private_set(&s);
    for name in [
        "tester",
        "proposer",
        "voter-one",
        "voter-two",
        "voter-three",
    ] {
        crate::test_helpers::join_test_agent(&s, name);
    }
    s.set_setting("human", "evolve.loop", "1").unwrap();
    s
}

pub(crate) fn voted_post_with_votes(
    s: &pulse_store::Store,
    input: pulse_board::PostInput,
    votes: &[&str],
) -> pulse_board::Post {
    let p = pulse_board::create_post(s, input).unwrap();
    for (i, vote) in votes.iter().enumerate() {
        let agent = ["voter-one", "voter-two", "voter-three"][i];
        pulse_board::create_post(
            s,
            pulse_board::PostInput {
                project: p.project.clone(),
                area: p.area.clone(),
                thread: p.thread.clone(),
                kind: "vote".into(),
                agent: agent.into(),
                summary: "independent evidence".into(),
                on: p.id.clone(),
                vote: (*vote).into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    p
}

pub(crate) fn voted_suggestion(
    s: &pulse_store::Store,
    thread: &str,
    class: &str,
    file: &str,
    votes: usize,
) -> pulse_board::Post {
    let votes = vec!["up"; votes];
    voted_post_with_votes(
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
        &votes,
    )
}

pub(crate) fn cardless_suggestion(
    s: &pulse_store::Store,
    thread: &str,
    summary: &str,
    votes: &[&str],
) -> pulse_board::Post {
    voted_post_with_votes(
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
        votes,
    )
}

pub(crate) fn finish_card_job(s: &pulse_store::Store, id: &str, status: &str) {
    s.conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET status = 'leased', lease_agent = 'card-writer' WHERE id = ?",
            [id],
        )
        .unwrap();
    pulse_board::finish_job(
        s,
        id,
        "card-writer",
        status,
        "SUMMARY: card posted",
        "",
        None,
    )
    .unwrap();
}

pub(crate) fn fake_archive_outcome(s: &pulse_store::Store, job_id: &str, kept: bool) -> Result<()> {
    let job = pulse_board::job_by_id(s, job_id)?
        .ok_or_else(|| anyhow!("fake runner could not read its queued job"))?;
    let suggestion = job
        .source
        .strip_prefix("suggestion:")
        .unwrap_or(&job.source)
        .to_string();
    if job.kind != "experiment"
        || !job.brief.contains("Hypothesis:")
        || !job.brief.contains("Suggestion:")
        || job.branch != format!("evolve/{suggestion}")
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
            project: job.project,
            suggestion_post: suggestion.clone(),
            hypothesis: "one constant improves the weighted score".into(),
            branch: format!("evolve/{suggestion}"),
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

pub(crate) fn fake_landing(s: &pulse_store::Store, job_id: &str, hold: Duration) -> Result<()> {
    fake_archive_outcome(s, job_id, true)?;
    std::thread::sleep(hold);
    pulse_board::finish_job(s, job_id, "tester", "done", "measured and kept", "", None)?;
    let archive = crate::fleet::archived_for_job(s, "demo", job_id)?
        .ok_or_else(|| anyhow!("fake runner could not read archive"))?;
    fake_finalize_archive(s, job_id, "land.landed", Some(&archive))
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
        [&current.run_id],
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
            serde_json::json!({"project": proof.project, "job": job_id, "archive": proof.id, "run_id": proof.run_id}),
        )?;
    }
    tx.commit()
}

pub(crate) fn put_constitution(s: &pulse_store::Store, project: &str, table: &str) {
    let body = format!(
        "# Constitution\n\n## Editable scope by stage\n\n{table}\n## Immutable paths\n\n| Path | Why |\n| --- | --- |\n| `protected.txt` | owner-owned |\n"
    );
    put_record(
        s,
        pulse_store::Record {
            id: format!("EVO-constitution-{project}"),
            project: project.into(),
            kind: "evolution".into(),
            slug: "constitution".into(),
            ..Default::default()
        },
        &body,
    );
}

pub(crate) const SCOPE_HEADER: &str = "| File class | Paths | Stage | Autonomy | Clean cycles required | Clean cycles so far | Incidents |\n| --- | --- | --- | --- | --- | --- | --- |\n";

pub(crate) fn heldout_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap()
}

pub(crate) fn heldout_file_state(path: impl AsRef<Path>) -> (String, u32) {
    let path = path.as_ref();
    let data = std::fs::read_to_string(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        (data, mode)
    }
    #[cfg(not(unix))]
    {
        (data, 0)
    }
}

pub(crate) fn ledger_stems(dir: impl AsRef<Path>) -> Vec<String> {
    let path = dir.as_ref().join(crate::private::GENERATED_LEDGER);
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::symlink_metadata(&path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "ledger mode {mode:o}, want 600");
    }
    data.split_whitespace().map(ToString::to_string).collect()
}

pub(crate) fn score_repo() -> (String, String) {
    let repo = crate::test_helpers::temp_path();
    crate::test_helpers::test_git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    crate::test_helpers::put_file(
        repo.join("README.md"),
        "# demo\n\nWe recieve requests.\n",
        0o644,
    );
    crate::test_helpers::put_file(
        repo.join("AGENTS.md"),
        "# Agents\n\nWarn before running unsandboxed.\n",
        0o644,
    );
    crate::test_helpers::put_file(repo.join("main.go"), "package main\n", 0o644);
    crate::test_helpers::put_file(repo.join("tests/main_test.go"), "package main\n", 0o644);
    crate::test_helpers::test_git(&repo, &["add", "."]);
    crate::test_helpers::test_git(&repo, &["commit", "--quiet", "-m", "start"]);
    let commit = crate::test_helpers::test_git(&repo, &["rev-parse", "HEAD"]);
    (repo.display().to_string(), commit)
}

pub(crate) fn test_pulse() -> String {
    let wrapper = crate::test_helpers::temp_path().join("measuring pulse");
    let body = "#!/bin/sh\nset -eu\nif [ \"$1\" != evolve ] || [ \"$2\" != score ] || [ \"$3\" != starter-hardstop-history ]; then echo 'the test pulse only runs evolve score' >&2; exit 2; fi\nhead=$(git rev-parse --verify --quiet HEAD^{commit}) || exit 2\nif [ \"$head\" != \"$PULSE_MEASURE_COMMIT\" ]; then echo 0; echo 'score 0: HEAD moved away from the measured commit' >&2; exit 0; fi\nchanges=$(git status --porcelain --untracked-files=normal | wc -l | tr -d ' ')\nif [ \"$changes\" != 0 ]; then echo 0; echo \"score 0: $changes changed path(s)\" >&2; exit 0; fi\necho 1\n";
    crate::test_helpers::put_file(&wrapper, body, 0o700);
    wrapper.display().to_string()
}

fn test_executable_hook() -> std::io::Result<String> {
    Ok(TEST_EXECUTABLE.lock().unwrap().clone())
}

pub(crate) fn use_test_executable(path: &str) {
    *TEST_EXECUTABLE.lock().unwrap() = path.to_string();
    crate::measure::set_executable(Some(test_executable_hook));
}

pub(crate) fn clear_test_executable() {
    crate::measure::set_executable(None);
    TEST_EXECUTABLE.lock().unwrap().clear();
}

#[test]
fn score_process_bridge() {
    let _serial = crate::test_helpers::serial();
    if std::env::var("PULSE_TEST_SCORE").ok().as_deref() != Some("1") {
        return;
    }
    let args = std::env::var("PULSE_TEST_SCORE_ARGS").unwrap_or_default();
    if let Ok(cwd) = std::env::var("PULSE_TEST_SCORE_CWD") {
        std::env::set_current_dir(cwd).unwrap();
    }
    let argv = crate::process::command_words(&args).unwrap_or_default();
    let code = if argv.first().map(String::as_str) != Some("evolve") {
        eprintln!("the test pulse only runs evolve score");
        2
    } else {
        let mut ctx = pulse_cli::Context {
            store: None,
            home: String::new(),
            out: Box::new(std::io::stdout()),
            err: Box::new(std::io::stderr()),
            json: false,
            quiet: false,
            color: false,
            agent: String::new(),
            project: String::new(),
            session: String::new(),
            now: String::new(),
            args: Vec::new(),
        };
        let fs = pulse_cli::FlagSet::new("evolve");
        match crate::commands::run_evolve(&mut ctx, &fs, &argv[1..]) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        }
    };
    std::process::exit(code);
}

pub(crate) fn run_judge(
    path: impl AsRef<Path>,
    repo: &str,
    commit: &str,
    pulse: &str,
) -> (String, String) {
    let home = crate::test_helpers::temp_path();
    let mut cmd = Command::new(path.as_ref());
    cmd.current_dir(repo);
    cmd.env_clear();
    for entry in crate::measure::scorer_env(&home.display().to_string(), commit, pulse) {
        if let Some((k, v)) = entry.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.env("PULSE_TEST_SCORE", "1");
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "judge: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

pub(crate) fn lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToString::to_string)
        .collect()
}

pub(crate) fn path_join(path: &str, name: &str) -> PathBuf {
    Path::new(path).join(name)
}
