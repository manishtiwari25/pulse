//! In-process event bus, store tailing, and agent-facing event redaction.

mod bus;
mod redact;
mod store;

pub use bus::{Bus, Event, Filter, glob, new, reset_default};
pub use bus::{default, detail_field, install_store_observer};
pub use redact::{PRIVATE_SETTINGS, PRIVATE_VALUE, private_needles, redact, redactor};
pub use store::{
    CancellationToken, DEFAULT_INTERVAL, bridge, bridge_on, detail_text, emit, emit_on, last_id,
    latest, publish_after, since, tail,
};

#[cfg(test)]
pub(crate) fn test_global_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .expect("pulse-bus test lock poisoned")
}
