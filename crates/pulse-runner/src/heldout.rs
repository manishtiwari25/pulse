#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/heldout.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

#[derive(Debug, Clone, Default)]
pub(crate) struct HeldoutGen {
    pub(crate) inflight: bool,
    pub(crate) last_failure_at: String,
    pub(crate) lines: Vec<String>,
}

pub(crate) static HELDOUT_GEN: LazyLock<parking_lot::Mutex<HeldoutGen>> =
    LazyLock::new(|| parking_lot::Mutex::new(HeldoutGen::default()));
pub(crate) type SealedCallFn = dyn Fn(&pulse_bus::CancellationToken, &pulse_store::Store, String) -> anyhow::Result<String>
    + Send
    + Sync;
pub(crate) static SEALED_CALL: LazyLock<RwLock<Box<SealedCallFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(run_sealed_call)));
pub(crate) fn set_sealed_call(f: Box<SealedCallFn>) {
    *SEALED_CALL.write().expect("sealed call lock") = f;
}

pub(crate) const HELDOUT_RETRY_MINUTES: i32 = 60;
pub(crate) const HELDOUT_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3 * 60);

/// Go: func runSealedCall(ctx context.Context, s *store.Store, prompt string) (string, error)
pub(crate) fn run_sealed_call(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    prompt: String,
) -> anyhow::Result<String> {
    todo!("runner::heldout::run_sealed_call")
}

/// Go: func sealedEnv(environ []string) []string
pub(crate) fn sealed_env(environ: &[String]) -> Vec<String> {
    todo!("runner::heldout::sealed_env")
}

/// Go: func tendHeldout(s *store.Store, now string) []string
pub(crate) fn tend_heldout(s: &pulse_store::Store, now: String) -> Vec<String> {
    todo!("runner::heldout::tend_heldout")
}

/// Go: func generateHeldout(s *store.Store, now string)
pub(crate) fn generate_heldout(s: &pulse_store::Store, now: String) {
    todo!("runner::heldout::generate_heldout")
}

/// Go: func heldoutFailure(err error) string
pub(crate) fn heldout_failure(err: anyhow::Error) -> String {
    todo!("runner::heldout::heldout_failure")
}
