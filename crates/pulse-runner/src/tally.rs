#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/tally.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

/// Go: func suggestionRecords(s *store.Store, slug, now string) []tally.Suggestion
pub(crate) fn suggestion_records(
    s: &pulse_store::Store,
    slug: String,
    now: String,
) -> Vec<pulse_tally::Suggestion> {
    todo!("runner::tally::suggestion_records")
}

/// Go: func ballotItemsFor(s *store.Store, slug, agent, now, session string) []ballotItem
pub(crate) fn ballot_items_for(
    s: &pulse_store::Store,
    slug: String,
    agent: String,
    now: String,
    session: String,
) -> Vec<serde_json::Value> {
    todo!("runner::tally::ballot_items_for")
}
