//! SQLite store foundation for PULSE.

pub mod ctx;
pub mod ddl;
pub mod evolve_ddl;
pub mod filepath;
pub mod forget;
pub mod gojson;
pub use erased_serde;
pub mod harness;
pub mod harness_ddl;
pub mod hooks_ddl;
pub mod posts;
pub mod projects;
pub mod pyjson;
pub mod records;
pub mod records_ddl;
pub mod runner_ddl;
pub mod settings;
pub mod store;
pub mod tasks_ddl;

pub use ctx::{CancellationToken, ContextError};
pub use ddl::DDL;
pub use evolve_ddl::EVOLVE_DDL;
pub use forget::*;
pub use harness::*;
pub use harness_ddl::{HARNESS_COLUMNS, HARNESS_DDL};
pub use hooks_ddl::HOOKS_DDL;
pub use posts::*;
pub use projects::*;
pub use pyjson::*;
pub use records::*;
pub use records_ddl::{RECORDS_DDL, RECORDS_VERSION};
pub use runner_ddl::{RUNNER_COLUMNS, RUNNER_DDL};
pub use settings::*;
pub use store::*;
pub use tasks_ddl::TASKS_DDL;

#[cfg(test)]
pub(crate) mod test_helpers {
    pub(crate) fn serial() -> impl Drop {
        pulse_testutil::env_lock()
    }
}
