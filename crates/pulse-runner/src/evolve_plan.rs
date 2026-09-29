#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/evolve_plan.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) static EVOLVE_PLAN_REASONS: LazyLock<RwLock<BTreeMap<String, String>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

/// Go: func resetEvolvePlanReasons()
pub(crate) fn reset_evolve_plan_reasons() {
    todo!("runner::evolve_plan::reset_evolve_plan_reasons")
}

/// Go: func rememberEvolveReason(slug, reason string) bool
pub(crate) fn remember_evolve_reason(slug: String, reason: String) -> bool {
    todo!("runner::evolve_plan::remember_evolve_reason")
}

/// Go: func recordEvolveReason(slug, reason string)
pub(crate) fn record_evolve_reason(slug: String, reason: String) {
    todo!("runner::evolve_plan::record_evolve_reason")
}

/// Go: func tendEvolve(s *store.Store, now, actor string, slugs []string) []string
pub(crate) fn tend_evolve(
    s: &pulse_store::Store,
    now: String,
    actor: String,
    slugs: &[String],
) -> Vec<String> {
    todo!("runner::evolve_plan::tend_evolve")
}

/// Go: func hardStopText(text string) bool
pub(crate) fn hard_stop_text(text: String) -> bool {
    todo!("runner::evolve_plan::hard_stop_text")
}
