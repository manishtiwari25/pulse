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

use crate::api::{ArchiveRow, FixtureScore, Scores};

/// ScorePair is the public archive fixture payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScorePair {
    #[serde(rename = "baseline")]
    pub baseline: Option<Scores>,
    #[serde(rename = "variant")]
    pub variant: Option<Scores>,
}

/// Archived is one archive row as shown by the CLI/API.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Archived {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "project")]
    pub project: String,
    #[serde(rename = "suggestion_post")]
    pub suggestion_post: String,
    #[serde(rename = "hypothesis")]
    pub hypothesis: String,
    #[serde(rename = "branch")]
    pub branch: String,
    #[serde(rename = "baseline_score")]
    pub baseline_score: f64,
    #[serde(rename = "variant_score")]
    pub variant_score: f64,
    #[serde(rename = "tolerance")]
    pub tolerance: f64,
    #[serde(rename = "kept")]
    pub kept: bool,
    #[serde(rename = "fixtures")]
    pub fixtures: ScorePair,
    #[serde(rename = "sandbox")]
    pub sandbox: String,
    #[serde(rename = "run_id")]
    pub run_id: String,
    #[serde(rename = "at")]
    pub at: String,
    #[serde(rename = "note")]
    pub note: String,
}

/// Go: func init() { Archive = archive }
pub(crate) fn init_archive() {
    todo!("evolve::archive::init")
}

/// Go: func publicScores(in *Scores) (*Scores, error)
pub(crate) fn public_scores(input: Option<&Scores>) -> Result<Scores> {
    todo!("evolve::archive::public_scores")
}

/// Go: func baselineShape(row ArchiveRow, baseline, variant *Scores) bool
pub(crate) fn baseline_shape(row: &ArchiveRow, baseline: &Scores, variant: &Scores) -> bool {
    todo!("evolve::archive::baseline_shape")
}

/// Go: func archive(s *store.Store, row ArchiveRow) (string, error)
pub(crate) fn archive(s: &pulse_store::Store, row: ArchiveRow) -> Result<String> {
    todo!("evolve::archive::archive")
}

/// Go: func ListArchive(s *store.Store, project string) ([]Archived, error)
pub fn list_archive(s: &pulse_store::Store, project: &str) -> Result<Vec<Archived>> {
    todo!("evolve::archive::list_archive")
}
