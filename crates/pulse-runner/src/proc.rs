#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

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
    let mut child = match unsafe {
        cmd.pre_exec(|| {
            // Put the child in its own process group before exec, matching Go Setpgid.
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
        cmd.spawn()
    } {
        Ok(c) => c,
        Err(e) => return Err(e.into()),
    };
    if let Some(mut sin) = child.stdin.take() {
        let _ = std::thread::spawn(move || {
            use std::io::Write;
            let _ = sin.write_all(&input);
        });
    }
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let out_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let started = Instant::now();
    let mut observed = false;
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = out_thread.join().unwrap_or_default();
            let stderr = err_thread.join().unwrap_or_default();
            let code = status.code().or_else(|| {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    status.signal().map(|s| 128 + s)
                }
                #[cfg(not(unix))]
                {
                    None
                }
            });
            return Ok(finish(code, &stdout, &stderr, false));
        }
        if cancel.is_cancelled() || (budget > Duration::ZERO && started.elapsed() >= budget) {
            if let Some(o) = observer {
                if let Some(stop) = &o.stop {
                    stop();
                }
            }
            kill_group_after(child.id() as i32, *KILL_AFTER.read().unwrap());
            let _ = child.wait();
            let stdout = out_thread.join().unwrap_or_default();
            let stderr = err_thread.join().unwrap_or_default();
            return Ok(finish(None, &stdout, &stderr, true));
        }
        if !observed {
            if let Some(o) = observer {
                if let Some(beat) = &o.beat {
                    beat();
                }
            }
            observed = true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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
    let spawn = unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
        cmd.spawn()
    };
    let mut child = match spawn {
        Ok(c) => c,
        Err(e) => return (Some(e.into()), false, false),
    };
    let mut observed = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return (None, false, true),
            Ok(None) => {}
            Err(e) => return (Some(e.into()), false, true),
        }
        if cancel.is_cancelled() || (budget > Duration::ZERO && start.elapsed() >= budget) {
            if let Some(o) = observer {
                if let Some(stop) = &o.stop {
                    stop();
                }
            }
            kill_group_after(child.id() as i32, grace);
            let _ = child.wait();
            return (None, true, true);
        }
        if !observed {
            if let Some(o) = observer {
                if let Some(beat) = &o.beat {
                    beat();
                }
            }
            observed = true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
/// Go: func killGroup(pid int, done <-chan error)
pub(crate) fn kill_group(pid: i32) {
    kill_group_after(pid, Duration::from_secs(5));
}
/// Go: func killGroupAfter(pid int, done <-chan error, grace time.Duration)
pub(crate) fn kill_group_after(pid: i32, grace: Duration) {
    let _ = nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    );
    std::thread::sleep(grace);
    let _ = nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    );
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
    crate::util::strip_hook_git_env(
        &std::env::vars()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>(),
    )
}
