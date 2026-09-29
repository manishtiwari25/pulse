//! Store hooks and harness-side hooks for PULSE.

pub mod commands;
pub mod hooks;
pub mod install;

pub use commands::*;
pub use hooks::*;
pub use install::*;
#[cfg(test)]
pub(crate) mod test_helpers {
    pub(crate) fn serial() -> impl Drop {
        pulse_testutil::env_lock()
    }
}
