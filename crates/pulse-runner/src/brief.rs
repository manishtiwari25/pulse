#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/brief.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) static DENIED_TOOLS: &[&str] = &["shell(git push)", "shell(git remote)", "shell(gh:*)"];
pub(crate) static CARD_FIELD_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r";\s*(?i:class|file class|file|path|variable|hypothesis|tier|signal|check|target|target score|refines)\s*:").expect("card field regex")
});
pub(crate) static CARD_KEY_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(?i:class|file class|file|path|variable|hypothesis|tier|signal|check|target|target score|refines)\s*:").expect("card key regex")
});

#[derive(Debug, Clone, Default)]
pub(crate) struct BallotItem {
    pub id: String,
    pub key: String,
    pub summary: String,
    pub body: String,
    pub up: i32,
    pub down: i32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BriefOpts {
    pub job: Row,
    pub project: Row,
    pub agent: String,
    pub clone: String,
    pub minutes: i32,
    pub ballot: Vec<BallotItem>,
    pub has_ball: bool,
    pub remote: bool,
    pub setup: String,
    pub recipe: serde_json::Value,
    pub found: bool,
    pub lessons: Vec<String>,
    pub store: serde_json::Value,
    pub now: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Vote {
    pub on: String,
    pub vote: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Decision {
    pub post_id: String,
    pub answer: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Suggestion {
    pub area: String,
    pub thread: String,
    pub card: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Report {
    pub summary: String,
    pub questions: Vec<String>,
    pub findings: Vec<String>,
    pub suggestions: Vec<Suggestion>,
    pub votes: Vec<Vote>,
    pub decisions: Vec<Decision>,
    pub found: bool,
}

/// Go: func ownerPathsTouched(clone, base string) []string
pub(crate) fn owner_paths_touched(clone: String, base: String) -> Vec<String> {
    todo!("runner::brief::owner_paths_touched")
}

/// Go: func writeBrief(o briefOpts) (string, error)
pub(crate) fn write_brief(o: BriefOpts) -> anyhow::Result<String> {
    todo!("runner::brief::write_brief")
}

/// Go: func writeFocusBriefSection(b *[]string, s *store.Store, project, agent, now string, remote bool)
pub(crate) fn write_focus_brief_section(
    b: Option<Box<&[String]>>,
    s: &pulse_store::Store,
    project: String,
    agent: String,
    now: String,
    remote: bool,
) {
    todo!("runner::brief::write_focus_brief_section")
}

/// Go: func briefStore(s *store.Store, now string) (*store.Store, func())
pub(crate) fn brief_store(
    s: &pulse_store::Store,
    now: String,
) -> (pulse_store::Store, serde_json::Value) {
    todo!("runner::brief::brief_store")
}

/// Go: func writeMentionsBriefSection(b *[]string, s *store.Store, project, agent, now string, remote bool)
pub(crate) fn write_mentions_brief_section(
    b: Option<Box<&[String]>>,
    s: &pulse_store::Store,
    project: String,
    agent: String,
    now: String,
    remote: bool,
) {
    todo!("runner::brief::write_mentions_brief_section")
}

/// Go: func mentionLines(s *store.Store, project, agent, now string) []string
pub(crate) fn mention_lines(
    s: &pulse_store::Store,
    project: String,
    agent: String,
    now: String,
) -> Vec<String> {
    todo!("runner::brief::mention_lines")
}

/// Go: func mentionLine(p board.Post) string
pub(crate) fn mention_line(p: pulse_board::Post) -> String {
    todo!("runner::brief::mention_line")
}

/// Go: func focusLines(s *store.Store, project, exclude, now string) []string
pub(crate) fn focus_lines(
    s: &pulse_store::Store,
    project: String,
    exclude: String,
    now: String,
) -> Vec<String> {
    todo!("runner::brief::focus_lines")
}

/// Go: func plainCancelSummary(summary string) bool
pub(crate) fn plain_cancel_summary(summary: String) -> bool {
    todo!("runner::brief::plain_cancel_summary")
}

/// Go: func parseResultText(text *string) report
pub(crate) fn parse_result_text(text: Option<String>) -> Report {
    todo!("runner::brief::parse_result_text")
}

/// Go: func cardBody(text string) (body, summary string, ok bool)
pub(crate) fn card_body(text: String) -> (String, String, bool) {
    todo!("runner::brief::card_body")
}

/// Go: func parseSuggestionLine(text string) suggestion
pub(crate) fn parse_suggestion_line(text: String) -> Suggestion {
    todo!("runner::brief::parse_suggestion_line")
}

impl Report {
    /// Go: func (r report) reported() bool
    pub(crate) fn reported(&mut self) -> bool {
        todo!("runner::brief::reported")
    }
    /// Go: func (r report) incomplete() bool
    pub(crate) fn incomplete(&mut self) -> bool {
        todo!("runner::brief::incomplete")
    }
}
