#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/preflight.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

/// Go: func pulseOnPath(env []string) string
pub(crate) fn pulse_on_path(env: &[String]) -> String {
    todo!("runner::preflight::pulse_on_path")
}

/// Go: func preflightLine(found, cli string) string
pub(crate) fn preflight_line(found: String, cli: String) -> String {
    todo!("runner::preflight::preflight_line")
}

/// Go: func preflightPulseCLI(found, cli string) error
pub(crate) fn preflight_pulse_cli(found: String, cli: String) -> anyhow::Result<()> {
    todo!("runner::preflight::preflight_pulse_cli")
}
