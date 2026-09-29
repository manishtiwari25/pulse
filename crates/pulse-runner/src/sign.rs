#![allow(unused_variables, dead_code, unused_imports)]
use anyhow::Result;
use std::io::Write;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;
pub(crate) const SIGNING_IDENTIFIER: &str = pulse_cli::SIGNING_IDENTIFIER;
pub(crate) const SIGNING_REQUIREMENT_PREFIX: &str = "=designated => identifier ";
pub(crate) const SIGNING_TIMEOUT: Duration = Duration::from_secs(120);
pub(crate) static SIGNING_SYSTEM: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new(String::new()));
/// Go: func signingOS() string
pub(crate) fn signing_os() -> String {
    std::env::consts::OS.to_string()
}
/// Go: func signed(path string) bool
pub(crate) fn signed(path: &str) -> bool {
    todo!("runner::sign::signed")
}
/// Go: func sign(path string, running bool) error
pub(crate) fn sign(path: &str, running: bool) -> Result<()> {
    todo!("runner::sign::sign")
}
/// Go: func ensureSigned(out io.Writer, binary string)
pub(crate) fn ensure_signed(out: &mut dyn Write, binary: &str) {
    todo!("runner::sign::ensure_signed")
}
/// Go: func signNewBuild(_ *store.Store, path string) error
pub(crate) fn sign_new_build(s: &pulse_store::Store, path: &str) -> Result<()> {
    todo!("runner::sign::sign_new_build")
}
/// Go: func copyFile(src, dst string) error
pub(crate) fn copy_file(src: &str, dst: &str) -> Result<()> {
    std::fs::copy(src, dst)?;
    Ok(())
}
