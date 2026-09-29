#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const EXPERIMENT_MEASURE_BUDGET: Duration = Duration::from_secs(90 * 60);
pub(crate) const DEFAULT_EVOLVE_TOLERANCE: f64 = 0.02;

/// Go: func briefFields(body string) map[string]string
pub(crate) fn brief_fields(body: String) -> BTreeMap<String, String> {
    todo!("runner::experiment::brief_fields")
}

/// Go: func evolveTolerance(s *store.Store) float64
pub(crate) fn evolve_tolerance(s: &pulse_store::Store) -> f64 {
    todo!("runner::experiment::evolve_tolerance")
}

/// Go: func experimentSuggestion(job row) (suggestion, hypothesis string)
pub(crate) fn experiment_suggestion(job: crate::util::Row) -> (String, String) {
    todo!("runner::experiment::experiment_suggestion")
}

/// Go: func experimentThread(s *store.Store, slug, suggestion string) (area, thread, on string)
pub(crate) fn experiment_thread(
    s: &pulse_store::Store,
    slug: String,
    suggestion: String,
) -> (String, String, String) {
    todo!("runner::experiment::experiment_thread")
}

/// Go: func lastRunID(s *store.Store, jobID string) string
pub(crate) fn last_run_id(s: &pulse_store::Store, job_id: String) -> String {
    todo!("runner::experiment::last_run_id")
}

/// Go: func scoreLine(sc *evolve.Scores) string
pub(crate) fn score_line(sc: &pulse_evolve::api::Scores) -> String {
    todo!("runner::experiment::score_line")
}

/// Go: func landExperiment(s *store.Store, project row, job row, actor, now string, verify bool) (state, note string)
pub(crate) fn land_experiment(
    s: &pulse_store::Store,
    project: crate::util::Row,
    job: crate::util::Row,
    actor: String,
    now: String,
    verify: bool,
) -> (String, String) {
    todo!("runner::experiment::land_experiment")
}

/// Go: func landExperimentContext(parent context.Context, s *store.Store, project row, job row, actor, now string, verify bool) (state, note string)
pub(crate) fn land_experiment_context(
    parent: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: crate::util::Row,
    job: crate::util::Row,
    actor: String,
    now: String,
    verify: bool,
) -> (String, String) {
    todo!("runner::experiment::land_experiment_context")
}
