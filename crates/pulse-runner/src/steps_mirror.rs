#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/steps_mirror.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

#[derive(Debug, Clone, Default)]
pub(crate) struct RemoteStepsMirror {
    pub(crate) e: Option<Box<crate::exec::SshExec>>,
    pub(crate) id: String,
    pub(crate) offset: i64,
    pub(crate) pending: Vec<u8>,
    pub(crate) begun: bool,
    pub(crate) finished: bool,
    pub(crate) reported: bool,
    pub(crate) stopped: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RemoteStepsBegin {
    #[serde(rename = "event")]
    pub event: String,
    #[serde(rename = "session")]
    pub session: String,
    #[serde(rename = "run_id")]
    pub run_id: String,
    #[serde(rename = "job_id")]
    pub job_id: String,
    #[serde(rename = "agent")]
    pub agent: String,
    #[serde(rename = "provider")]
    pub provider: String,
    #[serde(rename = "model")]
    pub model: String,
    #[serde(rename = "cwd")]
    pub cwd: String,
    #[serde(rename = "at")]
    pub at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RemoteStepsStep {
    #[serde(rename = "event")]
    pub event: String,
    #[serde(rename = "n")]
    pub n: i32,
    #[serde(rename = "at")]
    pub at: String,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "content")]
    pub content: String,
    #[serde(rename = "command")]
    pub command: String,
    #[serde(rename = "exit_code")]
    pub exit: Option<Box<i32>>,
    #[serde(rename = "duration_ms")]
    pub ms: i64,
    #[serde(rename = "usage")]
    pub usage: pulse_provider::Usage,
    #[serde(rename = "model")]
    pub model: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RemoteStepsFinish {
    #[serde(rename = "event")]
    pub event: String,
    #[serde(rename = "exit_status")]
    pub exit_status: String,
    #[serde(rename = "submission")]
    pub submission: String,
    #[serde(rename = "error")]
    pub error: String,
    #[serde(rename = "cost")]
    pub cost: f64,
    #[serde(rename = "steps")]
    pub steps: i32,
    #[serde(rename = "at")]
    pub at: String,
}

/// Go: func newRemoteStepsMirror(e *sshExec, id string) *remoteStepsMirror
pub(crate) fn new_remote_steps_mirror(
    e: Option<crate::exec::SshExec>,
    id: String,
) -> Option<RemoteStepsMirror> {
    todo!("runner::steps_mirror::new_remote_steps_mirror")
}

impl RemoteStepsMirror {
    /// Go: func (m *remoteStepsMirror) watch(ctx context.Context)
    pub(crate) fn watch(&mut self, cancel: &pulse_bus::CancellationToken) {
        todo!("runner::steps_mirror::watch")
    }
    /// Go: func (m *remoteStepsMirror) fetch(ctx context.Context) bool
    pub(crate) fn fetch(&mut self, cancel: &pulse_bus::CancellationToken) -> bool {
        todo!("runner::steps_mirror::fetch")
    }
    /// Go: func (m *remoteStepsMirror) apply(chunk []byte) error
    pub(crate) fn apply(&mut self, chunk: Vec<u8>) -> anyhow::Result<()> {
        todo!("runner::steps_mirror::apply")
    }
    /// Go: func (m *remoteStepsMirror) finishIfMissing(out outcome, runErr error)
    pub(crate) fn finish_if_missing(&mut self, out: crate::proc::Outcome, run_err: anyhow::Error) {
        todo!("runner::steps_mirror::finish_if_missing")
    }
    /// Go: func (m *remoteStepsMirror) unavailable(err error)
    pub(crate) fn unavailable(&mut self, err: anyhow::Error) {
        todo!("runner::steps_mirror::unavailable")
    }
}
