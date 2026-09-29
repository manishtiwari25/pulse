#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const SELF_UPDATE_AREA: &str = "runner";
pub(crate) const SELF_UPDATE_THREAD: &str = "release";
pub(crate) const SELF_UPDATE_EVENT: &str = "daemon.self_update";
pub(crate) const SELF_UPDATE_BUILD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub(crate) const RESTORED_MARKER: &str = ".restored";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SelfUpdateState {
    pub(crate) inflight: bool,
    pub(crate) lines: Vec<String>,
    pub(crate) last_failure_at: String,
    pub(crate) last_skip: String,
    pub(crate) installed: String,
}

pub(crate) static SELF_UPDATE_STATE: LazyLock<RwLock<SelfUpdateState>> =
    LazyLock::new(|| RwLock::new(SelfUpdateState::default()));

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SelfUpdateEnv {
    pub binary: String,
    pub go_cmd: String,
    pub timeout: std::time::Duration,
    pub revision: String,
    pub actor: String,
    pub inflight: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SelfUpdateOutcome {
    pub(crate) line: String,
    pub(crate) installed: bool,
}

/// Go: func shortRev(rev string) string
pub(crate) fn short_rev(rev: String) -> String {
    todo!("runner::selfupdate::short_rev")
}

/// Go: func tendBinary(s *store.Store, now string, env selfUpdateEnv) []string
pub(crate) fn tend_binary(s: &pulse_store::Store, now: String, env: SelfUpdateEnv) -> Vec<String> {
    todo!("runner::selfupdate::tend_binary")
}

/// Go: func buildAndInstall(s *store.Store, repo, rev string, env selfUpdateEnv) selfUpdateOutcome
pub(crate) fn build_and_install(
    s: &pulse_store::Store,
    repo: String,
    rev: String,
    env: SelfUpdateEnv,
) -> SelfUpdateOutcome {
    todo!("runner::selfupdate::build_and_install")
}

/// Go: func selfUpdateFailure(s *store.Store, rev string, env selfUpdateEnv, stage, output string, err error) selfUpdateOutcome
pub(crate) fn self_update_failure(
    s: &pulse_store::Store,
    rev: String,
    env: SelfUpdateEnv,
    stage: String,
    output: String,
    err: anyhow::Error,
) -> SelfUpdateOutcome {
    todo!("runner::selfupdate::self_update_failure")
}

/// Go: func selfUpdateFinding(s *store.Store, actor, summary, body string) error
pub(crate) fn self_update_finding(
    s: &pulse_store::Store,
    actor: String,
    summary: String,
    body: String,
) -> anyhow::Result<()> {
    todo!("runner::selfupdate::self_update_finding")
}

/// Go: func runIn(dir string, timeout time.Duration, name string, args ...string) (string, error)
pub(crate) fn run_in(
    dir: String,
    timeout: std::time::Duration,
    name: String,
    args: &[String],
) -> anyhow::Result<String> {
    todo!("runner::selfupdate::run_in")
}

/// Go: func gitReason(results ...gitResult) string
pub(crate) fn git_reason(results: &[serde_json::Value]) -> String {
    todo!("runner::selfupdate::git_reason")
}

/// Go: func firstNonEmpty(values ...string) string
pub(crate) fn first_non_empty(values: &[String]) -> String {
    todo!("runner::selfupdate::first_non_empty")
}

/// Go: func checkRestored(s *store.Store, env selfUpdateEnv) []string
pub(crate) fn check_restored(s: &pulse_store::Store, env: SelfUpdateEnv) -> Vec<String> {
    todo!("runner::selfupdate::check_restored")
}

/// Go: func jsonField(text, key string) string
pub(crate) fn json_field(text: String, key: String) -> String {
    todo!("runner::selfupdate::json_field")
}
