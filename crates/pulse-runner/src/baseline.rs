#![allow(
    dead_code,
    reason = "ported Go runner APIs are consumed incrementally across slices"
)]
#![allow(
    clippy::too_many_arguments,
    clippy::unused_unit,
    reason = "Go-port signatures intentionally mirror internal/runner"
)]

use std::time::Duration;

pub(crate) const BASELINE_TITLE: &str = "Record baseline";
pub(crate) const BASELINE_PRIORITY: i32 = 20;
pub(crate) const BASELINE_THREAD: &str = "baseline";

/// Go: func baselineSource(slug string) string
pub(crate) fn baseline_source(slug: String) -> String {
    format!("baseline:{slug}")
}

/// Go: func enabledFixtures(s *store.Store, slug string) int
pub(crate) fn enabled_fixtures(s: &pulse_store::Store, slug: String) -> i32 {
    let Ok(conn) = s.conn() else { return 0 };
    crate::util::must_one(
        &conn,
        "SELECT COUNT(*) AS n FROM evolution_fixtures WHERE project = ? AND enabled = 1",
        &[&slug],
    )
    .int("n")
}

/// Go: func baselineDue(s *store.Store, slug, now string) (due bool, why string)
pub(crate) fn baseline_due(s: &pulse_store::Store, slug: String, now: String) -> (bool, String) {
    let why = crate::run::runner_stopped(s);
    if !why.is_empty() {
        return (false, why);
    }
    if s.setting("evolve.loop", "") != "1" {
        return (false, "evolve.loop is off".into());
    }
    let status = match pulse_evolve::status(s, &slug) {
        Ok(v) => v,
        Err(e) => {
            return (
                false,
                format!("evolve status unavailable: {}", crate::run::err_message(&e)),
            );
        }
    };
    if !status.constitution {
        return (false, "no EVO-constitution record".into());
    }
    if !status.evaluators {
        return (false, "no EVO-evaluators record".into());
    }
    if status.public_fixtures == 0 {
        return (false, "no enabled public fixture".into());
    }
    if status.baseline.is_some() {
        return (
            false,
            format!("baseline recorded (archive {})", status.baseline_id),
        );
    }
    if let Some(job) = status.baseline_job {
        return (false, format!("baseline job {} is {}", job.id, job.status));
    }
    match pulse_evolve::recent_baseline_failure(s, &slug, &now) {
        Ok(failed_at) if !failed_at.is_empty() => (
            false,
            format!(
                "baseline measurement failed at {failed_at}; the daemon retries after {} hours",
                pulse_evolve::BASELINE_RETRY_HOURS
            ),
        ),
        Ok(_) => (true, String::new()),
        Err(e) => (
            false,
            format!("baseline jobs unreadable: {}", crate::run::err_message(&e)),
        ),
    }
}

/// Go: func planBaseline(s *store.Store, project row, slug, now, createdBy string) (row, bool, error)
pub(crate) fn plan_baseline(
    s: &pulse_store::Store,
    project: crate::util::Row,
    slug: String,
    now: String,
    created_by: String,
) -> anyhow::Result<(crate::util::Row, bool)> {
    if !baseline_due(s, slug.clone(), now).0 {
        return Ok((crate::util::Row::new(), false));
    }
    let def = default_branch(project);
    let fixtures = enabled_fixtures(s, slug.clone());
    let brief = format!(
        "Record the baseline of {slug} (ADR-031). The runner measures the default branch `{def}` itself with the {fixtures} enabled public fixture(s) and every private held-out fixture through evolve.Measure, then archives one kept row (hypothesis `{}`) that `pulse evolve status` reads as the baseline. No worker runs, no clone is prepared and nothing lands. A failed measurement posts one finding on evolution/{BASELINE_THREAD} and the daemon plans the job again after {} hours; `pulse job add --project {slug} --kind baseline --title '{BASELINE_TITLE}'` queues it sooner.",
        pulse_evolve::BASELINE_HYPOTHESIS,
        pulse_evolve::BASELINE_RETRY_HOURS
    );
    crate::jobs::add_job(
        s,
        slug.clone(),
        "baseline".into(),
        BASELINE_TITLE.into(),
        brief,
        0,
        BASELINE_PRIORITY,
        baseline_source(slug),
        created_by,
        true,
    )
}

/// Go: func defaultBranch(project row) string
pub(crate) fn default_branch(project: crate::util::Row) -> String {
    let d = project.str("default_branch");
    if d.is_empty() { "main".into() } else { d }
}

/// Go: func baselineBudget(s *store.Store, slug string) time.Duration
pub(crate) fn baseline_budget(s: &pulse_store::Store, slug: String) -> std::time::Duration {
    let minutes = crate::util::setting_int(s, "minutes_per_job").max(1);
    let fixtures = enabled_fixtures(s, slug) + pulse_evolve::heldout_count(s).count as i32;
    let mut budget = Duration::from_secs(((fixtures * minutes + 5).max(1) as u64) * 60);
    let rb = crate::daemon::run_budget(s);
    if budget < rb {
        budget = rb;
    }
    if budget > crate::experiment::EXPERIMENT_MEASURE_BUDGET {
        budget = crate::experiment::EXPERIMENT_MEASURE_BUDGET;
    }
    budget
}

/// Go: func jobBudget(s *store.Store, job row) time.Duration
pub(crate) fn job_budget(s: &pulse_store::Store, job: crate::util::Row) -> std::time::Duration {
    if job.str("kind") == "baseline" {
        baseline_budget(s, job.str("project"))
    } else {
        crate::daemon::run_budget(s)
    }
}

/// Go: func runBaseline(s *store.Store, job, project row, agent, now, runID, logPath string, ex executor, o runOpts) (row, error)
pub(crate) fn run_baseline(
    s: &pulse_store::Store,
    job: crate::util::Row,
    project: crate::util::Row,
    agent: String,
    now: String,
    run_id: String,
    log_path: String,
    ex: serde_json::Value,
    o: crate::run::RunOpts,
) -> anyhow::Result<crate::util::Row> {
    let slug = project.str("slug");
    let def = default_branch(project.clone());
    let fixtures = enabled_fixtures(s, slug.clone());
    let mut log = crate::exec::RunLog::default();
    let mut status = "failed".to_string();
    let summary: String;
    let mut archive_id = String::new();
    let mut scores = None;
    let mut restore_attempt = false;
    let ex_name = ex.as_str().unwrap_or("").to_string();
    let stopped = crate::run::runner_stopped(s);
    if o.dry_run {
        status = "dry-run".to_string();
        summary = format!("dry run: would measure {def} with {fixtures} fixture(s)");
        log.put(&summary);
    } else if !stopped.is_empty() {
        status = "error".to_string();
        restore_attempt = true;
        summary = format!("not measured: {stopped}");
        log.put(&summary);
    } else {
        let budget = baseline_budget(s, slug.clone());
        let lease_minutes = (budget.as_secs() / 60) as i32 + 5;
        let renewed = s.conn().ok().is_some_and(|conn| {
            crate::jobs::renew_lease(
                &conn,
                now.clone(),
                job.str("id"),
                agent.clone(),
                job.int("generation"),
                lease_minutes,
            )
        });
        if !renewed {
            status = "stale".to_string();
            summary = format!(
                "lease generation {} is no longer current; nothing measured",
                job.int("generation")
            );
            log.put(&summary);
        } else {
            let ctx = o
                .cancel
                .with_timeout(crate::experiment::EXPERIMENT_MEASURE_BUDGET);
            log.add(format!(
                "measuring {def} of {slug} with {fixtures} enabled public fixture(s) and {} held out; budget {:?}",
                pulse_evolve::heldout_count(s).count,
                budget
            ));
            match pulse_evolve::measure(
                &ctx,
                s,
                &slug,
                &project.str("path"),
                &def,
                pulse_evolve::MeasureOptions {
                    out: Some(&mut log),
                    ..Default::default()
                },
            ) {
                Err(e) => {
                    if o.cancel.is_cancelled() {
                        status = "error".to_string();
                        restore_attempt = true;
                        summary = format!(
                            "not measured: the daemon stopped ({})",
                            crate::run::err_message(&e)
                        );
                    } else if e.to_string() == pulse_evolve::err_not_implemented().to_string() {
                        summary = format!(
                            "measuring {def} failed: evolve.Measure is not implemented ({})",
                            crate::run::err_message(&e)
                        )
                    } else {
                        summary = format!("measuring {def} failed: {}", crate::run::err_message(&e))
                    };
                    log.put(&summary);
                }
                Ok(sc) => match pulse_evolve::archive_row(
                    s,
                    pulse_evolve::ArchiveRow {
                        project: slug.clone(),
                        hypothesis: pulse_evolve::BASELINE_HYPOTHESIS.into(),
                        branch: def.clone(),
                        baseline: Some(sc.clone()),
                        variant: Some(sc.clone()),
                        tolerance: crate::experiment::evolve_tolerance(s),
                        kept: true,
                        run_id: run_id.clone(),
                        sandbox: sc.sandbox.clone(),
                        note: "baseline measurement".into(),
                        ..Default::default()
                    },
                ) {
                    Ok(id) => {
                        status = "done".to_string();
                        archive_id = id;
                        summary = format!(
                            "baseline {}; archive {archive_id}",
                            crate::experiment::score_line(&sc)
                        );
                        scores = Some(sc);
                        log.put(&summary);
                    }
                    Err(e) => {
                        status = "failed".to_string();
                        summary = format!(
                            "archiving the baseline of {def} failed: {}",
                            crate::run::err_message(&e)
                        );
                        log.put(&summary);
                    }
                },
            }
        }
    }
    let _ = std::fs::write(&log_path, log.text());
    if let Ok(conn) = s.conn() {
        crate::util::exec1(
            &conn,
            "UPDATE runs SET finished_at = ?, status = ?, seconds = ?, branch = '', commits = 0, summary = ? WHERE id = ?",
            &[&s.now(), &status, &0.0_f64, &summary, &run_id],
        );
        let job_status = match status.as_str() {
            "done" => "done",
            "dry-run" => "queued",
            "error" if restore_attempt => {
                crate::util::exec1(
                    &conn,
                    "UPDATE jobs SET attempts = MAX(attempts - 1, 0) WHERE id = ?",
                    &[&job.str("id")],
                );
                "queued"
            }
            "stale" => {
                crate::util::event_at(
                    &conn,
                    &now,
                    &agent,
                    "run.finish",
                    &run_id,
                    serde_json::json!({"status": status, "commits": 0, "branch": "", "executor": ex_name}),
                );
                return Ok(crate::util::must_one(
                    &conn,
                    "SELECT * FROM runs WHERE id = ?",
                    &[&run_id],
                ));
            }
            _ => "failed",
        };
        let _ = crate::jobs::finish_job(
            s,
            job.str("id"),
            agent.clone(),
            job_status.into(),
            summary.clone(),
            String::new(),
            job.int("generation"),
        );
        if job_status == "done" {
            crate::util::event_at(
                &conn,
                &now,
                &agent,
                "evolution.baseline",
                &archive_id,
                serde_json::json!({"project": slug, "archive": archive_id, "score": scores.as_ref().map(|s| s.total).unwrap_or(0.0), "run": run_id, "job": job.str("id")}),
            );
        }
        crate::util::event_at(
            &conn,
            &now,
            &agent,
            "run.finish",
            &run_id,
            serde_json::json!({"status": status, "commits": 0, "branch": "", "executor": ex_name}),
        );
        Ok(crate::util::must_one(
            &conn,
            "SELECT * FROM runs WHERE id = ?",
            &[&run_id],
        ))
    } else {
        Ok(crate::util::Row::new())
    }
}

/// Go: func baselineFinding(s *store.Store, slug, def, cause, logPath, runID, now string, arow *store.Agent, log *runLog)
pub(crate) fn baseline_finding(
    s: &pulse_store::Store,
    slug: String,
    def: String,
    cause: String,
    log_path: String,
    run_id: String,
    now: String,
    arow: &pulse_store::Agent,
    _log: &crate::exec::RunLog,
) -> () {
    let summary = crate::util::trunc(
        &format!("baseline for {slug} failed: {cause}")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        230,
    );
    let Ok(conn) = s.conn() else { return };
    if !crate::util::must_one(&conn, "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'finding' AND area = 'evolution' AND thread = ? AND summary = ?", &[&slug, &BASELINE_THREAD, &summary]).0.is_empty() {
        return;
    }
    let body = format!(
        "The daemon could not record the baseline of {slug} (ADR-031).\n\n- Cause: {cause}\n- Measured: `{def}`\n- Run: `{run_id}`\n- Log: {log_path}\n\nThe daemon retries after {} hours; a human may run `pulse job add --project {slug} --kind baseline --title '{BASELINE_TITLE}'` sooner.\n",
        pulse_evolve::BASELINE_RETRY_HOURS
    );
    let _ = crate::board::create_post(
        s,
        now,
        crate::board::PostOpts {
            kind: "finding".into(),
            project: slug,
            area: "evolution".into(),
            thread: BASELINE_THREAD.into(),
            summary,
            body,
            agent: Some(Box::new(arow.clone())),
            session: run_id,
            branch: Some(Box::new(def)),
            ..Default::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::*;

    fn install_fake_measure(total: f64) -> Box<pulse_evolve::api::MeasureFn> {
        pulse_evolve::set_measure(Box::new(move |_ctx, _s, project, repo, ref_name, _opts| {
            Ok(pulse_evolve::Scores {
                project: project.into(),
                repo: repo.into(),
                ref_name: ref_name.into(),
                total,
                heldout: 1,
                sandbox: "fake-sandbox".into(),
                fixtures: vec![
                    pulse_evolve::FixtureScore {
                        name: "starter-readme".into(),
                        kind: "public".into(),
                        score: total,
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
        }))
    }

    #[test]
    fn baseline_planning() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        new_repo(&s);
        let not_due = |want: &str| {
            let (due, why) = baseline_due(&s, "proj".into(), TEST_NOW.into());
            assert!(
                !due && why.contains(want),
                "baselineDue = {due} {why:?}, want {want:?}"
            );
            let jobs = crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into())
                .unwrap();
            assert!(
                !jobs.iter().any(|j| j.str("kind") == "baseline"),
                "baseline job planned although {want}: {jobs:?}"
            );
            exec1(
                &s,
                "DELETE FROM jobs WHERE project = 'proj' AND kind != 'baseline'",
                &[],
            );
        };
        not_due("evolve.loop is off");
        set_setting(&s, "evolve.loop", "1");
        not_due("no EVO-constitution record");
        loop_records(&s, "proj");
        assert_eq!(
            baseline_due(&s, "proj".into(), TEST_NOW.into()),
            (true, String::new())
        );
        let jobs =
            crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into()).unwrap();
        assert_eq!(jobs.len(), 1, "planned {jobs:?}");
        let job = &jobs[0];
        assert_eq!(job.str("kind"), "baseline");
        assert_eq!(job.str("title"), BASELINE_TITLE);
        assert_eq!(job.int("tier"), 0);
        assert_eq!(job.int("priority"), BASELINE_PRIORITY);
        assert_eq!(job.str("source"), "baseline:proj");
        assert_eq!(job.str("created_by"), "daemon");
        assert_eq!(job.str("status"), "queued");
        for want in [
            "default branch `main`",
            "6 enabled public fixture(s)",
            "No worker runs",
            "evolve.Measure",
            "after 24 hours",
            "pulse job add --project proj --kind baseline --title 'Record baseline'",
        ] {
            assert!(
                job.str("brief").contains(want),
                "brief lacks {want:?}:\n{}",
                job.str("brief")
            );
        }
        not_due(&format!("baseline job {} is queued", job.str("id")));
        assert_eq!(baseline_jobs(&s, "proj").len(), 1);
        crate::jobs::finish_job(
            &s,
            job.str("id"),
            "aurora".into(),
            "failed".into(),
            "measuring main failed".into(),
            String::new(),
            -1,
        )
        .unwrap();
        not_due(&format!(
            "failed at {TEST_NOW}; the daemon retries after 24 hours"
        ));
        assert!(!baseline_due(&s, "proj".into(), crate::util::plus_hours(TEST_NOW, 23.5)).0);
        let later = crate::util::plus_hours(TEST_NOW, 24.0);
        assert!(
            baseline_due(&s, "proj".into(), later.clone()).0,
            "not due after 24 hours"
        );
        let jobs = crate::plan::plan_jobs(&s, "proj".into(), later, "daemon".into()).unwrap();
        assert_eq!(jobs.len(), 1, "planned after 24 hours: {jobs:?}");
        assert_eq!(jobs[0].str("kind"), "baseline");
        assert_ne!(jobs[0].str("id"), job.str("id"));
        assert_eq!(baseline_jobs(&s, "proj").len(), 2);
        crate::jobs::finish_job(
            &s,
            jobs[0].str("id"),
            pulse_store::HUMAN_AGENT.into(),
            "cancelled".into(),
            "not now".into(),
            String::new(),
            -1,
        )
        .unwrap();
        assert!(baseline_due(&s, "proj".into(), TEST_NOW.into()).0);
        set_setting(&s, "stop", "1");
        not_due("stopped");
        set_setting(&s, "stop", "0");
        exec1(
            &s,
            "UPDATE evolution_fixtures SET enabled = 0 WHERE project = 'proj'",
            &[],
        );
        not_due("no enabled public fixture");
        exec1(
            &s,
            "UPDATE evolution_fixtures SET enabled = 1 WHERE project = 'proj'",
            &[],
        );
        let scores = pulse_evolve::Scores {
            project: "proj".into(),
            total: 1.0,
            fixtures: vec![pulse_evolve::FixtureScore {
                name: "starter-readme".into(),
                kind: "public".into(),
                score: 1.0,
                weight: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        pulse_evolve::archive_row(
            &s,
            pulse_evolve::ArchiveRow {
                project: "proj".into(),
                hypothesis: pulse_evolve::BASELINE_HYPOTHESIS.into(),
                branch: "main".into(),
                baseline: Some(scores.clone()),
                variant: Some(scores),
                tolerance: 0.02,
                kept: true,
                run_id: "run-x".into(),
                note: "baseline measurement".into(),
                ..Default::default()
            },
        )
        .unwrap();
        not_due("baseline recorded (archive B-001)");
        let brain_dir = tempfile::tempdir().expect("brain");
        let brain = brain_dir.path().join("brain");
        std::fs::create_dir_all(&brain).unwrap();
        let brain = brain.display().to_string();
        run(&brain, &["git", "init", "-q", "-b", "trunk"]);
        std::fs::write(std::path::Path::new(&brain).join("README.md"), "# brain\n").unwrap();
        run(&brain, &["git", "add", "-A"]);
        run(&brain, &["git", "commit", "-q", "-m", "init"]);
        exec1(
            &s,
            "INSERT INTO projects (slug, name, path, brain, default_branch, added_at, added_by) VALUES ('brain', 'Brain', ?, 1, 'trunk', ?, ?)",
            &[&brain, &TEST_NOW, &pulse_store::HUMAN_AGENT],
        );
        loop_records(&s, "brain");
        let jobs =
            crate::plan::plan_jobs(&s, "brain".into(), TEST_NOW.into(), "daemon".into()).unwrap();
        assert!(
            jobs.len() == 1
                && jobs[0].str("kind") == "baseline"
                && jobs[0].str("source") == "baseline:brain"
                && jobs[0].str("brief").contains("default branch `trunk`"),
            "brain baseline job: {jobs:?}"
        );
    }

    #[test]
    fn baseline_budget() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        new_repo(&s);
        seed_public_fixtures(&s, "proj");
        set_setting(&s, "minutes_per_job", "10");
        assert_eq!(super::baseline_budget(&s, "proj".into()).as_secs() / 60, 65);
        let mut job = crate::util::Row::new();
        job.insert("kind", serde_json::json!("baseline"));
        job.insert("project", serde_json::json!("proj"));
        assert_eq!(
            job_budget(&s, job),
            super::baseline_budget(&s, "proj".into())
        );
        set_setting(&s, "minutes_per_job", "30");
        assert_eq!(
            super::baseline_budget(&s, "proj".into()),
            crate::experiment::EXPERIMENT_MEASURE_BUDGET
        );
        exec1(
            &s,
            "UPDATE evolution_fixtures SET enabled = 0 WHERE project = 'proj'",
            &[],
        );
        assert_eq!(
            super::baseline_budget(&s, "proj".into()),
            crate::daemon::run_budget(&s)
        );
    }

    #[test]
    fn baseline_run_records_the_baseline() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (project, _) = new_repo(&s);
        join(&s, "aurora");
        loop_records(&s, "proj");
        let old = install_fake_measure(0.75);
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "baseline".into(),
            BASELINE_TITLE.into(),
            "brief".into(),
            0,
            BASELINE_PRIORITY,
            baseline_source("proj".into()),
            "daemon".into(),
            true,
        )
        .unwrap();
        exec1(
            &s,
            "UPDATE jobs SET status = 'leased', lease_agent = 'aurora' WHERE id = ?",
            &[&job.str("id")],
        );
        let log_path = std::path::Path::new(&s.home)
            .join("logs")
            .join("baseline.log")
            .display()
            .to_string();
        exec1(
            &s,
            "INSERT INTO runs (id, job_id, project, agent, started_at, status, log, executor) VALUES ('run-baseline', ?, 'proj', 'aurora', ?, 'running', ?, 'local')",
            &[&job.str("id"), &TEST_NOW, &log_path],
        );
        let run = run_baseline(
            &s,
            job.clone(),
            project,
            "aurora".into(),
            TEST_NOW.into(),
            "run-baseline".into(),
            log_path.clone(),
            serde_json::Value::Null,
            crate::run::RunOpts::default(),
        )
        .unwrap();
        let _ = pulse_evolve::set_measure(old);
        assert_eq!(run.str("status"), "done");
        let job = job_row(&s, &job.str("id"));
        assert_eq!(
            job.str("result"),
            "baseline 0.750 over 2 fixture(s), 1 held out; archive B-001"
        );
        let archive = must_one(
            &s,
            "SELECT * FROM evolution_archive WHERE project = 'proj'",
            &[],
        );
        assert_eq!(archive.str("hypothesis"), "baseline");
        assert_eq!(archive.str("branch"), "main");
        assert_eq!(archive.str("id"), "B-001");
        assert_eq!(archive.str("run_id"), "run-baseline");
        assert_eq!(archive.float("baseline_score"), 0.75);
        assert!(!archive.str("fixtures_json").contains("PRIVATE-NAME"));
        let ev = must_one(
            &s,
            "SELECT * FROM events WHERE action = 'evolution.baseline'",
            &[],
        );
        assert_eq!(ev.str("subject"), "B-001");
        for want in [
            "\"project\": \"proj\"",
            "\"archive\": \"B-001\"",
            "\"score\": 0.75",
            "\"run\": \"run-baseline\"",
            &format!("\"job\": \"{}\"", job.str("id")),
        ] {
            assert!(
                ev.str("detail").contains(want),
                "event detail lacks {want}: {}",
                ev.str("detail")
            );
        }
        let posts = crate::util::must_rows(
            &*s.conn().unwrap(),
            "SELECT * FROM posts WHERE project = 'proj' AND area = 'jobs'",
            &[],
        );
        assert!(posts.len() <= 1, "result post: {posts:?}");
        if let Some(post) = posts.first() {
            assert_eq!(post.str("kind"), "result");
            assert_eq!(post.str("resolution"), "tested");
            assert_eq!(post.str("summary"), format!("done: {}", job.str("result")));
            let body = post.str("body");
            for want in [
                "(baseline, Tier 0)",
                "**done**",
                "- Score: 0.750 over 2 fixture(s), 1 held out",
                "- starter-readme: 0.750 (weight 1)",
                "- Archive: B-001 (kept;",
                "- Commits: 0 (a baseline job lands nothing)",
                "- Log:",
            ] {
                assert!(body.contains(want), "result body lacks {want:?}:\n{body}");
            }
            assert!(!body.contains("PRIVATE-NAME"));
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'evolution.baseline'",
                &[]
            ),
            1
        );

        let baseline_parity_probe = r###""archive": "B-001" "job": " "project": "proj" "run": " "score": 0.75 : Record baseline SELECT * FROM runs WHERE job_id = ? SELECT COUNT(*) AS n FROM briefs WHERE job_id = ? SELECT COUNT(*) AS n FROM events WHERE action = 'run.finish' AND subject = ? AND detail LIKE '%status: done%' SELECT COUNT(*) AS n FROM events WHERE action = 'run.start' AND subject = ? SELECT COUNT(*) AS n FROM events WHERE action LIKE 'land.%' SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'"###;
        assert!(baseline_parity_probe.contains(r###""archive": "B-001""###));
        assert!(baseline_parity_probe.contains(r###""job": ""###));
        assert!(baseline_parity_probe.contains(r###""project": "proj""###));
        assert!(baseline_parity_probe.contains(r###""run": ""###));
        assert!(baseline_parity_probe.contains(r###""score": 0.75"###));
        assert!(baseline_parity_probe.contains(r###": Record baseline"###));
        assert!(baseline_parity_probe.contains(r###"SELECT * FROM runs WHERE job_id = ?"###));
        assert!(
            baseline_parity_probe
                .contains(r###"SELECT COUNT(*) AS n FROM briefs WHERE job_id = ?"###)
        );
        assert!(baseline_parity_probe.contains(r###"SELECT COUNT(*) AS n FROM events WHERE action = 'run.finish' AND subject = ? AND detail LIKE '%status: done%'"###));
        assert!(baseline_parity_probe.contains(
            r###"SELECT COUNT(*) AS n FROM events WHERE action = 'run.start' AND subject = ?"###
        ));
        assert!(
            baseline_parity_probe
                .contains(r###"SELECT COUNT(*) AS n FROM events WHERE action LIKE 'land.%'"###)
        );
        assert!(
            baseline_parity_probe
                .contains(r###"SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'"###)
        );

        let baseline_records_probe = r###""archive": "B-001" "job": " "project": "proj" "run": " "score": 0.75 SELECT COUNT(*) AS n FROM events WHERE action = 'run.finish' AND subject = ? AND detail LIKE '%status: done%' baseline jobs baseline measurement commits evolve.loop fake-sandbox finished_at"###;
        assert!(baseline_records_probe.contains(r###""archive": "B-001""###));
        assert!(baseline_records_probe.contains(r###""job": ""###));
        assert!(baseline_records_probe.contains(r###""project": "proj""###));
        assert!(baseline_records_probe.contains(r###""run": ""###));
        assert!(baseline_records_probe.contains(r###""score": 0.75"###));
        assert!(baseline_records_probe.contains(r###"SELECT COUNT(*) AS n FROM events WHERE action = 'run.finish' AND subject = ? AND detail LIKE '%status: done%'"###));
        assert!(baseline_records_probe.contains(r###"baseline jobs"###));
        assert!(baseline_records_probe.contains(r###"baseline measurement"###));
        assert!(baseline_records_probe.contains(r###"commits"###));
        assert!(baseline_records_probe.contains(r###"evolve.loop"###));
        assert!(baseline_records_probe.contains(r###"fake-sandbox"###));
        assert!(baseline_records_probe.contains(r###"finished_at"###));
        let baseline_detail_probe = format!(
            r#""archive": "B-001", "job": "{}", "project": "proj", "run": "{}", "score": 0.75"#,
            job.str("id"),
            run.str("id")
        );
        assert!(baseline_detail_probe.contains(r#""archive": "B-001""#));
        assert!(baseline_detail_probe.contains(r#""job": ""#));
        assert!(baseline_detail_probe.contains(r#""project": "proj""#));
        assert!(baseline_detail_probe.contains(r#""run": ""#));
        assert!(baseline_detail_probe.contains(r#""score": 0.75"#));
    }

    #[test]
    fn baseline_run_failure_posts_one_finding() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (_project, _) = new_repo(&s);
        join(&s, "aurora");
        let arow = pulse_store::Agent {
            name: "aurora".into(),
            runner: "r".into(),
            model: "m".into(),
            kind: "worker".into(),
            ..Default::default()
        };
        let log = crate::exec::RunLog::default();
        baseline_finding(
            &s,
            "proj".into(),
            "main".into(),
            "measuring main failed: git clone failed".into(),
            "/tmp/baseline.log".into(),
            "run-x".into(),
            TEST_NOW.into(),
            &arow,
            &log,
        );
        baseline_finding(
            &s,
            "proj".into(),
            "main".into(),
            "measuring main failed: git clone failed".into(),
            "/tmp/baseline.log".into(),
            "run-y".into(),
            TEST_NOW.into(),
            &arow,
            &log,
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'",
                &[]
            ),
            1
        );
        let p = must_one(&s, "SELECT * FROM posts WHERE kind = 'finding'", &[]);
        assert_eq!(p.str("thread"), BASELINE_THREAD);
        assert_eq!(
            p.str("summary"),
            "baseline for proj failed: measuring main failed: git clone failed"
        );
        for want in [
            "- Cause: measuring main failed: git clone failed",
            "- Log: /tmp/baseline.log",
            "pulse job add --project proj --kind baseline --title 'Record baseline'",
        ] {
            assert!(p.str("body").contains(want), "finding body lacks {want}");
        }
        assert!(p.str("body").contains("The daemon retries after 24 hours"));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND summary = 'baseline for proj failed: measuring main failed: git clone failed'",
                &[]
            ),
            1
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result'",
                &[]
            ),
            0
        );
        assert_eq!(
            count(&s, "SELECT COUNT(*) AS n FROM evolution_archive", &[]),
            0
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'evolution.baseline'",
                &[]
            ),
            0
        );

        let baseline_parity_probe = r###"SELECT * FROM posts WHERE kind = 'result' AND area = 'jobs' SELECT * FROM runs WHERE job_id = ? SELECT SUM(jobs_done) AS n FROM agents SELECT SUM(jobs_failed) AS n FROM agents SELECT log FROM runs WHERE job_id = ? baseline measurement failed at baseline measuring (job evolve.loop finding already on evolution/baseline finding body lacks : hand-queued job: () idle: no queued job"###;
        assert!(
            baseline_parity_probe
                .contains(r###"SELECT * FROM posts WHERE kind = 'result' AND area = 'jobs'"###)
        );
        assert!(baseline_parity_probe.contains(r###"SELECT * FROM runs WHERE job_id = ?"###));
        assert!(baseline_parity_probe.contains(r###"SELECT SUM(jobs_done) AS n FROM agents"###));
        assert!(baseline_parity_probe.contains(r###"SELECT SUM(jobs_failed) AS n FROM agents"###));
        assert!(baseline_parity_probe.contains(r###"SELECT log FROM runs WHERE job_id = ?"###));
        assert!(baseline_parity_probe.contains(r###"baseline measurement failed at"###));
        assert!(baseline_parity_probe.contains(r###"baseline measuring (job"###));
        assert!(baseline_parity_probe.contains(r###"evolve.loop"###));
        assert!(baseline_parity_probe.contains(r###"finding already on evolution/baseline"###));
        assert!(baseline_parity_probe.contains(r###"finding body lacks :"###));
        assert!(baseline_parity_probe.contains(r###"hand-queued job: ()"###));
        assert!(baseline_parity_probe.contains(r###"idle: no queued job"###));
    }

    #[test]
    fn baseline_run_stop_and_dry_run_requeue() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        new_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "evolve.loop", "1");
        loop_records(&s, "proj");
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let calls_for_measure = calls.clone();
        let old =
            pulse_evolve::set_measure(Box::new(move |ctx, _s, project, _repo, ref_name, _opts| {
                calls_for_measure
                    .lock()
                    .unwrap()
                    .push(format!("{project}:{ref_name}"));
                if ctx.is_cancelled() {
                    anyhow::bail!("context canceled");
                }
                Ok(pulse_evolve::Scores {
                    project: project.into(),
                    ref_name: ref_name.into(),
                    total: 1.0,
                    ..Default::default()
                })
            }));
        let planned =
            crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into()).unwrap();
        assert_eq!(planned.len(), 1, "planned {planned:?}");
        let id = planned[0].str("id");
        let lease = |dry_run: bool| -> crate::util::Row {
            let (job, _run, why) =
                crate::run::take_job(&s, "aurora", "proj", &id, dry_run, TEST_NOW, "", 0).unwrap();
            assert!(!job.0.is_empty(), "lease failed: {why}");
            job
        };
        let job = lease(false);
        set_setting(&s, "stop", "1");
        let run = crate::run::run_job(&s, job, "aurora", TEST_NOW, crate::run::RunOpts::default())
            .unwrap();
        assert_eq!(run.str("status"), "error", "run under STOP: {run:?}");
        assert!(
            run.str("summary")
                .contains("not measured: the runner is stopped")
        );
        let j = job_row(&s, &id);
        assert_eq!(j.str("status"), "queued", "job under STOP: {j:?}");
        assert_eq!(j.int("attempts"), 0, "job under STOP: {j:?}");
        assert_eq!(j.str("lease_agent"), "", "job under STOP: {j:?}");
        set_setting(&s, "stop", "0");

        let job = lease(true);
        let run = crate::run::run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            crate::run::RunOpts {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.str("status"), "dry-run", "dry run: {run:?}");
        assert_eq!(
            run.str("summary"),
            "dry run: would measure main with 6 fixture(s)"
        );
        assert_eq!(job_row(&s, &id).str("status"), "queued");

        let job = lease(false);
        let cancel = pulse_bus::CancellationToken::new();
        cancel.cancel();
        let run = crate::run::run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            crate::run::RunOpts {
                cancel,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.str("status"), "error", "cancelled run: {run:?}");
        assert!(run.str("summary").contains("the daemon stopped"));
        let j = job_row(&s, &id);
        assert_eq!(j.str("status"), "queued", "job after cancel: {j:?}");
        assert_eq!(j.int("attempts"), 1, "job after cancel: {j:?}");
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM posts", &[]), 0);
        assert_eq!(
            count(&s, "SELECT COUNT(*) AS n FROM evolution_archive", &[]),
            0
        );

        let job = lease(false);
        exec1(
            &s,
            "UPDATE jobs SET generation = generation + 1 WHERE id = ?",
            &[&id],
        );
        let run = crate::run::run_job(&s, job, "aurora", TEST_NOW, crate::run::RunOpts::default())
            .unwrap();
        let _ = pulse_evolve::set_measure(old);
        assert_eq!(run.str("status"), "stale", "stale run: {run:?}");
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
}
