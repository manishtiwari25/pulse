//! PULSE: the built-in agent harness: bash-tool loop, limits, deny rules, trajectories, sessions.

pub mod agents;
pub mod background;
pub(crate) mod buildenv;
pub mod chat;
pub mod chat_command;
pub mod commands;
pub mod compact;
pub mod conversation;
pub mod fetch;
pub mod harness;
pub mod history;
pub mod hooks;
pub mod instructions;
pub mod localenv;
pub mod memory;
pub mod permissions;
pub mod readonly;
pub mod rewind;
pub mod search;
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
    chat_command::register();
}
