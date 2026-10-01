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

/// Go: `t.Setenv(key, value)` - the variable holds `value` while the guard lives and reverts to its
/// previous state (set or unset) when the guard drops. Callers hold [`env_lock`] (or run serially).
#[must_use = "the variable reverts as soon as the guard drops"]
pub struct SetenvGuard {
    key: String,
    prev: Option<std::ffi::OsString>,
}

pub fn setenv(key: &str, value: impl AsRef<std::ffi::OsStr>) -> SetenvGuard {
    let prev = std::env::var_os(key);
    // SAFETY: tests serialize environment mutation behind env_lock()/the crate's serial guard.
    unsafe { std::env::set_var(key, value) };
    SetenvGuard {
        key: key.to_string(),
        prev,
    }
}

/// Go: `t.Setenv` followed by `os.Unsetenv` - unset for the guard's lifetime, restored on drop.
pub fn unsetenv(key: &str) -> SetenvGuard {
    let prev = std::env::var_os(key);
    // SAFETY: as in setenv.
    unsafe { std::env::remove_var(key) };
    SetenvGuard {
        key: key.to_string(),
        prev,
    }
}

impl Drop for SetenvGuard {
    fn drop(&mut self) {
        // SAFETY: as in setenv.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(&self.key, v),
                None => std::env::remove_var(&self.key),
            }
        }
    }
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

/// Clear the runner's `PULSE_*` variables once per test process so a store
/// created inside a PULSE job (where `PULSE_JOB` trips the worker guard and
/// `PULSE_AGENT` changes the caller's group) behaves like the owner's shell.
/// Call it from a crate's `temp_store()` before `Store::open`.
pub fn neutralize_runner_environment() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _guard = env_lock();
        for name in ["PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT"] {
            // SAFETY: done once per process, behind env_lock, before any test
            // in this crate opens a store.
            unsafe { std::env::set_var(name, "") };
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn git_env_drops_hook_variables() {
        let _guard = super::env_lock();
        let set = [
            ("GIT_DIR", "/tmp/x"),
            ("GIT_WORK_TREE", "/tmp/x"),
            ("GIT_INDEX_FILE", "/tmp/x/index"),
            ("GIT_AUTHOR_NAME", "hook"),
            ("PULSE_TESTUTIL_KEEP", "1"),
        ];
        let previous: Vec<(&str, Option<String>)> = set
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        for (k, v) in set {
            unsafe { std::env::set_var(k, v) };
        }

        let env = super::git_env();
        for kv in &env {
            assert!(
                !pulse_gitenv::hook_var(kv),
                "GitEnv leaked hook variable {kv:?}"
            );
        }
        let dev_null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let mut want = std::collections::BTreeMap::from([
            ("PULSE_TESTUTIL_KEEP=1".to_string(), false),
            ("GIT_CONFIG_NOSYSTEM=1".to_string(), false),
            (format!("GIT_CONFIG_GLOBAL={dev_null}"), false),
        ]);
        for kv in &env {
            if let Some(seen) = want.get_mut(kv) {
                *seen = true;
            }
        }
        for (kv, seen) in &want {
            assert!(seen, "GitEnv missing {kv:?}");
        }

        for (k, old) in previous {
            match old {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}
