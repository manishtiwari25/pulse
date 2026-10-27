//! Process-wide guards for tests that mutate the environment: the harness
//! runs tests in parallel, and `CARGO_TARGET_DIR` and friends are shared, so
//! a test that sets or clears one holds this lock for its whole body. The
//! parent of this experiment (B-023) calls it from
//! `process::tests::measurement_cargo_target_reuses_the_recipe_target_dir`.
//!
//! It aliases `pulse_testutil`'s guard so a parent's tree (where this module
//! grew its own lock) and this crate's `pulse_testutil::env_lock()` users
//! serialize on the same mutex.

use std::sync::{LazyLock, Mutex, MutexGuard};

static E2_GUARD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Take the process-wide environment lock; hold it for the test body.
pub(crate) fn e2_guard() -> MutexGuard<'static, ()> {
    match E2_GUARD.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    }
}

/// Set `name` to `value` for the current process; call under [`e2_guard`].
pub(crate) fn set_env(name: &str, value: &str) {
    // SAFETY: the caller holds the module's process-wide guard.
    unsafe { std::env::set_var(name, value) };
}

/// Remove `name` from the environment; call under [`e2_guard`].
pub(crate) fn unset_env(name: &str) {
    // SAFETY: the caller holds the module's process-wide guard.
    unsafe { std::env::remove_var(name) };
}
