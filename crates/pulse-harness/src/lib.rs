//! PULSE: the built-in agent harness: bash-tool loop, limits, deny rules, trajectories, sessions.

pub mod commands;
pub mod harness;
pub mod history;
pub mod localenv;
#[cfg(test)]
pub(crate) mod test_helpers;

pub use commands::*;
pub use harness::*;
pub use localenv::*;

pub fn register() {
    commands::register();
}
