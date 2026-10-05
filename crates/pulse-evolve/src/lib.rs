//! PULSE: the self-improvement loop: fixtures, measure, archive, cycle, reflect, fleet, usage (port of Go package internal/evolve).
//!
//! One module per Go file. `process` folds process_unix.go / process_other.go behind #[cfg]. Every module
//! re-exports its public items here so callers write `pulse_evolve::foo` like Go callers wrote `evolve.Foo`.

pub mod api;
pub mod archive;
pub mod baseline_cache;
pub mod card;
pub mod card_job;
pub mod clones;
pub mod commands;
pub mod cycle;
pub mod doc; // crate-private items only; not re-exported
pub mod draft;
pub mod failed_gate;
pub mod feedback;
pub mod finalization; // crate-private items only; not re-exported
pub mod fixtures;
pub mod fleet;
pub mod format; // crate-private items only; not re-exported
pub(crate) mod gopath;
pub mod heldout_auto;
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
pub use draft::*;
pub use failed_gate::*;
pub use feedback::*;
pub use fixtures::*;
pub use fleet::*;
pub use heldout_auto::*;
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
pub(crate) mod test_helpers_e1;
#[cfg(test)]
pub(crate) mod test_helpers_e2;
#[cfg(test)]
pub(crate) mod test_helpers_e3;

/// The Go package's `init()` side effects that are not command registration:
/// evolve seeds a project's starter records when the board registers one
/// (`starter.go: board.OnProjectCreated = seedOnCreate`). The binary calls this
/// once at start-up, after [`register`]. `Measure`/`Archive` need no wiring: the
/// statics in [`api`] default to the real implementations.
pub fn init() {
    starter::init();
}
