//! PULSE: daemon, planner, workers, landing, executors, leads, compute providers, experiments, recovery, hygiene sweep (port of Go package internal/runner).
//!
//! One module per Go file (`internal/runner/<name>.go` -> `<name>.rs`). Every module re-exports its public items
//! here so callers write `pulse_runner::foo` like Go callers wrote `runner.Foo`.

pub mod amend;
pub mod auto_adopt;
pub mod aws;
pub mod azure;
pub mod baseline;
pub mod board;
pub mod brief;
pub mod checkout;
pub mod commands;
pub mod compute;
pub mod daemon;
pub mod daemon_maintenance;
pub mod daemon_watchdog;
pub mod db;
pub mod diskroom;
pub mod env;
pub mod evolve_plan;
pub mod exec;
pub mod executor;
pub mod experiment;
pub mod failure;
pub mod gcp;
pub mod hands;
pub mod heartbeat;
pub mod heldout;
pub mod hygiene;
pub mod idle;
pub mod idle_log;
pub mod jobs;
pub mod keys;
pub mod land;
pub mod land_lock;
pub mod landing_context;
pub mod landing_evidence;
pub mod landing_hold;
pub mod landing_workers;
pub mod lead;
pub mod lessons;
pub mod liveness;
pub mod login_gate;
pub mod multipass;
pub mod onboard;
pub mod pair;
pub mod pair_inspect;
pub mod plan;
pub mod preflight;
pub mod proc;
pub mod provider;
pub mod recipe;
pub mod recover;
pub mod recovery_api;
pub mod research;
pub mod retire;
pub mod run;
pub mod schedule;
pub mod selfupdate;
pub mod service;
pub mod sign;
pub mod steps_mirror;
pub mod sweep;
pub mod tally;
pub mod tidy;
pub mod tmpspace;
pub mod token;
pub mod util;
pub mod verify_lock;
pub mod verify_memo;

pub(crate) use board::*;
pub use commands::*;
pub use compute::*;
pub use daemon::*;
pub use executor::*;
pub use keys::*;
pub use recipe::*;
pub use recovery_api::*;
pub use service::*;

#[cfg(test)]
pub(crate) mod test_helpers;
#[cfg(test)]
pub(crate) mod test_helpers_f1a;
#[cfg(test)]
pub(crate) mod test_helpers_f1b;
#[cfg(test)]
pub(crate) mod test_helpers_f2;
#[cfg(test)]
pub(crate) mod test_helpers_f3;
#[cfg(test)]
pub(crate) mod test_helpers_f3b;
#[cfg(test)]
pub(crate) mod test_helpers_f4;
#[cfg(test)]
pub(crate) mod test_helpers_f5;
#[cfg(test)]
pub(crate) mod test_helpers_f6;
#[cfg(test)]
pub(crate) mod test_helpers_f7;
#[cfg(test)]
pub(crate) mod test_helpers_landing;
#[cfg(test)]
pub(crate) mod test_helpers_login_reuse;
#[cfg(test)]
pub(crate) mod test_helpers_selfupdate;
#[cfg(test)]
pub(crate) mod test_helpers_token;
