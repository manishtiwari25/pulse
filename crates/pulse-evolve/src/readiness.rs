#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/readiness.go.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanningStatus {
    pub state: String,
    pub reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub job: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub suggestion: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub question: String,
}

/// Go: func planningStatus(s *store.Store, status *LoopStatus) (PlanningStatus, error) {
pub(crate) fn planning_status(
    s: &pulse_store::Store,
    status: &crate::status::LoopStatus,
) -> Result<PlanningStatus> {
    todo!("evolve::readiness::planning_status")
}

/// Go: func experimentFinalizing(s *store.Store, job *board.Job) (bool, error) {
pub(crate) fn experiment_finalizing(
    s: &pulse_store::Store,
    job: &pulse_board::Job,
) -> Result<bool> {
    todo!("evolve::readiness::experiment_finalizing")
}
