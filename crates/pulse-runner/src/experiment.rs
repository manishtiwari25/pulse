use std::collections::BTreeMap;
use std::time::Duration;

pub(crate) const EXPERIMENT_MEASURE_BUDGET: Duration = Duration::from_secs(90 * 60);
pub(crate) const DEFAULT_EVOLVE_TOLERANCE: f64 = 0.02;

/// Go: func briefFields(body string) map[string]string
pub(crate) fn brief_fields(body: String) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in body.lines() {
        let line = line.trim().trim_start_matches(['-', '*', ' ']);
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim_matches(['*', ' ', '\t']).to_lowercase();
        let value = value.trim().trim_matches('`').to_string();
        if out.contains_key(&key) {
            out.insert(key, String::new());
        } else {
            out.insert(key, value);
        }
    }
    out
}

/// Go: func evolveTolerance(s *store.Store) float64
pub(crate) fn evolve_tolerance(s: &pulse_store::Store) -> f64 {
    s.setting("evolve.tolerance", "")
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| *v >= 0.0)
        .unwrap_or(DEFAULT_EVOLVE_TOLERANCE)
}

/// Go: func experimentSuggestion(job row) (suggestion, hypothesis string)
pub(crate) fn experiment_suggestion(job: crate::util::Row) -> (String, String) {
    let f = brief_fields(job.str("brief"));
    let mut suggestion = f.get("suggestion").cloned().unwrap_or_default();
    let hypothesis = f.get("hypothesis").cloned().unwrap_or_default();
    if suggestion.is_empty()
        && let Some(rest) = job.str("source").strip_prefix("suggestion:")
    {
        suggestion = rest.trim().to_string();
    }
    (suggestion, hypothesis)
}

/// Go: func experimentThread(s *store.Store, slug, suggestion string) (area, thread, on string)
pub(crate) fn experiment_thread(
    s: &pulse_store::Store,
    slug: String,
    suggestion: String,
) -> (String, String, String) {
    if !suggestion.is_empty()
        && let Ok(conn) = s.conn()
    {
        let p = crate::util::must_one(
            &conn,
            "SELECT area, thread FROM posts WHERE id = ? AND project = ?",
            &[&suggestion, &slug],
        );
        if !p.0.is_empty() {
            return (p.str("area"), p.str("thread"), suggestion);
        }
    }
    let mut thread = crate::util::slugify(&suggestion, 40);
    if thread.is_empty() || thread == "x" {
        thread = "experiments".to_string();
    }
    ("evolution".into(), thread, String::new())
}

/// Go: func lastRunID(s *store.Store, jobID string) string
pub(crate) fn last_run_id(s: &pulse_store::Store, job_id: String) -> String {
    let Ok(conn) = s.conn() else {
        return String::new();
    };
    crate::util::must_one(
        &conn,
        "SELECT id FROM runs WHERE job_id = ? ORDER BY started_at DESC, id DESC LIMIT 1",
        &[&job_id],
    )
    .str("id")
}

/// Go: func scoreLine(sc *evolve.Scores) string
pub(crate) fn score_line(sc: &pulse_evolve::api::Scores) -> String {
    format!(
        "{:.3} over {} fixture(s), {} held out",
        sc.total,
        sc.fixtures.len(),
        sc.heldout
    )
}

/// Go: func landExperiment(s *store.Store, project row, job row, actor, now string, verify bool) (state, note string)
pub(crate) fn land_experiment(
    s: &pulse_store::Store,
    project: crate::util::Row,
    job: crate::util::Row,
    actor: String,
    now: String,
    verify: bool,
) -> (String, String) {
    let ctx = pulse_bus::CancellationToken::new();
    land_experiment_context(&ctx, s, project, job, actor, now, verify)
}

/// Go: func landExperimentContext(parent context.Context, s *store.Store, project row, job row, actor, now string, verify bool) (state, note string)
pub(crate) fn land_experiment_context(
    parent: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: crate::util::Row,
    job: crate::util::Row,
    actor: String,
    now: String,
    verify: bool,
) -> (String, String) {
    let root = project.str("path");
    let def = {
        let d = project.str("default_branch");
        if d.is_empty() { "main".to_string() } else { d }
    };
    let slug = project.str("slug");
    let branch = job.str("branch");
    let (suggestion, hypothesis) = experiment_suggestion(job.clone());
    let (area, thread, on) = experiment_thread(s, slug.clone(), suggestion.clone());
    let agent = crate::board::actor_agent(s, actor.clone());
    let post = |resolution: &str, summary: String, body: String| {
        let _ = crate::board::create_post(
            s,
            now.clone(),
            crate::board::PostOpts {
                kind: "result".into(),
                project: slug.clone(),
                area: area.clone(),
                thread: thread.clone(),
                summary: crate::util::trunc(&summary, 230),
                body,
                agent: agent.clone(),
                on: on.clone(),
                resolution: resolution.into(),
                date: now.clone(),
                ..Default::default()
            },
        );
    };
    if let why = crate::landing_context::landing_stop(parent, s, root.clone())
        && !why.is_empty()
    {
        return ("stopped".into(), why);
    }
    let run_id = last_run_id(s, job.str("id"));
    let prior = match crate::landing_evidence::experiment_archive(
        s,
        slug.clone(),
        branch.clone(),
        run_id.clone(),
    ) {
        Ok(r) => r,
        Err(e) => return ("error".into(), crate::run::err_message(&e)),
    };
    let (exists, unreadable) = crate::land::branch_exists(root.clone(), branch.clone());
    if !unreadable.is_empty() {
        return ("unreadable".into(), format!("{branch} kept: {unreadable}"));
    }
    if !exists && (prior.0.is_empty() || prior.int("kept") != 0) {
        let archive_id = prior.str("id");
        let why = format!(
            "experiment artifact {branch} is missing; delivery was not confirmed; any measured archive is retained"
        );
        if let Err(e) = crate::landing_evidence::fail_missing_experiment(
            s,
            project.clone(),
            job.clone(),
            actor.clone(),
            run_id,
            archive_id,
            why.clone(),
        ) {
            return ("error".into(), crate::run::err_message(&e));
        }
        post(
            "dropped",
            format!("experiment {suggestion} failed: branch {branch} is missing"),
            format!(
                "Job {} has no branch artifact (`{branch}` does not exist in {root}). A measured keep is not proof of delivery. The job failed; its measurements remain unchanged. Owner review is needed.\n",
                job.str("id")
            ),
        );
        return ("missing".into(), why);
    }
    let tol = evolve_tolerance(s);
    let discard = |archive_id: String, reason: String, measured: String| -> (String, String) {
        if let why = crate::landing_context::landing_stop(parent, s, root.clone())
            && !why.is_empty()
        {
            return ("stopped".into(), why);
        }
        if exists {
            let removed = crate::util::git_timeout(
                &root,
                Duration::from_secs(30),
                &["branch", "-D", &branch],
            );
            if !removed.ok() {
                return (
                    "deferred".into(),
                    format!(
                        "experiment discard (archive {archive_id}) awaits branch removal: {}",
                        removed.stderr.trim()
                    ),
                );
            }
        }
        if let Err(e) = crate::landing_evidence::record_landing(
            s,
            project.clone(),
            branch.clone(),
            actor.clone(),
            "land.dropped".into(),
            format!(" | dropped: {reason}"),
            crate::land::LandOpts {
                job: job.clone(),
                archive_id: archive_id.clone(),
                run_id: run_id.clone(),
                ..Default::default()
            },
            BTreeMap::from([("why".into(), serde_json::json!(reason.clone()))]),
        ) {
            return ("error".into(), crate::run::err_message(&e));
        }
        post(
            "dropped",
            format!("experiment {suggestion} dropped: {reason}"),
            format!(
                "Branch `{branch}` was measured and discarded (archive {archive_id}): {measured}.\n\nThe branch is deleted; nothing changed on `{def}`.\n"
            ),
        );
        (
            "dropped".into(),
            format!("{branch} dropped ({reason}); archive {archive_id}"),
        )
    };
    if !prior.0.is_empty() {
        if prior.int("kept") == 0 {
            return discard(prior.str("id"), prior.str("note"), prior.str("note"));
        }
        match crate::land::land_branch(
            s,
            project.clone(),
            branch.clone(),
            actor.clone(),
            now.clone(),
            crate::land::LandOpts {
                job: job.clone(),
                verify,
                ctx: Some(parent.clone()),
                archive_id: prior.str("id"),
                run_id: run_id.clone(),
                ..Default::default()
            },
        ) {
            Ok((state, note)) if state == "landed" || state == "merged" => {
                post(
                    "tested",
                    format!("experiment {suggestion} kept: {}", prior.str("note")),
                    format!(
                        "Branch `{branch}` was measured and kept earlier (archive {}): {}.\n\nIt landed under ADR-024: {note}.\n",
                        prior.str("id"),
                        prior.str("note")
                    ),
                );
                return (
                    state,
                    format!("{branch} kept (archive {}); {note}", prior.str("id")),
                );
            }
            Ok((state, note)) => {
                return (
                    state,
                    format!(
                        "experiment kept earlier (archive {}) but {note}",
                        prior.str("id")
                    ),
                );
            }
            Err(e) => return ("error".into(), crate::run::err_message(&e)),
        }
    }
    let ctx = parent.with_timeout(EXPERIMENT_MEASURE_BUDGET);
    let unavailable = |what: String, err: anyhow::Error| -> (String, String) {
        let summary = format!("experiment {suggestion} waits: measurement unavailable ({what})");
        let job_thread = crate::util::slugify(&job.str("id"), 40);
        if let Ok(conn) = s.conn() {
            let p = crate::util::must_one(
                &conn,
                "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'result' AND area = 'jobs' AND thread = ? AND summary = ?",
                &[&slug, &job_thread, &crate::util::trunc(&summary, 230)],
            );
            if p.0.is_empty() {
                let _ = crate::board::create_post(
                    s,
                    now.clone(),
                    crate::board::PostOpts {
                        kind: "result".into(),
                        project: slug.clone(),
                        area: "jobs".into(),
                        thread: job_thread,
                        summary: crate::util::trunc(&summary, 230),
                        body: format!(
                            "Branch `{branch}` of experiment {suggestion} is not landed: {what} ({}). The job stays in review with its branch until the measurement is available; a human may land it with `pulse land --project {slug} --branch {branch}`.\n",
                            crate::run::err_message(&err)
                        ),
                        agent: agent.clone(),
                        date: now.clone(),
                        ..Default::default()
                    },
                );
            }
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "land.unmeasured",
                &branch,
                serde_json::json!({"project": slug, "job": job.str("id"), "what": what}),
            );
        }
        (
            "unmeasured".into(),
            format!("{branch} not landed: measurement unavailable ({what})"),
        )
    };
    let baseline = match pulse_evolve::measure(
        &ctx,
        s,
        &slug,
        &root,
        &def,
        pulse_evolve::MeasureOptions::default(),
    ) {
        Ok(v) => v,
        Err(e) if e.to_string() == pulse_evolve::err_not_implemented().to_string() => {
            return unavailable("evolve.Measure is not implemented".into(), e);
        }
        Err(e) => {
            return (
                "deferred".into(),
                format!(
                    "{branch} not landed: measuring {def} failed: {}",
                    crate::run::err_message(&e)
                ),
            );
        }
    };
    if let why = crate::landing_context::landing_stop(parent, s, root.clone())
        && !why.is_empty()
    {
        return ("stopped".into(), why);
    }
    let variant = match pulse_evolve::measure(
        &ctx,
        s,
        &slug,
        &root,
        &branch,
        pulse_evolve::MeasureOptions::default(),
    ) {
        Ok(v) => v,
        Err(e) if e.to_string() == pulse_evolve::err_not_implemented().to_string() => {
            return unavailable("evolve.Measure is not implemented".into(), e);
        }
        Err(e) => {
            return (
                "deferred".into(),
                format!(
                    "{branch} not landed: measuring {branch} failed: {}",
                    crate::run::err_message(&e)
                ),
            );
        }
    };
    let (keep, reason) = pulse_evolve::keep(Some(&baseline), Some(&variant), tol);
    let sandbox = if variant.sandbox.is_empty() {
        baseline.sandbox.clone()
    } else {
        variant.sandbox.clone()
    };
    let archive_id = match pulse_evolve::archive_row(
        s,
        pulse_evolve::ArchiveRow {
            project: slug.clone(),
            suggestion_post: suggestion.clone(),
            hypothesis: hypothesis.clone(),
            branch: branch.clone(),
            baseline: Some(baseline.clone()),
            variant: Some(variant.clone()),
            tolerance: tol,
            kept: keep,
            run_id: run_id.clone(),
            sandbox,
            note: reason.clone(),
        },
    ) {
        Ok(v) => v,
        Err(e) if e.to_string() == pulse_evolve::err_not_implemented().to_string() => {
            return unavailable("evolve.Archive is not implemented".into(), e);
        }
        Err(e) => {
            return (
                "deferred".into(),
                format!(
                    "{branch} not landed: archiving failed: {}",
                    crate::run::err_message(&e)
                ),
            );
        }
    };
    let measured = format!(
        "baseline {}; variant {}; tolerance {:.3}; {}",
        score_line(&baseline),
        score_line(&variant),
        tol,
        reason
    );
    let hyp = if hypothesis.is_empty() {
        String::new()
    } else {
        format!("\n\nHypothesis: {hypothesis}")
    };
    if !keep {
        return discard(archive_id, reason, measured + &hyp);
    }
    match crate::land::land_branch(
        s,
        project,
        branch.clone(),
        actor.clone(),
        now.clone(),
        crate::land::LandOpts {
            job: job.clone(),
            verify,
            ctx: Some(parent.clone()),
            archive_id: archive_id.clone(),
            run_id,
            ..Default::default()
        },
    ) {
        Ok((state, note)) if state == "landed" || state == "merged" => {
            post(
                "tested",
                format!("experiment {suggestion} kept: {reason}"),
                format!(
                    "Branch `{branch}` was measured and kept (archive {archive_id}): {measured}.{hyp}\n\nIt landed under ADR-024: {note}.\n"
                ),
            );
            (
                state,
                format!("{branch} kept ({reason}); archive {archive_id}; {note}"),
            )
        }
        Ok((state, note)) => {
            if let Ok(conn) = s.conn() {
                crate::util::event_at(
                    &conn,
                    &now,
                    &actor,
                    "land.kept-waiting",
                    &branch,
                    serde_json::json!({"project": slug, "job": job.str("id"), "archive": archive_id, "state": state}),
                );
            }
            (
                state,
                format!("experiment kept ({reason}; archive {archive_id}) but {note}"),
            )
        }
        Err(e) => ("error".into(), crate::run::err_message(&e)),
    }
}
