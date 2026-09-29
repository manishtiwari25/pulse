#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/failure.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) const SIGNATURE_HEAD: usize = 72;

#[derive(Debug, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub(crate) struct FailureClass(pub String);

#[derive(Debug, Clone, Default)]
pub(crate) struct Failure {
    pub class: crate::failure::FailureClass,
    pub reason: String,
    pub signature: String,
}

/// Go: func environmentFailureReason(result, runSummary string) string
pub(crate) fn environment_failure_reason(result: String, run_summary: String) -> String {
    todo!("runner::failure::environment_failure_reason")
}

/// Go: func classifyFailure(result, runStatus, runSummary string, askedOwner bool) failure
pub(crate) fn classify_failure(
    result: String,
    run_status: String,
    run_summary: String,
    asked_owner: bool,
) -> crate::failure::Failure {
    todo!("runner::failure::classify_failure")
}

/// Go: func containsAny(text string, marks []string) bool
pub(crate) fn contains_any(text: String, marks: &[String]) -> bool {
    todo!("runner::failure::contains_any")
}

/// Go: func failureSignature(class failureClass, reason string) string
pub(crate) fn failure_signature(class: crate::failure::FailureClass, reason: String) -> String {
    todo!("runner::failure::failure_signature")
}

/// Go: func recoverBackoff(n int) time.Duration
pub(crate) fn recover_backoff(n: i32) -> std::time::Duration {
    todo!("runner::failure::recover_backoff")
}

/// Go: func recoverDue(finishedAt string, n int) string
pub(crate) fn recover_due(finished_at: String, n: i32) -> String {
    todo!("runner::failure::recover_due")
}
