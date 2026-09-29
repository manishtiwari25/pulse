#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const CHECKOUT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CheckoutProbes {
    pub(crate) pending: BTreeMap<String, serde_json::Value>,
    pub(crate) noted: BTreeMap<String, String>,
}

pub(crate) static CHECKOUT_HOLD: LazyLock<RwLock<BTreeMap<String, String>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

/// Go: func readDirOnce(path string) error
pub(crate) fn read_dir_once(path: String) -> anyhow::Result<()> {
    todo!("runner::checkout::read_dir_once")
}

/// Go: func macOSHint(path string) string
pub(crate) fn mac_os_hint(path: String) -> String {
    todo!("runner::checkout::mac_os_hint")
}

/// Go: func holdCheckouts(held map[string]string)
pub(crate) fn hold_checkouts(held: BTreeMap<String, String>) -> () {
    todo!("runner::checkout::hold_checkouts")
}

/// Go: func heldCheckouts() map[string]string
pub(crate) fn held_checkouts() -> BTreeMap<String, String> {
    todo!("runner::checkout::held_checkouts")
}

impl CheckoutProbes {
    /// Go: func (p *checkoutProbes) ready(projects []row, timeout time.Duration) (ok []row, held map[string]string, lines []string)
    pub(crate) fn ready(
        &mut self,
        projects: Vec<crate::util::Row>,
        timeout: std::time::Duration,
    ) -> (Vec<crate::util::Row>, BTreeMap<String, String>, Vec<String>) {
        todo!("runner::checkout::ready")
    }
}
