#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/fleet.go.

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub(crate) static MANAGED_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9_-]{1,100}$").unwrap());

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Fleet {
    pub id: String,
    pub project: String,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub workers: i64,
    pub paths: Vec<String>,
    pub at: String,
    pub created_by: String,
    pub cleaned_at: String,
    #[serde(skip)]
    pub(crate) keys: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FleetWorker {
    pub worker: i64,
    pub path: String,
    pub detached: bool,
    pub no_remote: bool,
    pub no_tags: bool,
    pub stripped: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub problem: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FleetJob {
    pub id: String,
    pub status: String,
    pub result: String,
    pub branch: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FleetVerification {
    pub fleet: Option<Fleet>,
    pub ok: bool,
    pub workers: Vec<FleetWorker>,
    pub jobs: Vec<FleetJob>,
}

/// Go: func fleetPaths(home string, f *Fleet) {
pub(crate) fn fleet_paths(home: &str, f: &mut Fleet) {
    todo!("evolve::fleet::fleet_paths")
}

/// Go: func PrepareFleet(ctx context.Context, s *store.Store, project, ref string, n int, actor string) (fleet *Fleet, retErr error) {
pub fn prepare_fleet(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    ref_: &str,
    n: i64,
    actor: &str,
) -> Result<Fleet> {
    todo!("evolve::fleet::prepare_fleet")
}

/// Go: func ListFleets(s *store.Store, project string) ([]Fleet, error) {
pub fn list_fleets(s: &pulse_store::Store, project: &str) -> Result<Vec<Fleet>> {
    todo!("evolve::fleet::list_fleets")
}

/// Go: func getFleet(s *store.Store, id string) (*Fleet, error) {
pub(crate) fn get_fleet(s: &pulse_store::Store, id: &str) -> Result<Fleet> {
    todo!("evolve::fleet::get_fleet")
}

/// Go: func VerifyFleet(ctx context.Context, s *store.Store, id string) (*FleetVerification, error) {
pub fn verify_fleet(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    id: &str,
) -> Result<FleetVerification> {
    todo!("evolve::fleet::verify_fleet")
}

/// Go: func CleanFleet(s *store.Store, id, actor string) error {
pub fn clean_fleet(s: &pulse_store::Store, id: &str, actor: &str) -> Result<()> {
    todo!("evolve::fleet::clean_fleet")
}

/// Go: func archivedForJob(s *store.Store, project, job string) (*Archived, error) {
pub(crate) fn archived_for_job(
    s: &pulse_store::Store,
    project: &str,
    job: &str,
) -> Result<Option<crate::archive::Archived>> {
    todo!("evolve::fleet::archived_for_job")
}
