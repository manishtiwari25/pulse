#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, Once};
use std::time::Duration;

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
pub fn measure(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    repo: &str,
    ref_name: &str,
    opts: MeasureOptions<'_>,
) -> Result<Scores> {
    todo!("evolve::api::measure")
}

/// Go: func Keep(baseline, variant *Scores, tolerance float64) (keep bool, reason string)
pub fn keep(baseline: Option<&Scores>, variant: Option<&Scores>, tolerance: f64) -> (bool, String) {
    todo!("evolve::api::keep")
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
    todo!("evolve::api::is_baseline_row")
}

/// Go: var Archive = func(s *store.Store, row ArchiveRow) (id string, err error)
pub fn archive_row(s: &pulse_store::Store, row: ArchiveRow) -> Result<String> {
    todo!("evolve::api::archive_row")
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}
