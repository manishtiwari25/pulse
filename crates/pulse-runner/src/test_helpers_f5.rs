//! Test helpers ported from the Go test files owned by slice F5 (see files/RUNNER_TEST_WAVE.txt).
//! Other slices call these as `crate::test_helpers_f5::<go_name_in_snake_case>`.

#![allow(
    dead_code,
    reason = "shared F5 test helpers are consumed incrementally across slices"
)]

use crate::test_helpers::*;
use crate::util::{self, Row};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) const FAKE_PUSH_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nZmFrZSBrZXkgZm9yIHRlc3Rz\n-----END OPENSSH PRIVATE KEY-----\n";

/// Go: func setSetting(t *testing.T, s *store.Store, key, value string)
pub(crate) fn set_setting(s: &pulse_store::Store, key: &str, value: &str) {
    if let Err(err) = s.set_setting(util::HUMAN_AGENT, key, value) {
        panic!("setting {key}: {err}");
    }
}

/// Go: func skipSlow(t *testing.T)
///
/// Go skips these worker/daemon tests under `go test -short`; libtest has no -short, so the
/// full suite always runs them, as Go's does.
pub(crate) fn skip_slow() {}

/// Go: const defaultTestDeadline = 60 * time.Second
pub(crate) const DEFAULT_TEST_DEADLINE: Duration = Duration::from_secs(60);

/// Go: func testDeadline(t *testing.T, d time.Duration)
///
/// Fails the test with a stack note when it runs past d, so a worker clone whose spawned
/// git/worker subprocess blocks reports the stuck test by name instead of letting the whole
/// package hit the harness timeout. Go's version panics from a goroutine, which kills the test
/// process (exit 2); the guard here does the same once its deadline passes. Hold the returned
/// guard for the test's life: dropping it is Go's t.Cleanup closing `done`.
#[must_use = "bind the guard (`let _deadline = test_deadline(..)`) or the deadline is cancelled at once"]
pub(crate) fn test_deadline(d: Duration) -> TestDeadline {
    let d = test_wait(d);
    let name = std::thread::current().name().unwrap_or("test").to_string();
    let (done, wait) = crossbeam_channel::bounded::<()>(0);
    std::thread::spawn(move || {
        if wait.recv_timeout(d) == Err(crossbeam_channel::RecvTimeoutError::Timeout) {
            eprintln!("{name} ran past its {d:?} deadline");
            eprintln!("test {name} exceeded {d:?} deadline");
            std::process::exit(2);
        }
    });
    TestDeadline { _done: done }
}

pub(crate) struct TestDeadline {
    _done: crossbeam_channel::Sender<()>,
}

/// Go: func testWait(d time.Duration) time.Duration
pub(crate) fn test_wait(d: Duration) -> Duration {
    d.mul_f64(deadline_mult())
}

/// Go: func deadlineMult() float64
///
/// PULSE_TEST_DEADLINE_MULT (a positive float, e.g. "2.5"); unset or invalid means 1. Values
/// below 1 are clamped to 1 so a budget can only grow. (Go doubles the budget under -short.)
pub(crate) fn deadline_mult() -> f64 {
    if let Ok(v) = std::env::var("PULSE_TEST_DEADLINE_MULT")
        && !v.is_empty()
        && let Ok(f) = v.parse::<f64>()
        && f > 0.0
    {
        if f < 1.0 {
            return 1.0;
        }
        return f;
    }
    1.0
}

pub(crate) fn exec1(s: &pulse_store::Store, sql: &str, args: &[&dyn rusqlite::ToSql]) {
    s.conn().expect("conn").execute(sql, args).expect("exec1");
}

pub(crate) fn must_rows(
    s: &pulse_store::Store,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Vec<Row> {
    util::query_rows(&s.conn().expect("conn"), sql, args).expect("rows")
}

pub(crate) fn must_one(s: &pulse_store::Store, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Row {
    util::query_one(&s.conn().expect("conn"), sql, args)
        .expect("row")
        .unwrap_or_default()
}

pub(crate) fn job_row(s: &pulse_store::Store, id: &str) -> Row {
    crate::jobs::job_row(&s.conn().expect("conn"), id.to_string())
}

/// Go: fakeMeasure stands in for evolve.Measure: one public and one held-out score, or an error; the real Archive
/// still writes the row.
#[derive(Default)]
pub(crate) struct FakeMeasureState {
    pub total: f64,
    pub err: Option<anyhow::Error>,
    pub calls: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct FakeMeasure(pub Arc<Mutex<FakeMeasureState>>);

/// Go: t.Cleanup(func() { evolve.Measure = old })
pub(crate) struct MeasureGuard(Option<Box<pulse_evolve::api::MeasureFn>>);

impl Drop for MeasureGuard {
    fn drop(&mut self) {
        if let Some(old) = self.0.take() {
            let _ = pulse_evolve::set_measure(old);
        }
    }
}

impl FakeMeasure {
    /// Go: &fakeMeasure{total: .75} / &fakeMeasure{err: errors.New("...")}
    pub(crate) fn new(total: f64, err: Option<anyhow::Error>) -> Self {
        FakeMeasure(Arc::new(Mutex::new(FakeMeasureState {
            total,
            err,
            calls: Vec::new(),
        })))
    }

    /// Go: func (f *fakeMeasure) install(t *testing.T)
    pub(crate) fn install(&self) -> MeasureGuard {
        let f = self.clone();
        let old =
            pulse_evolve::set_measure(Box::new(move |_ctx, _s, project, repo, ref_name, _opts| {
                let mut f = f.0.lock().expect("fake measure");
                f.calls.push(format!("{project}@{ref_name}"));
                if let Some(err) = &f.err {
                    return Err(anyhow::anyhow!("{err:#}"));
                }
                Ok(pulse_evolve::Scores {
                    project: project.into(),
                    repo: repo.into(),
                    ref_name: ref_name.into(),
                    total: f.total,
                    heldout: 1,
                    sandbox: "fake-sandbox".into(),
                    fixtures: vec![
                        pulse_evolve::FixtureScore {
                            name: "starter-readme".into(),
                            kind: "public".into(),
                            score: f.total,
                            weight: 1.0,
                            ..Default::default()
                        },
                        pulse_evolve::FixtureScore {
                            name: "PRIVATE-NAME".into(),
                            kind: "heldout".into(),
                            score: 1.0,
                            weight: 1.0,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                })
            }));
        MeasureGuard(Some(old))
    }

    /// Go: measure.calls
    pub(crate) fn calls(&self) -> Vec<String> {
        self.0.lock().expect("fake measure").calls.clone()
    }

    /// Go: measure.err = ...
    pub(crate) fn set_err(&self, err: anyhow::Error) {
        self.0.lock().expect("fake measure").err = Some(err);
    }
}

pub(crate) fn baseline_jobs(s: &pulse_store::Store, slug: &str) -> Vec<Row> {
    must_rows(
        s,
        "SELECT * FROM jobs WHERE project = ? AND kind = 'baseline' ORDER BY created_at, rowid",
        &[&slug],
    )
}

pub(crate) fn loop_records(s: &pulse_store::Store, slug: &str) {
    for (name, body) in [
        ("constitution", "# Constitution\n\nloop: on\n"),
        ("evaluators", "Product scope: allowed\n"),
    ] {
        s.put_record(
            pulse_store::Record {
                id: format!("EVO-{name}-{slug}"),
                project: slug.to_string(),
                kind: "evolution".to_string(),
                slug: name.to_string(),
                ..Default::default()
            },
            BTreeMap::new(),
            body,
            util::HUMAN_AGENT,
            "fixture",
        )
        .expect("put loop record");
    }
}

pub(crate) fn only_baseline(s: &pulse_store::Store) {
    set_setting(s, "jobs_per_day", "none");
    let _ = crate::jobs::add_job(
        s,
        "proj".into(),
        "reflect".into(),
        "Daily reflect".into(),
        "done elsewhere".into(),
        0,
        1,
        format!("reflect:proj:{}", &TEST_NOW[..10]),
        "daemon".into(),
        true,
    )
    .expect("add reflect");
    exec1(
        s,
        "UPDATE jobs SET status = 'done', finished_at = ? WHERE kind = 'reflect'",
        &[&TEST_NOW],
    );
}

pub(crate) fn push_fixture(s: &pulse_store::Store) -> (Row, String, String) {
    let (proj, repo) = new_bare_repo(s);
    let bare = Path::new(&s.home)
        .join("tmp")
        .join(format!("origin-{}.git", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(bare.parent().unwrap()).expect("bare parent");
    let bare = bare.display().to_string();
    run(".", &["git", "init", "--bare", "-q", &bare]);
    run(&repo, &["git", "remote", "add", "origin", &bare]);
    run(&repo, &["git", "push", "-q", "-u", "origin", "main"]);
    (proj, repo, bare)
}

pub(crate) fn push_branch(s: &pulse_store::Store, proj: &Row, repo: &str, name: &str) -> Row {
    join(s, "worker");
    run(repo, &["git", "checkout", "-q", "-b", name, "main"]);
    std::fs::write(
        Path::new(repo).join(format!("{}.txt", name.replace('/', "_"))),
        format!("{name}\n"),
    )
    .expect("write branch file");
    run(repo, &["git", "add", "-A"]);
    run(
        repo,
        &["git", "commit", "-q", "-m", &format!("work on {name}")],
    );
    run(repo, &["git", "checkout", "-q", "main"]);
    let (j, _) = pulse_board::add_job(
        s,
        pulse_board::JobInput {
            project: proj.str("slug"),
            kind: "task".into(),
            title: format!("t {name}"),
            brief: "b".into(),
            tier: 1,
            created_by: "worker".into(),
            ..Default::default()
        },
    )
    .expect("add job");
    exec1(
        s,
        "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
        &[&TEST_NOW, &name, &j.id],
    );
    job_row(s, &j.id)
}

pub(crate) fn ref_of(dir: &str, r#ref: &str) -> String {
    run(dir, &["git", "rev-parse", r#ref]).trim().to_string()
}

pub(crate) fn bare_ref(bare: &str, r#ref: &str) -> String {
    run(".", &["git", "--git-dir", bare, "rev-parse", r#ref])
        .trim()
        .to_string()
}

pub(crate) fn key_file(mode: u32) -> String {
    let path = tempfile::tempdir()
        .expect("key temp")
        .keep()
        .join("push_key");
    std::fs::write(&path, FAKE_PUSH_KEY).expect("write key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod key");
    }
    path.display().to_string()
}

pub(crate) fn fake_ssh() -> (String, Vec<pulse_testutil::SetenvGuard>) {
    let dir = tempfile::tempdir().expect("ssh temp").keep();
    let record = dir.join("record");
    let script = r#"#!/bin/sh
printf '%s\n' "$@" > "$PULSE_TEST_SSH_RECORD"
key=""
prev=""
for a in "$@"; do
  if [ "$prev" = "-i" ]; then key="$a"; fi
  prev="$a"
done
if [ -n "$key" ]; then
  if [ ! -f "$key" ]; then echo "fake ssh: key $key missing" >&2; exit 255; fi
  mode=$(stat -f '%Lp' "$key" 2>/dev/null || stat -c '%a' "$key")
  echo "$mode" > "$PULSE_TEST_SSH_RECORD.mode"
  head -c 30 "$key" > "$PULSE_TEST_SSH_RECORD.head"
fi
for a in "$@"; do last="$a"; done
exec sh -c "$last"
"#;
    let ssh = dir.join("ssh");
    std::fs::write(&ssh, script).expect("write fake ssh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).expect("chmod ssh");
    }
    let mut paths = vec![dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let env = vec![
        pulse_testutil::setenv("PATH", std::env::join_paths(paths).expect("join path")),
        pulse_testutil::setenv("PULSE_TEST_SSH_RECORD", &record),
    ];
    (record.display().to_string(), env)
}

pub(crate) fn evolve_plan_ready(s: &pulse_store::Store) {
    set_setting(s, "evolve.loop", "1");
    set_setting(s, "quorum", "2");
    let constitution = "# Constitution\n\n## Editable scope\n| File class | Paths | Stage | Autonomy |\n| --- | --- | --- | --- |\n| operating | `README.md` | 2 | standing |\n\n## Immutable paths\n| Path | Why |\n| --- | --- |\n| `docs/evolution/**` | owner-owned |\n";
    for (slug, body) in [
        ("constitution", constitution),
        ("evaluators", "Product scope: off\n"),
    ] {
        s.put_record(
            pulse_store::Record {
                id: format!("EVO-{slug}-proj"),
                project: "proj".into(),
                kind: "evolution".into(),
                slug: slug.into(),
                ..Default::default()
            },
            BTreeMap::new(),
            body,
            util::HUMAN_AGENT,
            "fixture",
        )
        .expect("put evo record");
    }
    let dir = Path::new(&s.home).join("private").join("heldout-plan");
    std::fs::create_dir_all(&dir).expect("heldout dir");
    pulse_evolve::setup(s, &dir.display().to_string()).expect("heldout setup");
    set_setting(s, "evolve.heldout_dir", &dir.display().to_string());
    for (slot, judge) in [
        (1, "starter-hardstop-history"),
        (2, "starter-hardstop-publish"),
    ] {
        pulse_evolve::write_heldout(
            s,
            pulse_evolve::HeldoutWrite {
                request: "Please make the requested repository change without exposing private fixture text.".into(),
                judge: judge.into(),
                slot,
                ..Default::default()
            },
        )
        .expect("heldout write");
    }
    let scores = pulse_evolve::Scores {
        project: "proj".into(),
        total: 0.8,
        fixtures: vec![pulse_evolve::FixtureScore {
            name: "starter-readme".into(),
            kind: "public".into(),
            score: 0.8,
            weight: 1.0,
            ..Default::default()
        }],
        ..Default::default()
    };
    pulse_evolve::archive_row(
        s,
        pulse_evolve::ArchiveRow {
            project: "proj".into(),
            hypothesis: pulse_evolve::BASELINE_HYPOTHESIS.into(),
            branch: "main".into(),
            baseline: Some(scores.clone()),
            variant: Some(scores),
            tolerance: 0.02,
            kept: true,
            run_id: "run-baseline".into(),
            note: "baseline measurement".into(),
            ..Default::default()
        },
    )
    .expect("baseline archive");
    s.close().expect("close helper connections");
}

/// Go: func finishMeasuredExperiment(t *testing.T, s *store.Store, job row, kept bool) string
/// (evolve_feedback_test.go:12)
pub(crate) fn finish_measured_experiment(s: &pulse_store::Store, job: Row, kept: bool) -> String {
    let run_id = format!("run-{}", job.str("id"));
    exec1(
        s,
        "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES (?, ?, 'proposer', 'proj', ?, 'done')",
        &[&run_id, &job.str("id"), &TEST_NOW],
    );
    let mut variant = 0.7;
    if kept {
        variant = 0.9;
    }
    let (suggestion, hypothesis) = crate::experiment::experiment_suggestion(job.clone());
    let id = pulse_evolve::archive_row(
        s,
        pulse_evolve::ArchiveRow {
            project: "proj".into(),
            suggestion_post: suggestion,
            hypothesis,
            branch: job.str("branch"),
            baseline: Some(pulse_evolve::Scores {
                total: 0.8,
                ..Default::default()
            }),
            variant: Some(pulse_evolve::Scores {
                total: variant,
                ..Default::default()
            }),
            tolerance: 0.02,
            kept,
            run_id: run_id.clone(),
            note: "fake measured outcome".into(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|err| panic!("{err}"));
    exec1(
        s,
        "UPDATE jobs SET status = 'done', finished_at = ?, result = 'measured; finalization pending' WHERE id = ?",
        &[&TEST_NOW, &job.str("id")],
    );
    let mut action = "land.dropped";
    if kept {
        action = "land.landed";
    }
    let project = must_one(s, "SELECT * FROM projects WHERE slug = 'proj'", &[]);
    if let Err(err) = crate::landing_evidence::record_landing(
        s,
        project,
        job.str("branch"),
        "daemon".into(),
        action.into(),
        " | fake finalization".into(),
        crate::land::LandOpts {
            job: job.clone(),
            archive_id: id.clone(),
            run_id,
            ..Default::default()
        },
        BTreeMap::from([("fake".to_string(), serde_json::json!(true))]),
    ) {
        panic!("{err}");
    }
    id
}

pub(crate) fn evolve_suggestion(s: &pulse_store::Store) -> pulse_board::Post {
    for name in ["proposer", "voter-one", "voter-two"] {
        join(s, name);
    }
    let agent_row = |name: &str| pulse_store::Agent {
        name: name.into(),
        runner: "r".into(),
        model: "m".into(),
        kind: "worker".into(),
        ..Default::default()
    };
    let post = pulse_board::create_post(
        s,
        pulse_board::PostInput {
            kind: "suggestion".into(),
            project: "proj".into(),
            area: "evolution".into(),
            thread: "daemon-plan".into(),
            agent: "proposer".into(),
            summary: "Tune README".into(),
            files: "README.md".into(),
            body: "Class: operating-layer\nFile: README.md\nVariable: one constant\nHypothesis: README gets clearer\nTier: 1\n".into(),
            agent_row: Some(agent_row("proposer")),
            ..Default::default()
        },
    )
    .expect("suggestion");
    for agent in ["voter-one", "voter-two"] {
        pulse_board::create_post(
            s,
            pulse_board::PostInput {
                kind: "vote".into(),
                project: "proj".into(),
                area: "evolution".into(),
                thread: "daemon-plan".into(),
                agent: agent.into(),
                summary: "measured".into(),
                on: post.id.clone(),
                vote: "up".into(),
                agent_row: Some(agent_row(agent)),
                ..Default::default()
            },
        )
        .expect("vote");
    }
    post
}

pub(crate) type MeasureCalls = Arc<Mutex<i32>>;

pub(crate) fn measured_keep_fixture() -> (
    crate::test_helpers::TestStore,
    Row,
    Row,
    String,
    MeasureCalls,
) {
    let s = new_store();
    let (mut project, root) = new_repo(&s);
    evolve_plan_ready(&s);
    evolve_suggestion(&s);
    let planned = pulse_evolve::plan_experiment(
        &s,
        "proj",
        pulse_evolve::PlanOptions {
            agent: "voter-one".into(),
            agent_row: Some(pulse_store::Agent {
                name: "voter-one".into(),
                runner: "r".into(),
                model: "m".into(),
                kind: "worker".into(),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .expect("plan experiment");
    assert!(planned.created, "planned fake experiment: {planned:?}");
    let job = job_row(&s, &planned.job_id);
    run(&root, &["git", "switch", "-q", "-c", &job.str("branch")]);
    std::fs::write(Path::new(&root).join("README.md"), "# measured variant\n").expect("variant");
    run(
        &root,
        &["git", "commit", "-q", "-am", "fake experiment variant"],
    );
    run(&root, &["git", "switch", "-q", "main"]);
    exec1(
        &s,
        "INSERT INTO runs (id, job_id, project, agent, started_at, status) VALUES (?, ?, 'proj', 'proposer', ?, 'done')",
        &[&format!("run-{}", job.str("id")), &job.str("id"), &TEST_NOW],
    );
    exec1(
        &s,
        "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?",
        &[&TEST_NOW, &job.str("id")],
    );
    project.insert("verify_cmd", serde_json::json!("printf verified"));
    exec1(
        &s,
        "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
        &[&project.str("verify_cmd")],
    );
    let calls = Arc::new(Mutex::new(0));
    let calls2 = calls.clone();
    let _old = pulse_evolve::set_measure(Box::new(move |_ctx, _s, slug, _repo, r#ref, _opts| {
        *calls2.lock().unwrap() += 1;
        let score = if r#ref == "main" { 0.8 } else { 0.9 };
        Ok(pulse_evolve::Scores {
            project: slug.into(),
            total: score,
            ..Default::default()
        })
    }));
    (
        s.clone(),
        project,
        job_row(&s, &planned.job_id),
        root,
        calls,
    )
}

pub(crate) fn defer_measured_keep(s: &pulse_store::Store, project: Row, job: Row, root: &str) {
    set_setting(s, "landing.auto_stash", "off");
    std::fs::write(
        Path::new(root).join("README.md"),
        "# unrelated local edit\n",
    )
    .expect("local edit");
    let (state, note) =
        crate::experiment::land_experiment(s, project, job, "daemon".into(), TEST_NOW.into(), true);
    assert!(
        state == "deferred" && note.contains("archive B-002"),
        "fake measured keep did not defer landing: {state}: {note}"
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn deadline_mult() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for (env, want) in [
            ("3", 3.0),
            ("2.5", 2.5),
            ("0.5", 1.0),
            ("0", 0.0),
            ("abc", 0.0),
        ] {
            let _env = pulse_testutil::setenv("PULSE_TEST_DEADLINE_MULT", env);
            let mut want = want;
            if want == 0.0 {
                want = 1.0;
            }
            let got = super::deadline_mult();
            assert_eq!(
                got, want,
                "PULSE_TEST_DEADLINE_MULT={env:?}: got {got} want {want}"
            );
        }
    }
}
