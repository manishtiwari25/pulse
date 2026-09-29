//! Shared test helpers for PULSE crates.

use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Global guard for tests that mutate process environment variables.
pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env test mutex poisoned")
}

/// Runs `git` with hook variables scrubbed, mirroring the tiny Go test helper.
pub fn git(args: &[&str]) -> anyhow::Result<String> {
    let env_pairs = pulse_gitenv::env();
    let mut cmd = Command::new("git");
    cmd.args(args).env_clear();
    for kv in env_pairs {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    let out = cmd.output()?;
    if !out.status.success() {
        anyhow::bail!(
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
