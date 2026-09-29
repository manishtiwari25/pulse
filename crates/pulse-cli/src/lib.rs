//! Command registry, context, help, tables, flags and version helpers shared by PULSE commands.

pub mod cli;
pub mod flags;
pub mod help;
pub mod signed;
pub mod table;
pub mod version;

pub use cli::*;
pub use flags::{Flag, FlagError, FlagSet, FlagValue, format_duration, parse_duration};
pub use help::{GLOBAL_USAGE, TITLE, edit_distance, help, help_command, suggest};
pub use signed::{SIGNING_IDENTIFIER, code_signed, executable};
pub use table::{Style, table};
pub use version::{build_version, revision, version};
#[cfg(test)]
pub(crate) mod test_helpers {
    pub(crate) fn serial() -> impl Drop {
        pulse_testutil::env_lock()
    }
}
