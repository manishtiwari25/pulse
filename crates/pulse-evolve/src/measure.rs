#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/measure.go.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{LazyLock, RwLock};

pub(crate) type ExecutableHook = fn() -> std::io::Result<String>;

/// Go: var executable = os.Executable
pub(crate) static EXECUTABLE: LazyLock<RwLock<Option<ExecutableHook>>> =
    LazyLock::new(|| RwLock::new(None));

pub(crate) fn set_executable(f: Option<ExecutableHook>) {
    *EXECUTABLE.write().expect("executable hook poisoned") = f;
}

/// Go: func init() { Measure = measure }
pub(crate) fn init() {
    todo!("evolve::measure::init")
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
    todo!("evolve::measure::pulse_scorer")
}

/// Go: func selfScorer(scorer, exe string) string {
pub(crate) fn self_scorer(scorer: &str, exe: &str) -> String {
    todo!("evolve::measure::self_scorer")
}

/// Go: func scorerEnv(home, commit, exe string) []string {
pub(crate) fn scorer_env(home: &str, commit: &str, exe: &str) -> Vec<String> {
    todo!("evolve::measure::scorer_env")
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
    todo!("evolve::measure::measure")
}

/// Go: func measurementFailure(ctx context.Context, label, step string, err error) error {
pub(crate) fn measurement_failure(
    cancel: &pulse_bus::CancellationToken,
    label: &str,
    step: &str,
    err: anyhow::Error,
) -> anyhow::Error {
    todo!("evolve::measure::measurement_failure")
}

/// Go: func scoreClone(ctx context.Context, clone string, env, scorer []string) (value float64, reason string, retErr error) {
pub(crate) fn score_clone(
    cancel: &pulse_bus::CancellationToken,
    clone: &str,
    env: &[String],
    scorer: &[String],
) -> Result<(f64, String)> {
    todo!("evolve::measure::score_clone")
}

/// Go: func firstLine(text string, limit int) string {
pub(crate) fn first_line(text: &str, limit: usize) -> String {
    todo!("evolve::measure::first_line")
}

/// Go: func parseScore(text string) (float64, error) {
pub(crate) fn parse_score(text: &str) -> Result<f64> {
    todo!("evolve::measure::parse_score")
}
