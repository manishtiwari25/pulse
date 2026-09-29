#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/clones.go.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

pub(crate) const DEFAULT_ANSWER_KEYS: &[&str] = &[
    ".pulse",
    "docs/evolution/evaluators.md",
    "docs/evolution/archive.md",
    "docs/board/evolution",
    "docs/board/governance",
    "docs/memory/mistakes.md",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViolationError {
    pub reason: String,
}

impl fmt::Display for ViolationError {
    /// Go: func (e *ViolationError) Error() string { return "evolution constraint violation: " + e.Reason }
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("evolve::clones::ViolationError::fmt")
    }
}

impl Error for ViolationError {}

/// Go: func projectAnswerKeys(s *store.Store, project string) ([]string, error) {
pub(crate) fn project_answer_keys(s: &pulse_store::Store, project: &str) -> Result<Vec<String>> {
    todo!("evolve::clones::project_answer_keys")
}

/// Go: func fleetRoot(s *store.Store, create bool) (string, error) {
pub(crate) fn fleet_root(s: &pulse_store::Store, create: bool) -> Result<String> {
    todo!("evolve::clones::fleet_root")
}

/// Go: func resolveRevision(ctx context.Context, repo, ref string) (string, string, error) {
pub(crate) fn resolve_revision(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    ref_: &str,
) -> Result<(String, String)> {
    todo!("evolve::clones::resolve_revision")
}

/// Go: func strippedClone(ctx context.Context, repo, commit, dest string, keys []string) error {
pub(crate) fn stripped_clone(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    commit: &str,
    dest: &str,
    keys: &[String],
) -> Result<()> {
    todo!("evolve::clones::stripped_clone")
}

/// Go: func hideStripped(ctx context.Context, dest string, keys []string) error {
pub(crate) fn hide_stripped(
    cancel: &pulse_bus::CancellationToken,
    dest: &str,
    keys: &[String],
) -> Result<()> {
    todo!("evolve::clones::hide_stripped")
}

/// Go: func cloneFailure(ctx context.Context, step string) error {
pub(crate) fn clone_failure(cancel: &pulse_bus::CancellationToken, step: &str) -> anyhow::Error {
    todo!("evolve::clones::clone_failure")
}

/// Go: func checkClone(ctx context.Context, dest string, keys []string) error {
pub(crate) fn check_clone(
    cancel: &pulse_bus::CancellationToken,
    dest: &str,
    keys: &[String],
) -> Result<()> {
    todo!("evolve::clones::check_clone")
}

/// Go: func removeManaged(s *store.Store, id string) error {
pub(crate) fn remove_managed(s: &pulse_store::Store, id: &str) -> Result<()> {
    todo!("evolve::clones::remove_managed")
}
