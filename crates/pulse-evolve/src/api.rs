use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{LazyLock, RwLock};

pub fn err_not_implemented() -> anyhow::Error {
    anyhow!("evolve: not implemented yet")
}

/// FixtureScore is one fixture's result on one tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FixtureScore {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "score")]
    pub score: f64,
    #[serde(rename = "weight")]
    pub weight: f64,
    #[serde(rename = "metric", skip_serializing_if = "is_false", default)]
    pub metric: bool,
    #[serde(rename = "note", skip_serializing_if = "String::is_empty", default)]
    pub note: String,
}

/// Scores is one measurement of a tree against a project's fixtures.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Scores {
    #[serde(rename = "project")]
    pub project: String,
    #[serde(rename = "repo")]
    pub repo: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
    #[serde(rename = "fixtures")]
    pub fixtures: Vec<FixtureScore>,
    #[serde(rename = "total")]
    pub total: f64,
    #[serde(rename = "heldout")]
    pub heldout: i64,
    #[serde(
        rename = "heldout_generated",
        skip_serializing_if = "is_zero_i64",
        default
    )]
    pub heldout_generated: i64,
    #[serde(rename = "sandbox")]
    pub sandbox: String,
    #[serde(rename = "shadow", skip_serializing_if = "is_false", default)]
    pub shadow: bool,
    /// The card's own check (`Signal: check:<…>`) scored on this tree, when
    /// the card named one; the `card-check` fixture's score.
    #[serde(
        rename = "card_check",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub card_check: Option<f64>,
    /// The fixtures that counted only inside the card's changed files on this
    /// measurement; their base scores 0.5 and is never the cached project one.
    #[serde(rename = "diff_scoped", skip_serializing_if = "Vec::is_empty", default)]
    pub diff_scoped: Vec<String>,
}

/// DiffScope is one card's change as a measurement sees it: the base commit
/// and the files that differ between it and the variant. Every scorer gets it
/// as `PULSE_MEASURE_BASE`, `PULSE_MEASURE_CHANGED` (a file listing the
/// changed paths, one per line) and `PULSE_MEASURE_BASE_TREE` (a read-only
/// checkout of the base). A metric fixture that honours it counts only inside
/// those files and takes the base tree's count as its scale, so the base
/// scores 0.5 and the variant's delta is relative to what the card touched.
/// Both trees must be measured with the same scope; a cached whole-tree
/// baseline never stands in for a diff-scoped fixture.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffScope {
    #[serde(rename = "base")]
    pub base: String,
    #[serde(rename = "changed")]
    pub changed: Vec<String>,
}

/// The lock files a change may touch without touching code; a diff made of
/// them alone has nothing a fixture could measure.
pub const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
];

/// The verdict a lockfile-only change gets instead of a measurement: it goes
/// to landing as a tie and is archived as `neutral` with the reason.
pub const LOCKFILE_ONLY_VERDICT: &str =
    "neutral (lockfile-only: the change touches lock files alone, so there is nothing to measure)";

impl DiffScope {
    /// No file differs: the scorers see the whole tree, as without a scope.
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }

    /// Every changed file is a lock file ([`LOCKFILES`]).
    pub fn lockfile_only(&self) -> bool {
        !self.changed.is_empty()
            && self.changed.iter().all(|path| {
                let name = path.rsplit('/').next().unwrap_or(path);
                LOCKFILES.contains(&name)
            })
    }
}

/// MeasureOptions tunes Measure.
#[derive(Default)]
pub struct MeasureOptions<'a> {
    pub fixtures: Vec<String>,
    /// With `fixtures`: measure exactly those, a held-out fixture only when
    /// named by its archive name (`H1`, `H2`, ...). Without it a subset always
    /// includes the owner's held-out set, so no by-hand measurement can leave
    /// them out. The runner's second look at a disputed fixture sets it.
    pub exact: bool,
    pub shadow: bool,
    pub out: Option<&'a mut dyn Write>,
    /// The card's diff scope ([`DiffScope`]); give the base measurement the
    /// same one, so both trees count the same files.
    pub scope: Option<DiffScope>,
    /// The card's own check (`Signal: check:<command>`), run from the tree
    /// root on each tree as the `card-check` fixture (weight 1.0): its exit
    /// status scores 1 or 0, a single integer on stdout a lower-is-better
    /// count against the base tree's. The score is `Scores::card_check`.
    pub card_check: String,
}

/// MeasuredPair is one card's base and variant measured in the same scope.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeasuredPair {
    pub base: Scores,
    pub variant: Scores,
    pub scope: DiffScope,
    /// Why the variant was not measured (`lockfile-only`): it then carries
    /// the base's scores, so an archive row still has its numbers, and
    /// [`keep_pair`] gives the matching `neutral` verdict. `None` when both
    /// trees were measured.
    pub skipped: Option<String>,
}

/// The short reason [`MeasuredPair::skipped`] carries for a lock-file-only
/// change.
pub const LOCKFILE_ONLY: &str = "lockfile-only";

pub type MeasureFn = dyn for<'a> Fn(
        &pulse_bus::CancellationToken,
        &pulse_store::Store,
        &str,
        &str,
        &str,
        MeasureOptions<'a>,
    ) -> Result<Scores>
    + Send
    + Sync
    + 'static;

pub static MEASURE: LazyLock<RwLock<Box<MeasureFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(crate::measure::measure)));

pub fn set_measure(f: Box<MeasureFn>) -> Box<MeasureFn> {
    std::mem::replace(
        &mut *MEASURE.write().expect("evolve Measure lock poisoned"),
        f,
    )
}

pub fn measure(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    repo: &str,
    ref_name: &str,
    opts: MeasureOptions<'_>,
) -> Result<Scores> {
    (MEASURE.read().expect("evolve Measure lock poisoned"))(
        cancel, s, project, repo, ref_name, opts,
    )
}

/// The files that differ between two commits of a checkout, as a
/// [`DiffScope`] for measuring the second against the first.
pub fn diff_scope(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    base_ref: &str,
    variant_ref: &str,
) -> Result<DiffScope> {
    let (root, base) = crate::clones::resolve_revision(cancel, repo, base_ref)?;
    let (_, variant) = crate::clones::resolve_revision(cancel, repo, variant_ref)?;
    let mut changed: Vec<String> = Vec::new();
    if base != variant {
        let text = crate::process::git_raw(
            cancel,
            &root,
            &[
                "diff".into(),
                "--name-only".into(),
                "--no-renames".into(),
                "-z".into(),
                base.clone(),
                variant,
                "--".into(),
            ],
        )?;
        changed = text
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        changed.sort();
        changed.dedup();
    }
    Ok(DiffScope { base, changed })
}

/// Measure one card: its variant and its base in the same diff scope, so a
/// metric fixture that counts inside the changed files is compared like for
/// like. `cached_base` is the whole-tree baseline when the cache holds one;
/// the fixtures the variant measured diff-scoped (and the card's check) are
/// measured again on the base and laid over it, because a diff-scoped score
/// is per card and the cached one is not. Without a cached base the whole
/// base is measured in the scope. The measurement goes through
/// [`measure`], so a hook set with [`set_measure`] sees both calls.
#[allow(clippy::too_many_arguments)]
pub fn measure_pair(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    repo: &str,
    base_ref: &str,
    variant_ref: &str,
    cached_base: Option<Scores>,
    card_check: &str,
    mut out: Option<&mut dyn Write>,
) -> Result<MeasuredPair> {
    let scope = diff_scope(cancel, repo, base_ref, variant_ref)?;
    if scope.lockfile_only() {
        // Nothing a fixture could measure changed: the variant takes the
        // base's numbers and goes to landing as a tie.
        let base = match cached_base {
            Some(cached) => cached,
            None => measure(
                cancel,
                s,
                project,
                repo,
                base_ref,
                MeasureOptions {
                    out,
                    ..Default::default()
                },
            )?,
        };
        return Ok(MeasuredPair {
            variant: base.clone(),
            base,
            scope,
            skipped: Some(LOCKFILE_ONLY.to_string()),
        });
    }
    let variant = measure(
        cancel,
        s,
        project,
        repo,
        variant_ref,
        MeasureOptions {
            scope: Some(scope.clone()),
            card_check: card_check.to_string(),
            out: out.as_deref_mut().map(|w| w as &mut dyn Write),
            ..Default::default()
        },
    )?;
    let mut scoped: Vec<String> = variant.diff_scoped.clone();
    if variant.card_check.is_some() && !scoped.iter().any(|n| n == crate::measure::CARD_CHECK) {
        scoped.push(crate::measure::CARD_CHECK.to_string());
    }
    let base = match cached_base {
        Some(cached) if scoped.is_empty() => cached,
        Some(cached) => {
            let fresh = measure(
                cancel,
                s,
                project,
                repo,
                base_ref,
                MeasureOptions {
                    fixtures: scoped,
                    exact: true,
                    scope: Some(scope.clone()),
                    card_check: card_check.to_string(),
                    out: out.as_deref_mut().map(|w| w as &mut dyn Write),
                    ..Default::default()
                },
            )?;
            overlay_scores(cached, &fresh)
        }
        None => measure(
            cancel,
            s,
            project,
            repo,
            base_ref,
            MeasureOptions {
                scope: Some(scope.clone()),
                card_check: card_check.to_string(),
                out,
                ..Default::default()
            },
        )?,
    };
    Ok(MeasuredPair {
        base,
        variant,
        scope,
        skipped: None,
    })
}

/// [`keep`] for a [`MeasuredPair`]: a pair whose variant was not measured
/// is a `neutral` tie with the reason it was skipped for
/// ([`LOCKFILE_ONLY_VERDICT`] for a lock-file-only change).
pub fn keep_pair(pair: &MeasuredPair, tolerance: f64) -> (bool, String) {
    match pair.skipped.as_deref() {
        None => keep(Some(&pair.base), Some(&pair.variant), tolerance),
        Some(LOCKFILE_ONLY) => (true, LOCKFILE_ONLY_VERDICT.to_string()),
        Some(why) => (
            true,
            format!("neutral ({why}: the variant was not measured)"),
        ),
    }
}

/// The cached whole-tree scores with the freshly measured fixtures laid over
/// them: same-name fixtures replaced, new ones (the card's check) added, the
/// total recomputed, the scope notes taken from the fresh measurement.
pub fn overlay_scores(mut cached: Scores, fresh: &Scores) -> Scores {
    for f in &fresh.fixtures {
        match cached
            .fixtures
            .iter_mut()
            .find(|c| c.name == f.name && c.kind == f.kind)
        {
            Some(slot) => *slot = f.clone(),
            None => cached.fixtures.push(f.clone()),
        }
    }
    cached.total = crate::dispute::weighted_total(&cached.fixtures);
    cached.card_check = fresh.card_check;
    cached.diff_scoped = fresh.diff_scoped.clone();
    if !fresh.sandbox.is_empty() {
        cached.sandbox = fresh.sandbox.clone();
    }
    cached
}

/// A variant is discarded when it regresses the score by more than the
/// tolerance; otherwise it goes on to the ordinary landing lane, where the
/// project's own checks decide. The verdict says what the measurement saw:
/// `improved` when the delta reached the tolerance, or when a tie came with
/// the card's own check (`Scores::card_check`) rising; `neutral` for any other
/// tie, which is no win - the hypothesis went unmeasured, and the row earns no
/// lineage credit. Only `discarded` deletes the branch.
pub fn keep(baseline: Option<&Scores>, variant: Option<&Scores>, tolerance: f64) -> (bool, String) {
    let (Some(baseline), Some(variant)) = (baseline, variant) else {
        return (false, "no measurement".to_string());
    };
    let delta = variant.total - baseline.total;
    if delta < -tolerance {
        (
            false,
            crate::format::format_delta("discarded", delta, tolerance),
        )
    } else if delta >= tolerance {
        (
            true,
            crate::format::format_delta("improved", delta, tolerance),
        )
    } else if card_check_rose(baseline, variant) {
        (
            true,
            crate::format::format_delta(
                "improved (a tie on the composite, but the card's own check rose)",
                delta,
                tolerance,
            ),
        )
    } else {
        (
            true,
            crate::format::format_delta(NEUTRAL_VERDICT, delta, tolerance),
        )
    }
}

/// The verdict a tie gets: the branch still lands if the project's own checks
/// pass, but nothing was shown to move.
pub const NEUTRAL_VERDICT: &str = "neutral (a tie: the hypothesis went unmeasured, no fixture moved; the project's own check decides at landing)";

/// Whether the card's own check scored higher on the variant than on the
/// baseline; both trees must have run it.
pub fn card_check_rose(baseline: &Scores, variant: &Scores) -> bool {
    matches!(
        (baseline.card_check, variant.card_check),
        (Some(b), Some(v)) if v.is_finite() && b.is_finite() && v > b + 1e-9
    )
}

/// ArchiveRow is what the runner records after an experiment was measured.
#[derive(Debug, Clone, Default)]
pub struct ArchiveRow {
    pub project: String,
    pub suggestion_post: String,
    pub hypothesis: String,
    pub branch: String,
    pub parent: String,
    pub patch: String,
    pub base_commit: String,
    pub baseline: Option<Scores>,
    pub variant: Option<Scores>,
    pub tolerance: f64,
    pub kept: bool,
    pub run_id: String,
    pub sandbox: String,
    pub note: String,
}

pub const BASELINE_HYPOTHESIS: &str = "baseline";

pub fn is_baseline_row(hypothesis: &str, suggestion_post: &str) -> bool {
    hypothesis == BASELINE_HYPOTHESIS && suggestion_post.is_empty()
}

pub type ArchiveFn =
    dyn Fn(&pulse_store::Store, ArchiveRow) -> Result<String> + Send + Sync + 'static;

pub static ARCHIVE: LazyLock<RwLock<Box<ArchiveFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(crate::archive::archive)));

pub fn set_archive(f: Box<ArchiveFn>) -> Box<ArchiveFn> {
    std::mem::replace(
        &mut *ARCHIVE.write().expect("evolve Archive lock poisoned"),
        f,
    )
}

pub fn archive_row(s: &pulse_store::Store, row: ArchiveRow) -> Result<String> {
    (ARCHIVE.read().expect("evolve Archive lock poisoned"))(s, row)
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lockfile_only_diff_is_told_apart() {
        let scope = |changed: &[&str]| DiffScope {
            base: "abc".into(),
            changed: changed.iter().map(|c| c.to_string()).collect(),
        };
        assert!(scope(&["Cargo.lock"]).lockfile_only());
        assert!(
            scope(&["web/hub/pnpm-lock.yaml", "package-lock.json", "yarn.lock"]).lockfile_only()
        );
        assert!(!scope(&["Cargo.lock", "src/lib.rs"]).lockfile_only());
        assert!(!scope(&["Cargo.lockfile"]).lockfile_only());
        assert!(!scope(&[]).lockfile_only());
        assert!(scope(&[]).is_empty());
    }

    #[test]
    fn overlay_replaces_the_measured_fixtures_and_recomputes_the_total() {
        let fixture = |name: &str, kind: &str, score: f64, weight: f64| FixtureScore {
            name: name.into(),
            kind: kind.into(),
            score,
            weight,
            metric: true,
            note: String::new(),
        };
        let cached = Scores {
            ref_name: "base".into(),
            fixtures: vec![
                fixture("tests", "public", 0.9, 1.0),
                fixture("markers", "public", 0.1, 3.0),
                fixture("H1", "heldout", 1.0, 1.0),
            ],
            total: 0.44,
            heldout: 1,
            ..Default::default()
        };
        let fresh = Scores {
            fixtures: vec![
                fixture("markers", "public", 0.5, 3.0),
                fixture("card-check", "card", 1.0, 1.0),
            ],
            card_check: Some(1.0),
            diff_scoped: vec!["markers".into()],
            sandbox: "measure-2".into(),
            ..Default::default()
        };
        let laid = overlay_scores(cached, &fresh);
        let names: Vec<&str> = laid.fixtures.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["tests", "markers", "H1", "card-check"]);
        assert_eq!(laid.fixtures[1].score, 0.5);
        assert!((laid.total - (0.9 + 1.5 + 1.0 + 1.0) / 6.0).abs() < 1e-9);
        assert_eq!(laid.card_check, Some(1.0));
        assert_eq!(laid.diff_scoped, ["markers"]);
        assert_eq!(laid.ref_name, "base");
        assert_eq!(laid.heldout, 1);
        assert_eq!(laid.sandbox, "measure-2");
    }
}
