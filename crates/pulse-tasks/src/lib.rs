//! PULSE: owner tasks, messages, compute requests and caps.

pub mod commands;
pub mod compute;
pub mod leads;
pub mod messages;
pub mod schedules;
pub mod suggestions;
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
