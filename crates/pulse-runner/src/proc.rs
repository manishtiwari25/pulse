#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Process-tree control shared with the harness: own process groups for job
/// subprocesses, bounded SIGTERM/SIGKILL termination, and the straggler sweep
/// that kills whatever still works under a job directory before it is removed.
///
/// `kill_tree_under(dir)` is what to call right before a finished job's
/// workdir or clone is deleted (`LocalExec::cleanup` and the hands `clean`
/// verb do); it never touches the calling process, its parent, or anything
/// outside `dir`, and refuses directories too broad to be a job directory.
pub use pulse_cli::procs::{
    DEFAULT_KILL_GRACE, KilledProc, ProcInfo, ProcessGroup, describe_killed, kill_tree_under,
    kill_tree_under_with_grace, own_process_group, processes_under, sweepable_dir,
};

/// How long the output readers may lag once every process we know of is dead.
const OUTPUT_DRAIN: Duration = Duration::from_secs(2);

/// Reads a pipe on its own thread into a shared buffer; the receiver fires at EOF.
fn drain(
    reader: impl Read + Send + 'static,
) -> (Arc<Mutex<Vec<u8>>>, crossbeam_channel::Receiver<()>) {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = crossbeam_channel::bounded(1);
    let sink = buf.clone();
    std::thread::spawn(move || {
        let mut r = reader;
        let mut chunk = [0u8; 8192];
        loop {
            match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink.lock().unwrap().extend_from_slice(&chunk[..n]),
            }
        }
        let _ = tx.send(());
    });
    (buf, rx)
}

/// Waits for a reader until `deadline`, then takes whatever it has: a pipe
/// still held by something we could not kill must not hang the caller.
fn collect(
    buf: &Arc<Mutex<Vec<u8>>>,
    done: &crossbeam_channel::Receiver<()>,
    deadline: Instant,
) -> String {
    let _ = done.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    String::from_utf8_lossy(&std::mem::take(&mut *buf.lock().unwrap())).into_owned()
}

/// How long the wait sleeps between looks at the clock and the cancel flag
/// when neither the child, a heartbeat tick nor the cancel channel wakes it
/// first; a step of many minutes must not keep a core busy polling.
const WAIT_TICK: Duration = Duration::from_millis(250);

/// What ended the wait for a child.
enum Waited {
    /// The child exited by itself: its status, or the error `wait` gave.
    Exited(std::io::Result<std::process::ExitStatus>),
    /// The token was cancelled or the budget ran out while it was running.
    Interrupted,
}

/// A child parked in `waitpid` on a thread of its own, so the caller sleeps
/// on channels instead of polling `try_wait`. The thread reaps the child the
/// moment it exits and hands its status over.
struct Reaper {
    pid: i32,
    exit: crossbeam_channel::Receiver<std::io::Result<std::process::ExitStatus>>,
}

impl Reaper {
    fn start(mut child: Child) -> Self {
        let pid = child.id() as i32;
        let (tx, exit) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(child.wait());
        });
        Reaper { pid, exit }
    }

    /// Blocks until the child exits, `cancel` is done, or `budget` (when set)
    /// has passed since `started`. The observer's beat runs once at the start
    /// and again on every heartbeat tick meanwhile.
    fn await_exit<T>(
        &self,
        cancel: &pulse_bus::CancellationToken,
        started: Instant,
        budget: Duration,
        observer: Option<&ProcessObserver>,
        beats: Option<&crossbeam_channel::Receiver<T>>,
    ) -> Waited {
        let beat = observer.and_then(|o| o.beat.as_ref());
        if let Some(beat) = beat {
            beat();
        }
        let done = cancel.done();
        let never: crossbeam_channel::Receiver<T> = crossbeam_channel::never();
        let mut beats = beats;
        loop {
            let elapsed = started.elapsed();
            if cancel.is_cancelled() || (budget > Duration::ZERO && elapsed >= budget) {
                return Waited::Interrupted;
            }
            let nap = if budget > Duration::ZERO {
                WAIT_TICK.min(budget - elapsed)
            } else {
                WAIT_TICK
            };
            crossbeam_channel::select! {
                recv(self.exit) -> status => {
                    return Waited::Exited(status.unwrap_or_else(|_| {
                        Err(std::io::Error::other("the wait thread went away"))
                    }));
                }
                recv(done) -> _ => return Waited::Interrupted,
                recv(beats.unwrap_or(&never)) -> tick => match tick {
                    Ok(_) => {
                        if let Some(beat) = beat {
                            beat();
                        }
                    }
                    // The ticker is gone; nothing more to wake up for there.
                    Err(_) => beats = None,
                },
                default(nap) => {}
            }
        }
    }

    /// Ends the child's process group, so `cmd &`, nohup children and servers
    /// go with the command; the parked thread reaps the child itself. Returns
    /// the members that had to be signalled, the child excluded.
    fn end_group(&self, grace: Duration) -> Vec<KilledProc> {
        ProcessGroup::with_grace(self.pid, grace)
            .terminate()
            .into_iter()
            .filter(|k| k.pid != self.pid)
            .collect()
    }

    /// The child's status once the thread has it; bounded, since a child
    /// even SIGKILL could not end is not worth waiting for.
    fn status(&self, wait: Duration) -> Option<std::io::Result<std::process::ExitStatus>> {
        self.exit.recv_timeout(wait).ok()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Outcome {
    pub(crate) timeout: bool,
    pub(crate) exit_code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

#[derive(Clone, Default)]
pub(crate) struct ProcessObserver {
    pub(crate) beat: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(crate) stop: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl std::fmt::Debug for ProcessObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessObserver").finish_non_exhaustive()
    }
}

/// Go: func runGroup(ctx context.Context, argv []string, dir string, env []string, stdin []byte, budget time.Duration) (outcome, error)
/// Runs argv in a child process group; the fill keeps the Go shape with `std::process::Command`, `pre_exec`/process groups and kill-on-cancel via `nix::sys::signal::killpg`.
pub(crate) fn run_group(
    cancel: &pulse_bus::CancellationToken,
    argv: &[String],
    dir: &str,
    env: &[String],
    stdin: &[u8],
    budget: Duration,
) -> Result<Outcome> {
    run_group_observed(cancel, argv, dir, env, stdin, budget, None)
}

/// Go: func runGroupObserved(ctx context.Context, argv []string, dir string, env []string, stdin []byte, budget time.Duration, observer *processObserver) (outcome, error)
/// Observed process execution shares the same process-group cancellation path and heartbeat callbacks.
pub(crate) fn run_group_observed(
    cancel: &pulse_bus::CancellationToken,
    argv: &[String],
    dir: &str,
    env: &[String],
    stdin: &[u8],
    budget: Duration,
    observer: Option<&ProcessObserver>,
) -> Result<Outcome> {
    if argv.is_empty() {
        return Err(anyhow::anyhow!("empty command"));
    }
    if cancel.is_cancelled() {
        return Ok(finish(None, "", "", true));
    }
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    if !dir.is_empty() {
        cmd.current_dir(dir);
    }
    cmd.env_clear();
    for kv in env {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let input = stdin.to_vec();
    // The child leads its own process group (Go: Setpgid) so the whole tree
    // it spawns ends with it, at the nice level the daemon set for its jobs.
    own_process_group(&mut cmd);
    crate::daemon_load::nice_child(&mut cmd);
    let mut child = cmd.spawn()?;
    if let Some(mut sin) = child.stdin.take() {
        let _ = std::thread::spawn(move || {
            use std::io::Write;
            let _ = sin.write_all(&input);
        });
    }
    let (out_buf, out_done) = drain(child.stdout.take().expect("stdout piped"));
    let (err_buf, err_done) = drain(child.stderr.take().expect("stderr piped"));

    let started = Instant::now();
    let (beats, stop_ticks) = observer
        .map(|_| {
            (crate::heartbeat::HEARTBEAT_TICKS
                .read()
                .expect("heartbeat ticks lock"))(pulse_board::HEARTBEAT_INTERVAL)
        })
        .unzip();
    let reaper = Reaper::start(child);
    let waited = reaper.await_exit(cancel, started, budget, observer, beats.as_ref());
    if let Some(stop) = observer.and_then(|o| o.stop.as_ref()) {
        stop();
    }
    if let Some(stop) = stop_ticks {
        stop();
    }
    // Whatever the command left behind in its group (`cmd &`, nohup
    // children, servers) ends with the command.
    reaper.end_group(*KILL_AFTER.read().unwrap());
    let deadline = Instant::now() + OUTPUT_DRAIN;
    let (code, timed_out) = match waited {
        Waited::Exited(Err(e)) => return Err(e.into()),
        Waited::Exited(Ok(status)) => (exit_code(status), false),
        Waited::Interrupted => {
            let _ = reaper.status(deadline.saturating_duration_since(Instant::now()));
            (None, true)
        }
    };
    let stdout = collect(&out_buf, &out_done, deadline);
    let stderr = collect(&err_buf, &err_done, deadline);
    Ok(finish(code, &stdout, &stderr, timed_out))
}

/// The exit code a status stands for: the code, or 128 plus the signal.
fn exit_code(status: std::process::ExitStatus) -> Option<i32> {
    status.code().or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            status.signal().map(|s| 128 + s)
        }
        #[cfg(not(unix))]
        {
            None
        }
    })
}

/// Go: func waitCommand(ctx context.Context, cmd *exec.Cmd, budget, grace time.Duration, observer *processObserver) (error, bool, bool)
pub(crate) fn wait_command(
    cancel: &pulse_bus::CancellationToken,
    cmd: &mut Command,
    budget: Duration,
    grace: Duration,
    observer: Option<&ProcessObserver>,
) -> (Option<anyhow::Error>, bool, bool) {
    if cancel.is_cancelled() {
        return (Some(anyhow::anyhow!("cancelled")), true, false);
    }
    let start = Instant::now();
    own_process_group(cmd);
    crate::daemon_load::nice_child(cmd);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (Some(e.into()), false, false),
    };
    let (beats, stop_ticks) = observer
        .map(|_| {
            (crate::heartbeat::HEARTBEAT_TICKS
                .read()
                .expect("heartbeat ticks lock"))(pulse_board::HEARTBEAT_INTERVAL)
        })
        .unzip();
    let reaper = Reaper::start(child);
    let waited = reaper.await_exit(cancel, start, budget, observer, beats.as_ref());
    if let Some(stop) = observer.and_then(|o| o.stop.as_ref()) {
        stop();
    }
    if let Some(stop) = stop_ticks {
        stop();
    }
    reaper.end_group(grace);
    match waited {
        Waited::Exited(Err(e)) => (Some(e.into()), false, true),
        Waited::Exited(Ok(_)) => (None, false, true),
        Waited::Interrupted => {
            let _ = reaper.status(OUTPUT_DRAIN);
            (None, true, true)
        }
    }
}
/// Go: func killGroup(pid int, done <-chan error)
pub(crate) fn kill_group(pid: i32) {
    kill_group_after(pid, Duration::from_secs(5));
}
/// Go: func killGroupAfter(pid int, done <-chan error, grace time.Duration)
/// SIGTERM the group, wait up to `grace` for it to empty, then SIGKILL it.
pub(crate) fn kill_group_after(pid: i32, grace: Duration) {
    ProcessGroup::with_grace(pid, grace).terminate();
}
/// Kills what still works under a job directory and writes one log line per
/// process; for the moment right before the directory is removed.
pub(crate) fn sweep_job_dir(dir: &str, log: &mut dyn FnMut(String)) -> Vec<KilledProc> {
    let killed = kill_tree_under(Path::new(dir));
    if !killed.is_empty() {
        log(format!(
            "killed {} straggler process(es) under {dir}: {}",
            killed.len(),
            describe_killed(&killed)
        ));
    }
    killed
}
pub(crate) static KILL_AFTER: LazyLock<RwLock<Duration>> =
    LazyLock::new(|| RwLock::new(Duration::from_secs(5)));
/// Go: func finish(err error, stdout, stderr string, timedOut bool) outcome
pub(crate) fn finish(err: Option<i32>, stdout: &str, stderr: &str, timed_out: bool) -> Outcome {
    Outcome {
        timeout: timed_out,
        exit_code: if timed_out { -1 } else { err.unwrap_or(0) },
        stdout: stdout.into(),
        stderr: stderr.into(),
    }
}
/// Go: func envWith(base []string, extra map[string]string) []string
pub(crate) fn env_with(base: &[String], extra: &BTreeMap<String, String>) -> Vec<String> {
    if extra.is_empty() {
        return base.to_vec();
    }
    let mut out = Vec::new();
    for kv in base {
        let k = kv.split_once('=').map(|x| x.0).unwrap_or(kv);
        if !extra.contains_key(k) {
            out.push(kv.clone())
        }
    }
    for (k, v) in extra {
        out.push(format!("{k}={v}"));
    }
    out
}
/// Go: func indexByte(s string, c byte) int
pub(crate) fn index_byte(s: &str, c: u8) -> i32 {
    s.as_bytes()
        .iter()
        .position(|b| *b == c)
        .map(|i| i as i32)
        .unwrap_or(-1)
}
/// Go: func sortedKeys(m map[string]string) []string
pub(crate) fn sorted_keys(m: &BTreeMap<String, String>) -> Vec<String> {
    m.keys().cloned().collect()
}
/// Go: func sortStrings(a []string)
pub(crate) fn sort_strings(a: &mut [String]) {
    a.sort();
}
/// Go: func environ() []string
pub(crate) fn environ() -> Vec<String> {
    let mut env = crate::util::strip_hook_git_env(
        &std::env::vars()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>(),
    );
    crate::daemon_load::build_env(&mut env);
    env
}

#[cfg(test)]
mod process_tree_tests {
    use super::*;

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    fn env() -> Vec<String> {
        vec!["PATH=/usr/bin:/bin".into()]
    }

    fn bg_pid(dir: &Path) -> i32 {
        std::fs::read_to_string(dir.join("bg.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    #[test]
    fn run_group_ends_the_background_children_of_a_finished_command() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_group(
            &pulse_bus::CancellationToken::new(),
            &sh("sleep 300 & echo $! > bg.pid; echo done"),
            &dir.path().display().to_string(),
            &env(),
            b"",
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(out.exit_code, 0, "{out:?}");
        assert!(!out.timeout);
        assert_eq!(out.stdout, "done\n");
        let bg = bg_pid(dir.path());
        assert!(!alive(bg), "sleep 300 (pid {bg}) outlived the command");
    }

    #[test]
    fn run_group_returns_although_a_background_child_held_the_pipes() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let out = run_group(
            &pulse_bus::CancellationToken::new(),
            &sh("sleep 300 & echo held"),
            &dir.path().display().to_string(),
            &env(),
            b"",
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "took {:?}",
            start.elapsed()
        );
        assert_eq!(out.stdout, "held\n");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn run_group_on_timeout_kills_the_whole_group() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let out = run_group(
            &pulse_bus::CancellationToken::new(),
            &sh("sleep 300 & echo $! > bg.pid; sleep 300"),
            &dir.path().display().to_string(),
            &env(),
            b"",
            Duration::from_millis(200),
        )
        .unwrap();
        assert!(out.timeout);
        assert_eq!(out.exit_code, -1);
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "took {:?}",
            start.elapsed()
        );
        let bg = bg_pid(dir.path());
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(bg) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !alive(bg),
            "background sleep (pid {bg}) survived the timeout"
        );
    }

    #[test]
    fn wait_command_ends_the_group_after_a_normal_exit() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("sleep 300 & echo $! > bg.pid")
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let (err, timed_out, _) = wait_command(
            &pulse_bus::CancellationToken::new(),
            &mut cmd,
            Duration::from_secs(10),
            Duration::from_millis(500),
            None,
        );
        assert!(err.is_none());
        assert!(!timed_out);
        let bg = bg_pid(dir.path());
        assert!(!alive(bg), "sleep 300 (pid {bg}) outlived wait_command");
    }

    #[test]
    fn sweep_job_dir_logs_what_it_killed_and_spares_the_rest() {
        let job = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let spawn = |dir: &Path| {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c")
                .arg("sleep 300")
                .current_dir(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            own_process_group(&mut cmd);
            cmd.spawn().unwrap()
        };
        let mut inside = spawn(job.path());
        let mut bystander = spawn(other.path());
        let inside_pid = inside.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !processes_under(job.path())
            .iter()
            .any(|p| p.pid == inside_pid)
        {
            assert!(
                Instant::now() < deadline,
                "process never showed under the job dir"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut lines = Vec::new();
        let killed = sweep_job_dir(&job.path().display().to_string(), &mut |l| lines.push(l));
        assert!(killed.iter().any(|k| k.pid == inside_pid), "{killed:?}");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("killed 1 straggler process(es) under"),
            "{lines:?}"
        );
        assert!(lines[0].contains(&format!("pid {inside_pid}")), "{lines:?}");
        assert!(!inside.wait().unwrap().success());
        std::thread::sleep(Duration::from_millis(100));
        assert!(bystander.try_wait().unwrap().is_none(), "bystander died");
        ProcessGroup::with_grace(bystander.id() as i32, Duration::from_secs(2)).terminate_reaping(
            || {
                let _ = bystander.try_wait();
            },
        );
        let _ = bystander.wait();
        let mut quiet = Vec::new();
        assert!(
            sweep_job_dir(&job.path().display().to_string(), &mut |l| quiet.push(l)).is_empty()
        );
        assert!(quiet.is_empty());
    }
}
