#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::process::Command;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

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
    todo!("runner::proc::run_group_observed")
}

/// Go: func waitCommand(ctx context.Context, cmd *exec.Cmd, budget, grace time.Duration, observer *processObserver) (error, bool, bool)
pub(crate) fn wait_command(
    cancel: &pulse_bus::CancellationToken,
    cmd: &mut Command,
    budget: Duration,
    grace: Duration,
    observer: Option<&ProcessObserver>,
) -> (Option<anyhow::Error>, bool, bool) {
    todo!("runner::proc::wait_command")
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
