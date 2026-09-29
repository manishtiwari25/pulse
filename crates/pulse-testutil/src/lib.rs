//! Shared test helpers for PULSE crates.

pub mod httptest;

use std::process::Command;
use std::sync::OnceLock;

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};

static ENV_LOCK: OnceLock<ReentrantMutex<()>> = OnceLock::new();

/// Guard returned by [`env_lock`].
pub type EnvGuard = ReentrantMutexGuard<'static, ()>;

/// Global guard for tests that mutate process environment variables.
///
/// Reentrant so a test that holds the lock may call helpers (such as a
/// `test_store()`) that take it again on the same thread; never poisoned, so
/// one panicking test does not cascade into every later one.
pub fn env_lock() -> EnvGuard {
    ENV_LOCK.get_or_init(|| ReentrantMutex::new(())).lock()
}

/// Environment for test git invocations: hook vars stripped and host config disabled.
pub fn git_env() -> Vec<String> {
    let mut env =
        pulse_gitenv::strip_hook(std::env::vars().map(|(k, v)| format!("{k}={v}")).collect());
    env.push("GIT_CONFIG_NOSYSTEM=1".to_string());
    env.push(format!(
        "GIT_CONFIG_GLOBAL={}",
        if cfg!(windows) { "NUL" } else { "/dev/null" }
    ));
    env
}

/// Runs `git` with the test environment.
pub fn git(args: &[&str]) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args).env_clear();
    for kv in git_env() {
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

#[cfg(test)]
mod tests {
    #[test]
    fn git_env_drops_hook_variables() {
        let env = vec![
            "GIT_DIR=/tmp/repo/.git".to_string(),
            "GIT_SSH_COMMAND=ssh -F none".to_string(),
            "PATH=/bin".to_string(),
        ];
        let got = pulse_gitenv::strip_hook(env);
        assert!(!got.iter().any(|kv| kv.starts_with("GIT_DIR=")));
        assert!(got.iter().any(|kv| kv.starts_with("GIT_SSH_COMMAND=")));
    }
}
