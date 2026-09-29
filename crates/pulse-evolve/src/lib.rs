//! PULSE: the self-improvement loop: fixtures, measure, archive, cycle, reflect, fleet, usage (port of Go package internal/evolve).
//!
//! One module per Go file. `process` folds process_unix.go / process_other.go behind #[cfg]. Every module
//! re-exports its public items here so callers write `pulse_evolve::foo` like Go callers wrote `evolve.Foo`.

pub mod api;
pub mod archive;
pub mod card;
pub mod card_job;
pub mod clones;
pub mod commands;
pub mod cycle;
pub mod doc;
pub mod draft;
pub mod failed_gate;
pub mod feedback;
pub mod finalization;
pub mod fixtures;
pub mod fleet;
pub mod format;
pub mod heldout_auto;
pub mod measure;
pub mod plan;
pub mod private;
pub mod process;
pub mod readiness;
pub mod readonly;
pub mod reflect;
pub mod register;
pub mod report;
pub mod score;
pub mod starter;
pub mod status;
pub mod usage;

pub use api::*;
pub use archive::*;
pub use card::*;
pub use card_job::*;
pub use clones::*;
pub use commands::*;
pub use cycle::*;
pub use doc::*;
pub use draft::*;
pub use failed_gate::*;
pub use feedback::*;
pub use finalization::*;
pub use fixtures::*;
pub use fleet::*;
pub use format::*;
pub use heldout_auto::*;
pub use measure::*;
pub use plan::*;
pub use private::*;
pub use process::*;
pub use readiness::*;
pub use readonly::*;
pub use reflect::*;
pub use register::*;
pub use report::*;
pub use score::*;
pub use starter::*;
pub use status::*;
pub use usage::*;

#[cfg(test)]
pub(crate) mod test_helpers;
