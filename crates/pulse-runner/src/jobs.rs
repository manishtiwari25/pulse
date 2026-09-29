#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/jobs.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) const OCCUPIED_JOB_SQL: &str = "(k.status = 'leased' OR (k.status != 'queued' AND EXISTS (SELECT 1 FROM runs r WHERE r.job_id = k.id AND r.project = k.project AND r.agent = k.lease_agent AND r.status = 'running')))";

#[derive(Debug, Clone, Default)]
pub(crate) struct LeaseOpts {
    pub project: String,
    pub job_id: String,
    pub minutes: i32,
    pub projects: Vec<String>,
    pub any_proj: bool,
}

/// Go: func queueOnboardingJob(s *store.Store, slug, createdBy string) (row, bool, error)
pub(crate) fn queue_onboarding_job(
    s: &pulse_store::Store,
    slug: String,
    created_by: String,
) -> anyhow::Result<(Row, bool)> {
    todo!("runner::jobs::queue_onboarding_job")
}

/// Go: func addJob(s *store.Store, project, kind, title, brief string, tier, priority int, source, createdBy string, dedupe bool) (row, bool, error)
pub(crate) fn add_job(
    s: &pulse_store::Store,
    project: String,
    kind: String,
    title: String,
    brief: String,
    tier: i32,
    priority: i32,
    source: String,
    created_by: String,
    dedupe: bool,
) -> anyhow::Result<(Row, bool)> {
    todo!("runner::jobs::add_job")
}

/// Go: func jobRow(q querier, id string) row
pub(crate) fn job_row(q: &Querier, id: String) -> Row {
    todo!("runner::jobs::job_row")
}

/// Go: func parallelPerProject(s *store.Store) int
pub(crate) fn parallel_per_project(s: &pulse_store::Store) -> i32 {
    todo!("runner::jobs::parallel_per_project")
}

/// Go: func leaseJob(s *store.Store, agent, now string, o leaseOpts) (job row, err error)
pub(crate) fn lease_job(
    s: &pulse_store::Store,
    agent: String,
    now: String,
    o: LeaseOpts,
) -> anyhow::Result<Row> {
    todo!("runner::jobs::lease_job")
}

/// Go: func finishJob(s *store.Store, jobID, agent, status, result, branch string, generation int) (row, error)
pub(crate) fn finish_job(
    s: &pulse_store::Store,
    job_id: String,
    agent: String,
    status: String,
    result: String,
    branch: String,
    generation: i32,
) -> anyhow::Result<Row> {
    todo!("runner::jobs::finish_job")
}

/// Go: func renewLease(q querier, now, jobID, agent string, generation, minutes int) bool
pub(crate) fn renew_lease(
    q: &Querier,
    now: String,
    job_id: String,
    agent: String,
    generation: i32,
    minutes: i32,
) -> bool {
    todo!("runner::jobs::renew_lease")
}
