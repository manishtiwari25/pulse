#![allow(unused_variables, dead_code, unused_imports)]

//! Port of internal/evolve/readonly.go.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct TreeEntry {
    pub(crate) name: String,
    pub(crate) mode: u32,
}

#[derive(Debug)]
pub(crate) struct TreeRoot;

/// Go: func freezeTree(root *os.Root) (func() error, error) {
pub(crate) fn freeze_tree(
    root: &TreeRoot,
) -> Result<Box<dyn FnOnce() -> Result<()> + Send + 'static>> {
    todo!("evolve::readonly::freeze_tree")
}

/// Go: func treeFingerprint(root *os.Root) (string, error) {
pub(crate) fn tree_fingerprint(root: &TreeRoot) -> Result<String> {
    todo!("evolve::readonly::tree_fingerprint")
}
