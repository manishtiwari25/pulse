//! PULSE: board digest, ballot, proposals, queue, amendments, constitution.

use std::sync::{Arc, PoisonError, RwLock};

pub mod amend;
pub mod cache;
pub mod card_class;
pub mod digest;
pub mod gate;
pub mod plan_context;
pub mod queue;
pub mod records;
pub mod register;
pub mod suggestion_votes;
pub mod tally;
pub mod text;
pub mod thresholds;

pub use amend::*;
pub use card_class::*;
pub use digest::*;
pub use gate::*;
pub use plan_context::*;
pub use queue::*;
pub use records::*;
pub use register::*;
pub use suggestion_votes::*;
pub use tally::*;
pub use text::*;
pub use thresholds::*;

pub type LogHook = Arc<dyn Fn(&str) + Send + Sync>;
static LOG: RwLock<Option<LogHook>> = RwLock::new(None);

pub fn log_hook() -> Option<LogHook> {
    LOG.read().unwrap_or_else(PoisonError::into_inner).clone()
}

pub fn set_log(hook: Option<LogHook>) -> Option<LogHook> {
    std::mem::replace(
        &mut *LOG.write().unwrap_or_else(PoisonError::into_inner),
        hook,
    )
}

pub(crate) fn log_line(line: String) {
    if let Some(log) = log_hook() {
        log(&line);
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod test_helpers {
    pub(crate) fn serial() -> impl Drop {
        pulse_testutil::env_lock()
    }
}
