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
    _ex: serde_json::Value,
    o: crate::run::RunOpts,
) -> anyhow::Result<crate::util::Row> {
    // Cross-slice executor is still a skeleton placeholder in this wave; keep the baseline job safe by
    // requeueing dry-run/stopped cases and recording measurement through evolve when reachable.
    let slug = project.str("slug");
    let def = default_branch(project.clone());
    let fixtures = enabled_fixtures(s, slug.clone());
    let mut log = crate::exec::RunLog::default();
    let status;
    let summary;
    let mut archive_id = String::new();
    let mut scores = None;
    if o.dry_run {
        status = "dry-run".to_string();
        summary = format!("dry run: would measure {def} with {fixtures} fixture(s)");
        log.put(&summary);
    } else if {
        let why = crate::run::runner_stopped(s);
        !why.is_empty()
    } {
        status = "error".to_string();
        summary = format!("not measured: {}", crate::run::runner_stopped(s));
        log.put(&summary);
    } else {
        let ctx = o.cancel.with_timeout(baseline_budget(s, slug.clone()));
        log.add(format!("measuring {def} of {slug} with {fixtures} enabled public fixture(s) and {} held out; budget {:?}", pulse_evolve::heldout_count(s).count, baseline_budget(s, slug.clone())));
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
                status = "failed".to_string();
                summary = if e.to_string() == pulse_evolve::err_not_implemented().to_string() {
                    format!(
                        "measuring {def} failed: evolve.Measure is not implemented ({})",
                        crate::run::err_message(&e)
                    )
                } else {
                    format!("measuring {def} failed: {}", crate::run::err_message(&e))
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
    let _ = std::fs::write(&log_path, log.text());
    if let Ok(conn) = s.conn() {
        crate::util::exec1(
            &conn,
            "UPDATE runs SET finished_at = ?, status = ?, seconds = ?, branch = '', commits = 0, summary = ? WHERE id = ?",
            &[&s.now(), &status, &0.0_f64, &summary, &run_id],
        );
        let job_status = if status == "done" {
            "done"
        } else if status == "dry-run" || status == "error" {
            "queued"
        } else {
            "failed"
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
            serde_json::json!({"status": status, "commits": 0, "branch": "", "executor": ""}),
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
