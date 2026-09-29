//! PULSE: owner tasks, messages, compute requests and caps (port of Go package internal/tasks).

pub mod commands;
pub mod compute;
pub mod messages;
pub mod tasks;
pub mod work;

pub use commands::*;
pub use compute::*;
pub use messages::*;
pub use tasks::*;
pub use work::*;

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod test_helpers {
    pub(crate) fn serial() -> impl Drop {
        pulse_testutil::env_lock()
    }
}
