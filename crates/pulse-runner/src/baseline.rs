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
    match pulse_evolve::recent_baseline_failure_hold(s, &slug, &now) {
        Ok(Some((failed_at, retry_text))) => (
            false,
            format!(
                "baseline measurement failed at {failed_at}; the daemon retries after {retry_text}"
            ),
        ),
        Ok(None) => (true, String::new()),
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
        "Record the baseline of {slug} (ADR-031). The runner measures the default branch `{def}` itself with the {fixtures} enabled public fixture(s) and every private held-out fixture through evolve.Measure, then archives one kept row (hypothesis `{}`) that `pulse evolve status` reads as the baseline. No worker runs, no clone is prepared and nothing lands. A failed measurement posts one finding on evolution/{BASELINE_THREAD} and the daemon plans the job again after {} hours ({} minutes for transient failures); `pulse job add --project {slug} --kind baseline --title '{BASELINE_TITLE}'` queues it sooner.",
        pulse_evolve::BASELINE_HYPOTHESIS,
        pulse_evolve::baseline_retry_hours(s),
        pulse_evolve::baseline_short_retry_minutes(s)
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_baseline(
    s: &pulse_store::Store,
    job: crate::util::Row,
    project: crate::util::Row,
    agent: String,
    now: String,
    run_id: String,
    log_path: String,
    ex: &dyn crate::exec::Executor,
    o: crate::run::RunOpts,
) -> anyhow::Result<crate::util::Row> {
    let slug = project.str("slug");
    let def = default_branch(project.clone());
    let fixtures = enabled_fixtures(s, slug.clone());
    let mut log = crate::exec::RunLog::default();
    let started = crate::run::wall_clock();
    let mut status = "failed".to_string();
    let summary: String;
    let mut archive_id = String::new();
    let mut scores: Option<pulse_evolve::Scores> = None;
    let mut restore_attempt = false;
    'measure: {
        if o.dry_run {
            status = "dry-run".into();
            summary = format!("dry run: would measure {def} with {fixtures} fixture(s)");
            log.put(&summary);
            break 'measure;
        }
        let why = crate::run::runner_stopped(s);
        if !why.is_empty() {
            status = "error".into();
            summary = format!("not measured: {why}");
            restore_attempt = true;
            log.put(&summary);
            break 'measure;
        }
        let budget = baseline_budget(s, slug.clone());
        // The lease must outlive the measurement or another worker takes the job halfway through.
        let renewed = s.conn().is_ok_and(|conn| {
            crate::jobs::renew_lease(
                &conn,
                now.clone(),
                job.str("id"),
                agent.clone(),
                job.int("generation"),
                (budget.as_secs() / 60) as i32 + 5,
            )
        });
        if !renewed {
            status = "stale".into();
            summary = format!(
                "lease generation {} is no longer current; nothing measured",
                job.int("generation")
            );
            log.put(&summary);
            break 'measure;
        }
        let parent = o.cancel.clone();
        let ctx = parent.with_timeout(crate::experiment::EXPERIMENT_MEASURE_BUDGET);
        log.add(format!(
            "measuring {def} of {slug} with {fixtures} enabled public fixture(s) and {} held out; budget {}",
            pulse_evolve::heldout_count(s).count,
            pulse_cli::format_duration(round_to(budget, std::time::Duration::from_secs(60)))
        ));
        let measured = pulse_evolve::measure(
            &ctx,
            s,
            &slug,
            &project.str("path"),
            &def,
            pulse_evolve::MeasureOptions {
                out: Some(&mut log),
                ..Default::default()
            },
        );
        match measured {
            Err(err) if parent.is_cancelled() => {
                // The daemon is stopping or restarting: no failure, no finding, the job is planned again.
                status = "error".into();
                summary = format!(
                    "not measured: the daemon stopped ({})",
                    crate::run::err_message(&err)
                );
                restore_attempt = true;
            }
            Err(err) if err.to_string() == pulse_evolve::err_not_implemented().to_string() => {
                summary = format!(
                    "measuring {def} failed: evolve.Measure is not implemented ({})",
                    crate::run::err_message(&err)
                );
            }
            Err(err) => {
                summary = format!("measuring {def} failed: {}", crate::run::err_message(&err));
            }
            Ok(sc) => {
                scores = Some(sc.clone());
                let commit = crate::experiment::default_commit(&project.str("path"), &def);
                if let Ok(fixture_hash) = pulse_evolve::baseline_fixture_hash(s, &slug)
                    && let Err(err) = pulse_evolve::upsert_baseline(
                        s,
                        &slug,
                        &commit,
                        &fixture_hash,
                        &sc,
                        &run_id,
                    )
                {
                    log.add(format!(
                        "baseline cache skipped: {}",
                        crate::run::err_message(&err)
                    ));
                }
                match pulse_evolve::archive_row(
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
                    Err(err) => {
                        summary = format!(
                            "archiving the baseline of {def} failed: {}",
                            crate::run::err_message(&err)
                        );
                    }
                    Ok(id) => {
                        archive_id = id;
                        status = "done".into();
                        summary = format!(
                            "baseline {}; archive {archive_id}",
                            crate::experiment::score_line(&sc)
                        );
                    }
                }
            }
        }
        log.put(&summary);
    }
    let seconds = ((crate::run::wall_clock()
        .duration_since(started)
        .unwrap_or_default()
        .as_secs_f64()
        * 10.0
        + 0.5) as i64) as f64
        / 10.0;
    let _ = std::fs::write(&log_path, log.text());
    let conn = s.conn()?;
    crate::util::exec1(
        &conn,
        "UPDATE runs SET finished_at = ?, status = ?, seconds = ?, branch = '', commits = 0, summary = ? WHERE id = ?",
        &[&s.now(), &status, &seconds, &summary, &run_id],
    );
    crate::executor::note_executor_run(s, &now, &ex.name(), &status, &summary, false, false);
    let run_row = || crate::util::must_one(&conn, "SELECT * FROM runs WHERE id = ?", &[&run_id]);
    let finish_event = |st: &str| {
        crate::util::event_at(
            &conn,
            &now,
            &agent,
            "run.finish",
            &run_id,
            serde_json::json!({"status": st, "commits": 0, "branch": "", "executor": ex.name()}),
        );
    };
    if status == "stale" {
        finish_event(&status);
        return Ok(run_row());
    }
    let mut job_status = "failed";
    if status == "done" {
        job_status = "done";
    } else if status == "dry-run" {
        job_status = "queued";
    } else if status == "error" && restore_attempt {
        job_status = "queued";
        crate::util::exec1(
            &conn,
            "UPDATE jobs SET attempts = MAX(attempts - 1, 0) WHERE id = ?",
            &[&job.str("id")],
        );
    }
    if let Err(err) = crate::jobs::finish_job(
        s,
        job.str("id"),
        agent.clone(),
        job_status.into(),
        summary.clone(),
        String::new(),
        job.int("generation"),
    ) {
        log.put(&crate::run::err_message(&err));
        let _ = std::fs::write(&log_path, log.text());
        crate::util::exec1(
            &conn,
            "UPDATE runs SET status = 'stale', summary = ? WHERE id = ?",
            &[
                &format!("{summary} ({})", crate::run::err_message(&err)),
                &run_id,
            ],
        );
        finish_event("stale");
        return Ok(run_row());
    }
    if job_status == "done" {
        crate::util::exec1(
            &conn,
            "UPDATE agents SET jobs_done = jobs_done + 1 WHERE name = ?",
            &[&agent],
        );
        crate::board::touch_agent(s, agent.clone(), false, now.clone());
        crate::util::event_at(
            &conn,
            &now,
            &agent,
            "evolution.baseline",
            &archive_id,
            serde_json::json!({"project": slug, "archive": archive_id, "score": scores.as_ref().map(|sc| sc.total), "run": run_id, "job": job.str("id")}),
        );
    } else if job_status == "failed" {
        crate::util::exec1(
            &conn,
            "UPDATE agents SET jobs_failed = jobs_failed + 1 WHERE name = ?",
            &[&agent],
        );
    }
    if status != "dry-run" && status != "error" {
        let arow = crate::board::actor_agent(s, agent.clone());
        let mut lines = vec![
            format!(
                "Run `{run_id}` of job `{}` (baseline, Tier {}) by {agent} on executor `{}`: **{status}** in {seconds}s.",
                job.str("id"),
                job.int("tier"),
                ex.name()
            ),
            String::new(),
            format!("- Measured: `{def}` of {slug} with {fixtures} enabled public fixture(s)"),
        ];
        if let Some(sc) = &scores {
            lines.push(format!("- Score: {}", crate::experiment::score_line(sc)));
            for f in &sc.fixtures {
                if f.kind == "heldout" {
                    continue; // held-out fixtures are counted, never named (ADR-012)
                }
                lines.push(format!(
                    "- {}: {:.3} (weight {})",
                    f.name, f.score, f.weight
                ));
            }
        }
        if !archive_id.is_empty() {
            lines.push(format!(
                "- Archive: {archive_id} (kept; `pulse evolve status --project {slug}` reads it as the baseline)"
            ));
        }
        lines.push("- Commits: 0 (a baseline job lands nothing)".into());
        lines.push(format!("- Log: {log_path}"));
        let mut resolution = "";
        if job_status == "done" {
            resolution = "tested";
        }
        if let Err(err) = crate::board::create_post(
            s,
            now.clone(),
            crate::board::PostOpts {
                kind: "result".into(),
                project: slug.clone(),
                area: "jobs".into(),
                thread: job.str("id"),
                summary: crate::util::trunc(&format!("{status}: {summary}"), 230),
                body: lines.join("\n") + "\n",
                agent: arow.clone(),
                session: run_id.clone(),
                branch: Some(Box::new(def.clone())),
                resolution: resolution.into(),
                ..Default::default()
            },
        ) {
            log.add(format!("post skipped: {}", crate::run::err_message(&err)));
        }
        if job_status == "failed" {
            baseline_finding(
                s,
                slug.clone(),
                def.clone(),
                summary.clone(),
                log_path.clone(),
                run_id.clone(),
                now.clone(),
                arow,
                &log,
            );
        }
        let _ = std::fs::write(&log_path, log.text());
    }
    finish_event(&status);
    Ok(run_row())
}

/// Go: time.Duration.Round(m) for a non-negative d (halves round away from zero).
fn round_to(d: std::time::Duration, m: std::time::Duration) -> std::time::Duration {
    let (d, m) = (d.as_nanos(), m.as_nanos());
    if m == 0 {
        return std::time::Duration::from_nanos(d as u64);
    }
    let r = d % m;
    if r + r < m {
        std::time::Duration::from_nanos((d - r) as u64)
    } else {
        std::time::Duration::from_nanos((d + m - r) as u64)
    }
}

/// Go: func baselineFinding(s *store.Store, slug, def, cause, logPath, runID, now string, arow *store.Agent, log *runLog)
#[allow(clippy::too_many_arguments)]
pub(crate) fn baseline_finding(
    s: &pulse_store::Store,
    slug: String,
    def: String,
    cause: String,
    log_path: String,
    run_id: String,
    now: String,
    arow: Option<Box<pulse_store::Agent>>,
    log: &crate::exec::RunLog,
) {
    let summary = crate::util::trunc(
        &format!("baseline for {slug} failed: {cause}")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        230,
    );
    let Ok(conn) = s.conn() else { return };
    if !crate::util::must_one(&conn, "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'finding' AND area = 'evolution' AND thread = ? AND summary = ?", &[&slug, &BASELINE_THREAD, &summary]).0.is_empty() {
        log.add(format!(
            "finding already on evolution/{BASELINE_THREAD}: {summary}"
        ));
        return;
    }
    let body = format!(
        "The daemon could not record the baseline of {slug} (ADR-031).\n\n- Cause: {cause}\n- Measured: `{def}`\n- Run: `{run_id}`\n- Log: {log_path}\n\n\
         The daemon retries after {}; a human may run `pulse job add --project {slug} --kind baseline --title '{BASELINE_TITLE}'` sooner.\n",
        pulse_evolve::baseline_retry_text(s, &cause)
    );
    if let Err(err) = crate::board::create_post(
        s,
        now,
        crate::board::PostOpts {
            kind: "finding".into(),
            project: slug,
            area: "evolution".into(),
            thread: BASELINE_THREAD.into(),
            summary,
            body,
            agent: arow,
            session: run_id,
            branch: Some(Box::new(def)),
            ..Default::default()
        },
    ) {
        log.add(format!(
            "finding skipped: {}",
            crate::run::err_message(&err)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::*;

    fn set_retry_setting(s: &pulse_store::Store, key: &str, value: &str) {
        exec1(
            s,
            "INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, 'test')
             ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            &[&key, &value, &TEST_NOW],
        );
    }

    #[test]
    fn baseline_planning() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        pulse_bus::reset_default();
        let s = new_store();
        new_repo(&s);
        set_retry_setting(&s, "baseline.retry_hours", "2");
        set_retry_setting(&s, "lane.short_retry_minutes", "3");
        let not_due = |want: &str| {
            let (due, why) = baseline_due(&s, "proj".into(), TEST_NOW.into());
            if due || !why.contains(want) {
                panic!("baselineDue = {due} {why:?}, want a reason with {want:?}");
            }
            let jobs = crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into())
                .unwrap();
            for j in &jobs {
                if j.str("kind") == "baseline" {
                    panic!("a baseline job was planned although {want}");
                }
            }
            // The planner falls through to the daily reflect job; it must not hold the project below parallel_per_project here.
            exec1(
                &s,
                "DELETE FROM jobs WHERE project = 'proj' AND kind != 'baseline'",
                &[],
            );
        };
        not_due("evolve.loop is off");
        set_setting(&s, "evolve.loop", "1");
        // Registration copied the loop's records from the bundled templates
        // (ADR-038); an emptied constitution counts as missing, and the baseline
        // is not due.
        pulse_records::amend(
            &s,
            "EVO-constitution-proj",
            "",
            crate::util::HUMAN_AGENT,
            "emptied",
        )
        .unwrap();
        not_due("no EVO-constitution record");
        loop_records(&s, "proj");
        let (due, why) = baseline_due(&s, "proj".into(), TEST_NOW.into());
        if !due || !why.is_empty() {
            panic!("baselineDue = {due} {why:?}");
        }
        let jobs = crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
        if jobs.is_err() || jobs.as_ref().unwrap().len() != 1 {
            panic!("planned {jobs:?}");
        }
        let jobs = jobs.unwrap();
        let job = jobs[0].clone();
        if job.str("kind") != "baseline"
            || job.str("title") != "Record baseline"
            || job.int("tier") != 0
            || job.int("priority") != 20
            || job.str("source") != "baseline:proj"
            || job.str("created_by") != "daemon"
            || job.str("status") != "queued"
        {
            panic!("baseline job: {job:?}");
        }
        for want in [
            "default branch `main`",
            "6 enabled public fixture(s)",
            "No worker runs",
            "evolve.Measure",
            "after 2 hours",
            "pulse job add --project proj --kind baseline --title 'Record baseline'",
        ] {
            if !job.str("brief").contains(want) {
                panic!("brief lacks {want:?}:\n{}", job.str("brief"));
            }
        }
        // Planned once: the open job holds the planner, whether it is queued or leased.
        not_due(&format!("baseline job {} is queued", job.str("id")));
        if baseline_jobs(&s, "proj").len() != 1 {
            panic!("baseline jobs: {:?}", baseline_jobs(&s, "proj"));
        }
        // A failure holds the planner for 2 hours, then the job is planned again.
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
            "failed at {TEST_NOW}; the daemon retries after 2 hours"
        ));
        if baseline_due(&s, "proj".into(), crate::util::plus_hours(TEST_NOW, 1.5)).0 {
            panic!("due before 2 hours passed");
        }
        let later = crate::util::plus_hours(TEST_NOW, 2.0);
        let (due, why) = baseline_due(&s, "proj".into(), later.clone());
        if !due {
            panic!("not due after 2 hours: {why:?}");
        }
        let jobs = crate::plan::plan_jobs(&s, "proj".into(), later, "daemon".into());
        if jobs.is_err()
            || jobs.as_ref().unwrap().len() != 1
            || jobs.as_ref().unwrap()[0].str("kind") != "baseline"
            || jobs.as_ref().unwrap()[0].str("id") == job.str("id")
        {
            panic!("planned after 2 hours: {jobs:?}");
        }
        let jobs = jobs.unwrap();
        if baseline_jobs(&s, "proj").len() != 2 {
            panic!("baseline jobs: {:?}", baseline_jobs(&s, "proj"));
        }
        // A cancelled job ends the hold: the newest finished job is what counts.
        crate::jobs::finish_job(
            &s,
            jobs[0].str("id"),
            crate::util::HUMAN_AGENT.into(),
            "cancelled".into(),
            "not now".into(),
            String::new(),
            -1,
        )
        .unwrap();
        let (due, why) = baseline_due(&s, "proj".into(), TEST_NOW.into());
        if !due {
            panic!("not due after a cancelled job: {why:?}");
        }
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

        // The brain is planned like any other project once it meets the conditions.
        let brain_dir = tempfile::tempdir().expect("brain");
        let dir = brain_dir.path().join("brain");
        let _ = std::fs::create_dir_all(&dir);
        let dir = dir.display().to_string();
        run(&dir, &["git", "init", "-q", "-b", "trunk"]);
        let _ = std::fs::write(std::path::Path::new(&dir).join("README.md"), "# brain\n");
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "init"]);
        crate::board::add_project(
            &s,
            dir,
            "Brain".into(),
            String::new(),
            true,
            String::new(),
            crate::util::HUMAN_AGENT.into(),
            "brain".into(),
        )
        .unwrap();
        loop_records(&s, "brain");
        let jobs = crate::plan::plan_jobs(&s, "brain".into(), TEST_NOW.into(), "daemon".into());
        if jobs.is_err()
            || jobs.as_ref().unwrap().len() != 1
            || jobs.as_ref().unwrap()[0].str("kind") != "baseline"
            || jobs.as_ref().unwrap()[0].str("source") != "baseline:brain"
            || !jobs.as_ref().unwrap()[0]
                .str("brief")
                .contains("default branch `trunk`")
        {
            panic!("brain baseline job: {jobs:?}");
        }
    }

    #[test]
    fn baseline_budget() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        new_repo(&s);
        set_setting(&s, "minutes_per_job", "10");
        // Six starters at ten minutes plus slack is more than the worker budget, less than the experiment cap.
        let got = super::baseline_budget(&s, "proj".into());
        if got.as_secs() / 60 != 65 {
            panic!("baseline budget: {got:?}");
        }
        let mut job = crate::util::Row::new();
        job.insert("kind", serde_json::json!("baseline"));
        job.insert("project", serde_json::json!("proj"));
        let mut improve = crate::util::Row::new();
        improve.insert("kind", serde_json::json!("improve"));
        improve.insert("project", serde_json::json!("proj"));
        if job_budget(&s, job) != super::baseline_budget(&s, "proj".into())
            || job_budget(&s, improve) != crate::daemon::run_budget(&s)
        {
            panic!("job budgets");
        }
        set_setting(&s, "minutes_per_job", "30");
        let got = super::baseline_budget(&s, "proj".into());
        if got != crate::experiment::EXPERIMENT_MEASURE_BUDGET {
            panic!("capped budget: {got:?}");
        }
        exec1(
            &s,
            "UPDATE evolution_fixtures SET enabled = 0 WHERE project = 'proj'",
            &[],
        );
        let got = super::baseline_budget(&s, "proj".into());
        if got != crate::daemon::run_budget(&s) {
            panic!(
                "floor budget: {got:?} vs {:?}",
                crate::daemon::run_budget(&s)
            );
        }
    }

    #[test]
    fn baseline_run_records_the_baseline() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        pulse_bus::reset_default();
        let s = new_store();
        new_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "evolve.loop", "1");
        loop_records(&s, "proj");
        only_baseline(&s);
        let measure = FakeMeasure::new(0.75, None);
        let _measure = measure.install();
        let log = once_daemon(&s, crate::daemon::DaemonOpts::default());
        let jobs = baseline_jobs(&s, "proj");
        if jobs.len() != 1
            || !log.contains(&format!(
                "planned baseline {}: Record baseline",
                jobs[0].str("id")
            ))
        {
            panic!("baseline jobs {jobs:?}\n{log}");
        }
        let job = jobs[0].clone();
        if job.str("status") != "done"
            || !job.str("branch").is_empty()
            || job.str("result") != "baseline 0.750 over 2 fixture(s), 1 held out; archive B-001"
        {
            panic!("job after the run: {job:?}\n{log}");
        }
        if measure.calls().len() != 1 || measure.calls()[0] != "proj@main" {
            panic!("measured {:?}", measure.calls());
        }
        let runs = must_rows(&s, "SELECT * FROM runs WHERE job_id = ?", &[&job.str("id")]);
        if runs.len() != 1
            || runs[0].str("status") != "done"
            || runs[0].str("summary") != job.str("result")
            || runs[0].str("finished_at").is_empty()
            || !runs[0].str("branch").is_empty()
            || runs[0].int("commits") != 0
            || runs[0].str("executor") != crate::executor::LOCAL_EXECUTOR
        {
            panic!("runs: {runs:?}");
        }
        let run_id = runs[0].str("id");
        let text = std::fs::read_to_string(runs[0].str("log"));
        if text.is_err()
            || !text
                .as_ref()
                .unwrap()
                .contains("measuring main of proj with 6 enabled public fixture(s)")
            || !text.as_ref().unwrap().contains("archive B-001")
        {
            panic!(
                "run log: {:?}\n{}",
                text.as_ref().err(),
                text.as_deref().unwrap_or_default()
            );
        }
        let archive = must_one(
            &s,
            "SELECT * FROM evolution_archive WHERE project = 'proj'",
            &[],
        );
        if archive.0.is_empty()
            || archive.str("id") != "B-001"
            || archive.str("hypothesis") != "baseline"
            || !archive.str("suggestion_post").is_empty()
            || archive.str("branch") != "main"
            || archive.int("kept") != 1
            || archive.str("run_id") != run_id
            || archive.str("note") != "baseline measurement"
            || archive.str("sandbox") != "fake-sandbox"
            || archive.float("baseline_score") != 0.75
            || archive.float("variant_score") != 0.75
        {
            panic!("archive row: {archive:?}");
        }
        if archive.str("fixtures_json").contains("PRIVATE-NAME") {
            panic!(
                "held-out name in the archive: {}",
                archive.str("fixtures_json")
            );
        }
        let ev = must_one(
            &s,
            "SELECT * FROM events WHERE action = 'evolution.baseline'",
            &[],
        );
        if ev.0.is_empty()
            || ev.str("subject") != "B-001"
            || ev.str("actor") != job.str("lease_agent")
        {
            panic!("evolution.baseline event: {ev:?}");
        }
        for want in [
            "\"project\": \"proj\"".to_string(),
            "\"archive\": \"B-001\"".to_string(),
            "\"score\": 0.75".to_string(),
            format!("\"run\": \"{run_id}\""),
            format!("\"job\": \"{}\"", job.str("id")),
        ] {
            if !ev.str("detail").contains(&want) {
                panic!("event detail lacks {want}: {}", ev.str("detail"));
            }
        }
        if count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'run.start' AND subject = ?",
            &[&run_id],
        ) != 1
            || count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'run.finish' AND subject = ? AND detail LIKE '%\"status\": \"done\"%'",
                &[&run_id],
            ) != 1
        {
            panic!("run.start / run.finish events");
        }
        let posts = must_rows(
            &s,
            "SELECT * FROM posts WHERE project = 'proj' AND area = 'jobs'",
            &[],
        );
        if posts.len() != 1
            || posts[0].str("kind") != "result"
            || posts[0].str("thread") != crate::util::slugify(&job.str("id"), 40)
            || posts[0].str("resolution") != "tested"
            || posts[0].str("summary") != format!("done: {}", job.str("result"))
            || posts[0].str("session") != run_id
            || posts[0].str("branch") != "main"
        {
            panic!("result post: {posts:?}");
        }
        let body = posts[0].str("body");
        for want in [
            "(baseline, Tier 0)",
            "**done**",
            "- Score: 0.750 over 2 fixture(s), 1 held out",
            "- starter-readme: 0.750 (weight 1)",
            "- Archive: B-001 (kept;",
            "- Commits: 0 (a baseline job lands nothing)",
            &format!("- Log: {}", runs[0].str("log")),
        ] {
            if !body.contains(want) {
                panic!("result body lacks {want:?}:\n{body}");
            }
        }
        if body.contains("PRIVATE-NAME") {
            panic!("held-out name in the result post:\n{body}");
        }
        if count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'",
            &[],
        ) != 0
        {
            panic!("a finding after a successful baseline");
        }
        // No worker ran: no brief, no clone, no agent session; the worker's counter still moved.
        if count(
            &s,
            "SELECT COUNT(*) AS n FROM briefs WHERE job_id = ?",
            &[&job.str("id")],
        ) != 0
            || crate::util::file_exists(
                &std::path::Path::new(&s.home)
                    .join("work")
                    .join(job.str("id"))
                    .display()
                    .to_string(),
            )
        {
            panic!("a brief or a clone was made for the baseline job");
        }
        let a = crate::board::agent_row(&*s.conn().unwrap(), job.str("lease_agent"));
        if a.0.is_empty() || a.int("jobs_done") != 1 || a.int("jobs_failed") != 0 {
            panic!("worker counters: {a:?}");
        }
        let status = pulse_evolve::status(&s, "proj");
        if status.is_err()
            || status.as_ref().unwrap().baseline != Some(0.75)
            || status.as_ref().unwrap().baseline_id != "B-001"
            || status.as_ref().unwrap().level != "C"
            || status.as_ref().unwrap().kept != 0
            || status.as_ref().unwrap().cycles != 0
            || status.as_ref().unwrap().baseline_job.is_some()
        {
            panic!("status after the baseline: {status:?}");
        }
        // Nothing lands and nothing is planned again: the archive row is the baseline.
        let log = once_daemon(&s, crate::daemon::DaemonOpts::default());
        if baseline_jobs(&s, "proj").len() != 1
            || format!("\n{log}").contains("\nlanding ")
            || !log.contains("idle: no queued job")
        {
            panic!("second tick: {:?}\n{log}", baseline_jobs(&s, "proj"));
        }
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action LIKE 'land.%'",
            &[],
        );
        if n != 0 {
            panic!("baseline produced {n} landing events");
        }
    }

    #[test]
    fn baseline_run_failure_posts_one_finding() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        pulse_bus::reset_default();
        let s = new_store();
        new_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "evolve.loop", "1");
        set_retry_setting(&s, "baseline.retry_hours", "2");
        set_retry_setting(&s, "lane.short_retry_minutes", "3");
        loop_records(&s, "proj");
        only_baseline(&s);
        let measure = FakeMeasure::new(
            0.0,
            Some(anyhow::anyhow!(
                "Measurement failed for starter-readme: harness failed; command output is withheld"
            )),
        );
        let _measure = measure.install();
        let log = once_daemon(&s, crate::daemon::DaemonOpts::default());
        let jobs = baseline_jobs(&s, "proj");
        let want = "measuring main failed: Measurement failed for starter-readme: harness failed; command output is withheld";
        if jobs.len() != 1 || jobs[0].str("status") != "failed" || jobs[0].str("result") != want {
            panic!("job after a failed measurement: {jobs:?}\n{log}");
        }
        let runs = must_rows(
            &s,
            "SELECT * FROM runs WHERE job_id = ?",
            &[&jobs[0].str("id")],
        );
        if runs.len() != 1 || runs[0].str("status") != "failed" || runs[0].str("summary") != want {
            panic!("runs: {runs:?}");
        }
        if count(&s, "SELECT COUNT(*) AS n FROM evolution_archive", &[]) != 0
            || count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'evolution.baseline'",
                &[],
            ) != 0
        {
            panic!("a failed measurement was archived");
        }
        let results = must_rows(
            &s,
            "SELECT * FROM posts WHERE kind = 'result' AND area = 'jobs'",
            &[],
        );
        if results.len() != 1
            || !results[0].str("resolution").is_empty()
            || results[0].str("summary") != format!("failed: {want}")
        {
            panic!("result posts: {results:?}");
        }
        let findings = must_rows(&s, "SELECT * FROM posts WHERE kind = 'finding'", &[]);
        if findings.len() != 1
            || findings[0].str("area") != "evolution"
            || findings[0].str("thread") != "baseline"
            || findings[0].str("summary") != format!("baseline for proj failed: {want}")
            || findings[0].str("session") != runs[0].str("id")
        {
            panic!("findings: {findings:?}");
        }
        for text in [
            format!("- Cause: {want}"),
            format!("- Log: {}", runs[0].str("log")),
            "The daemon retries after 2 hours".to_string(),
            "pulse job add --project proj --kind baseline --title 'Record baseline'".to_string(),
        ] {
            if !findings[0].str("body").contains(&text) {
                panic!("finding body lacks {text:?}:\n{}", findings[0].str("body"));
            }
        }
        let status = pulse_evolve::status(&s, "proj");
        if status.is_err()
            || status.as_ref().unwrap().baseline.is_some()
            || status.as_ref().unwrap().baseline_failed_at != TEST_NOW
            || status.as_ref().unwrap().baseline_job.is_some()
            || status.as_ref().unwrap().reason
                != format!(
                    "baseline measurement failed at {TEST_NOW}; the daemon retries after 2 hours"
                )
        {
            panic!("status after the failure: {status:?}");
        }
        // The daemon plans nothing for 2 hours; a human queues the job by hand and the same failure adds no finding.
        let log = once_daemon(&s, crate::daemon::DaemonOpts::default());
        if baseline_jobs(&s, "proj").len() != 1 || !log.contains("idle: no queued job") {
            panic!("planned again inside 2 hours:\n{log}");
        }
        pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "baseline".into(),
                title: "Record baseline".into(),
                created_by: crate::util::HUMAN_AGENT.into(),
                ..Default::default()
            },
        )
        .unwrap();
        let status = pulse_evolve::status(&s, "proj").unwrap();
        if status.baseline_job.is_none() || !status.reason.starts_with("baseline measuring (job ") {
            panic!("status with a hand-queued job: {status:?}");
        }
        let log = once_daemon(&s, crate::daemon::DaemonOpts::default());
        let jobs = baseline_jobs(&s, "proj");
        if jobs.len() != 2 || jobs[1].str("status") != "failed" || measure.calls().len() != 2 {
            panic!("hand-queued job: {jobs:?} ({:?})\n{log}", measure.calls());
        }
        if count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'",
            &[],
        ) != 1
            || count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result'",
                &[],
            ) != 2
        {
            panic!("the repeated failure was not posted once as a finding and twice as a result");
        }
        let text = std::fs::read_to_string(
            must_one(
                &s,
                "SELECT log FROM runs WHERE job_id = ?",
                &[&jobs[1].str("id")],
            )
            .str("log"),
        )
        .unwrap_or_default();
        if !text.contains("finding already on evolution/baseline") {
            panic!("second run log:\n{text}");
        }
        if count(&s, "SELECT SUM(jobs_failed) AS n FROM agents", &[]) != 2
            || count(&s, "SELECT SUM(jobs_done) AS n FROM agents", &[]) != 0
        {
            panic!("worker counters after two failed baseline jobs");
        }
        // A different transient cause is a new finding and uses the short retry.
        measure.set_err(anyhow::anyhow!("database is locked"));
        pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "baseline".into(),
                title: "Record baseline".into(),
                created_by: crate::util::HUMAN_AGENT.into(),
                ..Default::default()
            },
        )
        .unwrap();
        once_daemon(&s, crate::daemon::DaemonOpts::default());
        if count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND summary = 'baseline for proj failed: measuring main failed: database is locked'",
            &[],
        ) != 1
            || count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'",
                &[],
            ) != 2
        {
            panic!("a new cause did not post a new finding");
        }
        let transient = must_one(
            &s,
            "SELECT body FROM posts WHERE kind = 'finding' AND summary = 'baseline for proj failed: measuring main failed: database is locked'",
            &[],
        );
        if !transient
            .str("body")
            .contains("The daemon retries after 3 minutes")
        {
            panic!("transient finding did not use the short retry: {transient:?}");
        }
    }

    #[test]
    fn baseline_run_stop_and_dry_run_requeue() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        pulse_bus::reset_default();
        let s = new_store();
        new_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "evolve.loop", "1");
        loop_records(&s, "proj");
        let measure = FakeMeasure::new(1.0, None);
        let _measure = measure.install();
        let planned = crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
        if planned.is_err() || planned.as_ref().unwrap().len() != 1 {
            panic!("planned {planned:?}");
        }
        let id = planned.unwrap()[0].str("id");
        let lease = |dry_run: bool| -> crate::util::Row {
            let leased = crate::run::take_job(&s, "aurora", "proj", &id, dry_run, TEST_NOW, "", 0);
            match leased {
                Ok((job, _, _)) if !job.0.is_empty() => job,
                Ok((_, _, why)) => panic!("lease: None {why:?}"),
                Err(err) => panic!("lease: {err} \"\""),
            }
        };
        // STOP engaged after the lease: no measurement, no finding, the job goes back with its attempt restored.
        let job = lease(false);
        set_setting(&s, "stop", "1");
        let r = crate::run::run_job(&s, job, "aurora", TEST_NOW, crate::run::RunOpts::default());
        if r.is_err()
            || r.as_ref().unwrap().str("status") != "error"
            || !r
                .as_ref()
                .unwrap()
                .str("summary")
                .contains("not measured: the runner is stopped")
        {
            panic!("run under STOP: {r:?}");
        }
        let j = job_row(&s, &id);
        if j.str("status") != "queued" || j.int("attempts") != 0 || !j.str("lease_agent").is_empty()
        {
            panic!("job under STOP: {j:?}");
        }
        set_setting(&s, "stop", "0");
        // A dry run measures nothing and requeues the job.
        let job = lease(true);
        let r = crate::run::run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            crate::run::RunOpts {
                dry_run: true,
                ..Default::default()
            },
        );
        if r.is_err()
            || r.as_ref().unwrap().str("status") != "dry-run"
            || r.as_ref().unwrap().str("summary") != "dry run: would measure main with 6 fixture(s)"
        {
            panic!("dry run: {r:?}");
        }
        let j = job_row(&s, &id);
        if j.str("status") != "queued" {
            panic!("job after the dry run: {j:?}");
        }
        // A daemon that stops mid-measurement is no failure either.
        let job = lease(false);
        let ctx = pulse_bus::CancellationToken::new();
        ctx.cancel();
        measure.set_err(anyhow::anyhow!(pulse_bus::ContextError::Canceled));
        let r = crate::run::run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            crate::run::RunOpts {
                cancel: ctx,
                ..Default::default()
            },
        );
        if r.is_err()
            || r.as_ref().unwrap().str("status") != "error"
            || !r
                .as_ref()
                .unwrap()
                .str("summary")
                .contains("the daemon stopped")
        {
            panic!("run under a cancelled context: {r:?}");
        }
        let j = job_row(&s, &id);
        if j.str("status") != "queued" || j.int("attempts") != 1 {
            panic!("job after the cancelled run: {j:?}");
        }
        if measure.calls().len() != 1
            || count(&s, "SELECT COUNT(*) AS n FROM posts", &[]) != 0
            || count(&s, "SELECT COUNT(*) AS n FROM evolution_archive", &[]) != 0
        {
            panic!(
                "measured {:?}, posts {}",
                measure.calls(),
                count(&s, "SELECT COUNT(*) AS n FROM posts", &[])
            );
        }
        // A lost lease measures nothing.
        let job = lease(false);
        exec1(
            &s,
            "UPDATE jobs SET generation = generation + 1 WHERE id = ?",
            &[&id],
        );
        let r = crate::run::run_job(&s, job, "aurora", TEST_NOW, crate::run::RunOpts::default());
        if r.is_err() || r.as_ref().unwrap().str("status") != "stale" || measure.calls().len() != 1
        {
            panic!("stale run: {r:?} measured {:?}", measure.calls());
        }
    }
}
