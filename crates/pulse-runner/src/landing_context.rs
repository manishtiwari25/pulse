#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

/// Go: func landingStop(ctx context.Context, s *store.Store, root string) string
pub(crate) fn landing_stop(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    root: String,
) -> String {
    todo!("runner::landing_context::landing_stop")
}

/// Go: func shWithContext(ctx context.Context, dir, cmd string, budget time.Duration) (rc *int, stdout, stderr string)
pub(crate) fn sh_with_context(
    ctx: &pulse_bus::CancellationToken,
    dir: String,
    cmd: String,
    budget: std::time::Duration,
) -> (Option<Box<i32>>, String, String) {
    todo!("runner::landing_context::sh_with_context")
}
