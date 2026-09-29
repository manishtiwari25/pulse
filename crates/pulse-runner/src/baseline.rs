#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const BASELINE_TITLE: &str = "Record baseline";
pub(crate) const BASELINE_PRIORITY: i32 = 20;
pub(crate) const BASELINE_THREAD: &str = "baseline";

/// Go: func baselineSource(slug string) string
pub(crate) fn baseline_source(slug: String) -> String {
    todo!("runner::baseline::baseline_source")
}

/// Go: func enabledFixtures(s *store.Store, slug string) int
pub(crate) fn enabled_fixtures(s: &pulse_store::Store, slug: String) -> i32 {
    todo!("runner::baseline::enabled_fixtures")
}

/// Go: func baselineDue(s *store.Store, slug, now string) (due bool, why string)
pub(crate) fn baseline_due(s: &pulse_store::Store, slug: String, now: String) -> (bool, String) {
    todo!("runner::baseline::baseline_due")
}

/// Go: func planBaseline(s *store.Store, project row, slug, now, createdBy string) (row, bool, error)
pub(crate) fn plan_baseline(
    s: &pulse_store::Store,
    project: crate::util::Row,
    slug: String,
    now: String,
    created_by: String,
) -> anyhow::Result<(crate::util::Row, bool)> {
    todo!("runner::baseline::plan_baseline")
}

/// Go: func defaultBranch(project row) string
pub(crate) fn default_branch(project: crate::util::Row) -> String {
    todo!("runner::baseline::default_branch")
}

/// Go: func baselineBudget(s *store.Store, slug string) time.Duration
pub(crate) fn baseline_budget(s: &pulse_store::Store, slug: String) -> std::time::Duration {
    todo!("runner::baseline::baseline_budget")
}

/// Go: func jobBudget(s *store.Store, job row) time.Duration
pub(crate) fn job_budget(s: &pulse_store::Store, job: crate::util::Row) -> std::time::Duration {
    todo!("runner::baseline::job_budget")
}

/// Go: func runBaseline(s *store.Store, job, project row, agent, now, runID, logPath string, ex executor, o runOpts) (row, error)
pub(crate) fn run_baseline(
    s: &pulse_store::Store,
    job: crate::util::Row,
    project: crate::util::Row,
    agent: String,
    now: String,
    run_id: String,
    log_path: String,
    ex: serde_json::Value,
    o: crate::run::RunOpts,
) -> anyhow::Result<crate::util::Row> {
    todo!("runner::baseline::run_baseline")
}

/// Go: func baselineFinding(s *store.Store, slug, def, cause, logPath, runID, now string, arow *store.Agent, log *runLog)
pub(crate) fn baseline_finding(
    s: &pulse_store::Store,
    slug: String,
    def: String,
    cause: String,
    log_path: String,
    run_id: String,
    now: String,
    arow: &pulse_store::Agent,
    log: &crate::exec::RunLog,
) -> () {
    todo!("runner::baseline::baseline_finding")
}
