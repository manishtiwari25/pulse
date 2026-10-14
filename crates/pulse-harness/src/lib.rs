//! PULSE: the built-in agent harness: bash-tool loop, limits, deny rules, trajectories, sessions.

pub mod chat;
pub mod commands;
pub mod compact;
pub mod fetch;
pub mod harness;
pub mod history;
pub mod hooks;
pub mod localenv;
pub mod readonly;
pub mod skills;
#[cfg(test)]
pub(crate) mod test_helpers;

pub use commands::*;
pub use compact::*;
pub use fetch::*;
pub use harness::*;
pub use hooks::*;
pub use localenv::*;
pub use readonly::{TreeSnapshot, read_only_refusal};
pub use skills::*;

pub fn register() {
    commands::register();
}
