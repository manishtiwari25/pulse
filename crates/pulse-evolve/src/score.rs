#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/score.go.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct StatusEntry {
    pub(crate) index: u8,
    pub(crate) tree: u8,
    pub(crate) path: String,
}

impl StatusEntry {
    /// Go: func (e statusEntry) isNew() bool   { return e.index == '?' || e.index == 'A' }
    pub(crate) fn is_new(&self) -> bool {
        todo!("evolve::score::statusEntry::is_new")
    }

    /// Go: func (e statusEntry) deleted() bool { return e.index == 'D' || e.tree == 'D' }
    pub(crate) fn deleted(&self) -> bool {
        todo!("evolve::score::statusEntry::deleted")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ScoredTree {
    pub(crate) head: String,
    pub(crate) commit: String,
    pub(crate) entries: Vec<StatusEntry>,
    pub(crate) v4_tag: bool,
    pub(crate) head_count: i64,
    pub(crate) commit_count: i64,
    pub(crate) records: i64,
}

impl ScoredTree {
    /// Go: func (t *scoredTree) atCommit() (bool, string) {
    pub(crate) fn at_commit(&self) -> (bool, String) {
        todo!("evolve::score::scoredTree::at_commit")
    }

    /// Go: func (t *scoredTree) clean() (bool, string) {
    pub(crate) fn clean(&self) -> (bool, String) {
        todo!("evolve::score::scoredTree::clean")
    }
}

pub(crate) struct StarterRule {
    pub(crate) records: bool,
    pub(crate) judge: fn(&ScoredTree) -> (bool, String),
}

fn starter_rule_placeholder(tree: &ScoredTree) -> (bool, String) {
    todo!("evolve::score::starter_rule_placeholder")
}

/// Go: var starterRules = map[string]starterRule{
pub(crate) static STARTER_RULES: LazyLock<BTreeMap<String, StarterRule>> =
    LazyLock::new(BTreeMap::new);

/// Go: func starterNames() []string {
pub(crate) fn starter_names() -> Vec<String> {
    todo!("evolve::score::starter_names")
}

/// Go: func scoreTree(ctx context.Context, dir, name, commit string) (score int, reason string, err error) {
pub(crate) fn score_tree(
    cancel: &pulse_bus::CancellationToken,
    dir: &str,
    name: &str,
    commit: &str,
) -> Result<(i64, String)> {
    todo!("evolve::score::score_tree")
}

/// Go: func inspectTree(ctx context.Context, dir, commit string) (*scoredTree, error) {
pub(crate) fn inspect_tree(
    cancel: &pulse_bus::CancellationToken,
    dir: &str,
    commit: &str,
) -> Result<ScoredTree> {
    todo!("evolve::score::inspect_tree")
}

/// Go: func scoreFailure(ctx context.Context, step string) error {
pub(crate) fn score_failure(cancel: &pulse_bus::CancellationToken, step: &str) -> anyhow::Error {
    todo!("evolve::score::score_failure")
}

/// Go: func parseStatus(raw string) ([]statusEntry, error) {
pub(crate) fn parse_status(raw: &str) -> Result<Vec<StatusEntry>> {
    todo!("evolve::score::parse_status")
}

/// Go: func isolatedRecords() (int, error) {
pub(crate) fn isolated_records() -> Result<i64> {
    todo!("evolve::score::isolated_records")
}
