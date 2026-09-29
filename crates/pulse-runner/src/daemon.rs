#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/daemon.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

/// Go: const DefaultDaemonIntervalSeconds = 300
pub const DEFAULT_DAEMON_INTERVAL_SECONDS: i32 = 300;
pub(crate) const DISPATCH_TICK_SECONDS: i32 = 15;
pub(crate) const HANDS_WANTED_MINUTES: i32 = 30;
pub(crate) static BINARY_STAMP: LazyLock<RwLock<fn() -> String>> =
    LazyLock::new(|| RwLock::new(pulse_cli::binary_stamp));
pub(crate) fn set_binary_stamp(f: fn() -> String) {
    *BINARY_STAMP.write().expect("binary stamp lock") = f;
}
pub(crate) static DAEMON_WAKE: &[&str] = &[
    "message.sent",
    "post.question",
    "post.handoff",
    "task.created",
    "task.amended",
    "task.status",
    "compute.approved",
    "setting",
    "job.add",
];

#[derive(Debug, Clone, Default)]
pub struct Daemon {
    pub bus: serde_json::Value,
    pub store: serde_json::Value,
    pub log: serde_json::Value,
}

impl Daemon {
    /// Go: func daemon(ctx context.Context, s *store.Store, o daemonOpts) string
    pub fn run(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        o: DaemonOpts,
    ) -> String {
        todo!("runner::daemon::Daemon::run")
    }

    /// Go: func daemon(ctx context.Context, s *store.Store, o daemonOpts) string
    pub fn once(&mut self, cancel: &pulse_bus::CancellationToken, s: &pulse_store::Store) -> bool {
        todo!("runner::daemon::Daemon::once")
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Dispatch {
    pub(crate) slot: String,
    pub(crate) worker: String,
    pub(crate) job: Row,
    pub(crate) run: Row,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DaemonOpts {
    pub agent: String,
    pub interval: i32,
    pub dry_run: bool,
    pub worker_cmd: String,
    pub max_loops: i32,
    pub restart_on_change: bool,
    pub grace: std::time::Duration,
    pub out: serde_json::Value,
    pub sleep: serde_json::Value,
    pub hub_home: String,
    pub self_update: Option<Box<crate::selfupdate::SelfUpdateEnv>>,
    pub no_watchdog: bool,
}

/// Go: func daemonHeartbeat(s *store.Store, loops, interval int, note string)
pub(crate) fn daemon_heartbeat(s: &pulse_store::Store, loops: i32, interval: i32, note: String) {
    todo!("runner::daemon::daemon_heartbeat")
}

/// Go: func capacityNote(s *store.Store, queued, slots, inflight int, starvedSince, held, now string)
pub(crate) fn capacity_note(
    s: &pulse_store::Store,
    queued: i32,
    slots: i32,
    inflight: i32,
    starved_since: String,
    held: String,
    now: String,
) {
    todo!("runner::daemon::capacity_note")
}

/// Go: func handsWanted(s *store.Store, brain row, queued, slots, minutes int, now, actor string) row
pub(crate) fn hands_wanted(
    s: &pulse_store::Store,
    brain: Row,
    queued: i32,
    slots: i32,
    minutes: i32,
    now: String,
    actor: String,
) -> Row {
    todo!("runner::daemon::hands_wanted")
}

/// Go: func runBudget(s *store.Store) time.Duration
pub(crate) fn run_budget(s: &pulse_store::Store) -> std::time::Duration {
    todo!("runner::daemon::run_budget")
}

/// Go: func daemon(ctx context.Context, s *store.Store, o daemonOpts) string
pub(crate) fn daemon(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    o: DaemonOpts,
) -> String {
    todo!("runner::daemon::daemon")
}

/// Go: func boolKeys(m map[string]bool) map[string]string
pub(crate) fn bool_keys(m: BTreeMap<String, bool>) -> BTreeMap<String, String> {
    todo!("runner::daemon::bool_keys")
}
