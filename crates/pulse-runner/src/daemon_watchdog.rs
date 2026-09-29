//! Port of internal/runner/daemon_watchdog.go.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

// The loop body is meant to be quick - landing verification, measurement and the runs themselves are off
// the loop by design - so a heartbeat older than this is a loop that hangs, not one that is busy. The
// factor keeps a long owner interval honest; the floor leaves the planner's model calls (120s per request,
// retried) and the checkout probes their room.
pub(crate) const DAEMON_STALL_FACTOR: u32 = 6;
pub(crate) const DAEMON_STALL_FLOOR: Duration = Duration::from_secs(30 * 60);
/// watchdogKillGrace is how long the runs in flight get to see their cancelled context before exit.
pub(crate) const WATCHDOG_KILL_GRACE: Duration = Duration::from_secs(15);
/// dumps older than the newest this many are removed
pub(crate) const STALL_DUMP_KEEP: usize = 5;

type SayFn = Arc<dyn Fn(String) + Send + Sync>;
type NoteFn = Arc<dyn Fn(String, String) + Send + Sync>;
type VoidFn = Arc<dyn Fn() + Send + Sync>;
type ExitFn = Arc<dyn Fn(i32) + Send + Sync>;
type NowFn = Arc<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>;
type DumpFn = Arc<dyn Fn() -> Vec<u8> + Send + Sync>;

/// daemonStallBound is how long the loop may go without a heartbeat before the watchdog acts.
///
/// Go: func daemonStallBound(interval int) time.Duration
pub(crate) fn daemon_stall_bound(interval: i32) -> Duration {
    let d = Duration::from_secs(interval.max(0) as u64) * DAEMON_STALL_FACTOR;
    if d < DAEMON_STALL_FLOOR {
        return DAEMON_STALL_FLOOR;
    }
    d
}

/// Go: the fields daemonWatchdog guards with mu.
pub(crate) struct WatchdogState {
    pub(crate) beat_at: chrono::DateTime<chrono::Utc>,
    pub(crate) fired: bool,
}

/// daemonWatchdog restarts a daemon whose loop stopped beating. The loop cannot leave on its own when one
/// of its synchronous steps blocks (a syscall macOS holds behind a consent dialog, a lock never released),
/// so `pulse status` said "silent" for as long as nobody looked. Now the stall is written down - a thread
/// dump under logs/, a note in the heartbeat row - the runs in flight are cancelled, and the process exits
/// 3, the code the service launcher restarts on (ADR-035). SIGUSR1 writes the same dump without exiting,
/// for looking at a slow loop while it runs.
///
/// Go: type daemonWatchdog struct
pub(crate) struct DaemonWatchdog {
    /// Go: mu, beatAt, fired
    pub(crate) state: parking_lot::Mutex<WatchdogState>,
    pub(crate) bound: Duration,
    pub(crate) logs: String,
    pub(crate) say: SayFn,
    /// records the stall in the heartbeat row and the event log
    pub(crate) note: Option<NoteFn>,
    /// cancels the runs in flight
    pub(crate) kill: Option<VoidFn>,
    /// os.Exit unless a test says otherwise
    pub(crate) exit: ExitFn,
    pub(crate) now: NowFn,
    pub(crate) dump: DumpFn,
    /// the runs' time to see their cancelled context before exit
    pub(crate) grace: Duration,
}

/// Go: func newDaemonWatchdog(bound time.Duration, logs string, say func(string, ...any), note func(note, dump string), kill func()) *daemonWatchdog
pub(crate) fn new_daemon_watchdog(
    bound: Duration,
    logs: String,
    say: SayFn,
    note: Option<NoteFn>,
    kill: Option<VoidFn>,
) -> DaemonWatchdog {
    DaemonWatchdog {
        state: parking_lot::Mutex::new(WatchdogState {
            beat_at: chrono::Utc::now(),
            fired: false,
        }),
        bound,
        logs,
        say,
        note,
        kill,
        exit: Arc::new(|code| std::process::exit(code)),
        now: Arc::new(chrono::Utc::now),
        dump: Arc::new(goroutine_dump),
        grace: WATCHDOG_KILL_GRACE,
    }
}

impl DaemonWatchdog {
    /// beat is called by the loop each time it writes its heartbeat.
    ///
    /// Go: func (w *daemonWatchdog) beat()
    pub(crate) fn beat(&self) {
        self.state.lock().beat_at = (self.now)();
    }

    /// stalled reports how long the loop has been silent past its bound, or 0.
    ///
    /// Go: func (w *daemonWatchdog) stalled() time.Duration
    pub(crate) fn stalled(&self) -> Duration {
        let state = self.state.lock();
        let since = (self.now)()
            .signed_duration_since(state.beat_at)
            .to_std()
            .unwrap_or_default();
        if since > self.bound {
            return since;
        }
        Duration::ZERO
    }

    /// run checks the beat every tick until ctx ends; on a stall it dumps, notes, kills and exits once.
    ///
    /// Go: func (w *daemonWatchdog) run(ctx context.Context, tick time.Duration)
    pub(crate) fn run(&self, cancel: &pulse_bus::CancellationToken, tick: Duration) {
        let timer = crossbeam_channel::tick(tick);
        let usr1 = Usr1::notify();
        let done = cancel.done();
        loop {
            crossbeam_channel::select! {
                recv(done) -> _ => return,
                recv(usr1.signals) -> _ => match self.write_dump("signal") {
                    Err(err) => (self.say)(format!("watchdog: goroutine dump failed: {err}")),
                    Ok(path) => (self.say)(format!("watchdog: goroutine dump on SIGUSR1 at {path}")),
                },
                recv(timer) -> _ => {
                    let since = self.stalled();
                    if since > Duration::ZERO {
                        self.fire(since);
                        return;
                    }
                }
            }
        }
    }

    /// Go: func (w *daemonWatchdog) fire(since time.Duration)
    pub(crate) fn fire(&self, since: Duration) {
        {
            let mut state = self.state.lock();
            if state.fired {
                return;
            }
            state.fired = true;
        }
        let (path, where_) = match self.write_dump("stall") {
            Ok(path) => (path.clone(), path),
            Err(err) => (String::new(), format!("no dump: {err}")),
        };
        let rounded = pulse_cli::format_duration(round_seconds(since));
        let line = format!(
            "watchdog: the loop has not beaten for {rounded} (bound {}); goroutines at {where_}; cancelling runs and exiting 3 so the service restarts the daemon",
            pulse_cli::format_duration(self.bound)
        );
        (self.say)(line);
        if let Some(note) = &self.note {
            note(format!("stalled {rounded}; restarting"), path);
        }
        if let Some(kill) = &self.kill {
            kill();
            std::thread::sleep(self.grace);
        }
        (self.exit)(3);
    }

    /// writeDump writes every thread's stack to logs/daemon-<why>-<utc>.log and prunes old dumps.
    ///
    /// Go: func (w *daemonWatchdog) writeDump(why string) (string, error)
    pub(crate) fn write_dump(&self, why: &str) -> std::io::Result<String> {
        create_dir_all_0700(Path::new(&self.logs))?;
        let name = format!("daemon-{why}-{}.log", (self.now)().format("%Y%m%dT%H%M%SZ"));
        let path = Path::new(&self.logs).join(name);
        let mut body = format!(
            "pulse daemon pid {}, {why}, {} UTC\n\n",
            std::process::id(),
            (self.now)().format("%Y-%m-%dT%H:%M:%SZ")
        )
        .into_bytes();
        body.extend((self.dump)());
        write_file_0600(&path, &body)?;
        prune_stall_dumps(&self.logs, STALL_DUMP_KEEP);
        Ok(path.display().to_string())
    }
}

/// Go: signal.Notify(usr1, syscall.SIGUSR1) with `defer signal.Stop(usr1)` - the handler is
/// unregistered when the watchdog's run ends.
struct Usr1 {
    signals: crossbeam_channel::Receiver<()>,
    handle: Option<signal_hook::iterator::Handle>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Usr1 {
    fn notify() -> Self {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let Ok(mut signals) = signal_hook::iterator::Signals::new([signal_hook::consts::SIGUSR1])
        else {
            return Self {
                signals: rx,
                handle: None,
                thread: None,
            };
        };
        let handle = signals.handle();
        let thread = std::thread::Builder::new()
            .name("daemon-watchdog-usr1".into())
            .spawn(move || {
                for _ in signals.forever() {
                    let _ = tx.try_send(());
                }
            })
            .ok();
        Self {
            signals: rx,
            handle: Some(handle),
            thread,
        }
    }
}

impl Drop for Usr1 {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.close();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Go: since.Round(time.Second) - the nearest whole second, halves rounding away from zero.
fn round_seconds(d: Duration) -> Duration {
    let nanos = d.as_nanos();
    let rem = nanos % 1_000_000_000;
    let floor = nanos - rem;
    if rem + rem < 1_000_000_000 {
        Duration::from_nanos(floor as u64)
    } else {
        Duration::from_nanos((floor + 1_000_000_000) as u64)
    }
}

/// Go: os.MkdirAll(w.logs, 0o700)
fn create_dir_all_0700(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Go: os.WriteFile(path, body, 0o600)
fn write_file_0600(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(body)
}

/// Go: func goroutineDump() []byte - Rust has no all-threads stack dump; the watchdog's own stack and
/// the process's thread count are what the port can record.
pub(crate) fn goroutine_dump() -> Vec<u8> {
    let backtrace = std::backtrace::Backtrace::force_capture();
    format!(
        "thread {:?} [watchdog]:\n{backtrace}\n",
        std::thread::current().name().unwrap_or("unnamed")
    )
    .into_bytes()
}

/// pruneStallDumps keeps the newest `keep` daemon-*.log dumps; names sort by their UTC stamp.
///
/// Go: func pruneStallDumps(logs string, keep int)
pub(crate) fn prune_stall_dumps(logs: &str, keep: usize) {
    let Ok(entries) = std::fs::read_dir(logs) else {
        return;
    };
    let mut names: Vec<String> = Vec::new();
    for e in entries.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if !e.file_type().is_ok_and(|t| t.is_dir())
            && (n.starts_with("daemon-stall-") || n.starts_with("daemon-signal-"))
            && n.ends_with(".log")
        {
            names.push(n);
        }
    }
    if names.len() <= keep {
        return;
    }
    // The UTC stamp is the only part that varies, so it orders the dumps oldest first.
    names.sort_by_key(|n| dump_stamp(n));
    for n in &names[..names.len() - keep] {
        let _ = std::fs::remove_file(Path::new(logs).join(n));
    }
}

/// Go: func dumpStamp(name string) string
pub(crate) fn dump_stamp(name: &str) -> String {
    match name.rfind('-') {
        None => name.to_string(),
        Some(i) => name[i + 1..].trim_end_matches(".log").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Mutex;

    /// Go: func testWait(d time.Duration) time.Duration
    fn test_wait(d: Duration) -> Duration {
        let mult = std::env::var("PULSE_TEST_DEADLINE_MULT")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|f| *f > 0.0)
            .map(|f| f.max(1.0))
            .unwrap_or(1.0);
        d.mul_f64(mult)
    }

    type Calls = Arc<Mutex<Vec<String>>>;
    type Clock = Arc<Mutex<chrono::DateTime<chrono::Utc>>>;

    /// newTestWatchdog: a watchdog with a fake clock and a captured exit; returns it and the recorded calls
    /// (and Go's t.TempDir(), which lives as long as the test).
    fn new_test_watchdog(
        bound: Duration,
    ) -> (Arc<DaemonWatchdog>, Calls, Clock, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs").display().to_string();
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
            Some(Arc::new(move |note, dump| {
                let base = Path::new(&dump)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                note_calls
                    .lock()
                    .unwrap()
                    .push(format!("note:{note}:{base}"));
            })),
            Some(Arc::new(move || {
                kill_calls.lock().unwrap().push("kill".into())
            })),
        );
        w.exit = Arc::new(move |_| exit_calls.lock().unwrap().push("exit".into()));
        w.grace = Duration::ZERO;
        w.now = Arc::new(move || *clock_now.lock().unwrap());
        w.state.lock().beat_at = *clock.lock().unwrap();
        w.dump = Arc::new(|| b"goroutine 1 [running]:\nmain.main()\n".to_vec());
        (Arc::new(w), calls, clock, dir)
    }

    #[test]
    fn daemon_stall_bound_scales_with_interval_above_the_floor() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            daemon_stall_bound(300),
            Duration::from_secs(30 * 60),
            "300s interval: want the 30m floor"
        );
        assert_eq!(
            daemon_stall_bound(5),
            Duration::from_secs(30 * 60),
            "5s interval: want the 30m floor"
        );
        assert_eq!(
            daemon_stall_bound(600),
            Duration::from_secs(60 * 60),
            "600s interval: want 6x"
        );
    }

    #[test]
    fn watchdog_fires_once_when_the_loop_stops_beating() {
        let _serial = crate::test_helpers::serial();
        let (w, calls, clock, _dir) = new_test_watchdog(Duration::from_secs(30 * 60));
        *clock.lock().unwrap() += chrono::TimeDelta::minutes(29);
        assert_eq!(
            w.stalled(),
            Duration::ZERO,
            "29m without a beat is within the bound"
        );
        w.beat();
        *clock.lock().unwrap() += chrono::TimeDelta::minutes(31);
        assert_eq!(
            w.stalled(),
            Duration::from_secs(31 * 60),
            "stalled = {:?}, want 31m after the last beat",
            w.stalled()
        );
        w.fire(w.stalled());
        w.fire(w.stalled());
        let got = calls.lock().unwrap().join(",");
        let entries: Vec<String> = std::fs::read_dir(&w.logs)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            entries.len() == 1 && entries[0].starts_with("daemon-stall-"),
            "dump files = {entries:?}"
        );
        assert_eq!(
            got,
            format!(
                "say,note:stalled 31m0s; restarting:{},kill,exit",
                entries[0]
            ),
            "calls = {got}"
        );
        let text = std::fs::read_to_string(Path::new(&w.logs).join(&entries[0])).unwrap();
        assert!(
            text.contains("goroutine 1 [running]") && text.contains("stall"),
            "dump = {text:?}"
        );
    }

    #[test]
    fn watchdog_run_stays_quiet_while_the_loop_beats() {
        let _serial = crate::test_helpers::serial();
        let (w, calls, _, _dir) = new_test_watchdog(Duration::from_secs(30 * 60));
        let cancel = pulse_bus::CancellationToken::new();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let run = Arc::clone(&w);
        let run_ctx = cancel.clone();
        std::thread::spawn(move || {
            run.run(&run_ctx, Duration::from_millis(1));
            let _ = done_tx.send(());
        });
        std::thread::sleep(Duration::from_millis(20));
        cancel.cancel();
        done_rx.recv().unwrap();
        assert!(
            calls.lock().unwrap().is_empty(),
            "a beating loop drew calls: {:?}",
            calls.lock().unwrap()
        );
    }

    #[test]
    fn watchdog_run_fires_from_its_tick() {
        let _serial = crate::test_helpers::serial();
        let (w, calls, clock, _dir) = new_test_watchdog(Duration::from_secs(30 * 60));
        *clock.lock().unwrap() += chrono::TimeDelta::hours(1);
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let run = Arc::clone(&w);
        std::thread::spawn(move || {
            run.run(
                &pulse_bus::CancellationToken::new(),
                Duration::from_millis(1),
            );
            let _ = done_tx.send(());
        });
        assert!(
            done_rx
                .recv_timeout(test_wait(Duration::from_secs(20)))
                .is_ok(),
            "run did not return after firing"
        );
        let got = calls.lock().unwrap().join(",");
        assert!(got.ends_with("kill,exit"), "calls = {got}");
    }

    #[test]
    fn prune_stall_dumps_keeps_the_newest() {
        let _serial = crate::test_helpers::serial();
        let logs = tempfile::tempdir().unwrap();
        for n in [
            "daemon-stall-20260914T100000Z.log",
            "daemon-signal-20260914T110000Z.log",
            "daemon-stall-20260914T120000Z.log",
            "daemon.log",
            "run-20260914T100000Z-abc.log",
        ] {
            std::fs::write(logs.path().join(n), b"x").unwrap();
        }
        prune_stall_dumps(&logs.path().display().to_string(), 2);
        let mut names: Vec<String> = std::fs::read_dir(logs.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        let want = "daemon-signal-20260914T110000Z.log daemon-stall-20260914T120000Z.log daemon.log run-20260914T100000Z-abc.log";
        assert_eq!(names.join(" "), want, "after prune: {}", names.join(" "));
    }
}
