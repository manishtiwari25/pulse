#![allow(dead_code, reason = "not every item has a caller in the crate")]

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub(crate) const EXPERIMENT_MEASURE_BUDGET: Duration = Duration::from_secs(90 * 60);
pub(crate) const DEFAULT_EVOLVE_TOLERANCE: f64 = 0.02;

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

pub(crate) fn evolve_tolerance(s: &pulse_store::Store) -> f64 {
    s.setting("evolve.tolerance", "")
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| *v >= 0.0)
        .unwrap_or(DEFAULT_EVOLVE_TOLERANCE)
}

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

pub(crate) fn experiment_parent(job: crate::util::Row) -> String {
    brief_fields(job.str("brief"))
        .get("parent")
        .and_then(|v| v.split_whitespace().next())
        .unwrap_or("")
        .trim()
        .to_string()
}

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

pub(crate) fn default_commit(root: &str, def: &str) -> String {
    let rev = crate::util::git_timeout(
        root,
        Duration::from_secs(30),
        &["rev-parse", "--verify", &format!("{def}^{{commit}}")],
    );
    if rev.ok() {
        rev.stdout.trim().to_string()
    } else {
        String::new()
    }
}

pub(crate) fn archive_patch_path(home: &str, slug: &str, archive_id: &str) -> String {
    Path::new(home)
        .join("patches")
        .join("evolve")
        .join(crate::util::slugify(slug, 80))
        .join(format!("{archive_id}.patch"))
        .display()
        .to_string()
}

fn next_archive_id(s: &pulse_store::Store, project: &str) -> String {
    s.conn()
        .and_then(|conn| {
            conn.query_row(
                "SELECT COALESCE(MAX(CAST(SUBSTR(id, 3) AS INTEGER)), 0) + 1 FROM evolution_archive WHERE project = ?",
                [project],
                |r| r.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
        .map(|next| format!("B-{next:03}"))
        .unwrap_or_default()
}

fn merge_base(root: &str, def: &str, branch: &str) -> String {
    let base =
        crate::util::git_timeout(root, Duration::from_secs(30), &["merge-base", def, branch]);
    if base.ok() {
        base.stdout.trim().to_string()
    } else {
        String::new()
    }
}

fn save_experiment_patch(
    s: &pulse_store::Store,
    root: &str,
    slug: &str,
    def: &str,
    branch: &str,
    archive_id: &str,
    log: (&str, &str),
) -> (String, String) {
    let base = merge_base(root, def, branch);
    if base.is_empty() || archive_id.is_empty() {
        return (String::new(), base);
    }
    let path = archive_patch_path(&s.home, slug, archive_id);
    let Some(parent) = Path::new(&path).parent() else {
        return (String::new(), base);
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        log_patch_skip(
            s,
            log.0,
            log.1,
            branch,
            format!("creating patch directory failed: {e}"),
        );
        return (String::new(), base);
    }
    let out = Command::new(pulse_gitenv::git_program())
        .args(["-C", root, "diff", "--binary", &base, branch])
        .output();
    match out {
        Ok(out) if out.status.success() => {
            if let Err(e) = std::fs::write(&path, out.stdout) {
                log_patch_skip(
                    s,
                    log.0,
                    log.1,
                    branch,
                    format!("writing patch failed: {e}"),
                );
                return (String::new(), base);
            }
            (path, base)
        }
        Ok(out) => {
            log_patch_skip(
                s,
                log.0,
                log.1,
                branch,
                format!(
                    "git diff failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            );
            (String::new(), base)
        }
        Err(e) => {
            log_patch_skip(s, log.0, log.1, branch, format!("git diff failed: {e}"));
            (String::new(), base)
        }
    }
}

fn log_patch_skip(s: &pulse_store::Store, actor: &str, now: &str, branch: &str, reason: String) {
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "evolution.patch_skipped",
            branch,
            serde_json::json!({"branch": branch, "reason": reason}),
        );
    }
}

/// Why a measurement must wait, if the machine is saturated. A measurement is
/// two builds, so the lane obeys the daemon's hold like a leased run does,
/// instead of adding load on top of the runs the governor already refused.
pub(crate) fn measure_hold(s: &pulse_store::Store) -> Option<String> {
    crate::daemon_load::hold_reason(&measure_limits(s), &crate::daemon_load::pressure())
}

/// The governor's limits as the lane sees them (no `--max-jobs` flag here:
/// the lane measures on the daemon's and on `pulse land`'s machine alike).
fn measure_limits(s: &pulse_store::Store) -> crate::daemon_load::Limits {
    crate::daemon_load::limits(s, &crate::daemon_load::Machine::detect(), 0)
}

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
    let parent_archive = experiment_parent(job.clone());
    let (area, thread, on) = experiment_thread(s, slug.clone(), suggestion.clone());
    let agent = crate::board::actor_agent(s, actor.clone());
    // A result post that fails must not undo the landing decision, but it must not
    // vanish either: owners read the board for "what landed", so the miss goes to
    // stderr, which the daemon log captures (same as land.rs).
    let post = |resolution: &str, summary: String, body: String| {
        if let Err(e) = crate::board::create_post(
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
        ) {
            eprintln!(
                "{}",
                experiment_result_post_failed_warning(&branch, resolution, &e)
            );
        }
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
    let limits = measure_limits(s);
    if let Some(why) = crate::daemon_load::hold_reason(&limits, &crate::daemon_load::pressure()) {
        return (
            "deferred".into(),
            format!("{branch} not landed: measuring waits - {why}"),
        );
    }
    // The measurement's builds obey daemon.build_jobs as a worker's do;
    // pulse-evolve has no store, so the cap is handed over here.
    pulse_evolve::process::set_build_jobs(usize::try_from(limits.build_jobs).unwrap_or(0));
    let _measuring = crate::daemon_load::MeasuringMark::new(&s.home);
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
            if p.0.is_empty()
                && let Err(e) = crate::board::create_post(
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
                )
            {
                eprintln!(
                    "{}",
                    experiment_result_post_failed_warning(&branch, "unmeasured", &e)
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
    let commit = default_commit(&root, &def);
    let fixture_hash = match pulse_evolve::baseline_fixture_hash(s, &slug) {
        Ok(v) => v,
        Err(e) => {
            return (
                "deferred".into(),
                format!(
                    "{branch} not landed: baseline cache key failed: {}",
                    crate::run::err_message(&e)
                ),
            );
        }
    };
    let mut baseline_cache_note = "baseline: measured".to_string();
    let cached = match pulse_evolve::cached_baseline(s, &slug, &commit, &fixture_hash) {
        Ok(v) => v,
        Err(e) => {
            return (
                "deferred".into(),
                format!(
                    "{branch} not landed: reading baseline cache failed: {}",
                    crate::run::err_message(&e)
                ),
            );
        }
    };
    let baseline = if let Some(cached) = cached {
        let short = commit.get(..7).unwrap_or(&commit);
        baseline_cache_note = format!("baseline: reused {}", cached.at);
        if let Ok(conn) = s.conn() {
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "evolution.baseline_cache",
                &commit,
                serde_json::json!({"project": slug, "commit": commit, "score": cached.score, "at": cached.at, "line": format!("baseline for {short} reused from {} (score {:.3})", cached.at, cached.score)}),
            );
        }
        cached.scores
    } else {
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
        if let Err(e) =
            pulse_evolve::upsert_baseline(s, &slug, &commit, &fixture_hash, &baseline, &run_id)
        {
            return (
                "deferred".into(),
                format!(
                    "{branch} not landed: writing baseline cache failed: {}",
                    crate::run::err_message(&e)
                ),
            );
        }
        baseline
    };
    if let why = crate::landing_context::landing_stop(parent, s, root.clone())
        && !why.is_empty()
    {
        return ("stopped".into(), why);
    }
    let (variant_rev, merged_note) = measured_variant(&root, &def, &branch);
    // The variant and its base are measured in the card's diff scope: the
    // whole-tree baseline stays the cached one, the fixtures the card scopes
    // (and its own check) are measured again on the base and laid over it.
    let card_check = pulse_evolve::card_check_of(&job.str("brief"));
    let pair = match pulse_evolve::measure_pair(
        &ctx,
        s,
        &slug,
        &root,
        &def,
        &variant_rev,
        Some(baseline),
        &card_check,
        None,
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
    let (keep, reason) = pulse_evolve::keep_pair(&pair, tol);
    let pulse_evolve::MeasuredPair {
        base: baseline,
        variant,
        scope,
        skipped,
    } = pair;
    let disputed = if skipped.is_some() {
        Vec::new()
    } else {
        pulse_evolve::hinges_on(&baseline, &variant, tol)
    };
    let (keep, reason, variant, dispute_note) = if disputed.is_empty() {
        (keep, reason, variant, String::new())
    } else {
        let measure_again = |r#ref: &str| {
            pulse_evolve::measure(
                &ctx,
                s,
                &slug,
                &root,
                r#ref,
                pulse_evolve::MeasureOptions {
                    fixtures: disputed.clone(),
                    exact: true,
                    scope: Some(scope.clone()),
                    card_check: card_check.clone(),
                    ..Default::default()
                },
            )
        };
        let again = measure_again(&def).and_then(|b| measure_again(&variant_rev).map(|v| (b, v)));
        let (baseline_again, variant_again) = match again {
            Ok(v) => v,
            Err(e) => {
                return (
                    "deferred".into(),
                    format!(
                        "{branch} not landed: measuring {} again failed: {}",
                        disputed.join(", "),
                        crate::run::err_message(&e)
                    ),
                );
            }
        };
        let r = pulse_evolve::reconcile(
            &baseline,
            &variant,
            &baseline_again,
            &variant_again,
            &disputed,
        );
        let (keep, reason) = pulse_evolve::keep(Some(&baseline), Some(&r.variant), tol);
        let note = format!(
            "the discard hinged on {}, measured again on both trees: {}",
            disputed.join(", "),
            r.notes.join("; ")
        );
        if let Ok(conn) = s.conn() {
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "evolution.measured_again",
                &branch,
                serde_json::json!({"project": slug, "fixtures": disputed, "noise": r.noise, "reproduced": r.reproduced, "kept": keep, "line": format!("{branch}: {note}; {reason}")}),
            );
        }
        (keep, reason, r.variant, note)
    };
    let archive_note = if dispute_note.is_empty() {
        format!("{baseline_cache_note}; {reason}")
    } else {
        format!("{baseline_cache_note}; {dispute_note}; {reason}")
    };
    let sandbox = if variant.sandbox.is_empty() {
        baseline.sandbox.clone()
    } else {
        variant.sandbox.clone()
    };
    let expected_archive_id = next_archive_id(s, &slug);
    let (patch, base_commit) = save_experiment_patch(
        s,
        &root,
        &slug,
        &def,
        &branch,
        &expected_archive_id,
        (&actor, &now),
    );
    let archive_id = match pulse_evolve::archive_row(
        s,
        pulse_evolve::ArchiveRow {
            project: slug.clone(),
            suggestion_post: suggestion.clone(),
            hypothesis: hypothesis.clone(),
            branch: branch.clone(),
            parent: parent_archive.clone(),
            patch,
            base_commit,
            baseline: Some(baseline.clone()),
            variant: Some(variant.clone()),
            tolerance: tol,
            kept: keep,
            run_id: run_id.clone(),
            sandbox,
            note: archive_note.clone(),
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
        archive_note
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

/// The stderr line written when an experiment-landing result post could not be
/// created: the landing outcome itself stands, so the owner needs the branch and the
/// resolution to find it without the board entry.
pub(crate) fn experiment_result_post_failed_warning(
    branch: &str,
    resolution: &str,
    err: &anyhow::Error,
) -> String {
    format!(
        "experiment landing: `{branch}` resolved as {resolution} but the result post could not be written to the board: {err:#}; the outcome is recorded in the landing evidence"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_daemon::*;

    #[test]
    fn experiment_result_post_failure_warning_names_branch_and_cause() {
        let err = anyhow::anyhow!("board unavailable").context("create_post");
        let line = experiment_result_post_failed_warning("evolve/B-013", "landed", &err);
        assert!(line.contains("evolve/B-013"), "{line}");
        assert!(line.contains("landed"), "{line}");
        assert!(line.contains("create_post: board unavailable"), "{line}");
        assert!(line.contains("result post could not be written"), "{line}");
    }

    #[test]
    fn archive_patch_path_uses_evolve_project_archive_folder() {
        assert_eq!(
            archive_patch_path("/home/me/.pulse", "proj name", "B-017"),
            "/home/me/.pulse/patches/evolve/proj-name/B-017.patch"
        );
    }

    #[test]
    fn experiment_parent_reads_parent_line_from_brief() {
        let mut job = crate::util::Row::new();
        job.insert(
            "brief",
            serde_json::Value::String(
                "Suggestion: S-1\nParent: B-017 (patch ~/.pulse/patches/evolve/proj/B-017.patch, forked from abc, cmp 0.67)\n"
                    .into(),
            ),
        );
        assert_eq!(experiment_parent(job), "B-017");
    }

    #[test]
    fn measurement_waits_while_the_machine_is_saturated() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, project, job, _root, calls) = measured_keep_fixture();
        set_setting(&s, "daemon.build_jobs", "3");
        let marks: std::sync::Arc<std::sync::Mutex<Vec<(usize, usize)>>> = Default::default();
        let seen = marks.clone();
        let home = s.home.clone();
        let _old =
            pulse_evolve::set_measure(Box::new(move |_ctx, _s, slug, _repo, r#ref, _opts| {
                seen.lock().unwrap().push((
                    crate::daemon_load::measuring(&home),
                    pulse_evolve::process::build_jobs(),
                ));
                let score = if r#ref == "main" { 0.8 } else { 0.9 };
                Ok(pulse_evolve::Scores {
                    project: slug.into(),
                    total: score,
                    ..Default::default()
                })
            }));
        let saturated = crate::daemon_load::pin_pressure(crate::daemon_load::Pressure {
            load1: Some(40.0),
            ..crate::daemon_load::CALM
        });
        let (state, note) = land_experiment(
            &s,
            project.clone(),
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(state, "deferred", "{note}");
        assert!(
            note.contains("not landed: measuring waits - machine saturated: load 40.0 on"),
            "{note}"
        );
        assert_eq!(*calls.lock().unwrap(), 0, "measured under a hold");
        assert_eq!(
            crate::daemon_maintenance::experiment_retry(&s, &state, &note),
            Duration::from_secs(120),
            "a held measurement waits the short retry"
        );
        assert_eq!(job_row(&s, &job.str("id")).str("branch"), job.str("branch"));
        drop(saturated);
        let (state, note) = land_experiment(
            &s,
            project,
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(state, "landed", "{note}");
        // Both measurements ran under the lane's mark with the daemon's build
        // cap handed to pulse-evolve, and the mark went with them.
        assert_eq!(marks.lock().unwrap().clone(), vec![(1, 3), (1, 3)]);
        assert_eq!(crate::daemon_load::measuring(&s.home), 0);
        pulse_evolve::process::set_build_jobs(0);
    }

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

    /// One metric fixture at weight 2 and one request fixture at weight 1:
    /// the request fixture flipping from 1 to 0 moves the total by a third,
    /// far beyond the tolerance.
    fn two_fixture_scores(slug: &str, request: f64, only: &[String]) -> pulse_evolve::Scores {
        let mut fixtures = vec![
            pulse_evolve::FixtureScore {
                name: "metric-size".into(),
                kind: "public".into(),
                score: 0.5,
                weight: 2.0,
                metric: true,
                note: String::new(),
            },
            pulse_evolve::FixtureScore {
                name: "starter-proposes-first".into(),
                kind: "public".into(),
                score: request,
                weight: 1.0,
                metric: false,
                note: String::new(),
            },
        ];
        if !only.is_empty() {
            fixtures.retain(|f| only.contains(&f.name));
        }
        pulse_evolve::Scores {
            project: slug.into(),
            total: pulse_evolve::weighted_total(&fixtures),
            fixtures,
            ..Default::default()
        }
    }

    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<String>, bool)>>>;

    /// A measurement whose request fixture scores `first` on the baseline
    /// and 0 on the variant the first time, then `again` on both trees when
    /// measured again.
    fn flaky_measure(first: f64, again: f64) -> Seen {
        let seen: Seen = Default::default();
        let log = seen.clone();
        let _old =
            pulse_evolve::set_measure(Box::new(move |_ctx, _s, slug, _repo, r#ref, opts| {
                let mut log = log.lock().unwrap();
                log.push((r#ref.to_string(), opts.fixtures.clone(), opts.exact));
                let request = match (log.len(), r#ref == "main") {
                    (1, true) => first,
                    (2, false) => 0.0,
                    (3, true) => again,
                    _ => 0.0,
                };
                Ok(two_fixture_scores(slug, request, &opts.fixtures))
            }));
        seen
    }

    #[test]
    fn a_discard_on_a_request_fixture_that_does_not_reproduce_is_kept() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, project, job, root, _calls) = measured_keep_fixture();
        let seen = flaky_measure(1.0, 0.0);
        let (state, note) = land_experiment(
            &s,
            project,
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(state, "landed", "{note}");
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 4, "both trees measured again: {seen:?}");
        assert_eq!(seen[0].0, "main");
        assert!(seen[0].1.is_empty() && seen[1].1.is_empty());
        assert_eq!(seen[2].0, "main");
        assert_eq!(
            seen[3].0, seen[1].0,
            "the same variant commit is measured again"
        );
        assert_eq!(seen[2].1, ["starter-proposes-first"]);
        assert_eq!(seen[3].1, ["starter-proposes-first"]);
        assert!(seen[2].2 && seen[3].2, "the second look is exact");
        let row = pulse_evolve::list_archive(&s, "proj").unwrap().remove(0);
        assert!(row.kept, "{row:?}");
        assert_eq!(row.baseline_score, row.variant_score);
        assert!(
            row.note.contains(
                "the discard hinged on starter-proposes-first, measured again on both trees: \
                 starter-proposes-first: baseline 1.0000 then 0.0000, variant 0.0000 then 0.0000; \
                 a difference that does not reproduce scores as a tie; neutral (a tie: the hypothesis went unmeasured"
            ),
            "{}",
            row.note
        );
        let variant = row.fixtures.variant.as_ref().unwrap();
        let fixture = variant
            .fixtures
            .iter()
            .find(|f| f.name == "starter-proposes-first")
            .unwrap();
        assert_eq!(fixture.score, 1.0);
        assert_eq!(
            fixture.note,
            "baseline 1.0000 then 0.0000, variant 0.0000 then 0.0000; scored as a tie"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'evolution.measured_again'",
                &[]
            ),
            1
        );
        let (exists, _) = crate::land::branch_exists(root, job.str("branch"));
        assert!(!exists, "a kept experiment lands and its branch goes");
    }

    #[test]
    fn a_discard_on_a_request_fixture_that_reproduces_stands() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, project, job, root, _calls) = measured_keep_fixture();
        let before = ref_of(&root, "HEAD");
        let seen = flaky_measure(1.0, 1.0);
        let (state, note) = land_experiment(
            &s,
            project,
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(state, "dropped", "{note}");
        assert_eq!(seen.lock().unwrap().len(), 4);
        let row = pulse_evolve::list_archive(&s, "proj").unwrap().remove(0);
        assert!(!row.kept);
        assert!(
            row.note.contains(
                "starter-proposes-first: baseline 1.0000 then 1.0000, variant 0.0000 then 0.0000; \
                 the difference reproduces; discarded"
            ),
            "{}",
            row.note
        );
        assert_eq!(ref_of(&root, "HEAD"), before, "nothing landed");
    }

    #[test]
    fn a_discard_on_a_metric_fixture_is_not_measured_again() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, project, job, _root, _calls) = measured_keep_fixture();
        let seen: Seen = Default::default();
        let log = seen.clone();
        let _old =
            pulse_evolve::set_measure(Box::new(move |_ctx, _s, slug, _repo, r#ref, opts| {
                log.lock()
                    .unwrap()
                    .push((r#ref.to_string(), opts.fixtures.clone(), opts.exact));
                let mut scores = two_fixture_scores(slug, 1.0, &[]);
                if r#ref != "main" {
                    scores.fixtures[0].score = 0.1;
                    scores.total = pulse_evolve::weighted_total(&scores.fixtures);
                }
                Ok(scores)
            }));
        let (state, note) =
            land_experiment(&s, project, job, "daemon".into(), TEST_NOW.into(), true);
        assert_eq!(state, "dropped", "{note}");
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "a metric regression needs no second look"
        );
    }

    fn finished_experiment_job(
        s: &pulse_store::Store,
        root: &str,
        source: String,
        branch: &str,
        body: &str,
    ) -> crate::util::Row {
        run(root, &["git", "switch", "-q", "-c", branch]);
        std::fs::write(std::path::Path::new(root).join("README.md"), body).unwrap();
        run(root, &["git", "commit", "-q", "-am", branch]);
        run(root, &["git", "switch", "-q", "main"]);
        let (job, _) = crate::jobs::add_job(
            s,
            "proj".into(),
            "experiment".into(),
            "Experiment".into(),
            "Suggestion: cached\nHypothesis: cache reuse\n".into(),
            1,
            0,
            source,
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        exec1(
            s,
            "UPDATE jobs SET status = 'done', branch = ?, finished_at = ? WHERE id = ?",
            &[&branch, &TEST_NOW, &job.str("id")],
        );
        exec1(
            s,
            "INSERT INTO runs (id, job_id, project, agent, started_at, status) VALUES (?, ?, 'proj', 'proposer', ?, 'done')",
            &[&format!("run-{}", job.str("id")), &job.str("id"), &TEST_NOW],
        );
        job_row(s, &job.str("id"))
    }

    #[test]
    fn experiment_reuses_cached_baseline_for_same_default_commit() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, project, job, root, calls) = measured_keep_fixture();
        let seen = calls.clone();
        let _old =
            pulse_evolve::set_measure(Box::new(move |_ctx, _s, slug, _repo, r#ref, _opts| {
                *seen.lock().unwrap() += 1;
                let score = if r#ref == "main" { 0.8 } else { 0.7 };
                Ok(pulse_evolve::Scores {
                    project: slug.into(),
                    total: score,
                    ..Default::default()
                })
            }));
        let (state, note) = land_experiment(
            &s,
            project.clone(),
            job.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(state, "dropped", "first discard: {note}");
        assert_eq!(*calls.lock().unwrap(), 2);
        let first_note: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT note FROM evolution_archive WHERE project = 'proj' ORDER BY at DESC, id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            first_note.contains("baseline: measured"),
            "first archive note: {first_note}"
        );

        let second = finished_experiment_job(
            &s,
            &root,
            job.str("source"),
            "pulse/cache-second",
            "# cached variant\n",
        );
        let (state, note) = land_experiment(
            &s,
            project.clone(),
            second.clone(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
        );
        assert_eq!(state, "dropped", "second discard: {note}");
        assert_eq!(
            *calls.lock().unwrap(),
            3,
            "second experiment should measure only the variant"
        );
        let second_note: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT note FROM evolution_archive WHERE branch = 'pulse/cache-second'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            second_note.contains("baseline: reused"),
            "second archive note: {second_note}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'evolution.baseline_cache'",
                &[],
            ),
            1
        );

        std::fs::write(std::path::Path::new(&root).join("new-main.txt"), "new\n").unwrap();
        run(&root, &["git", "add", "new-main.txt"]);
        run(&root, &["git", "commit", "-q", "-m", "move main"]);
        let third = finished_experiment_job(
            &s,
            &root,
            second.str("source"),
            "pulse/cache-third",
            "# cache miss variant\n",
        );
        let (state, note) =
            land_experiment(&s, project, third, "daemon".into(), TEST_NOW.into(), true);
        assert_eq!(state, "dropped", "third discard: {note}");
        assert_eq!(
            *calls.lock().unwrap(),
            5,
            "new default commit should miss the baseline cache"
        );
    }
}
