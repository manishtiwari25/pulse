#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

/// Go: func landingHeld(state string) bool
pub(crate) fn landing_held(state: String) -> bool {
    todo!("runner::landing_hold::landing_held")
}

/// Go: func tendLandings(s *store.Store, projects []row, actor, now string, prev map[string]string) (lines []string, cur map[string]string)
pub(crate) fn tend_landings(
    s: &pulse_store::Store,
    projects: Vec<crate::util::Row>,
    actor: String,
    now: String,
    prev: BTreeMap<String, String>,
) -> (Vec<String>, BTreeMap<String, String>) {
    todo!("runner::landing_hold::tend_landings")
}

/// Go: func tendLandingsContext(ctx context.Context, s *store.Store, projects []row, actor, now string, prev map[string]string) (lines []string, cur map[string]string)
pub(crate) fn tend_landings_context(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    projects: Vec<crate::util::Row>,
    actor: String,
    now: String,
    prev: BTreeMap<String, String>,
) -> (Vec<String>, BTreeMap<String, String>) {
    todo!("runner::landing_hold::tend_landings_context")
}

/// Go: func recordLandingHolds(s *store.Store, holds map[string]string, now string)
pub(crate) fn record_landing_holds(
    s: &pulse_store::Store,
    holds: BTreeMap<String, String>,
    now: String,
) -> () {
    todo!("runner::landing_hold::record_landing_holds")
}

/// Go: func pushOn(s *store.Store) bool
pub(crate) fn push_on(s: &pulse_store::Store) -> bool {
    todo!("runner::landing_hold::push_on")
}

/// Go: func pushOwed(s *store.Store, slug string) bool
pub(crate) fn push_owed(s: &pulse_store::Store, slug: String) -> bool {
    todo!("runner::landing_hold::push_owed")
}

/// Go: func pushLanded(s *store.Store, project row, actor, now string, landedNow bool) (line, note string)
pub(crate) fn push_landed(
    s: &pulse_store::Store,
    project: crate::util::Row,
    actor: String,
    now: String,
    landed_now: bool,
) -> (String, String) {
    todo!("runner::landing_hold::push_landed")
}

/// Go: func pushCredentialEnv(s *store.Store, root string) (env []string, cleanup func(), why string)
pub(crate) fn push_credential_env(
    s: &pulse_store::Store,
    root: String,
) -> (
    Vec<String>,
    Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
    String,
) {
    todo!("runner::landing_hold::push_credential_env")
}

/// Go: func pushEnv(root string) []string
pub(crate) fn push_env(root: String) -> Vec<String> {
    todo!("runner::landing_hold::push_env")
}
