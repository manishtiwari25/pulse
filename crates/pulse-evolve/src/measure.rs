//! Port of internal/evolve/measure.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) type ExecutableHook = fn() -> std::io::Result<String>;

/// Go: var executable = os.Executable
pub(crate) static EXECUTABLE: LazyLock<RwLock<Option<ExecutableHook>>> =
    LazyLock::new(|| RwLock::new(None));

#[allow(dead_code)]
pub(crate) fn set_executable(f: Option<ExecutableHook>) {
    *EXECUTABLE.write().expect("executable hook poisoned") = f;
}

/// Go: func init() { Measure = measure }
#[allow(dead_code)]
pub(crate) fn init() {
    // The Rust facade delegates directly to this module through crate::api::measure callers.
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MeasuredFixture {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) request: String,
    pub(crate) scorer: Vec<String>,
    pub(crate) weight: f64,
}

/// Go: func pulseScorer(scorer string) bool {
pub(crate) fn pulse_scorer(scorer: &str) -> bool {
    let trimmed = scorer.trim_start_matches([' ', '\t']);
    trimmed == "pulse" || trimmed.starts_with("pulse ") || trimmed.starts_with("pulse\t")
}

/// Go: func selfScorer(scorer, exe string) string {
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

/// Go: func scorerEnv(home, commit, exe string) []string {
pub(crate) fn scorer_env(home: &str, commit: &str, exe: &str) -> Vec<String> {
    let mut env = crate::process::isolated_env(home);
    env.push(format!("PULSE_MEASURE_COMMIT={commit}"));
    if !exe.is_empty() {
        env.push(format!("PULSE_MEASURE_BINARY={exe}"));
    }
    env
}

/// Go: func measure(ctx context.Context, s *store.Store, project, repo, ref string, opts MeasureOptions) (scores *Scores, retErr error) {
pub(crate) fn measure(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    repo: &str,
    ref_: &str,
    opts: crate::api::MeasureOptions,
) -> Result<crate::api::Scores> {
    crate::fixtures::valid_project(s, project)?;
    let fixtures = select_fixtures(s, project, &opts.fixtures)?;
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
            || f.request.trim().is_empty()
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
        });
    }
    for (i, f) in private.iter().enumerate() {
        matrix.push(MeasuredFixture {
            name: format!("H{}", i + 1),
            kind: "heldout".into(),
            request: f.request.clone(),
            scorer: vec![f.scorer.clone()],
            weight: 1.0,
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
    let cleanup_store = s.clone();
    let cleanup_id = id.clone();
    let mut scores = crate::api::Scores {
        project: project.to_string(),
        repo: repo.clone(),
        ref_name: commit.clone(),
        fixtures: Vec::new(),
        heldout: private.len() as i64,
        heldout_generated: state.generated,
        sandbox: root.clone(),
        shadow: opts.shadow,
        ..Default::default()
    };
    let result = (|| -> Result<()> {
        let max_weight = matrix.iter().map(|f| f.weight).fold(0.0_f64, f64::max);
        let mut total = 0.0;
        let mut weights = 0.0;
        let len = matrix.len();
        let mut out = opts.out;
        for (i, f) in matrix.iter().enumerate() {
            if let Some(err) = cancel.err() {
                return Err(err.into());
            }
            let label = if f.kind == "heldout" {
                "held-out fixture"
            } else {
                &f.name
            };
            if let Some(w) = out.as_deref_mut() {
                writeln!(w, "[{}/{}] {}: isolated measurement", i + 1, len, label)?;
            }
            let clone = Path::new(&root)
                .join((i + 1).to_string())
                .display()
                .to_string();
            crate::clones::stripped_clone(cancel, &repo, &commit, &clone, &keys)?;
            let home = Path::new(&root)
                .join("homes")
                .join((i + 1).to_string())
                .display()
                .to_string();
            fs::create_dir_all(&home)?;
            let argv = crate::process::harness_args(&s.setting("runner_cmd", ""), &f.request)?;
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
            crate::clones::check_clone(cancel, &clone, &keys)?;
            let score_cancel = cancel.with_timeout(timeout);
            let (value, reason) = score_clone(
                &score_cancel,
                &clone,
                &scorer_env(&home, &commit, &exe),
                &f.scorer,
            )
            .map_err(|e| measurement_failure(cancel, label, "read-only scorer", e))?;
            scores.fixtures.push(crate::api::FixtureScore {
                name: f.name.clone(),
                kind: f.kind.clone(),
                score: value,
                weight: f.weight,
                note: String::new(),
            });
            total += value * (f.weight / max_weight);
            weights += f.weight / max_weight;
            if let Some(w) = out.as_deref_mut() {
                if !reason.is_empty() && f.kind != "heldout" && !pulse_store::secret_shaped(&reason)
                {
                    writeln!(
                        w,
                        "[{}/{}] {}: {:.4} ({})",
                        i + 1,
                        len,
                        label,
                        value,
                        reason
                    )?;
                } else {
                    writeln!(w, "[{}/{}] {}: {:.4}", i + 1, len, label, value)?;
                }
            }
        }
        scores.total = total / weights;
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

/// Go: func measurementFailure(ctx context.Context, label, step string, err error) error {
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
    let mut why = format!("{step} failed; command output is withheld");
    if pulse_bus::ContextError::is(&err, pulse_bus::ContextError::DeadlineExceeded) {
        why = format!("{step} exceeded minutes_per_job");
    }
    crate::fixtures::input_error(
        &format!("Measurement failed for {label}"),
        &why,
        "pulse evolve fixture list (owner: review the scorer and runner_cmd)",
    )
}

/// Go: func scoreClone(ctx context.Context, clone string, env, scorer []string) (value float64, reason string, retErr error) {
pub(crate) fn score_clone(
    cancel: &pulse_bus::CancellationToken,
    clone: &str,
    env: &[String],
    scorer: &[String],
) -> Result<(f64, String)> {
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
    let value = parse_score(&String::from_utf8_lossy(&out.buf))?;
    Ok((
        value,
        first_line(&String::from_utf8_lossy(&stderr.buf), 200),
    ))
}

/// Go: func firstLine(text string, limit int) string {
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

/// Go: func parseScore(text string) (float64, error) {
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

fn select_fixtures(
    s: &pulse_store::Store,
    project: &str,
    selected: &[String],
) -> Result<Vec<crate::fixtures::Fixture>> {
    let fixtures = crate::fixtures::list_fixtures(s, project, true)?;
    if selected.is_empty() {
        return Ok(fixtures);
    }
    let mut out = Vec::new();
    for name in selected {
        if let Some(f) = fixtures.iter().find(|f| &f.name == name) {
            out.push(f.clone());
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
    use crate::test_helpers_e3::*;
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn measure_weighted_isolated_and_private() {
        let _serial = crate::test_helpers::serial();
        let _guard = e3_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        fixture(&s, "demo", "F01", "printf 'score: 0.2\\n'", 1.0);
        fixture(&s, "demo", "F02", "printf '0.6\\n'", 3.0);
        let (_, forbidden) = private_set(&s);
        let log = tempfile::tempdir().unwrap().keep().join("invocations");
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
        let _guard = e3_test_lock();
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
        assert!(
            super::measure(
                &cancel,
                &s,
                "demo",
                &repo,
                "HEAD",
                crate::api::MeasureOptions {
                    fixtures: vec!["missing".into()],
                    ..Default::default()
                },
            )
            .is_err()
        );
        assert_no_clones(&s);
    }

    #[test]
    fn measure_read_only_scorer_violation() {
        let _serial = crate::test_helpers::serial();
        let _guard = e3_test_lock();
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
    fn measure_failure_and_cancellation_cleanup() {
        let _serial = crate::test_helpers::serial();
        let _guard = e3_test_lock();
        for mode in ["harness failure", "bad scorer", "cancel"] {
            let s = test_store();
            let repo = test_project(&s, "demo");
            let scorer = if mode == "bad scorer" {
                "printf 'invalid score\\n'"
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
        let _guard = e3_test_lock();
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
    fn measure_runs_a_written_judge() {
        let _serial = crate::test_helpers::serial();
        let _guard = e3_test_lock();
        let s = test_store();
        let repo = test_project(&s, "demo");
        let dir = tempfile::tempdir().unwrap().keep();
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
