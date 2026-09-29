#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/lead.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) const LEAD_JOB_PRIORITY: i32 = 50;
pub(crate) const LEAD_KIND: &str = "lead";
pub(crate) static LEAD_HUMAN_ONLY_MARKER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)^[ \t]*(pair|onboard):[ \t]*\S").expect("lead marker regex")
});

#[derive(Debug, Clone, Default)]
pub(crate) struct DueTask {
    pub(crate) task: pulse_tasks::Task,
    pub why: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AwaitingProposal {
    pub id: String,
    pub date: String,
    pub agent: String,
    pub summary: String,
    pub body: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AwayBrief {
    pub proposals: Vec<AwaitingProposal>,
    pub owner_cards: Vec<pulse_tally::WorkItem>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct OwnerQuestion {
    pub id: String,
    pub area: String,
    pub thread: String,
    pub agent: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LeadBriefExtra {
    pub agent_lines: Vec<String>,
    pub owner_questions: Vec<OwnerQuestion>,
    pub mention_lines: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct JobsTally {
    pub(crate) active: i32,
    pub(crate) finished: i32,
    pub(crate) failed: i32,
    pub(crate) waiting: i32,
    pub(crate) last_failed: String,
    pub(crate) fixed: bool,
    pub(crate) details: Vec<String>,
}

/// Go: func leadName(slug string) string
pub(crate) fn lead_name(slug: String) -> String {
    todo!("runner::lead::lead_name")
}

/// Go: func ensureLead(s *store.Store, slug string) (*store.Agent, bool, error)
pub(crate) fn ensure_lead(
    s: &pulse_store::Store,
    slug: String,
) -> anyhow::Result<(Option<pulse_store::Agent>, bool)> {
    todo!("runner::lead::ensure_lead")
}

/// Go: func leadBrief(slug string, msgs []tasks.Message, due []dueTask, away *awayBrief, extras ...leadBriefExtra) string
pub(crate) fn lead_brief(
    slug: String,
    msgs: Vec<pulse_tasks::Message>,
    due: Vec<DueTask>,
    away: Option<AwayBrief>,
    extras: Vec<LeadBriefExtra>,
) -> String {
    todo!("runner::lead::lead_brief")
}

/// Go: func writeLeadAgentsSection(b *strings.Builder, slug string, lines []string)
pub(crate) fn write_lead_agents_section(b: Option<String>, slug: String, lines: &[String]) {
    todo!("runner::lead::write_lead_agents_section")
}

/// Go: func writeLeadMentionsSection(b *strings.Builder, slug string, lines []string)
pub(crate) fn write_lead_mentions_section(b: Option<String>, slug: String, lines: &[String]) {
    todo!("runner::lead::write_lead_mentions_section")
}

/// Go: func writeLeadOwnerQuestionsSection(b *strings.Builder, slug string, away *awayBrief, questions []ownerQuestion)
pub(crate) fn write_lead_owner_questions_section(
    b: Option<String>,
    slug: String,
    away: Option<AwayBrief>,
    questions: Vec<OwnerQuestion>,
) {
    todo!("runner::lead::write_lead_owner_questions_section")
}

/// Go: func writeOwnerAwaySection(b *strings.Builder, slug string, away *awayBrief)
pub(crate) fn write_owner_away_section(b: Option<String>, slug: String, away: Option<AwayBrief>) {
    todo!("runner::lead::write_owner_away_section")
}

/// Go: func ownerCards(items []tally.WorkItem) []tally.WorkItem
pub(crate) fn owner_cards(items: Vec<pulse_tally::WorkItem>) -> Vec<pulse_tally::WorkItem> {
    todo!("runner::lead::owner_cards")
}

/// Go: func awayLeadRules(slug string, away *awayBrief, start int) (rules string, next int)
pub(crate) fn away_lead_rules(slug: String, away: Option<AwayBrief>, start: i32) -> (String, i32) {
    todo!("runner::lead::away_lead_rules")
}

/// Go: func tallyJobs(jobs []tasks.Job) jobsTally
pub(crate) fn tally_jobs(jobs: Vec<pulse_tasks::Job>) -> JobsTally {
    todo!("runner::lead::tally_jobs")
}

/// Go: func followUpBase(priority int) time.Duration
pub(crate) fn follow_up_base(priority: i32) -> std::time::Duration {
    todo!("runner::lead::follow_up_base")
}

/// Go: func followUpDue(t tasks.Task, since string) string
pub(crate) fn follow_up_due(t: pulse_tasks::Task, since: String) -> String {
    todo!("runner::lead::follow_up_due")
}

/// Go: func leadDue(s *store.Store, slug, lead, now string) ([]tasks.Message, []dueTask, *awayBrief, error)
pub(crate) fn lead_due(
    s: &pulse_store::Store,
    slug: String,
    lead: String,
    now: String,
) -> anyhow::Result<(Vec<pulse_tasks::Message>, Vec<DueTask>, Option<AwayBrief>)> {
    todo!("runner::lead::lead_due")
}

/// Go: func awaitingLeadProposals(s *store.Store, slug, now string, paced bool) ([]awaitingProposal, error)
pub(crate) fn awaiting_lead_proposals(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    paced: bool,
) -> anyhow::Result<Vec<AwaitingProposal>> {
    todo!("runner::lead::awaiting_lead_proposals")
}

/// Go: func recentlyPresentedToLead(s *store.Store, slug, now, postID string) bool
pub(crate) fn recently_presented_to_lead(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    post_id: String,
) -> bool {
    todo!("runner::lead::recently_presented_to_lead")
}

/// Go: func ownerAddressedQuestions(s *store.Store, slug, now string) []ownerQuestion
pub(crate) fn owner_addressed_questions(
    s: &pulse_store::Store,
    slug: String,
    now: String,
) -> Vec<OwnerQuestion> {
    todo!("runner::lead::owner_addressed_questions")
}

/// Go: func mentionsProjectLead(p row) bool
pub(crate) fn mentions_project_lead(p: Row) -> bool {
    todo!("runner::lead::mentions_project_lead")
}

/// Go: func leadPeerMentions(s *store.Store, slug, lead, now, actor string) ([]string, int, error)
pub(crate) fn lead_peer_mentions(
    s: &pulse_store::Store,
    slug: String,
    lead: String,
    now: String,
    actor: String,
) -> anyhow::Result<(Vec<String>, i32)> {
    todo!("runner::lead::lead_peer_mentions")
}

/// Go: func tendLead(s *store.Store, slug, now, actor string) []string
pub(crate) fn tend_lead(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    actor: String,
) -> Vec<String> {
    todo!("runner::lead::tend_lead")
}

/// Go: func tendIdleLead(s *store.Store, slug, lead, now, actor string) []string
pub(crate) fn tend_idle_lead(
    s: &pulse_store::Store,
    slug: String,
    lead: String,
    now: String,
    actor: String,
) -> Vec<String> {
    todo!("runner::lead::tend_idle_lead")
}

/// Go: func leadIdleBrief(s *store.Store, slug string, items []tally.WorkItem, failed []row, maxJobs int) string
pub(crate) fn lead_idle_brief(
    s: &pulse_store::Store,
    slug: String,
    items: Vec<pulse_tally::WorkItem>,
    failed: Vec<Row>,
    max_jobs: i32,
) -> String {
    todo!("runner::lead::lead_idle_brief")
}

/// Go: func tendTasks(s *store.Store, slug, now, actor string) []string
pub(crate) fn tend_tasks(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    actor: String,
) -> Vec<String> {
    todo!("runner::lead::tend_tasks")
}

/// Go: func leadJobFor(s *store.Store, busy []string, dryRun bool, now, slot string, taken int) (lead string, job row, why string)
pub(crate) fn lead_job_for(
    s: &pulse_store::Store,
    busy: &[String],
    dry_run: bool,
    now: String,
    slot: String,
    taken: i32,
) -> (String, Row, String) {
    todo!("runner::lead::lead_job_for")
}

/// Go: func anyLeased(jobs []tasks.Job) bool
pub(crate) fn any_leased(jobs: Vec<pulse_tasks::Job>) -> bool {
    todo!("runner::lead::any_leased")
}

/// Go: func jobRowKind(s *store.Store, id string) string
pub(crate) fn job_row_kind(s: &pulse_store::Store, id: String) -> String {
    todo!("runner::lead::job_row_kind")
}

impl AwayBrief {
    /// Go: func (a *awayBrief) pending() int
    pub(crate) fn pending(&mut self) -> i32 {
        todo!("runner::lead::pending")
    }
}
