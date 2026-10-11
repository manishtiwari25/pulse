//! PULSE: credential setup: upstream remote and probe.

pub mod setup;

pub use setup::*;

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

    pub(crate) fn serial() -> MutexGuard<'static, ()> {
        SERIAL.get_or_init(|| Mutex::new(())).lock().unwrap()
    }
}
