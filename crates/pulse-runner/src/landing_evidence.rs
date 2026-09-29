#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const LANDING_EVIDENCE_MAX_BYTES: usize = 2 << 20;
pub(crate) const ERR_LANDING_EVIDENCE_TOO_LARGE: &str = "landing evidence too large";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CappedBuffer {
    pub(crate) buffer: Vec<u8>,
    pub(crate) limit: i32,
    pub(crate) exceeded: bool,
    pub(crate) cancel: serde_json::Value,
}

/// Go: func landingRefTransition(ctx context.Context, root, ref, target, action string) (string, string)
pub(crate) fn landing_ref_transition(
    ctx: &pulse_bus::CancellationToken,
    root: String,
    r#ref: String,
    target: String,
    action: String,
) -> (String, String) {
    todo!("runner::landing_evidence::landing_ref_transition")
}

/// Go: func landingObjectID(value string) bool
pub(crate) fn landing_object_id(value: String) -> bool {
    todo!("runner::landing_evidence::landing_object_id")
}

/// Go: func experimentArchive(s *store.Store, project, branch, run string) (row, error)
pub(crate) fn experiment_archive(
    s: &pulse_store::Store,
    project: String,
    branch: String,
    run: String,
) -> anyhow::Result<crate::util::Row> {
    todo!("runner::landing_evidence::experiment_archive")
}

/// Go: func landingCodeDeliveryEvidence(ctx context.Context, root, before, after string) codedelivery.Evidence
pub(crate) fn landing_code_delivery_evidence(
    ctx: &pulse_bus::CancellationToken,
    root: String,
    before: String,
    after: String,
) -> pulse_codedelivery::Evidence {
    todo!("runner::landing_evidence::landing_code_delivery_evidence")
}

/// Go: func limitedGitOutput(ctx context.Context, repo string, timeout time.Duration, maxBytes int, args ...string) ([]byte, string)
pub(crate) fn limited_git_output(
    ctx: &pulse_bus::CancellationToken,
    repo: String,
    timeout: std::time::Duration,
    max_bytes: i32,
    args: &[String],
) -> (Vec<u8>, String) {
    todo!("runner::landing_evidence::limited_git_output")
}

/// Go: func landingWorkSnapshot(s *store.Store, project, job row, o landOpts) codedelivery.WorkSnapshot
pub(crate) fn landing_work_snapshot(
    s: &pulse_store::Store,
    project: crate::util::Row,
    job: crate::util::Row,
    o: crate::land::LandOpts,
) -> pulse_codedelivery::WorkSnapshot {
    todo!("runner::landing_evidence::landing_work_snapshot")
}

/// Go: func recordLanding(s *store.Store, project row, branch, actor, action, result string, o landOpts, detail map[string]any) error
pub(crate) fn record_landing(
    s: &pulse_store::Store,
    project: crate::util::Row,
    branch: String,
    actor: String,
    action: String,
    result: String,
    o: crate::land::LandOpts,
    detail: BTreeMap<String, serde_json::Value>,
) -> anyhow::Result<()> {
    todo!("runner::landing_evidence::record_landing")
}

/// Go: func failMissingExperiment(s *store.Store, project, job row, actor, run, archive, reason string) error
pub(crate) fn fail_missing_experiment(
    s: &pulse_store::Store,
    project: crate::util::Row,
    job: crate::util::Row,
    actor: String,
    run: String,
    archive: String,
    reason: String,
) -> anyhow::Result<()> {
    todo!("runner::landing_evidence::fail_missing_experiment")
}

impl CappedBuffer {
    /// Go: func (b *cappedBuffer) Write(p []byte) (int, error)
    pub(crate) fn write(&mut self, p: Vec<u8>) -> anyhow::Result<i32> {
        todo!("runner::landing_evidence::write")
    }
}
