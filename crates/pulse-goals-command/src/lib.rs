//! PULSE: the pulse goal command (port of Go package internal/goals/command).

mod command;

pub use command::register;
#[cfg(test)]
pub(crate) mod test_helpers {
    pub(crate) fn serial() -> impl Drop {
        pulse_testutil::env_lock()
    }
}
