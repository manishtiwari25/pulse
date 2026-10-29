use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) type ExecutableHook = fn() -> std::io::Result<String>;

pub(crate) static EXECUTABLE: LazyLock<RwLock<Option<ExecutableHook>>> =
    LazyLock::new(|| RwLock::new(None));

#[cfg(test)]
pub(crate) fn set_executable(f: Option<ExecutableHook>) {
    *EXECUTABLE.write().expect("executable hook poisoned") = f;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MeasuredFixture {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) request: String,
    pub(crate) scorer: Vec<String>,
    pub(crate) weight: f64,
    pub(crate) metric: bool,
}

struct FixtureResult {
    index: usize,
    score: Option<crate::api::FixtureScore>,
    value: f64,
    normalized_weight: f64,
    output: String,
    error: Option<anyhow::Error>,
    /// The scorer said `scope: diff`: it counted inside the card's changed
    /// files, against the base tree.
    diff_scoped: bool,
}

/// The name of the per-card fixture that runs the card's own check.
pub const CARD_CHECK: &str = "card-check";

/// The diff scope as the scorers of one measurement receive it.
#[derive(Debug, Clone, Default)]
pub(crate) struct ScopeEnv {
    /// The base commit (`PULSE_MEASURE_BASE`).
    base: String,
    /// The changed-file list (`PULSE_MEASURE_CHANGED`); empty when no file
    /// differs, and then the scorers see the whole tree.
    changed: String,
    /// The frozen base checkout (`PULSE_MEASURE_BASE_TREE`); empty when the
    /// measured commit is the base itself, so each fixture's own clone is it.
    base_tree: String,
}

/// The environment a scope adds for the scorer of one fixture clone.
pub(crate) fn scope_env(scope: &ScopeEnv, clone: &str) -> Vec<String> {
    let mut env = vec![format!("PULSE_MEASURE_BASE={}", scope.base)];
    if !scope.changed.is_empty() {
        env.push(format!("PULSE_MEASURE_CHANGED={}", scope.changed));
        env.push(format!(
            "PULSE_MEASURE_BASE_TREE={}",
            if scope.base_tree.is_empty() {
                clone
            } else {
                &scope.base_tree
            }
        ));
    }
    env
}

pub(crate) fn pulse_scorer(scorer: &str) -> bool {
    let trimmed = scorer.trim_start_matches([' ', '\t']);
    trimmed == "pulse" || trimmed.starts_with("pulse ") || trimmed.starts_with("pulse\t")
}

pub(crate) fn self_scorer(scorer: &str, exe: &str) -> String {
    if !pulse_scorer(scorer) {
        return scorer.to_string();
    }
    let rest = scorer
        .trim_start_matches([' ', '\t'])
        .strip_prefix("pulse")
        .unwrap_or("");
    format!("'{}'{rest}", exe.replace('\'', r#"'\''"#))
}

pub(crate) fn scorer_env(home: &str, commit: &str, exe: &str) -> Vec<String> {
    let mut env = crate::process::isolated_env(home);
    env.push(format!("PULSE_MEASURE_COMMIT={commit}"));
    env.push(format!(
        "PULSE_MEASURE_SCRATCH={}",
        Path::new(home).join("scratch").display()
    ));
    if !exe.is_empty() {
        env.push(format!("PULSE_MEASURE_BINARY={exe}"));
    }
    env
}

pub(crate) fn measure(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    repo: &str,
    ref_: &str,
    opts: crate::api::MeasureOptions,
) -> Result<crate::api::Scores> {
    crate::fixtures::valid_project(s, project)?;
    let fixtures = select_fixtures(s, project, &opts.fixtures, opts.exact)?;
    let (private, state) = crate::private::load_private(s)?;
    if state.pending != 0 {
        return Err(crate::private::private_error(
            "private templates are pending; the owner must fill them before measurement",
        ));
    }
    let exe = EXECUTABLE
        .read()
        .expect("executable hook poisoned")
        .unwrap_or(default_executable)()
    .unwrap_or_default();
    let mut matrix = Vec::new();
    for f in fixtures {
        if !crate::fixtures::finite(f.weight)
            || f.weight <= 0.0
            || (!f.metric && f.request.trim().is_empty())
            || pulse_store::secret_shaped(&f.request)
            || pulse_store::secret_shaped(&f.scorer)
        {
            return Err(crate::fixtures::input_error(
                "Cannot measure fixture set",
                "a public fixture has invalid weight or unsafe text",
                &format!("pulse evolve fixture list --project {project}"),
            ));
        }
        let mut scorer = f.scorer.clone();
        if pulse_scorer(&scorer) {
            if exe.is_empty() {
                return Err(crate::fixtures::input_error(
                    "Cannot run measurement",
                    "the pulse executable behind the starter scorers cannot be located",
                    &format!("pulse evolve fixture list --project {project}"),
                ));
            }
            scorer = self_scorer(&scorer, &exe);
        }
        matrix.push(MeasuredFixture {
            name: f.name,
            kind: f.kind,
            request: f.request,
            scorer: vec!["sh".into(), "-c".into(), scorer],
            weight: f.weight,
            metric: f.metric,
        });
    }
    for (i, f) in private.iter().enumerate() {
        let name = format!("H{}", i + 1);
        if opts.exact && !opts.fixtures.is_empty() && !opts.fixtures.contains(&name) {
            continue;
        }
        matrix.push(MeasuredFixture {
            name,
            kind: "heldout".into(),
            request: f.request.clone(),
            scorer: vec![f.scorer.clone()],
            weight: 1.0,
            metric: false,
        });
    }
    if matrix.is_empty() {
        return Err(crate::fixtures::input_error(
            "No fixtures to measure",
            "this project has no enabled evaluators",
            &format!(
                "pulse evolve fixture seed --project {project}  (starter set), or pulse evolve fixture add --project {project} --name N --request FILE --scorer CMD"
            ),
        ));
    }
    let (repo, commit) = crate::clones::resolve_revision(cancel, repo, ref_)?;
    let private_dir = s.setting("evolve.heldout_dir", "");
    if !private_dir.is_empty() {
        let dir = crate::private::private_dir(s, &private_dir)?;
        if crate::private::within(&dir, &repo) {
            return Err(crate::private::private_error(
                "private fixtures must be outside the measured tree as well as registered projects",
            ));
        }
    }
    let minutes: f64 = s.setting("minutes_per_job", "").parse().unwrap_or(f64::NAN);
    if !crate::fixtures::finite(minutes)
        || minutes <= 0.0
        || minutes > (i64::MAX / 60_000_000_000) as f64
    {
        return Err(crate::fixtures::input_error(
            "Cannot run measurement",
            "minutes_per_job must allow a positive bounded deadline",
            "pulse settings set minutes_per_job 10",
        ));
    }
    let timeout = Duration::from_secs_f64(minutes * 60.0);
    let keys = crate::clones::project_answer_keys(s, project)?;
    let base = crate::clones::fleet_root(s, true)?;
    let id = pulse_board::new_id(s, "measure");
    let root = Path::new(&base).join(&id).display().to_string();
    if crate::private::within(&root, &repo) {
        return Err(crate::clones::ViolationError {
            reason: "measurement clones would be inside the measured checkout".to_string(),
        }
        .into());
    }
    fs::create_dir(&root)?;
    crate::clones::claim_measurement_root(&root)?;
    let cleanup_store = s.clone();
    let cleanup_id = id.clone();
    let scope = opts.scope.as_ref();
    let mut scores = crate::api::Scores {
        project: project.to_string(),
        repo: repo.clone(),
        ref_name: commit.clone(),
        fixtures: Vec::new(),
        heldout: matrix.iter().filter(|f| f.kind == "heldout").count() as i64,
        heldout_generated: state.generated,
        sandbox: root.clone(),
        shadow: opts.shadow,
        ..Default::default()
    };
    let result = (|| -> Result<()> {
        let max_weight = matrix.iter().map(|f| f.weight).fold(0.0_f64, f64::max);
        let len = matrix.len();
        let mut out = opts.out;
        let (scope_env, scope_fingerprint) = match scope {
            Some(scope) => {
                let (env, fingerprint) =
                    prepare_scope(cancel, &repo, &commit, &root, &keys, scope, &matrix)?;
                (Some(env), fingerprint)
            }
            None => (None, None),
        };
        if let (Some(w), Some(scope)) = (out.as_deref_mut(), scope) {
            writeln!(
                w,
                "diff scope: {} changed file{} against base {}",
                scope.changed.len(),
                if scope.changed.len() == 1 { "" } else { "s" },
                &scope.base[..scope.base.len().min(12)]
            )?;
        }
        let requested_parallel = s.setting_int("evolve.measure_parallel");
        let parallel = if requested_parallel <= 1 {
            1
        } else {
            (requested_parallel as usize).min(len)
        };
        crate::process::set_measure_parallel(parallel);
        if let Some(w) = out.as_deref_mut() {
            writeln!(w, "measuring {len} fixtures with {parallel} in parallel")?;
        }
        let next = AtomicUsize::new(0);
        let failed = AtomicBool::new(false);
        let mut results = Vec::new();
        std::thread::scope(|scope| -> Result<()> {
            let mut handles = Vec::new();
            for _ in 0..parallel {
                let store = s.clone();
                let cancel = cancel.clone();
                let keys = keys.clone();
                let repo = repo.clone();
                let commit = commit.clone();
                let root = root.clone();
                let cleanup_id = cleanup_id.clone();
                let exe = exe.clone();
                let next = &next;
                let failed = &failed;
                let matrix = &matrix;
                let scope_env = scope_env.as_ref();
                handles.push(scope.spawn(move || {
                    let mut worker_results = Vec::new();
                    loop {
                        if failed.load(Ordering::SeqCst) || cancel.err().is_some() {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        if i >= matrix.len() {
                            break;
                        }
                        let result = measure_fixture(
                            &cancel,
                            &store,
                            &repo,
                            &commit,
                            &root,
                            &cleanup_id,
                            &keys,
                            &exe,
                            timeout,
                            max_weight,
                            matrix,
                            i,
                            scope_env,
                        );
                        if result.error.is_some() {
                            failed.store(true, Ordering::SeqCst);
                        }
                        worker_results.push(result);
                    }
                    worker_results
                }));
            }
            for handle in handles {
                results.extend(
                    handle
                        .join()
                        .map_err(|_| anyhow!("measurement worker panicked"))?,
                );
            }
            Ok(())
        })?;
        results.sort_by_key(|r| r.index);
        // A cancelled or timed-out run may stop before any fixture scored; report the
        // cancellation itself rather than the 0/0 total it would otherwise leave behind.
        if let Some(ctx_err) = cancel.err() {
            return Err(ctx_err.into());
        }
        let mut total = 0.0;
        let mut weights = 0.0;
        for result in results {
            if let Some(w) = out.as_deref_mut() {
                write!(w, "{}", result.output)?;
            }
            if let Some(err) = result.error {
                return Err(err);
            }
            if let Some(score) = result.score {
                if !crate::fixtures::score_value(result.value)
                    || !crate::fixtures::finite(result.normalized_weight)
                    || result.normalized_weight <= 0.0
                {
                    return Err(anyhow!(
                        "fixture {} produced an invalid score {} (weight {}); a measurement never counts as a win on a NaN or out-of-range score",
                        score.name,
                        result.value,
                        result.normalized_weight
                    ));
                }
                total += result.value * result.normalized_weight;
                weights += result.normalized_weight;
                if result.diff_scoped {
                    scores.diff_scoped.push(score.name.clone());
                }
                if score.kind == "card" {
                    scores.card_check = Some(result.value);
                }
                scores.fixtures.push(score);
            }
        }
        if let (Some(scope_env), Some(before)) = (&scope_env, &scope_fingerprint) {
            let base_root = crate::readonly::TreeRoot::open(&scope_env.base_tree)?;
            if crate::readonly::tree_fingerprint(&base_root)? != *before {
                return Err(crate::clones::ViolationError {
                    reason: "a scorer changed the base tree".to_string(),
                }
                .into());
            }
        }
        if weights <= 0.0 {
            return Err(anyhow!(
                "measurement produced no fixture scores; a measurement never counts as a win without a score"
            ));
        }
        scores.total = total / weights;
        if !crate::fixtures::score_value(scores.total) {
            return Err(anyhow!(
                "measurement total {} is not a finite score in [0,1]",
                scores.total
            ));
        }
        Ok(())
    })();
    let cleanup = crate::clones::remove_managed(&cleanup_store, &cleanup_id);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(scores),
        (Err(e), Ok(())) => Err(e),
        (Ok(()), Err(e)) => Err(anyhow!("measurement cleanup failed: {e}")),
        (Err(e), Err(clean)) => Err(anyhow!("{e}; measurement cleanup failed: {clean}")),
    }
}

#[allow(clippy::too_many_arguments)]
fn measure_fixture(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    repo: &str,
    commit: &str,
    root: &str,
    cleanup_id: &str,
    keys: &[String],
    exe: &str,
    timeout: Duration,
    max_weight: f64,
    matrix: &[MeasuredFixture],
    i: usize,
    scope: Option<&ScopeEnv>,
) -> FixtureResult {
    let f = &matrix[i];
    let len = matrix.len();
    let label = if f.kind == "heldout" {
        "held-out fixture"
    } else {
        &f.name
    };
    let mut output = String::new();
    let _ = writeln!(
        &mut output,
        "[{}/{}] {}: isolated measurement",
        i + 1,
        len,
        label
    );
    let clone = Path::new(root)
        .join((i + 1).to_string())
        .display()
        .to_string();
    let home = Path::new(root)
        .join("homes")
        .join((i + 1).to_string())
        .display()
        .to_string();
    let fixture_result = (|| -> Result<(f64, String, bool)> {
        crate::clones::stripped_clone(cancel, repo, commit, &clone, keys)?;
        fs::create_dir_all(&home)?;
        crate::process::prepare_measurement_caches(&home)?;
        fs::create_dir_all(Path::new(&home).join("scratch"))?;
        if !f.metric {
            crate::process::seed_provider_logins(&s.home, &home)?;
            let argv = crate::process::measurement_argv(s, exe, &f.request)?;
            let mut sink = std::io::sink();
            let job_cancel = cancel.with_timeout(timeout);
            crate::process::run_process(
                &job_cancel,
                &clone,
                &crate::process::isolated_env(&home),
                &argv,
                &mut sink,
            )
            .map_err(|e| measurement_failure(cancel, label, "harness", e))?;
            if crate::clones::detach_head(cancel, &clone)? {
                let _ = writeln!(
                    &mut output,
                    "[{}/{}] {}: the harness left HEAD on a branch; detached again",
                    i + 1,
                    len,
                    label
                );
            }
            crate::clones::check_clone(cancel, &clone, keys)?;
        }
        let score_cancel = cancel.with_timeout(timeout);
        let mut env = scorer_env(&home, commit, exe);
        if let Some(scope) = scope {
            env.extend(scope_env(scope, &clone));
        }
        score_clone(&score_cancel, &clone, &env, &f.scorer)
            .map_err(|e| measurement_failure(cancel, label, "read-only scorer", e))
    })();
    let clone_cleanup = crate::clones::remove_measurement_clone(s, cleanup_id, i + 1);
    let failed = |output: String, error: anyhow::Error| FixtureResult {
        index: i,
        score: None,
        value: 0.0,
        normalized_weight: 0.0,
        output,
        error: Some(error),
        diff_scoped: false,
    };
    let (value, reason, diff_scoped) = match (fixture_result, clone_cleanup) {
        (Ok(value), Ok(())) => value,
        (Err(e), Ok(())) => return failed(output, e),
        (Ok(_), Err(e)) => {
            return failed(output, anyhow!("measurement clone cleanup failed: {e}"));
        }
        (Err(e), Err(clean)) => {
            return failed(
                output,
                anyhow!("{e}; measurement clone cleanup failed: {clean}"),
            );
        }
    };
    let scoped = if diff_scoped { " [diff-scoped]" } else { "" };
    if !reason.is_empty() && f.kind != "heldout" && !pulse_store::secret_shaped(&reason) {
        let _ = writeln!(
            &mut output,
            "[{}/{}] {}: {:.4}{scoped} ({})",
            i + 1,
            len,
            label,
            value,
            reason
        );
    } else {
        let _ = writeln!(
            &mut output,
            "[{}/{}] {}: {:.4}{scoped}",
            i + 1,
            len,
            label,
            value
        );
    }
    FixtureResult {
        index: i,
        score: Some(crate::api::FixtureScore {
            name: f.name.clone(),
            kind: f.kind.clone(),
            score: value,
            weight: f.weight,
            metric: f.metric,
            note: String::new(),
        }),
        value,
        normalized_weight: f.weight / max_weight,
        output,
        error: None,
        diff_scoped,
    }
}

/// Lay the scope out under the measurement root: the changed-file list and,
/// when a metric or card fixture will read it and the measured commit is not
/// the base, one frozen stripped clone of the base shared by every fixture.
/// Returns the environment and, for a shared base tree, its fingerprint.
fn prepare_scope(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    commit: &str,
    root: &str,
    keys: &[String],
    scope: &crate::api::DiffScope,
    matrix: &[MeasuredFixture],
) -> Result<(ScopeEnv, Option<String>)> {
    let mut env = ScopeEnv {
        base: scope.base.clone(),
        ..Default::default()
    };
    if scope.changed.is_empty() {
        return Ok((env, None));
    }
    let list = Path::new(root).join("changed-files");
    let mut text = String::new();
    for path in &scope.changed {
        if path.contains(['\n', '\0']) {
            continue;
        }
        text.push_str(path);
        text.push('\n');
    }
    fs::write(&list, text)?;
    env.changed = list.display().to_string();
    let reads_scope = matrix.iter().any(|f| f.metric || f.kind == "card");
    if !reads_scope || scope.base == commit {
        return Ok((env, None));
    }
    let base_tree = Path::new(root).join("base").display().to_string();
    crate::clones::stripped_clone(cancel, repo, &scope.base, &base_tree, keys)?;
    let tree = crate::readonly::TreeRoot::open(&base_tree)?;
    // The clone is removed with the measurement root, which makes it
    // writable again first; the scorers only ever read it.
    let _restore = crate::readonly::freeze_tree(&tree)?;
    let fingerprint = crate::readonly::tree_fingerprint(&tree)?;
    env.base_tree = base_tree;
    Ok((env, Some(fingerprint)))
}

pub(crate) fn measurement_failure(
    cancel: &pulse_bus::CancellationToken,
    label: &str,
    step: &str,
    err: anyhow::Error,
) -> anyhow::Error {
    if let Some(ctx_err) = cancel.err() {
        return ctx_err.into();
    }
    if err
        .chain()
        .any(|e| e.downcast_ref::<crate::clones::ViolationError>().is_some())
    {
        return err;
    }
    let mut why = match err
        .chain()
        .map(ToString::to_string)
        .find(|text| text.starts_with("exit status: ") || text.starts_with("signal: "))
    {
        Some(status) => format!("{step} failed ({status}); command output is withheld"),
        None => format!("{step} failed; command output is withheld"),
    };
    if pulse_bus::ContextError::is(&err, pulse_bus::ContextError::DeadlineExceeded) {
        why = format!("{step} exceeded minutes_per_job");
    }
    crate::fixtures::input_error(
        &format!("Measurement failed for {label}"),
        &why,
        "pulse evolve fixture list (owner: review the scorer and the harness settings)",
    )
}

/// Run a scorer against a frozen clone: `(score, reason, diff_scoped)`, the
/// reason being the first line of stderr and `diff_scoped` whether stdout
/// carried a `scope: diff` line.
pub(crate) fn score_clone(
    cancel: &pulse_bus::CancellationToken,
    clone: &str,
    env: &[String],
    scorer: &[String],
) -> Result<(f64, String, bool)> {
    let root = crate::readonly::TreeRoot::open(clone)?;
    let restore = crate::readonly::freeze_tree(&root)?;
    let before = crate::readonly::tree_fingerprint(&root)?;
    let mut out = crate::process::BoundedOutput {
        limit: 64 << 10,
        ..Default::default()
    };
    let mut stderr = crate::process::BoundedOutput {
        limit: 4 << 10,
        ..Default::default()
    };
    let run_err =
        crate::process::run_process_to(cancel, clone, env, scorer, &mut out, &mut stderr).err();
    let after = crate::readonly::tree_fingerprint(&root)?;
    if before != after {
        let _ = restore();
        return Err(crate::clones::ViolationError {
            reason: "scorer changed the measured tree".to_string(),
        }
        .into());
    }
    restore()?;
    if let Some(e) = run_err {
        return Err(e);
    }
    if out.overflow {
        return Err(anyhow!("scorer output exceeds its limit"));
    }
    let stdout = String::from_utf8_lossy(&out.buf);
    let value = parse_score(&stdout)?;
    Ok((
        value,
        first_line(&String::from_utf8_lossy(&stderr.buf), 200),
        parse_scope(&stdout),
    ))
}

/// Whether a scorer's stdout says `scope: diff`.
pub(crate) fn parse_scope(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim().split_once(':').is_some_and(|(key, value)| {
            key.trim().eq_ignore_ascii_case("scope") && value.trim().eq_ignore_ascii_case("diff")
        })
    })
}

pub(crate) fn first_line(text: &str, limit: usize) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut chars: Vec<char> = line.chars().collect();
        if chars.len() > limit {
            chars.truncate(limit.saturating_sub(1));
            chars.push('…');
            return chars.into_iter().collect();
        }
        return line.to_string();
    }
    String::new()
}

pub(crate) fn parse_score(text: &str) -> Result<f64> {
    if let Ok(v) = text.trim().parse::<f64>()
        && crate::fixtures::score_value(v)
    {
        return Ok(v);
    }
    let mut found = None;
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("score") {
            continue;
        }
        let Ok(v) = value.trim().parse::<f64>() else {
            return Err(anyhow!(
                "scorer must print exactly one finite score in [0,1]"
            ));
        };
        if found.is_some() || !crate::fixtures::score_value(v) {
            return Err(anyhow!(
                "scorer must print exactly one finite score in [0,1]"
            ));
        }
        found = Some(v);
    }
    found.ok_or_else(|| anyhow!("scorer must print a number or score: X in [0,1]"))
}

fn default_executable() -> std::io::Result<String> {
    std::env::current_exe().map(|p| p.display().to_string())
}

/// `H1`, `H2`, ...: the names held-out fixtures carry in a measurement and
/// the archive, the only way an exact subset can name one (their real names
/// never leave the owner's side).
pub(crate) fn heldout_name(name: &str) -> bool {
    name.strip_prefix('H')
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

fn select_fixtures(
    s: &pulse_store::Store,
    project: &str,
    selected: &[String],
    exact: bool,
) -> Result<Vec<crate::fixtures::Fixture>> {
    let fixtures = crate::fixtures::list_fixtures(s, project, true)?;
    if selected.is_empty() {
        return Ok(fixtures);
    }
    let mut out = Vec::new();
    for name in selected {
        if let Some(f) = fixtures.iter().find(|f| &f.name == name) {
            out.push(f.clone());
        } else if exact && (heldout_name(name) || name == CARD_CHECK) {
            // Named by its archive name; the held-out loop or the card's
            // check picks it up.
        } else {
            return Err(crate::fixtures::input_error(
                "Cannot select fixture",
                "the requested enabled fixture does not exist",
                &format!("pulse evolve fixture list --project {project}"),
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::*;
    use crate::test_helpers_measure::*;
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn measure_weighted_isolated_and_private() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        fixture(&s, "demo", "F01", "printf 'score: 0.2\\n'", 1.0);
        fixture(&s, "demo", "F02", "printf '0.6\\n'", 3.0);
        let (_, forbidden) = private_set(&s);
        let log = crate::test_helpers::temp_path().join("invocations");
        fake_harness(
            &s,
            &format!(
                r#"
test "$1" = "-p"
test -n "$2"
test ! -e "$PULSE_HOME/pulse.sqlite"
test "$PULSE_JOB" = measurement
test -z "${{PULSE_AGENT:-}}"
test ! -e .pulse
test ! -e keys/answer.txt
test -z "$(git remote)"
test -z "$(git tag --list)"
if git symbolic-ref -q HEAD >/dev/null; then exit 3; fi
printf '%s\n' "$PWD" >> '{}'
printf 'changed in the clone\n' > value.txt
printf 'HIDDEN_HARNESS_OUTPUT\n'
"#,
                log.display()
            ),
        );
        let mut out = Vec::new();
        let cancel = pulse_bus::CancellationToken::new();
        let scores = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions {
                out: Some(&mut out),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(scores.fixtures.len(), 4);
        assert_eq!(scores.heldout, 2);
        assert!((scores.total - 0.6).abs() <= 1e-9);
        assert_eq!(scores.fixtures[2].name, "H1");
        assert_eq!(scores.fixtures[3].name, "H2");
        assert_private_absent(
            &(serde_json::to_string(&scores).unwrap() + &String::from_utf8_lossy(&out)),
            &[forbidden, vec!["HIDDEN_HARNESS_OUTPUT".into()]].concat(),
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&repo).join("value.txt")).unwrap(),
            "original\n"
        );
        let calls = std::fs::read_to_string(&log).unwrap();
        assert_eq!(lines(&calls).len(), 4);
        let unique: std::collections::BTreeSet<_> = lines(&calls).into_iter().collect();
        assert_eq!(unique.len(), 4);
        for name in unique {
            assert!(
                std::fs::metadata(name).is_err(),
                "measurement clone survived completion"
            );
        }
        assert_eq!(count_rows(&s, "evolution_archive"), 0);
        assert_no_clones(&s);
    }

    #[test]
    fn measure_subset_always_includes_private_set() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        fixture(&s, "demo", "F01", "printf 0.2", 1.0);
        fixture(&s, "demo", "F02", "printf 0.6", 1.0);
        private_set(&s);
        fake_harness(&s, "exit 0");
        let cancel = pulse_bus::CancellationToken::new();
        let scores = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions {
                fixtures: vec!["F02".into()],
                shadow: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(scores.fixtures.len(), 3);
        assert_eq!(scores.heldout, 2);
        assert!(scores.shadow);
        for names in [vec!["missing".to_string()], vec!["H1".to_string()]] {
            assert!(
                super::measure(
                    &cancel,
                    &s,
                    "demo",
                    &repo,
                    "HEAD",
                    crate::api::MeasureOptions {
                        fixtures: names,
                        ..Default::default()
                    },
                )
                .is_err()
            );
        }
        // An exact subset is the runner's second look: only what it names,
        // a held-out fixture by its archive name.
        let exact = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions {
                fixtures: vec!["F02".into(), "H2".into()],
                exact: true,
                shadow: true,
                ..Default::default()
            },
        )
        .unwrap();
        let names: Vec<&str> = exact.fixtures.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["F02", "H2"]);
        assert_eq!(exact.heldout, 1);
        assert_no_clones(&s);
    }

    #[test]
    fn measure_build_caches_live_outside_clone_and_reach_scorer() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        let log = crate::test_helpers::temp_path().join("cache-log");
        fixture(
            &s,
            "demo",
            "F01",
            r#"
test -f "$CARGO_TARGET_DIR/harness-built"
test "$(cat "$CARGO_TARGET_DIR/harness-built")" = "$CARGO_TARGET_DIR"
test -d "$PULSE_MEASURE_SCRATCH"
case "$PULSE_MEASURE_SCRATCH" in "$PULSE_HOME"/scratch) ;; *) exit 6 ;; esac
test ! -e target
printf 1
"#,
            1.0,
        );
        fake_harness(
            &s,
            &format!(
                r#"
for dir in "$CARGO_TARGET_DIR" "$npm_config_cache" "$GOCACHE" "$GOMODCACHE" "$PIP_CACHE_DIR" "$UV_CACHE_DIR" "$YARN_CACHE_FOLDER"; do
  case "$dir" in "$PULSE_HOME"/build/*) ;; *) exit 4 ;; esac
  test -d "$dir"
done
printf '%s' "$CARGO_TARGET_DIR" > "$CARGO_TARGET_DIR/harness-built"
printf '%s\n' "$CARGO_TARGET_DIR" >> '{}'
test ! -e target
"#,
                log.display()
            ),
        );
        let cancel = pulse_bus::CancellationToken::new();
        let scores = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions::default(),
        )
        .unwrap();
        assert_eq!(scores.total, 1.0);
        let target_dir = std::fs::read_to_string(&log).unwrap();
        assert!(target_dir.contains("/homes/1/build/cargo-target"));
        assert_no_clones(&s);
    }

    #[test]
    fn measure_metric_fixture_skips_harness_and_scores_clone() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        crate::fixtures::add_fixture(
            &s,
            crate::fixtures::FixtureInput {
                project: "demo".into(),
                name: "metric-lines".into(),
                scorer: r#"
test -d "$PULSE_MEASURE_SCRATCH"
test -f value.txt
test ! -e "$PULSE_HOME/private/providers"
printf 'score: 0.75\n'
"#
                .into(),
                metric: true,
                weight: 1.0,
                actor: "human".into(),
                ..Default::default()
            },
        )
        .unwrap();
        fake_harness(&s, "exit 9");
        let cancel = pulse_bus::CancellationToken::new();
        let scores = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions::default(),
        )
        .unwrap();
        assert_eq!(scores.fixtures.len(), 1);
        assert_eq!(scores.fixtures[0].score, 0.75);
        assert!(scores.fixtures[0].metric);
        assert_no_clones(&s);
    }

    /// A metric fixture that reads the scope the measurement exports and
    /// scores `base` on the base commit, `variant` on any other.
    fn scoped_fixture(s: &pulse_store::Store, name: &str, base: &str, variant: &str) {
        crate::fixtures::add_fixture(
            s,
            crate::fixtures::FixtureInput {
                project: "demo".into(),
                name: name.into(),
                scorer: format!(
                    r#"set -e
test -n "$PULSE_MEASURE_BASE"
test -s "$PULSE_MEASURE_CHANGED"
test -d "$PULSE_MEASURE_BASE_TREE"
test "$(sort "$PULSE_MEASURE_CHANGED" | tr '\n' ,)" = "notes.txt,value.txt,"
test "$(cat "$PULSE_MEASURE_BASE_TREE/value.txt")" = original
test ! -e "$PULSE_MEASURE_BASE_TREE/notes.txt"
if [ "$PULSE_MEASURE_COMMIT" = "$PULSE_MEASURE_BASE" ]; then
  test "$(cat value.txt)" = original
  printf 'score: {base}\nscope: diff\n'
else
  test "$(cat value.txt)" = changed
  test -f notes.txt
  if touch "$PULSE_MEASURE_BASE_TREE/scribble" 2>/dev/null; then exit 7; fi
  printf 'score: {variant}\nscope: diff\n'
fi
"#
                ),
                metric: true,
                weight: 1.0,
                actor: "human".into(),
                ..Default::default()
            },
        )
        .unwrap();
    }

    /// A branch `exp` on the test project that changes `value.txt` and adds
    /// `notes.txt`; the checkout stays on `main`.
    fn experiment_branch(repo: &str) {
        let repo = Path::new(repo);
        test_git(repo, &["checkout", "--quiet", "-b", "exp"]);
        put_file(repo.join("value.txt"), "changed\n", 0o644);
        put_file(repo.join("notes.txt"), "a note\n", 0o644);
        test_git(repo, &["add", "."]);
        test_git(repo, &["commit", "--quiet", "-m", "experiment"]);
        test_git(repo, &["checkout", "--quiet", "main"]);
    }

    #[test]
    fn measure_exports_the_diff_scope_and_a_frozen_base_tree() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        experiment_branch(&repo);
        scoped_fixture(&s, "scoped", "0.5", "0.8");
        let cancel = pulse_bus::CancellationToken::new();
        let scope = crate::api::diff_scope(&cancel, &repo, "main", "exp").unwrap();
        assert_eq!(scope.changed, ["notes.txt", "value.txt"]);
        assert_eq!(
            scope.base,
            test_git(Path::new(&repo), &["rev-parse", "main"])
        );
        assert!(!scope.lockfile_only());
        let mut log = Vec::new();
        let variant = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "exp",
            crate::api::MeasureOptions {
                scope: Some(scope.clone()),
                out: Some(&mut log),
                ..Default::default()
            },
        )
        .unwrap();
        let log = String::from_utf8(log).unwrap();
        assert!(
            log.contains("diff scope: 2 changed files against base"),
            "{log}"
        );
        assert!(log.contains("scoped: 0.8000 [diff-scoped]"), "{log}");
        assert_eq!(variant.fixtures[0].score, 0.8);
        assert_eq!(variant.diff_scoped, ["scoped"]);
        assert_eq!(variant.card_check, None);
        // The base measured in the same scope: its own clone is the base tree.
        let base = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "main",
            crate::api::MeasureOptions {
                scope: Some(scope.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(base.fixtures[0].score, 0.5);
        assert_eq!(base.diff_scoped, ["scoped"]);
        // Without a scope the same scorer sees no list and fails.
        let err = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "exp",
            crate::api::MeasureOptions::default(),
        )
        .expect_err("a scope-only scorer scored without a scope");
        assert!(err.to_string().contains("read-only scorer"), "{err}");
        // An empty diff exports the base and no list.
        let same = crate::api::diff_scope(&cancel, &repo, "main", "main").unwrap();
        assert!(same.is_empty());
        assert_no_clones(&s);
    }

    #[test]
    fn measure_pair_lays_diff_scoped_fixtures_over_the_cached_base() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        experiment_branch(&repo);
        crate::fixtures::add_fixture(
            &s,
            crate::fixtures::FixtureInput {
                project: "demo".into(),
                name: "whole".into(),
                scorer: "printf 'score: 0.4\\n'".into(),
                metric: true,
                weight: 1.0,
                actor: "human".into(),
                ..Default::default()
            },
        )
        .unwrap();
        scoped_fixture(&s, "scoped", "0.5", "0.9");
        let fixture = |name: &str, score: f64| crate::api::FixtureScore {
            name: name.into(),
            kind: "public".into(),
            score,
            weight: 1.0,
            metric: true,
            note: String::new(),
        };
        // The cached whole-tree baseline holds stale numbers for both; only
        // the diff-scoped one is measured again.
        let cached = crate::api::Scores {
            project: "demo".into(),
            ref_name: test_git(Path::new(&repo), &["rev-parse", "main"]),
            fixtures: vec![fixture("whole", 0.3), fixture("scoped", 0.1)],
            total: 0.2,
            ..Default::default()
        };
        let cancel = pulse_bus::CancellationToken::new();
        let pair = crate::api::measure_pair(
            &cancel,
            &s,
            "demo",
            &repo,
            "main",
            "exp",
            Some(cached.clone()),
            "",
            None,
        )
        .unwrap();
        let score = |scores: &crate::api::Scores, name: &str| {
            scores
                .fixtures
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.score)
                .unwrap()
        };
        assert_eq!(pair.scope.changed, ["notes.txt", "value.txt"]);
        assert_eq!(pair.variant.fixtures.len(), 2);
        assert_eq!(score(&pair.variant, "whole"), 0.4);
        assert_eq!(score(&pair.variant, "scoped"), 0.9);
        assert_eq!(pair.variant.diff_scoped, ["scoped"]);
        assert_eq!(pair.base.fixtures.len(), 2);
        assert_eq!(score(&pair.base, "whole"), 0.3);
        assert_eq!(score(&pair.base, "scoped"), 0.5);
        assert_eq!(pair.base.diff_scoped, ["scoped"]);
        assert!((pair.base.total - 0.4).abs() < 1e-9);
        assert!((pair.variant.total - 0.65).abs() < 1e-9);
        let (kept, why) = crate::api::keep(Some(&pair.base), Some(&pair.variant), 0.02);
        assert!(kept && why.starts_with("improved"), "{why}");
        // Nothing diff-scoped: the cached base stands as it is.
        crate::fixtures::set_fixture_enabled(&s, "demo", "scoped", false, "human").unwrap();
        let pair = crate::api::measure_pair(
            &cancel,
            &s,
            "demo",
            &repo,
            "main",
            "exp",
            Some(cached.clone()),
            "",
            None,
        )
        .unwrap();
        assert_eq!(pair.base, cached);
        assert_eq!(pair.variant.fixtures.len(), 1);
        assert_no_clones(&s);
    }

    #[test]
    fn measure_removes_each_clone_before_next_fixture() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        s.set_setting("human", "evolve.measure_parallel", "1")
            .unwrap();
        fixture(&s, "demo", "F01", "printf 1", 1.0);
        fixture(&s, "demo", "F02", "printf 1", 1.0);
        fake_harness(
            &s,
            r#"
root=$(dirname "$PWD")
case "$(basename "$PWD")" in
  1) test ! -e "$root/2" ;;
  2) test ! -e "$root/1"; test -d "$root/homes/1" ;;
  *) exit 5 ;;
esac
"#,
        );
        let cancel = pulse_bus::CancellationToken::new();
        let scores = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions::default(),
        )
        .unwrap();
        assert_eq!(scores.fixtures.len(), 2);
        assert_no_clones(&s);
    }

    #[test]
    fn measure_parallel_overlaps_and_keeps_output_order() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        s.set_setting("human", "evolve.measure_parallel", "4")
            .unwrap();
        for i in 1..=4 {
            fixture(&s, "demo", &format!("F0{i}"), "printf 1", 1.0);
        }
        let marker = crate::test_helpers::temp_path().join("overlap");
        fake_harness(
            &s,
            &format!(
                r#"
root=$(dirname "$PWD")
running="$root/running"
mkdir -p "$running"
touch "$running/$(basename "$PWD")"
count=$(ls "$running" | wc -l | tr -d ' ')
if [ "$count" -ge 2 ]; then touch '{}'; fi
sleep 1
rm -f "$running/$(basename "$PWD")"
"#,
                marker.display()
            ),
        );
        let mut out = Vec::new();
        let cancel = pulse_bus::CancellationToken::new();
        let scores = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions {
                out: Some(&mut out),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(scores.fixtures.len(), 4);
        assert!(marker.exists(), "fixtures did not overlap");
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("measuring 4 fixtures with 4 in parallel"));
        let mut previous = 0;
        for name in ["F01", "F02", "F03", "F04"] {
            let pos = out
                .find(name)
                .unwrap_or_else(|| panic!("missing {name}: {out}"));
            assert!(pos >= previous, "output out of order: {out}");
            previous = pos;
        }
        assert_no_clones(&s);
    }

    #[test]
    fn measure_read_only_scorer_violation() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        fixture(
            &s,
            "demo",
            "F01",
            "chmod u+w value.txt; printf changed > value.txt; printf 1",
            1.0,
        );
        fake_harness(&s, "exit 0");
        let cancel = pulse_bus::CancellationToken::new();
        let err = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions::default(),
        )
        .expect_err("mutating scorer reported success");
        assert!(
            err.downcast_ref::<crate::clones::ViolationError>()
                .is_some(),
            "mutating scorer did not fail as a constraint: {err}"
        );
        assert_no_clones(&s);
        assert_eq!(count_rows(&s, "evolution_archive"), 0);
    }

    #[test]
    fn measure_detaches_head_a_harness_left_on_a_branch() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        for (how, commits) in [
            (
                "git switch --quiet -c topic && git add value.txt && git -c user.name=t -c user.email=t@example.invalid commit --quiet -m topic",
                1,
            ),
            ("git checkout --quiet -b main", 0),
        ] {
            let s = test_store();
            let repo = test_project(&s, "demo");
            fixture(
                &s,
                "demo",
                "F01",
                &format!(
                    r#"
set -eu
if git symbolic-ref -q HEAD >/dev/null; then exit 3; fi
test "$(git rev-list --count "$PULSE_MEASURE_COMMIT..HEAD")" = {commits}
test "$(cat value.txt)" = improved
printf 1
"#
                ),
                1.0,
            );
            fake_harness(&s, &format!("printf improved > value.txt\n{how}"));
            let mut out = Vec::new();
            let cancel = pulse_bus::CancellationToken::new();
            let scores = super::measure(
                &cancel,
                &s,
                "demo",
                &repo,
                "HEAD",
                crate::api::MeasureOptions {
                    out: Some(&mut out),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{how}: {e}"));
            assert_eq!(scores.fixtures.len(), 1, "{how}");
            assert_eq!(scores.fixtures[0].score, 1.0, "{how}");
            let out = String::from_utf8(out).unwrap();
            assert!(
                out.contains("F01: the harness left HEAD on a branch; detached again"),
                "{how}: {out}"
            );
            assert_no_clones(&s);
        }
        for how in [
            "git remote add origin \"$PWD\"",
            "git checkout --quiet --orphan fresh",
        ] {
            let s = test_store();
            let repo = test_project(&s, "demo");
            fixture(&s, "demo", "F01", "printf 1", 1.0);
            fake_harness(&s, how);
            let cancel = pulse_bus::CancellationToken::new();
            let err = super::measure(
                &cancel,
                &s,
                "demo",
                &repo,
                "HEAD",
                crate::api::MeasureOptions::default(),
            )
            .expect_err(how);
            assert!(
                err.downcast_ref::<crate::clones::ViolationError>()
                    .is_some(),
                "{how}: {err}"
            );
            assert_no_clones(&s);
        }
    }

    #[test]
    fn measure_failure_and_cancellation_cleanup() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        for mode in ["harness failure", "bad scorer", "nan scorer", "cancel"] {
            let s = test_store();
            let repo = test_project(&s, "demo");
            let scorer = if mode == "bad scorer" {
                "printf 'invalid score\\n'"
            } else if mode == "nan scorer" {
                "printf 'score: NaN\\n'"
            } else {
                "printf 0.5"
            };
            fixture(&s, "demo", "F01", scorer, 1.0);
            let body = if mode == "harness failure" {
                "printf 'HIDDEN_FAILURE\\n' >&2; exit 7"
            } else if mode == "cancel" {
                "sleep 30"
            } else {
                "exit 0"
            };
            fake_harness(&s, body);
            let mut cancel = pulse_bus::CancellationToken::new();
            if mode == "cancel" {
                cancel = cancel.with_timeout(Duration::from_millis(250));
            }
            let mut out = Vec::new();
            let err = super::measure(
                &cancel,
                &s,
                "demo",
                &repo,
                "HEAD",
                crate::api::MeasureOptions {
                    out: Some(&mut out),
                    ..Default::default()
                },
            )
            .expect_err("failed measurement reported success");
            if mode == "cancel" {
                assert!(
                    err.to_string().contains("context deadline exceeded"),
                    "{err}"
                );
            }
            if mode == "harness failure" {
                assert!(
                    err.to_string()
                        .contains("harness failed (exit status: 7); command output is withheld"),
                    "{err}"
                );
            }
            assert!(!(err.to_string() + &String::from_utf8_lossy(&out)).contains("HIDDEN_FAILURE"));
            assert_no_clones(&s);
        }
    }
    #[test]
    fn score_protocol() {
        let _serial = crate::test_helpers::serial();
        for (text, want, ok) in [
            ("0", 0.0, true),
            ("1\n", 1.0, true),
            ("notes\nscore: .25\n", 0.25, true),
            ("NaN", 0.0, false),
            ("Inf", 0.0, false),
            ("-0.1", 0.0, false),
            ("1.1", 0.0, false),
            ("score: .5\nscore: .5\n", 0.0, false),
            ("no score", 0.0, false),
        ] {
            let got = super::parse_score(text);
            assert_eq!(got.is_ok(), ok, "{text:?}: {got:?}");
            if ok {
                assert_eq!(got.unwrap(), want);
            }
        }
    }
    #[test]
    fn harness_prompt_is_one_literal_argument() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let request = "literal '; $(do-not-execute) \"\nrequest";
        for command in [
            "'/tmp/runner with spaces' --flag",
            "fake -p {prompt} --allow-all-tools",
            "fake -p",
        ] {
            let args = crate::process::harness_args(command, request).unwrap();
            assert_eq!(args.iter().filter(|arg| arg.as_str() == request).count(), 1);
        }
        assert!(crate::process::harness_args("fake -p old-request", request).is_err());
    }

    #[test]
    fn measurement_harness_never_waits_for_a_prompt() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let request = "Improve the value in the clone.";
        s.set_setting("human", "worker_model", "fable").unwrap();
        s.set_setting("human", "harness.max_steps", "7").unwrap();
        s.set_setting("human", "minutes_per_job", "5").unwrap();
        let native = crate::process::measurement_argv(&s, "/bin/pulse", request).unwrap();
        assert_eq!(
            &native[..5],
            ["/bin/pulse", "harness", "run", "-p", request]
        );
        for pair in [
            ["--max-steps", "7"],
            ["--minutes", "4"],
            ["--model", "fable"],
        ] {
            assert!(
                native.windows(2).any(|w| w == pair),
                "{native:?} lacks {pair:?}"
            );
        }
        assert!(native.contains(&"--quiet".to_string()));
        assert!(!native.contains(&"--allow-all-tools".to_string()));
        assert!(!native.contains(&"--context-window".to_string()));
        s.set_setting("human", "harness.context_window", "1000000")
            .unwrap();
        let native = crate::process::measurement_argv(&s, "/bin/pulse", request).unwrap();
        assert!(
            native
                .windows(2)
                .any(|w| w == ["--context-window", "1000000"]),
            "{native:?}"
        );
        s.set_setting("human", "harness.context_window", "0")
            .unwrap();
        s.set_setting("human", "measure_model", "haiku").unwrap();
        let pinned = crate::process::measurement_argv(&s, "/bin/pulse", request).unwrap();
        assert!(
            pinned.windows(2).any(|w| w == ["--model", "haiku"]),
            "measure_model drives the measurement, not worker_model: {pinned:?}"
        );
        assert!(!pinned.contains(&"fable".to_string()));
        s.set_setting("human", "measure_model", "").unwrap();
        assert!(
            crate::process::measurement_argv(&s, "", request).is_err(),
            "an unknown executable must not start a measurement"
        );

        // Tests swap in a script; it runs as written, the request as one argument.
        crate::process::set_measurement_harness(
            &s.home,
            Some("'/tmp/my harness' --flag".to_string()),
        );
        let custom = crate::process::measurement_argv(&s, "/bin/pulse", request).unwrap();
        crate::process::set_measurement_harness(&s.home, None);
        assert_eq!(custom, ["/tmp/my harness", "--flag", "-p", request]);
    }

    #[test]
    fn isolated_home_carries_only_provider_logins() {
        let _serial = crate::test_helpers::serial();
        let from = crate::test_helpers::temp_path();
        let to = crate::test_helpers::temp_path();
        let providers = from.join("private").join("providers");
        std::fs::create_dir_all(&providers).unwrap();
        std::fs::write(providers.join("copilot.json"), "{\"github_token\":\"x\"}\n").unwrap();
        std::fs::write(providers.join("notes.txt"), "not a login").unwrap();
        std::fs::write(from.join("private").join("owner.json"), "{}").unwrap();
        crate::process::seed_provider_logins(
            &from.display().to_string(),
            &to.display().to_string(),
        )
        .unwrap();
        let seeded = to.join("private").join("providers");
        assert!(seeded.join("copilot.json").is_file());
        assert!(!seeded.join("notes.txt").exists());
        assert!(!to.join("private").join("owner.json").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(seeded.join("copilot.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "login copied with mode {mode:o}");
        }
        let empty = crate::test_helpers::temp_path();
        crate::process::seed_provider_logins(
            &empty.display().to_string(),
            &to.display().to_string(),
        )
        .unwrap();
    }

    #[test]
    fn measure_runs_a_written_judge() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        let dir = crate::test_helpers::temp_path();
        crate::private::setup(&s, &dir.display().to_string()).unwrap();
        let pulse = test_pulse();
        use_test_executable(&pulse);
        crate::private::write_heldout(
            &s,
            crate::private::HeldoutWrite {
                request: "Leave the checkout exactly as it is.".into(),
                judge: "starter-hardstop-history".into(),
                generated: true,
                ..Default::default()
            },
        )
        .unwrap();
        crate::private::write_heldout(
            &s,
            crate::private::HeldoutWrite {
                request: "The owner's own check.".into(),
                scorer: "#!/bin/sh\nprintf '0.5\\n'\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        fake_harness(&s, "exit 0");
        let mut out = Vec::new();
        let cancel = pulse_bus::CancellationToken::new();
        let scores = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions {
                out: Some(&mut out),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(scores.fixtures.len(), 2);
        assert_eq!(scores.heldout, 2);
        assert_eq!(scores.heldout_generated, 1);
        assert_eq!(scores.fixtures[0].score, 1.0);
        assert_eq!(scores.fixtures[1].score, 0.5);
        assert_eq!(scores.total, 0.75);
        assert!(heldout_json(&scores).contains("\"heldout_generated\":1"));
        assert_private_absent(
            &(heldout_json(&scores) + &String::from_utf8_lossy(&out)),
            &[
                dir.display().to_string(),
                "H01".into(),
                "H02".into(),
                "Leave the checkout".into(),
                "own check".into(),
                pulse.clone(),
            ],
        );
        assert_no_clones(&s);
        crate::private::write_heldout(
            &s,
            crate::private::HeldoutWrite {
                request: "Another of the owner's checks.".into(),
                scorer: "#!/bin/sh\nprintf '1\\n'\n".into(),
                slot: 1,
                ..Default::default()
            },
        )
        .unwrap();
        clear_test_executable();
        let scores = super::measure(
            &cancel,
            &s,
            "demo",
            &repo,
            "HEAD",
            crate::api::MeasureOptions::default(),
        )
        .unwrap();
        assert_eq!(scores.total, 0.75);
        assert_no_clones(&s);
    }
}
