#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/draft.go.

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const DRAFT_SOURCE_PREFIX: &str = "draft:";

/// Go: func DraftHours(s *store.Store) int {
pub fn draft_hours(s: &pulse_store::Store) -> i64 {
    todo!("evolve::draft::draft_hours")
}

pub const DRAFT_JOB_PRIORITY: i64 = 6;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoopRecordState {
    pub name: String,
    pub id: String,
    pub rev: i64,
    pub seeded: bool,
    pub seeded_from: String,
    pub draft: Option<pulse_store::RecordDraft>,
    #[serde(skip)]
    pub(crate) body: String,
}

/// Go: func LoopRecords(s *store.Store, project string) ([]LoopRecordState, error) {
pub fn loop_records(s: &pulse_store::Store, project: &str) -> Result<Vec<LoopRecordState>> {
    todo!("evolve::draft::loop_records")
}

/// Go: func OpenDraftJob(s *store.Store, project string) (*JobRef, error) {
pub fn open_draft_job(
    s: &pulse_store::Store,
    project: &str,
) -> Result<Option<crate::status::JobRef>> {
    todo!("evolve::draft::open_draft_job")
}

/// Go: func PlanDraftJob(s *store.Store, project, actor, now string, force bool) (job *board.Job, created bool, reason string, err error) {
pub fn plan_draft_job(
    s: &pulse_store::Store,
    project: &str,
    actor: &str,
    now: &str,
    force: bool,
) -> Result<(Option<pulse_board::Job>, bool, String)> {
    todo!("evolve::draft::plan_draft_job")
}

/// Go: func loopRecordsPhrase(states []LoopRecordState) string {
pub(crate) fn loop_records_phrase(states: &[LoopRecordState]) -> String {
    todo!("evolve::draft::loop_records_phrase")
}

/// Go: func draftBrief(s *store.Store, p *store.Project, seeded []LoopRecordState) string {
pub(crate) fn draft_brief(
    s: &pulse_store::Store,
    p: &pulse_store::Project,
    seeded: &[LoopRecordState],
) -> String {
    todo!("evolve::draft::draft_brief")
}

/// Go: func tail(from string) string {
pub(crate) fn tail(from: &str) -> String {
    todo!("evolve::draft::tail")
}
