#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/research.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) const RESEARCH_SOURCE_PREFIX: &str = "research:";

#[derive(Debug, Clone, Default)]
pub(crate) struct ResearchFocus {
    pub name: String,
    pub title: String,
    pub look: String,
    pub card: String,
}

/// Go: func researchDue(s *store.Store, slug, now string) (researchFocus, string, bool)
pub(crate) fn research_due(
    s: &pulse_store::Store,
    slug: String,
    now: String,
) -> (ResearchFocus, String, bool) {
    todo!("runner::research::research_due")
}

/// Go: func planResearch(s *store.Store, slug, now, createdBy string) (row, bool, error)
pub(crate) fn plan_research(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    created_by: String,
) -> anyhow::Result<(Row, bool)> {
    todo!("runner::research::plan_research")
}

/// Go: func researchBrief(s *store.Store, slug string, focus researchFocus) string
pub(crate) fn research_brief(s: &pulse_store::Store, slug: String, focus: ResearchFocus) -> String {
    todo!("runner::research::research_brief")
}
