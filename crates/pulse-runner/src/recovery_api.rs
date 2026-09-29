#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/recovery_api.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub const FAILED_JOBS_DEFAULT_LIMIT: i32 = 50;
pub const FAILED_JOBS_MAX_LIMIT: i32 = 100;
pub const JOB_REPAIR_MAX_SELECTION: i32 = crate::recover::RECOVER_PER_TICK;

#[derive(Debug, Clone, Default)]
pub struct FailedJobsQuery {
    pub project: String,
    pub limit: i32,
    pub before: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FailedJobView {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "project")]
    pub project: String,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "tier")]
    pub tier: i32,
    #[serde(rename = "title")]
    pub title: String,
    #[serde(rename = "source")]
    pub source: String,
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "result")]
    pub result: String,
    #[serde(rename = "finished_at")]
    pub finished_at: String,
    #[serde(rename = "class")]
    pub class: String,
    #[serde(rename = "category")]
    pub category: String,
    #[serde(rename = "reason")]
    pub reason: String,
    #[serde(rename = "fix")]
    pub fix: String,
    #[serde(rename = "eligible")]
    pub eligible: bool,
    #[serde(rename = "root")]
    pub root: String,
    #[serde(rename = "recoveries")]
    pub recoveries: i32,
    #[serde(rename = "recovery_limit")]
    pub recovery_limit: i32,
    #[serde(rename = "retry_at", default, skip_serializing_if = "String::is_empty")]
    pub retry_at: String,
    #[serde(
        rename = "carrier_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub carrier_id: String,
    #[serde(
        rename = "carrier_status",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub carrier_status: String,
    #[serde(
        rename = "last_run_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub last_run_id: String,
    #[serde(
        rename = "last_run_status",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub last_run_status: String,
    #[serde(
        rename = "last_run_summary",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub last_run_summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FailedJobsPage {
    #[serde(rename = "project")]
    pub project: String,
    #[serde(rename = "historical_total")]
    pub historical_total: i32,
    #[serde(rename = "jobs")]
    pub jobs: Vec<FailedJobView>,
    #[serde(rename = "page_counts")]
    pub page_counts: BTreeMap<String, i32>,
    #[serde(rename = "limit")]
    pub limit: i32,
    #[serde(rename = "next_before")]
    pub next_before: String,
    #[serde(rename = "max_selection")]
    pub max_selection: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepairJobsInput {
    #[serde(rename = "job_ids")]
    pub job_i_ds: Vec<String>,
    #[serde(rename = "project", default, skip_serializing_if = "String::is_empty")]
    pub project: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepairJobResult {
    #[serde(rename = "job_id")]
    pub job_id: String,
    #[serde(rename = "outcome")]
    pub outcome: String,
    #[serde(rename = "category")]
    pub category: String,
    #[serde(rename = "reason")]
    pub reason: String,
    #[serde(rename = "fix")]
    pub fix: String,
    #[serde(
        rename = "carrier_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub carrier_id: String,
    #[serde(
        rename = "carrier_status",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub carrier_status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepairJobsResult {
    #[serde(rename = "requested")]
    pub requested: i32,
    #[serde(rename = "queued")]
    pub queued: i32,
    #[serde(rename = "skipped")]
    pub skipped: i32,
    #[serde(rename = "errors")]
    pub errors: i32,
    #[serde(rename = "results")]
    pub results: Vec<RepairJobResult>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RecoveryPolicy {
    pub(crate) enabled: bool,
    pub(crate) days: i32,
    pub(crate) limit: i32,
    pub(crate) stopped: String,
}

/// Go: func ReadFailedJobs(s *store.Store, in FailedJobsQuery) (out *FailedJobsPage, err error)
pub fn read_failed_jobs(
    s: &pulse_store::Store,
    input: FailedJobsQuery,
) -> anyhow::Result<Option<FailedJobsPage>> {
    todo!("runner::recovery_api::read_failed_jobs")
}

/// Go: func decodeFailedJobsCursor(cursor string) (string, string, error)
pub(crate) fn decode_failed_jobs_cursor(cursor: String) -> anyhow::Result<(String, String)> {
    todo!("runner::recovery_api::decode_failed_jobs_cursor")
}

/// Go: func readRecoveryPolicy(q querier) (recoveryPolicy, error)
pub(crate) fn read_recovery_policy(q: &Querier) -> anyhow::Result<RecoveryPolicy> {
    todo!("runner::recovery_api::read_recovery_policy")
}

/// Go: func recoveryStopFile(root string) (bool, error)
pub(crate) fn recovery_stop_file(root: String) -> anyhow::Result<bool> {
    todo!("runner::recovery_api::recovery_stop_file")
}

/// Go: func inspectRecoveryFailure(q querier, job row, policy recoveryPolicy, now string) (v FailedJobView, f failure, run row, err error)
pub(crate) fn inspect_recovery_failure(
    q: &Querier,
    job: Row,
    policy: RecoveryPolicy,
    now: String,
) -> anyhow::Result<(FailedJobView, crate::failure::Failure, Row)> {
    todo!("runner::recovery_api::inspect_recovery_failure")
}

/// Go: func RepairFailedJobs(s *store.Store, in RepairJobsInput) (out *RepairJobsResult, err error)
pub fn repair_failed_jobs(
    s: &pulse_store::Store,
    input: RepairJobsInput,
) -> anyhow::Result<Option<RepairJobsResult>> {
    todo!("runner::recovery_api::repair_failed_jobs")
}
