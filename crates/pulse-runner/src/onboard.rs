#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const DECISION_VALID_DAYS: i32 = 7;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct DecisionHit {
    pub post_id: String,
    pub answer: String,
    pub decided_at: String,
    pub valid: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct OnboardOpts {
    pub name: String,
    pub remote: String,
    pub verify_cmd: String,
    pub actor: String,
    pub brain: bool,
}

/// Go: func authorizingDecision(q querier, marker, now string) *decisionHit
pub(crate) fn authorizing_decision(
    q: &dyn crate::util::Querier,
    marker: String,
    now: String,
) -> Option<Box<DecisionHit>> {
    todo!("runner::onboard::authorizing_decision")
}

/// Go: func agentGate(q querier, marker, actor, now string) (string, error)
pub(crate) fn agent_gate(
    q: &dyn crate::util::Querier,
    marker: String,
    actor: String,
    now: String,
) -> anyhow::Result<String> {
    todo!("runner::onboard::agent_gate")
}

/// Go: func daemonStatus(s *store.Store, now string) string
pub(crate) fn daemon_status(s: &pulse_store::Store, now: String) -> String {
    todo!("runner::onboard::daemon_status")
}

/// Go: func onboardProject(s *store.Store, path, now string, o onboardOpts) ([]string, error)
pub(crate) fn onboard_project(
    s: &pulse_store::Store,
    path: String,
    now: String,
    o: OnboardOpts,
) -> anyhow::Result<Vec<String>> {
    todo!("runner::onboard::onboard_project")
}

/// Go: func starterCount(s *store.Store, slug string) int
pub(crate) fn starter_count(s: &pulse_store::Store, slug: String) -> i32 {
    todo!("runner::onboard::starter_count")
}

/// Go: func loopRecordLine(s *store.Store, slug, name string) string
pub(crate) fn loop_record_line(s: &pulse_store::Store, slug: String, name: String) -> String {
    todo!("runner::onboard::loop_record_line")
}

/// Go: func heldoutStep(s *store.Store) string
pub(crate) fn heldout_step(s: &pulse_store::Store) -> String {
    todo!("runner::onboard::heldout_step")
}
