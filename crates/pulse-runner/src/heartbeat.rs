#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/heartbeat.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) type HeartbeatTicksFn = dyn Fn(
        std::time::Duration,
    ) -> (
        crossbeam_channel::Receiver<chrono::DateTime<chrono::Utc>>,
        Box<dyn Fn() + Send + Sync>,
    ) + Send
    + Sync;
pub(crate) static HEARTBEAT_TICKS: LazyLock<RwLock<Box<HeartbeatTicksFn>>> = LazyLock::new(|| {
    RwLock::new(Box::new(|period| {
        todo!("runner::heartbeat::heartbeat_ticks")
    }))
});
pub(crate) fn set_heartbeat_ticks(f: Box<HeartbeatTicksFn>) {
    *HEARTBEAT_TICKS.write().expect("heartbeat ticks lock") = f;
}

pub(crate) type HeartbeatNowFn =
    dyn Fn(&pulse_store::Store) -> anyhow::Result<chrono::DateTime<chrono::Utc>> + Send + Sync;
pub(crate) static HEARTBEAT_NOW: LazyLock<RwLock<Box<HeartbeatNowFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(|s| todo!("runner::heartbeat::heartbeat_now"))));
pub(crate) fn set_heartbeat_now(f: Box<HeartbeatNowFn>) {
    *HEARTBEAT_NOW.write().expect("heartbeat now lock") = f;
}

/// Go: func heartbeatReporter(log *runLog) func(error)
pub(crate) fn heartbeat_reporter(log: Option<crate::exec::RunLog>) -> serde_json::Value {
    todo!("runner::heartbeat::heartbeat_reporter")
}

/// Go: func stepsCapable(fields map[string]string) bool
pub(crate) fn steps_capable(fields: BTreeMap<String, String>) -> bool {
    todo!("runner::heartbeat::steps_capable")
}

/// Go: func handsHeartbeatPath(dir, session string) (string, error)
pub(crate) fn hands_heartbeat_path(dir: String, session: String) -> anyhow::Result<String> {
    todo!("runner::heartbeat::hands_heartbeat_path")
}

/// Go: func handsStepsPath(dir, session string) (string, error)
pub(crate) fn hands_steps_path(dir: String, session: String) -> anyhow::Result<String> {
    todo!("runner::heartbeat::hands_steps_path")
}

/// Go: func writeHandsHeartbeat(path string, h board.ExecutionHeartbeat) error
pub(crate) fn write_hands_heartbeat(
    path: String,
    h: pulse_board::ExecutionHeartbeat,
) -> anyhow::Result<()> {
    todo!("runner::heartbeat::write_hands_heartbeat")
}

impl crate::exec::LocalExec {
    /// Go: func (e *localExec) execution(env map[string]string) board.ExecutionIdentity
    pub(crate) fn execution(
        &mut self,
        env: BTreeMap<String, String>,
    ) -> pulse_board::ExecutionIdentity {
        todo!("runner::heartbeat::execution")
    }
    /// Go: func (e *localExec) observer(id board.ExecutionIdentity) *processObserver
    pub(crate) fn observer(
        &mut self,
        id: pulse_board::ExecutionIdentity,
    ) -> Option<crate::proc::ProcessObserver> {
        todo!("runner::heartbeat::observer")
    }
    /// Go: func (e *localExec) stopHeartbeat(id board.ExecutionIdentity, report func(error))
    pub(crate) fn stop_heartbeat(
        &mut self,
        id: pulse_board::ExecutionIdentity,
        report: serde_json::Value,
    ) {
        todo!("runner::heartbeat::stop_heartbeat")
    }
}

impl crate::exec::SshExec {
    /// Go: func (e *sshExec) heartbeatCapable(ctx context.Context) bool
    pub(crate) fn heartbeat_capable(&mut self, cancel: &pulse_bus::CancellationToken) -> bool {
        todo!("runner::heartbeat::heartbeat_capable")
    }
    /// Go: func (e *sshExec) probeCapabilities(ctx context.Context) map[string]string
    pub(crate) fn probe_capabilities(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
    ) -> BTreeMap<String, String> {
        todo!("runner::heartbeat::probe_capabilities")
    }
    /// Go: func (e *sshExec) heartbeatCapableFrom(fields map[string]string) bool
    pub(crate) fn heartbeat_capable_from(&mut self, fields: BTreeMap<String, String>) -> bool {
        todo!("runner::heartbeat::heartbeat_capable_from")
    }
    /// Go: func (e *sshExec) watchRemote(ctx context.Context, id board.ExecutionIdentity)
    pub(crate) fn watch_remote(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        id: pulse_board::ExecutionIdentity,
    ) {
        todo!("runner::heartbeat::watch_remote")
    }
}

impl crate::hands::Hands {
    /// Go: func (h *hands) heartbeatObserver(job string, env map[string]string) (*processObserver, error)
    pub(crate) fn heartbeat_observer(
        &mut self,
        job: String,
        env: BTreeMap<String, String>,
    ) -> anyhow::Result<Option<crate::proc::ProcessObserver>> {
        todo!("runner::heartbeat::heartbeat_observer")
    }
    /// Go: func (h *hands) heartbeat(job, session string) error
    pub(crate) fn heartbeat(&mut self, job: String, session: String) -> anyhow::Result<()> {
        todo!("runner::heartbeat::heartbeat")
    }
    /// Go: func (h *hands) steps(job, session, offsetText string) error
    pub(crate) fn steps(
        &mut self,
        job: String,
        session: String,
        offset_text: String,
    ) -> anyhow::Result<()> {
        todo!("runner::heartbeat::steps")
    }
}
