#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/daemon_watchdog.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) const DAEMON_STALL_FACTOR: i32 = 6;
pub(crate) const DAEMON_STALL_FLOOR: std::time::Duration = std::time::Duration::from_secs(30 * 60);
pub(crate) const WATCHDOG_KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(15);
pub(crate) const STALL_DUMP_KEEP: usize = 5;

#[derive(Debug, Default)]
pub(crate) struct DaemonWatchdog {
    pub(crate) mu: parking_lot::Mutex<()>,
    pub(crate) beat_at: chrono::DateTime<chrono::Utc>,
    pub(crate) bound: std::time::Duration,
    pub(crate) logs: String,
    pub(crate) say: serde_json::Value,
    pub(crate) note: serde_json::Value,
    pub(crate) kill: serde_json::Value,
    pub(crate) exit: serde_json::Value,
    pub(crate) now: serde_json::Value,
    pub(crate) dump: serde_json::Value,
    pub(crate) grace: std::time::Duration,
    pub(crate) fired: bool,
}

/// Go: func daemonStallBound(interval int) time.Duration
pub(crate) fn daemon_stall_bound(interval: i32) -> std::time::Duration {
    todo!("runner::daemon_watchdog::daemon_stall_bound")
}

/// Go: func newDaemonWatchdog(bound time.Duration, logs string, say func(string, ...any), note func(note, dump string), kill func()) *daemonWatchdog
pub(crate) fn new_daemon_watchdog(
    bound: std::time::Duration,
    logs: String,
    say: serde_json::Value,
    note: serde_json::Value,
    kill: serde_json::Value,
) -> Option<DaemonWatchdog> {
    todo!("runner::daemon_watchdog::new_daemon_watchdog")
}

/// Go: func goroutineDump() []byte
pub(crate) fn goroutine_dump() -> Vec<u8> {
    todo!("runner::daemon_watchdog::goroutine_dump")
}

/// Go: func pruneStallDumps(logs string, keep int)
pub(crate) fn prune_stall_dumps(logs: String, keep: i32) {
    todo!("runner::daemon_watchdog::prune_stall_dumps")
}

/// Go: func dumpStamp(name string) string
pub(crate) fn dump_stamp(name: String) -> String {
    todo!("runner::daemon_watchdog::dump_stamp")
}

impl DaemonWatchdog {
    /// Go: func (w *daemonWatchdog) beat()
    pub(crate) fn beat(&mut self) {
        todo!("runner::daemon_watchdog::beat")
    }
    /// Go: func (w *daemonWatchdog) stalled() time.Duration
    pub(crate) fn stalled(&mut self) -> std::time::Duration {
        todo!("runner::daemon_watchdog::stalled")
    }
    /// Go: func (w *daemonWatchdog) run(ctx context.Context, tick time.Duration)
    pub(crate) fn run(&mut self, cancel: &pulse_bus::CancellationToken, tick: std::time::Duration) {
        todo!("runner::daemon_watchdog::run")
    }
    /// Go: func (w *daemonWatchdog) fire(since time.Duration)
    pub(crate) fn fire(&mut self, since: std::time::Duration) {
        todo!("runner::daemon_watchdog::fire")
    }
    /// Go: func (w *daemonWatchdog) writeDump(why string) (string, error)
    pub(crate) fn write_dump(&mut self, why: String) -> anyhow::Result<String> {
        todo!("runner::daemon_watchdog::write_dump")
    }
}
