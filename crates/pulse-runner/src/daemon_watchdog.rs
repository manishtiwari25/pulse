#![allow(dead_code)]
#![allow(clippy::type_complexity)]

//! Skeleton port of internal/runner/daemon_watchdog.go.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub(crate) const DAEMON_STALL_FACTOR: i32 = 6;
pub(crate) const DAEMON_STALL_FLOOR: std::time::Duration = std::time::Duration::from_secs(30 * 60);
pub(crate) const WATCHDOG_KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(15);
pub(crate) const STALL_DUMP_KEEP: usize = 5;

type SayFn = Arc<dyn Fn(String) + Send + Sync>;
type NoteFn = Arc<dyn Fn(String, String) + Send + Sync>;
type VoidFn = Arc<dyn Fn() + Send + Sync>;
type ExitFn = Arc<dyn Fn(i32) + Send + Sync>;
type NowFn = Arc<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>;
type DumpFn = Arc<dyn Fn() -> Vec<u8> + Send + Sync>;

pub(crate) struct DaemonWatchdog {
    pub(crate) mu: parking_lot::Mutex<()>,
    pub(crate) beat_at: chrono::DateTime<chrono::Utc>,
    pub(crate) bound: std::time::Duration,
    pub(crate) logs: String,
    pub(crate) say: SayFn,
    pub(crate) note: NoteFn,
    pub(crate) kill: VoidFn,
    pub(crate) exit: ExitFn,
    pub(crate) now: NowFn,
    pub(crate) dump: DumpFn,
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
    say: SayFn,
    note: NoteFn,
    kill: VoidFn,
) -> DaemonWatchdog {
    let now: NowFn = Arc::new(chrono::Utc::now);
    DaemonWatchdog {
        beat_at: now(),
        bound,
        logs,
        say,
        note,
        kill,
        exit: Arc::new(|code| std::process::exit(code)),
        now,
        dump: Arc::new(goroutine_dump),
        grace: WATCHDOG_KILL_GRACE,
        fired: false,
        mu: parking_lot::Mutex::new(()),
    }
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
        self.beat_at = (self.now)();
    }
    /// Go: func (w *daemonWatchdog) stalled() time.Duration
    pub(crate) fn stalled(&mut self) -> std::time::Duration {
        let _guard = self.mu.lock();
        let since = (self.now)()
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
    pub(crate) fn fire(&mut self, since: std::time::Duration) {
        {
            let _guard = self.mu.lock();
            if self.fired {
                return;
            }
            self.fired = true;
        }
        let Ok(dump) = self.write_dump("stall".into()) else {
            return;
        };
        (self.say)(format!(
            "daemon loop stalled for {}; restarting after dump {}",
            format_duration(since),
            dump
        ));
        (self.note)(
            format!("stalled {}; restarting", format_duration(since)),
            dump,
        );
        (self.kill)();
        std::thread::sleep(self.grace);
        (self.exit)(1);
    }
    /// Go: func (w *daemonWatchdog) writeDump(why string) (string, error)
    pub(crate) fn write_dump(&mut self, why: String) -> anyhow::Result<String> {
        std::fs::create_dir_all(&self.logs)?;
        let now = (self.now)();
        let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let path = Path::new(&self.logs).join(format!("daemon-{why}-{stamp}.log"));
        let mut body = format!(
            "pulse daemon pid {}, {}, {} UTC\n\n",
            std::process::id(),
            why,
            now.format("%Y-%m-%dT%H:%M:%SZ")
        )
        .into_bytes();
        body.extend((self.dump)());
        std::fs::write(&path, body)?;
        prune_stall_dumps(self.logs.clone(), STALL_DUMP_KEEP as i32);
        Ok(path.display().to_string())
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let minutes = secs / 60;
    let seconds = secs % 60;
    if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::{Arc, Mutex};

    fn new_test_watchdog(
        bound: Duration,
    ) -> (
        DaemonWatchdog,
        Arc<Mutex<Vec<String>>>,
        Arc<Mutex<chrono::DateTime<chrono::Utc>>>,
    ) {
        let logs = tempfile::tempdir()
            .unwrap()
            .keep()
            .join("logs")
            .display()
            .to_string();
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let clock = Arc::new(Mutex::new(
            chrono::Utc.with_ymd_and_hms(2026, 9, 14, 15, 0, 0).unwrap(),
        ));
        let say_calls = calls.clone();
        let note_calls = calls.clone();
        let kill_calls = calls.clone();
        let exit_calls = calls.clone();
        let clock_now = clock.clone();
        let mut w = new_daemon_watchdog(
            bound,
            logs,
            Arc::new(move |_| say_calls.lock().unwrap().push("say".into())),
            Arc::new(move |note, dump| {
                let base = Path::new(&dump)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                note_calls
                    .lock()
                    .unwrap()
                    .push(format!("note:{note}:{base}"));
            }),
            Arc::new(move || kill_calls.lock().unwrap().push("kill".into())),
        );
        w.exit = Arc::new(move |_| exit_calls.lock().unwrap().push("exit".into()));
        w.grace = Duration::ZERO;
        w.now = Arc::new(move || *clock_now.lock().unwrap());
        w.beat_at = *clock.lock().unwrap();
        w.dump = Arc::new(|| b"goroutine 1 [running]:\nmain.main()\n".to_vec());
        (w, calls, clock)
    }

    #[test]
    fn daemon_stall_bound_scales_with_interval_above_the_floor() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(daemon_stall_bound(300), Duration::from_secs(30 * 60));
        assert_eq!(daemon_stall_bound(5), Duration::from_secs(30 * 60));
        assert_eq!(daemon_stall_bound(600), Duration::from_secs(60 * 60));
    }

    #[test]
    fn watchdog_fires_once_when_the_loop_stops_beating() {
        let _serial = crate::test_helpers::serial();
        let (mut w, calls, clock) = new_test_watchdog(Duration::from_secs(30 * 60));
        *clock.lock().unwrap() += chrono::TimeDelta::minutes(29);
        assert_eq!(
            w.stalled(),
            Duration::ZERO,
            "29m without a beat is within the bound"
        );
        w.beat();
        *clock.lock().unwrap() += chrono::TimeDelta::minutes(31);
        assert_eq!(w.stalled(), Duration::from_secs(31 * 60));
        let stalled = w.stalled();
        w.fire(stalled);
        let stalled = w.stalled();
        w.fire(stalled);
        let mut entries: Vec<_> = std::fs::read_dir(&w.logs)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        entries.sort();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].starts_with("daemon-stall-"));
        assert_eq!(
            calls.lock().unwrap().join(","),
            format!(
                "say,note:stalled 31m0s; restarting:{},kill,exit",
                entries[0]
            )
        );
        let text = std::fs::read_to_string(Path::new(&w.logs).join(&entries[0])).unwrap();
        assert!(text.contains("goroutine 1 [running]"));
        assert!(text.contains("stall"));
    }

    #[test]
    fn watchdog_run_stays_quiet_while_the_loop_beats() {
        let _serial = crate::test_helpers::serial();
        let (mut w, calls, _) = new_test_watchdog(Duration::from_secs(30 * 60));
        let cancel = pulse_bus::CancellationToken::new();
        let child = cancel.child();
        let handle = std::thread::spawn(move || w.run(&child, Duration::from_millis(1)));
        std::thread::sleep(Duration::from_millis(20));
        cancel.cancel();
        handle.join().unwrap();
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn watchdog_run_fires_from_its_tick() {
        let _serial = crate::test_helpers::serial();
        let (mut w, calls, clock) = new_test_watchdog(Duration::from_secs(30 * 60));
        *clock.lock().unwrap() += chrono::TimeDelta::hours(1);
        let cancel = pulse_bus::CancellationToken::new();
        let child = cancel.child();
        let handle = std::thread::spawn(move || w.run(&child, Duration::from_millis(1)));
        handle.join().unwrap();
        assert!(calls.lock().unwrap().join(",").ends_with("kill,exit"));
    }

    #[test]
    fn prune_stall_dumps_keeps_the_newest() {
        let _serial = crate::test_helpers::serial();
        let logs = tempfile::tempdir().unwrap();
        for name in [
            "daemon-stall-20260914T100000Z.log",
            "daemon-signal-20260914T110000Z.log",
            "daemon-stall-20260914T120000Z.log",
            "daemon.log",
            "run-20260914T100000Z-abc.log",
        ] {
            std::fs::write(logs.path().join(name), b"x").unwrap();
        }
        prune_stall_dumps(logs.path().display().to_string(), 2);
        let mut names: Vec<_> = std::fs::read_dir(logs.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names.join(" "),
            "daemon-signal-20260914T110000Z.log daemon-stall-20260914T120000Z.log daemon.log run-20260914T100000Z-abc.log"
        );
    }
}
