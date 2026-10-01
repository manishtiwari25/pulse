#![allow(
    dead_code,
    reason = "ported Go runner APIs are consumed incrementally across slices"
)]

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
        out.entry(key)
            .and_modify(|v| *v = String::new())
            .or_insert(value);
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

/// The revision the lane measures for `branch`: the branch merged onto `def` the way
/// landing merges it, built from objects alone (`git merge-tree --write-tree` and
/// `commit-tree`) so neither the checkout nor the branch moves. A variant measured on its
/// own stale base would be compared against a baseline it never contained and would miss
/// every fix `def` gained since the branch forked. A branch that already contains `def`
/// is measured as it is; a conflicting one too, so that landing reports the conflict
/// itself. Returns the revision and a note for the log (empty when the branch is measured
/// as it is).
pub(crate) fn measured_variant(root: &str, def: &str, branch: &str) -> (String, String) {
    let short = Duration::from_secs(30);
    if crate::util::git_timeout(root, short, &["merge-base", "--is-ancestor", def, branch]).ok() {
        return (branch.to_string(), String::new());
    }
    let tree = crate::util::git_timeout(
        root,
        Duration::from_secs(120),
        &["merge-tree", "--write-tree", def, branch],
    );
    if !tree.ok() {
        return (branch.to_string(), String::new());
    }
    let tree_id = tree.stdout.lines().next().unwrap_or("").trim().to_string();
    if tree_id.len() < 40 {
        return (branch.to_string(), String::new());
    }
    let def_sha = crate::util::git_timeout(root, short, &["rev-parse", "--verify", def]);
    let branch_sha = crate::util::git_timeout(root, short, &["rev-parse", "--verify", branch]);
    if !def_sha.ok() || !branch_sha.ok() {
        return (branch.to_string(), String::new());
    }
    let message = format!("Measure {branch} on {def}");
    let commit = crate::util::git_timeout_env(
        root,
        short,
        &[
            "GIT_AUTHOR_NAME=pulse".into(),
            "GIT_AUTHOR_EMAIL=pulse@pulse.local".into(),
            "GIT_COMMITTER_NAME=pulse".into(),
            "GIT_COMMITTER_EMAIL=pulse@pulse.local".into(),
        ],
        &[
            "commit-tree",
            &tree_id,
            "-p",
            def_sha.stdout.trim(),
            "-p",
            branch_sha.stdout.trim(),
            "-m",
            &message,
        ],
    );
    let sha = commit.stdout.trim().to_string();
    if !commit.ok() || sha.len() < 40 {
        return (branch.to_string(), String::new());
    }
    (
        sha.clone(),
        format!("measured as {branch} merged onto {def} ({})", &sha[..12]),
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
    let (variant_rev, merged_note) = measured_variant(&root, &def, &branch);
    let variant = match pulse_evolve::measure(
        &ctx,
        s,
        &slug,
        &root,
        &variant_rev,
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
    let mut measured = format!(
        "baseline {}; variant {}; tolerance {:.3}; {}",
        score_line(&baseline),
        score_line(&variant),
        tol,
        reason
    );
    if !merged_note.is_empty() {
        measured.push_str("; ");
        measured.push_str(&merged_note);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::*;

    #[test]
    fn variant_is_measured_merged_onto_main_when_main_moved() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, project, job, root, _calls) = measured_keep_fixture();
        let branch = job.str("branch");
        // main gains a fix after the branch forked; the measured tree must carry both.
        std::fs::write(
            std::path::Path::new(&root).join("fix.txt"),
            "fixed on main\n",
        )
        .unwrap();
        run(&root, &["git", "add", "fix.txt"]);
        run(&root, &["git", "commit", "-q", "-m", "fix on main"]);
        let main_before = ref_of(&root, "main");
        let branch_before = ref_of(&root, &branch);
        let refs: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let seen = refs.clone();
        let _old =
            pulse_evolve::set_measure(Box::new(move |_ctx, _s, slug, _repo, r#ref, _opts| {
                seen.lock().unwrap().push(r#ref.to_string());
                let score = if r#ref == "main" { 0.8 } else { 0.9 };
                Ok(pulse_evolve::Scores {
                    project: slug.into(),
                    total: score,
                    ..Default::default()
                })
            }));
        let (state, note) = land_experiment(
            &s,
            project,
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(state, "landed", "{note}");
        let refs = refs.lock().unwrap().clone();
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0], "main");
        let measured = &refs[1];
        assert!(
            measured != &branch && measured != &branch_before && measured.len() >= 40,
            "variant measured on its stale base: {measured}"
        );
        let parents = run(
            &root,
            &["git", "rev-list", "--parents", "-n", "1", measured],
        );
        assert!(
            parents.contains(&main_before) && parents.contains(&branch_before),
            "measured commit does not join main and the branch: {parents}"
        );
        assert_eq!(
            run(&root, &["git", "show", &format!("{measured}:README.md")]),
            "# measured variant\n"
        );
        assert_eq!(
            run(&root, &["git", "show", &format!("{measured}:fix.txt")]),
            "fixed on main\n"
        );
        // Landing merged the branch commit itself (and removed the branch), not the
        // measurement commit, which stays an unreferenced object.
        run(
            &root,
            &["git", "merge-base", "--is-ancestor", &branch_before, "main"],
        );
        assert!(
            !run(&root, &["git", "rev-list", "--parents", "-n", "1", "main"]).contains(measured),
            "the measurement commit landed"
        );
    }

    #[test]
    fn measured_variant_keeps_the_branch_when_it_contains_main_or_conflicts() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (_s, _project, job, root, _calls) = measured_keep_fixture();
        let branch = job.str("branch");
        // The branch forked from the current main: nothing to merge.
        let (rev, note) = measured_variant(&root, "main", &branch);
        assert_eq!((rev.as_str(), note.as_str()), (branch.as_str(), ""));
        // main edits the same file the branch edited: the merge conflicts; landing reports it.
        std::fs::write(
            std::path::Path::new(&root).join("README.md"),
            "# conflicting main\n",
        )
        .unwrap();
        run(&root, &["git", "commit", "-q", "-am", "conflict on main"]);
        let (rev, note) = measured_variant(&root, "main", &branch);
        assert_eq!((rev.as_str(), note.as_str()), (branch.as_str(), ""));
        assert_eq!(
            run(&root, &["git", "status", "--porcelain"]).trim(),
            "",
            "merge-tree touched the checkout"
        );
    }

    #[test]
    fn missing_measured_keep_is_not_delivered() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, project, job, root, calls) = measured_keep_fixture();
        defer_measured_keep(&s, project.clone(), job.clone(), &root);
        run(&root, &["git", "branch", "-D", &job.str("branch")]);
        let (state, note) = land_experiment(
            &s,
            project,
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(state, "missing", "missing kept artifact: {state}: {note}");
        let current = job_row(&s, &job.str("id"));
        assert_eq!(
            current.str("status"),
            "failed",
            "missing kept artifact was not terminal non-success: {current:?}"
        );
        assert_eq!(current.str("branch"), "");
        assert!(
            pulse_evolve::plan_feedback(&s, "proj", "daemon")
                .unwrap()
                .is_none()
        );
        let archives = pulse_evolve::list_archive(&s, "proj").unwrap();
        assert_eq!(archives.len(), 2, "archives: {archives:?}");
        assert!(archives[0].kept);
        assert_eq!(archives[0].variant_score, 0.9);
        assert_eq!(*calls.lock().unwrap(), 2);
        assert_eq!(pulse_evolve::status(&s, "proj").unwrap().cycles, 1);
    }

    #[test]
    fn measured_landing_records_current_archive() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        for path in ["landed", "resumed", "merged"] {
            let (s, project, job, root, calls) = measured_keep_fixture();
            if path != "landed" {
                defer_measured_keep(&s, project.clone(), job.clone(), &root);
                std::fs::write(std::path::Path::new(&root).join("README.md"), "# proj\n").unwrap();
                if path == "merged" {
                    run(&root, &["git", "merge", "--ff-only", &job.str("branch")]);
                }
            }
            let (state, note) = land_experiment(
                &s,
                project,
                job.clone(),
                "daemon".into(),
                TEST_NOW.into(),
                true,
            );
            let want_state = if path == "merged" { "merged" } else { "landed" };
            assert_eq!(
                state, want_state,
                "successful fake landing: {state}: {note}"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = ?
                    AND json_extract(detail, '$.project') = 'proj' AND json_extract(detail, '$.job') = ?
                    AND json_extract(detail, '$.archive') = 'B-002' AND json_extract(detail, '$.run_id') = ?",
                    &[&format!("land.{want_state}"), &job.str("branch"), &job.str("id"), &last_run_id(&s, job.str("id"))],
                ),
                1,
                "{path} left no durable current-run/current-archive landing proof"
            );
            assert!(
                pulse_evolve::plan_feedback(&s, "proj", "daemon")
                    .unwrap()
                    .is_some()
            );
            assert!(
                pulse_evolve::plan_feedback(&s, "proj", "daemon")
                    .unwrap()
                    .is_none()
            );
            assert_eq!(*calls.lock().unwrap(), 2);
            assert_eq!(
                count(&s, "SELECT COUNT(*) AS n FROM evolution_archive", &[]),
                2
            );
        }
    }

    #[test]
    fn landing_receipt_failure_retries_finalization() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, project, job, _root, calls) = measured_keep_fixture();
        exec1(
            &s,
            "CREATE TRIGGER reject_finalization BEFORE INSERT ON events WHEN NEW.action IN ('land.landed', 'land.merged', 'land.dropped') BEGIN SELECT RAISE(ABORT, 'fake finalization refused'); END",
            &[],
        );
        let (state, note) = land_experiment(
            &s,
            project.clone(),
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(
            state, "error",
            "receipt failure was hidden: {state}: {note}"
        );
        assert!(
            note.contains("fake finalization refused"),
            "receipt failure was hidden: {state}: {note}"
        );
        let current = job_row(&s, &job.str("id"));
        assert_eq!(current.str("status"), "done");
        assert_eq!(current.str("branch"), job.str("branch"));
        assert!(
            pulse_evolve::plan_feedback(&s, "proj", "daemon")
                .unwrap()
                .is_none()
        );
        exec1(&s, "DROP TRIGGER reject_finalization", &[]);
        let (state, note) = land_experiment(
            &s,
            project,
            job_row(&s, &job.str("id")),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(
            state, "merged",
            "receipt retry did not finalize: {state}: {note}"
        );
        assert_eq!(*calls.lock().unwrap(), 2);
        assert_eq!(
            count(&s, "SELECT COUNT(*) AS n FROM evolution_archive", &[]),
            2
        );
        assert!(
            pulse_evolve::plan_feedback(&s, "proj", "daemon")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn landing_receipt_failure_after_evidence_keeps_carrier() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (project, root) = new_repo(&s);
        join(&s, "aurora");
        let branch = "pulse/aurora/receipt-failure";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        crate::test_helpers_landing::write_file(&root, "internal/api.go", "package internal\n");
        run(&root, &["git", "add", "-A"]);
        run(&root, &["git", "commit", "-q", "-m", "source change"]);
        run(&root, &["git", "switch", "-q", "main"]);
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "Deliver source".into(),
            "do it".into(),
            1,
            0,
            "manual".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', branch = ?, lease_agent = ? WHERE id = ?",
            &[&branch, &"aurora", &job.str("id")],
        );
        exec1(
            &s,
            "CREATE TRIGGER reject_code_delivery_receipt BEFORE INSERT ON events\n\t\tWHEN NEW.action = 'land.landed' BEGIN SELECT RAISE(ABORT, 'receipt refused'); END",
            &[],
        );

        let result = crate::land::land_branch(
            &s,
            project,
            branch.into(),
            "daemon".into(),
            TEST_NOW.into(),
            crate::land::LandOpts {
                job: job_row(&s, &job.str("id")),
                verify: false,
                ..Default::default()
            },
        );
        match &result {
            Err(err) if err.to_string().contains("receipt refused") => {}
            Ok((state, note)) => {
                panic!("receipt failure hidden: state={state:?} note={note:?} err=<nil>")
            }
            Err(err) => panic!("receipt failure hidden: state=\"\" note=\"\" err={err}"),
        }
        let current = job_row(&s, &job.str("id"));
        if current.str("branch") != branch || current.str("status") != "done" {
            panic!("failed receipt cleared carrier: {current:?}");
        }
        let (exists, why) = crate::land::branch_exists(root.clone(), branch.into());
        if !exists || !why.is_empty() {
            panic!("failed receipt removed branch: exists={exists} why={why:?}");
        }
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed'",
            &[],
        );
        if n != 0 {
            panic!("failed receipt left success event: {n}");
        }
    }

    #[test]
    fn discard_finalization_retries_without_another_measurement() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, project, job, root, calls) = measured_keep_fixture();
        let before = ref_of(&root, "HEAD");
        let _old =
            pulse_evolve::set_measure(Box::new(move |_ctx, _s, slug, _repo, r#ref, _opts| {
                *calls.lock().unwrap() += 1;
                let score = if r#ref == "main" { 0.8 } else { 0.7 };
                Ok(pulse_evolve::Scores {
                    project: slug.into(),
                    total: score,
                    ..Default::default()
                })
            }));
        let lock = std::path::Path::new(&root)
            .join(".git")
            .join("refs")
            .join("heads")
            .join(format!("{}.lock", job.str("branch")));
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(&lock, "").unwrap();
        let (state, note) = land_experiment(
            &s,
            project.clone(),
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(
            state, "deferred",
            "failed discard cleanup did not stay pending: {state}: {note}"
        );
        assert!(note.contains("archive B-002"));
        assert_eq!(job_row(&s, &job.str("id")).str("branch"), job.str("branch"));
        assert!(
            pulse_evolve::plan_feedback(&s, "proj", "daemon")
                .unwrap()
                .is_none()
        );
        std::fs::remove_file(lock).unwrap();
        let (state, note) = land_experiment(
            &s,
            project,
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(
            state, "dropped",
            "discard did not finish after its cleanup gate opened: {state}: {note}"
        );
        assert_eq!(
            count(&s, "SELECT COUNT(*) AS n FROM evolution_archive", &[]),
            2
        );
        assert_eq!(ref_of(&root, "HEAD"), before);
        let (exists, why) = crate::land::branch_exists(root, job.str("branch"));
        assert!(
            !exists && why.is_empty(),
            "finalized discard retained the branch: {exists}, {why}"
        );
    }
}
