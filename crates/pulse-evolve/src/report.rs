#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/report.go.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub project: String,
    pub days: i64,
    pub since: String,
    pub now: String,
    pub status: Option<crate::status::LoopStatus>,
    pub posts: Vec<KindCount>,
    pub suggestions: Funnel,
    pub card_jobs: CardJobCount,
    pub jobs: Vec<JobCount>,
    pub research: Research,
    pub landings: Landings,
    pub experiments: Experiments,
    pub owner: OwnerInbox,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Research {
    pub jobs: i64,
    pub suggestions: i64,
    pub cards: i64,
    pub votes: i64,
    pub findings: i64,
    pub answers: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KindCount {
    pub kind: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Funnel {
    pub posted: i64,
    pub voted: i64,
    pub needs_votes: i64,
    pub eligible: i64,
    pub rejected: i64,
    pub carded: i64,
    pub built: i64,
    pub tried: i64,
    pub expired: i64,
    pub closed: i64,
    pub with_card: i64,
    pub in_scope: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardJobCount {
    pub queued: i64,
    pub carded: i64,
    pub no_card: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobCount {
    pub kind: String,
    pub done_changed: i64,
    pub done_unchanged: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub open: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Landings {
    pub landed: i64,
    pub code_files: i64,
    pub docs_only: i64,
    pub pushed: i64,
    pub held: i64,
    pub conflict: i64,
    pub failed_verify: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Experiments {
    pub planned: i64,
    pub kept: i64,
    pub discarded: i64,
    #[serde(rename = "cycles_total")]
    pub cycles: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OwnerInbox {
    pub stop_questions: i64,
    pub questions: i64,
}

/// Go: func BuildReport(s *store.Store, project string, days int) (*Report, error) {
pub fn build_report(s: &pulse_store::Store, project: &str, days: i64) -> Result<Report> {
    todo!("evolve::report::build_report")
}

/// Go: func postCounts(s *store.Store, project, since string) ([]KindCount, error) {
pub(crate) fn post_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<Vec<KindCount>> {
    todo!("evolve::report::post_counts")
}

/// Go: func suggestionFunnel(s *store.Store, project, since string, status *LoopStatus) (Funnel, error) {
pub(crate) fn suggestion_funnel(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
    status: &crate::status::LoopStatus,
) -> Result<Funnel> {
    todo!("evolve::report::suggestion_funnel")
}

/// Go: func cardJobCounts(s *store.Store, project, since string) (CardJobCount, error) {
pub(crate) fn card_job_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<CardJobCount> {
    todo!("evolve::report::card_job_counts")
}

pub(crate) const RESEARCH_JOB_SQL: &str = "(j.kind = 'reflect' AND j.source LIKE 'research:%')";

/// Go: func jobCounts(s *store.Store, project, since string) ([]JobCount, error) {
pub(crate) fn job_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<Vec<JobCount>> {
    todo!("evolve::report::job_counts")
}

/// Go: func researchCounts(s *store.Store, project, since string) (Research, error) {
pub(crate) fn research_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<Research> {
    todo!("evolve::report::research_counts")
}

/// Go: func landingCounts(s *store.Store, project, since string) (Landings, error) {
pub(crate) fn landing_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
) -> Result<Landings> {
    todo!("evolve::report::landing_counts")
}

/// Go: func experimentCounts(s *store.Store, project, since string, status *LoopStatus) (Experiments, error) {
pub(crate) fn experiment_counts(
    s: &pulse_store::Store,
    project: &str,
    since: &str,
    status: &crate::status::LoopStatus,
) -> Result<Experiments> {
    todo!("evolve::report::experiment_counts")
}

/// Go: func ownerInbox(s *store.Store, project string) (OwnerInbox, error) {
pub(crate) fn owner_inbox(s: &pulse_store::Store, project: &str) -> Result<OwnerInbox> {
    todo!("evolve::report::owner_inbox")
}

/// Go: func reportRows(r *Report) [][]string {
pub(crate) fn report_rows(r: &Report) -> Vec<Vec<String>> {
    todo!("evolve::report::report_rows")
}
