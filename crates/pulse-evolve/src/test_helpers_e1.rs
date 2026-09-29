use crate::api::{ArchiveRow, Scores};
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

pub(crate) static E1_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(crate) fn e1_test_lock() -> MutexGuard<'static, ()> {
    E1_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) struct EnvVarGuard {
    key: &'static str,
    old: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    pub(crate) fn set_path(value: impl AsRef<std::path::Path>) -> Self {
        let old = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", value.as_ref()) };
        Self { key: "PATH", old }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(old) = &self.old {
                std::env::set_var(self.key, old);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
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
    crate::test_helpers::private_set(&s);
    baseline(&s, "demo");
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

pub(crate) fn voted_suggestion_with_votes(
    s: &pulse_store::Store,
    thread: &str,
    class: &str,
    file: &str,
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
            summary: format!("Improve {thread}"),
            files: file.into(),
            body: format!(
                "Class: {class}\nFile: {file}\nVariable: one constant\nHypothesis: {thread} improves the weighted score\nTier: 1\n"
            ),
            ..Default::default()
        },
        votes,
    )
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
    .unwrap();
    for agent in ["voter-one", "voter-two", "voter-three"]
        .into_iter()
        .take(votes)
    {
        pulse_board::create_post(
            s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: "evolution".into(),
                thread: thread.into(),
                kind: "vote".into(),
                agent: agent.into(),
                summary: "independent evidence".into(),
                on: p.id.clone(),
                vote: "up".into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    p
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

pub(crate) fn suggestion_status(s: &pulse_store::Store, id: &str) -> pulse_tally::Suggestion {
    let result = pulse_tally::compute(s, "demo", "", pulse_tally::Thresholds::default()).unwrap();
    result
        .suggestions
        .into_iter()
        .find(|sg| sg.id == id)
        .unwrap_or_else(|| panic!("suggestion {id} not in the tally"))
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

pub(crate) fn complete(body: &str) -> String {
    body.replace("<product paths, e.g. `src/`>", "`cmd/`, `internal/`")
}

pub(crate) fn seed_templates(s: &pulse_store::Store) {
    let previous = pulse_board::set_on_project_created(None);
    let repo = tempfile::tempdir().unwrap();
    crate::test_helpers::test_git(repo.path(), &["init", "--quiet", "--initial-branch=main"]);
    crate::test_helpers::put_file(repo.path().join("README.md"), "# brain\n", 0o644);
    crate::test_helpers::test_git(repo.path(), &["add", "."]);
    crate::test_helpers::test_git(repo.path(), &["commit", "--quiet", "-m", "start"]);
    pulse_board::add_project(
        s,
        repo.path().to_str().unwrap(),
        "brain",
        "",
        true,
        "",
        "human",
        "brain",
    )
    .unwrap();
    for slug in ["constitution", "evaluators"] {
        let title = format!(
            "<Project> Evolution {}{}",
            &slug[..1].to_uppercase(),
            &slug[1..]
        );
        let mut front = BTreeMap::new();
        front.insert("id".into(), serde_json::json!(slug.to_uppercase()));
        front.insert("title".into(), serde_json::json!(title.clone()));
        front.insert("status".into(), serde_json::json!("Active"));
        front.insert("loop".into(), serde_json::json!("off"));
        front.insert("limits".into(), serde_json::json!("none"));
        if slug == "evaluators" {
            front.insert("baseline".into(), serde_json::json!("pending"));
        }
        let path = format!("docs/evolution/_template-{slug}.md");
        let rec = pulse_store::Record {
            id: format!("TPL-evolution-{slug}"),
            project: "brain".into(),
            kind: "template".into(),
            slug: format!("evolution-{slug}"),
            title: title.clone(),
            status: "Active".into(),
            path: path.clone(),
            origin: format!("markdown:{path}@0000000"),
            ..Default::default()
        };
        s.put_record(
            rec,
            front,
            &format!(
                "# {title}\n\nWhat the <Project> loop may change: <product paths, e.g. `src/`>.\n"
            ),
            "human",
            &format!("import {path}"),
        )
        .unwrap();
    }
    pulse_board::set_on_project_created(previous);
}

pub(crate) fn seeded_project(s: &pulse_store::Store, slug: &str) {
    let repo = tempfile::tempdir().unwrap();
    crate::test_helpers::test_git(repo.path(), &["init", "--quiet", "--initial-branch=main"]);
    crate::test_helpers::put_file(repo.path().join("README.md"), &format!("# {slug}\n"), 0o644);
    crate::test_helpers::test_git(repo.path(), &["add", "."]);
    crate::test_helpers::test_git(repo.path(), &["commit", "--quiet", "-m", "start"]);
    pulse_board::add_project(
        s,
        repo.path().to_str().unwrap(),
        slug,
        "",
        false,
        "make check",
        "alice",
        slug,
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
    s.event(
        "tester",
        "land.landed",
        &archive.branch,
        serde_json::json!({"project": "demo", "job": job_id, "run_id": archive.run_id, "archive": archive.id}),
    )?;
    Ok(())
}
