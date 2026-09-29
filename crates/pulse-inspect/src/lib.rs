//! PULSE: status, projects, jobs, agents, events, settings, schema, verify, signers.

pub mod lists;
pub mod recipes;
pub mod register;
pub mod settings;
pub mod status;
pub mod verify;

#[cfg(test)]
pub(crate) mod test_helpers;

pub use lists::*;
pub use recipes::*;
pub use register::*;
pub use settings::*;
pub use status::*;
pub use verify::*;

pub fn register() {
    pulse_provider_copilot::register();
    register::register();
}
