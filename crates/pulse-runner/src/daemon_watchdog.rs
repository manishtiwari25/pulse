#![allow(dead_code)]

//! Skeleton port of internal/runner/daemon_watchdog.go.

use std::path::Path;
use std::time::Duration;

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
    let d = Duration::from_secs(interval.max(0) as u64).saturating_mul(DAEMON_STALL_FACTOR as u32);
    d.max(DAEMON_STALL_FLOOR)
}

/// Go: func newDaemonWatchdog(bound time.Duration, logs string, say func(string, ...any), note func(note, dump string), kill func()) *daemonWatchdog
pub(crate) fn new_daemon_watchdog(
    bound: std::time::Duration,
    logs: String,
    say: serde_json::Value,
    note: serde_json::Value,
    kill: serde_json::Value,
) -> Option<DaemonWatchdog> {
    Some(DaemonWatchdog {
        beat_at: chrono::Utc::now(),
        bound,
        logs,
        say,
        note,
        kill,
        exit: serde_json::Value::Null,
        now: serde_json::Value::Null,
        dump: serde_json::Value::Null,
        grace: WATCHDOG_KILL_GRACE,
        ..Default::default()
    })
}

/// Go: func goroutineDump() []byte
pub(crate) fn goroutine_dump() -> Vec<u8> {
    format!(
        "thread dump unavailable in Rust port\npid {}\nthread {:?}\n",
        std::process::id(),
        std::thread::current().id()
    )
    .into_bytes()
}

/// Go: func pruneStallDumps(logs string, keep int)
pub(crate) fn prune_stall_dumps(logs: String, keep: i32) {
    let Ok(entries) = std::fs::read_dir(&logs) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| {
            (n.starts_with("daemon-stall-") || n.starts_with("daemon-signal-"))
                && n.ends_with(".log")
        })
        .collect();
    if names.len() <= keep.max(0) as usize {
        return;
    }
    names.sort_by_key(|n| dump_stamp(n.clone()));
    let remove = names.len() - keep.max(0) as usize;
    for n in names.into_iter().take(remove) {
        let _ = std::fs::remove_file(Path::new(&logs).join(n));
    }
}

/// Go: func dumpStamp(name string) string
pub(crate) fn dump_stamp(name: String) -> String {
    name.rsplit_once('-')
        .map(|(_, s)| s.trim_end_matches(".log").to_string())
        .unwrap_or(name)
}

impl DaemonWatchdog {
    /// Go: func (w *daemonWatchdog) beat()
    pub(crate) fn beat(&mut self) {
        let _guard = self.mu.lock();
        self.beat_at = chrono::Utc::now();
    }
    /// Go: func (w *daemonWatchdog) stalled() time.Duration
    pub(crate) fn stalled(&mut self) -> std::time::Duration {
        let _guard = self.mu.lock();
        let since = chrono::Utc::now()
            .signed_duration_since(self.beat_at)
            .to_std()
            .unwrap_or_default();
        if since > self.bound {
            since
        } else {
            Duration::ZERO
        }
    }
    /// Go: func (w *daemonWatchdog) run(ctx context.Context, tick time.Duration)
    pub(crate) fn run(&mut self, cancel: &pulse_bus::CancellationToken, tick: std::time::Duration) {
        while !cancel.is_cancelled() {
            std::thread::sleep(tick);
            let since = self.stalled();
            if since > Duration::ZERO {
                self.fire(since);
                return;
            }
        }
    }
    /// Go: func (w *daemonWatchdog) fire(since time.Duration)
    pub(crate) fn fire(&mut self, _since: std::time::Duration) {
        {
            let _guard = self.mu.lock();
            if self.fired {
                return;
            }
            self.fired = true;
        }
        let _ = self.write_dump("stall".into());
        std::thread::sleep(self.grace);
    }
    /// Go: func (w *daemonWatchdog) writeDump(why string) (string, error)
    pub(crate) fn write_dump(&mut self, why: String) -> anyhow::Result<String> {
        std::fs::create_dir_all(&self.logs)?;
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let path = Path::new(&self.logs).join(format!("daemon-{why}-{stamp}.log"));
        let mut body = format!(
            "pulse daemon pid {}, {}, {} UTC\n\n",
            std::process::id(),
            why,
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
        )
        .into_bytes();
        body.extend(goroutine_dump());
        std::fs::write(&path, body)?;
        prune_stall_dumps(self.logs.clone(), STALL_DUMP_KEEP as i32);
        Ok(path.display().to_string())
    }
}
