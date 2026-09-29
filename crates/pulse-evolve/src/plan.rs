#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/plan.go.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanOptions {
    pub agent: String,
    pub session: String,
    pub agent_row: Option<pulse_store::Agent>,
    pub require_positive_score: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanResult {
    pub job_id: String,
    pub created: bool,
    pub reason: String,
    pub question: String,
    pub suggestion: String,
    pub card_job: String,
    pub card_created: bool,
}

pub(crate) type ReadPlanPreflight =
    fn(&pulse_store::Store, &str) -> Result<crate::status::LoopStatus>;

/// Go: var readPlanPreflight = loopStatus
pub(crate) static READ_PLAN_PREFLIGHT: LazyLock<RwLock<Option<ReadPlanPreflight>>> =
    LazyLock::new(|| RwLock::new(None));

pub(crate) fn set_read_plan_preflight(f: Option<ReadPlanPreflight>) {
    *READ_PLAN_PREFLIGHT
        .write()
        .expect("read plan preflight poisoned") = f;
}

/// Go: func PlanExperiment(s *store.Store, project string, opts PlanOptions) (PlanResult, error) {
pub fn plan_experiment(
    s: &pulse_store::Store,
    project: &str,
    opts: PlanOptions,
) -> Result<PlanResult> {
    todo!("evolve::plan::plan_experiment")
}

/// Go: func LoopPick(s *store.Store, project string) (suggestionID string, reason string, err error) {
pub fn loop_pick(s: &pulse_store::Store, project: &str) -> Result<(String, String)> {
    todo!("evolve::plan::loop_pick")
}

/// Go: func LoopPickForAgent(s *store.Store, project, agent string) (suggestionID string, reason string, err error) {
pub fn loop_pick_for_agent(
    s: &pulse_store::Store,
    project: &str,
    agent: &str,
) -> Result<(String, String)> {
    todo!("evolve::plan::loop_pick_for_agent")
}

/// Go: func LoopClaims(s *store.Store, project, agent string) (claims map[string]bool, reason string, err error) {
pub fn loop_claims_public(
    s: &pulse_store::Store,
    project: &str,
    agent: &str,
) -> Result<(BTreeMap<String, bool>, String)> {
    todo!("evolve::plan::loop_claims_public")
}

/// Go: func loopClaims(s *store.Store, project, agent string) (claims []string, reason string, err error) {
pub(crate) fn loop_claims(
    s: &pulse_store::Store,
    project: &str,
    agent: &str,
) -> Result<(Vec<String>, String)> {
    todo!("evolve::plan::loop_claims")
}

/// Go: func planExperiment(s *store.Store, project string, opts PlanOptions, seen map[string]bool) (PlanResult, error) {
pub(crate) fn plan_experiment_inner(
    s: &pulse_store::Store,
    project: &str,
    opts: PlanOptions,
    seen: Option<&mut BTreeMap<String, bool>>,
) -> Result<PlanResult> {
    todo!("evolve::plan::plan_experiment_inner")
}

/// Go: func planExperimentLocked(s *store.Store, tx *board.WriteTransaction, project string, opts PlanOptions, seen map[string]bool) (PlanResult, error) {
pub(crate) fn plan_experiment_locked(
    s: &pulse_store::Store,
    tx: &pulse_board::WriteTransaction<'_>,
    project: &str,
    opts: PlanOptions,
    seen: Option<&mut BTreeMap<String, bool>>,
) -> Result<PlanResult> {
    todo!("evolve::plan::plan_experiment_locked")
}

/// Go: func cardJobSuggestion(job *board.Job) string { return strings.TrimPrefix(job.Source, "suggestion:") }
pub(crate) fn card_job_suggestion(job: &pulse_board::Job) -> String {
    todo!("evolve::plan::card_job_suggestion")
}

/// Go: func cardJobReason(job *board.Job) string {
pub(crate) fn card_job_reason(job: &pulse_board::Job) -> String {
    todo!("evolve::plan::card_job_reason")
}

/// Go: func fixAlreadyLanded(s *store.Store, project, file, commit string) (bool, error) {
pub(crate) fn fix_already_landed(
    s: &pulse_store::Store,
    project: &str,
    file: &str,
    commit: &str,
) -> Result<bool> {
    todo!("evolve::plan::fix_already_landed")
}

/// Go: func experimentCarrierReason(job *board.Job) string {
pub(crate) fn experiment_carrier_reason(job: &pulse_board::Job) -> String {
    todo!("evolve::plan::experiment_carrier_reason")
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ExperimentCandidate {
    pub(crate) post: pulse_tally::Post,
    pub(crate) suggestion: pulse_tally::Suggestion,
}

/// Go: func experimentCandidates(s *store.Store, project string, status *LoopStatus, result *tally.Result, agent string, requirePositiveScore bool, seen map[string]bool) ([]experimentCandidate, error) {
pub(crate) fn experiment_candidates(
    s: &pulse_store::Store,
    project: &str,
    status: &crate::status::LoopStatus,
    result: &pulse_tally::TallyResult,
    agent: &str,
    require_positive_score: bool,
    seen: Option<&mut BTreeMap<String, bool>>,
) -> Result<Vec<ExperimentCandidate>> {
    todo!("evolve::plan::experiment_candidates")
}

/// Go: func heldByAnotherJob(s *store.Store, sg tally.Suggestion) (bool, error) {
pub(crate) fn held_by_another_job(
    s: &pulse_store::Store,
    sg: pulse_tally::Suggestion,
) -> Result<bool> {
    todo!("evolve::plan::held_by_another_job")
}

/// Go: func openExperimentForSuggestion(s *store.Store, project, suggestionID string) (*board.Job, error) {
pub(crate) fn open_experiment_for_suggestion(
    s: &pulse_store::Store,
    project: &str,
    suggestion_id: &str,
) -> Result<Option<pulse_board::Job>> {
    todo!("evolve::plan::open_experiment_for_suggestion")
}

/// Go: func inFlightExperiment(s *store.Store, project string, includeLanding bool) (string, bool, error) {
pub(crate) fn in_flight_experiment(
    s: &pulse_store::Store,
    project: &str,
    include_landing: bool,
) -> Result<Option<String>> {
    todo!("evolve::plan::in_flight_experiment")
}

/// Go: func threadHasCard(s *store.Store, project string, p *tally.Post, status *LoopStatus) (bool, error) {
pub(crate) fn thread_has_card(
    s: &pulse_store::Store,
    project: &str,
    p: &pulse_tally::Post,
    status: &crate::status::LoopStatus,
) -> Result<bool> {
    todo!("evolve::plan::thread_has_card")
}

/// Go: func questionOpenOnPost(s *store.Store, project string, p *tally.Post, agent string) bool {
pub(crate) fn question_open_on_post(
    s: &pulse_store::Store,
    project: &str,
    p: &pulse_tally::Post,
    agent: &str,
) -> bool {
    todo!("evolve::plan::question_open_on_post")
}
