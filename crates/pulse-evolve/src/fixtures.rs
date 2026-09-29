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

/// Fixture is a public evaluator.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Fixture {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "project")]
    pub project: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(skip)]
    pub request: String,
    #[serde(skip)]
    pub scorer: String,
    #[serde(rename = "weight")]
    pub weight: f64,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "enabled")]
    pub enabled: bool,
    #[serde(rename = "created_by")]
    pub created_by: String,
    #[serde(rename = "at")]
    pub at: String,
    #[serde(rename = "answer_keys")]
    pub answer_keys: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FixtureInput {
    pub project: String,
    pub name: String,
    pub request: String,
    pub scorer: String,
    pub kind: String,
    pub actor: String,
    pub weight: f64,
    pub answer_keys: Vec<String>,
}

/// Go: var fixtureName = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$`)
pub(crate) static FIXTURE_NAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$").unwrap());

/// Go: func finite(v float64) bool
pub(crate) fn finite(v: f64) -> bool {
    todo!("evolve::fixtures::finite")
}

/// Go: func scoreValue(v float64) bool
pub(crate) fn score_value(v: f64) -> bool {
    todo!("evolve::fixtures::score_value")
}

/// Go: func inputError(what, why, fix string) error
pub(crate) fn input_error(what: &str, why: &str, fix: &str) -> anyhow::Error {
    todo!("evolve::fixtures::input_error")
}

/// Go: func validProject(s *store.Store, slug string) (*store.Project, error)
pub(crate) fn valid_project(s: &pulse_store::Store, slug: &str) -> Result<pulse_store::Project> {
    todo!("evolve::fixtures::valid_project")
}

/// Go: func relativePath(value string) (string, error)
pub(crate) fn relative_path(value: &str) -> Result<String> {
    todo!("evolve::fixtures::relative_path")
}

/// Go: func AddFixture(s *store.Store, in FixtureInput) (*Fixture, error)
pub fn add_fixture(s: &pulse_store::Store, input: FixtureInput) -> Result<Fixture> {
    todo!("evolve::fixtures::add_fixture")
}

/// Go: func ListFixtures(s *store.Store, project string, enabledOnly bool) ([]Fixture, error)
pub fn list_fixtures(
    s: &pulse_store::Store,
    project: &str,
    enabled_only: bool,
) -> Result<Vec<Fixture>> {
    todo!("evolve::fixtures::list_fixtures")
}

/// Go: func SetFixtureEnabled(s *store.Store, project, name string, enabled bool, actor string) error
pub fn set_fixture_enabled(
    s: &pulse_store::Store,
    project: &str,
    name: &str,
    enabled: bool,
    actor: &str,
) -> Result<()> {
    todo!("evolve::fixtures::set_fixture_enabled")
}

/// Go: func selectFixtures(s *store.Store, project string, names []string) ([]Fixture, error)
pub(crate) fn select_fixtures(
    s: &pulse_store::Store,
    project: &str,
    names: &[String],
) -> Result<Vec<Fixture>> {
    todo!("evolve::fixtures::select_fixtures")
}
