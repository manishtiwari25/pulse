use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{LazyLock, RwLock};

/// Go: var ErrNotImplemented = errors.New("evolve: not implemented yet")
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
}

/// MeasureOptions tunes Measure.
#[derive(Default)]
pub struct MeasureOptions<'a> {
    pub fixtures: Vec<String>,
    pub shadow: bool,
    pub out: Option<&'a mut dyn Write>,
}

/// Go: var Measure = func(ctx context.Context, s *store.Store, project, repo, ref string, opts MeasureOptions) (*Scores, error)
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

/// Go: var Measure = measure
pub static MEASURE: LazyLock<RwLock<Box<MeasureFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(crate::measure::measure)));

/// Go: tests replace evolve.Measure and restore it.
pub fn set_measure(f: Box<MeasureFn>) -> Box<MeasureFn> {
    std::mem::replace(
        &mut *MEASURE.write().expect("evolve Measure lock poisoned"),
        f,
    )
}

/// Go: var Measure(...)
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

/// Go: func Keep(baseline, variant *Scores, tolerance float64) (keep bool, reason string)
///
/// A variant is kept unless it regresses the score by more than the tolerance:
/// an improvement is `kept`, a tie `kept (no regression)`. Held-out fixtures are
/// mostly pass/fail, so a voted change that keeps every fixture green measures
/// as a tie; it still has to pass the project's own check when it lands
/// (ADR-024), which is what decides a tie. Only `discarded` deletes the branch.
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
        (true, crate::format::format_delta("kept", delta, tolerance))
    } else {
        (
            true,
            crate::format::format_delta("kept (no regression)", delta, tolerance),
        )
    }
}

/// ArchiveRow is what the runner records after an experiment was measured.
#[derive(Debug, Clone, Default)]
pub struct ArchiveRow {
    pub project: String,
    pub suggestion_post: String,
    pub hypothesis: String,
    pub branch: String,
    pub baseline: Option<Scores>,
    pub variant: Option<Scores>,
    pub tolerance: f64,
    pub kept: bool,
    pub run_id: String,
    pub sandbox: String,
    pub note: String,
}

/// Go: const BaselineHypothesis = "baseline"
pub const BASELINE_HYPOTHESIS: &str = "baseline";

/// Go: func IsBaselineRow(hypothesis, suggestionPost string) bool
pub fn is_baseline_row(hypothesis: &str, suggestion_post: &str) -> bool {
    hypothesis == BASELINE_HYPOTHESIS && suggestion_post.is_empty()
}

/// Go: var Archive = func(s *store.Store, row ArchiveRow) (id string, err error)
pub type ArchiveFn =
    dyn Fn(&pulse_store::Store, ArchiveRow) -> Result<String> + Send + Sync + 'static;

/// Go: var Archive = archive
pub static ARCHIVE: LazyLock<RwLock<Box<ArchiveFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(crate::archive::archive)));

/// Go: tests replace evolve.Archive and restore it.
pub fn set_archive(f: Box<ArchiveFn>) -> Box<ArchiveFn> {
    std::mem::replace(
        &mut *ARCHIVE.write().expect("evolve Archive lock poisoned"),
        f,
    )
}

/// Go: var Archive(...)
pub fn archive_row(s: &pulse_store::Store, row: ArchiveRow) -> Result<String> {
    (ARCHIVE.read().expect("evolve Archive lock poisoned"))(s, row)
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}
