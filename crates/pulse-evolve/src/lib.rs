//! PULSE: the self-improvement loop: fixtures, measure, archive, cycle, reflect, fleet, usage.
//!
//! `process` folds the Unix and non-Unix process handling behind #[cfg]. Every module
//! re-exports its public items here so callers write `pulse_evolve::foo`.

pub mod api;
pub mod archive;
pub mod baseline_cache;
pub mod card;
pub mod card_job;
pub mod clones;
pub mod commands;
pub mod cycle;
pub mod derive;
pub mod dispute;
pub mod draft;
pub mod failed_gate;
pub mod feedback;
pub mod finalization; // crate-private items only; not re-exported
pub mod fixtures;
pub mod flat_gate;
pub mod fleet;
pub mod format; // crate-private items only; not re-exported
pub(crate) mod gopath;
pub mod heal;
pub mod heldout_auto;
pub mod lineage;
pub mod measure; // crate-private items only; not re-exported
pub mod metric;
pub mod outcome;
pub mod plan;
pub mod private;
pub mod process; // crate-private items only; not re-exported
pub mod readiness;
pub mod readonly; // crate-private items only; not re-exported
pub mod reflect;
pub mod register;
pub mod report;
pub mod score; // crate-private items only; not re-exported
pub mod starter;
pub mod status;
pub mod usage;

pub use api::*;
pub use archive::*;
pub use baseline_cache::*;
pub use card::*;
pub use card_job::*;
pub use clones::*;
pub use commands::*;
pub use cycle::*;
pub use derive::*;
pub use dispute::*;
pub use draft::*;
pub use failed_gate::*;
pub use feedback::*;
pub use fixtures::*;
pub use flat_gate::*;
pub use fleet::*;
pub use heal::*;
pub use heldout_auto::*;
pub use lineage::*;
pub use metric::*;
pub use outcome::*;
pub use plan::*;
pub use private::*;
pub use readiness::*;
pub use reflect::*;
pub use register::*;
pub use report::*;
pub use starter::*;
pub use status::*;
pub use usage::*;

#[cfg(test)]
pub(crate) mod test_helpers;
#[cfg(test)]
pub(crate) mod test_helpers_cycle;
#[cfg(test)]
pub(crate) mod test_helpers_e2;
#[cfg(test)]
pub(crate) mod test_helpers_measure;
#[cfg(test)]
pub(crate) mod test_helpers_plan;

/// Start-up wiring that is not command registration: installs the board's
/// project-created hook so a project's starter records are seeded when the
/// board registers it (`pulse_board::set_on_project_created`). The binary
/// calls this once at start-up, after [`register`]. `Measure`/`Archive` need
/// no wiring: the statics in [`api`] default to the real implementations.
pub fn init() {
    starter::init();
}
