//! SQLite store foundation for PULSE.

pub mod ddl;
pub mod evolve_ddl;
pub mod gojson;
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

pub use ddl::DDL;
pub use evolve_ddl::EVOLVE_DDL;
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

// Compatibility aliases for the committed partial port and Go names.
pub const RECORDS: &[&str] = RECORDS_DDL;
pub const TASKS: &[&str] = TASKS_DDL;
pub const HOOKS: &[&str] = HOOKS_DDL;
pub const RUNNER: &[&str] = RUNNER_DDL;
pub const EVOLVE: &[&str] = EVOLVE_DDL;
pub const HARNESS: &[&str] = HARNESS_DDL;
