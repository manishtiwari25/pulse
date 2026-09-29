#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/feedback.go.

use anyhow::Result;

/// Go: func PlanFeedback(s *store.Store, project, actor string) (*board.Job, error) {
pub fn plan_feedback(
    s: &pulse_store::Store,
    project: &str,
    actor: &str,
) -> Result<Option<pulse_board::Job>> {
    todo!("evolve::feedback::plan_feedback")
}
