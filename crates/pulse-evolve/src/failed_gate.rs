#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/failed_gate.go.

use anyhow::Result;

/// Go: func FailedExperimentGate(s *store.Store, project string) (job, question, reason string, err error) {
pub fn failed_experiment_gate(
    s: &pulse_store::Store,
    project: &str,
) -> Result<(String, String, String)> {
    todo!("evolve::failed_gate::failed_experiment_gate")
}
