#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnvExecutor {
    pub executor: String,
    pub fit: bool,
    pub why: String,
    pub phrase: String,
    pub hint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnvEntry {
    pub project: String,
    pub path: String,
    pub recipe: String,
    pub valid: bool,
    pub problems: Vec<String>,
    pub tools: Vec<String>,
    pub setup: Vec<String>,
    pub env: Vec<String>,
    pub services: Vec<String>,
    pub suggested_check: String,
    pub verify_cmd: String,
    pub held: Vec<String>,
    pub executors: Vec<EnvExecutor>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnvUnfit {
    pub project: String,
    pub executor: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnvReport {
    pub projects: Vec<EnvEntry>,
    pub unfit: Vec<EnvUnfit>,
}

/// Go: func currentProject(s *store.Store, explicit string) (row, error)
pub(crate) fn current_project(
    s: &pulse_store::Store,
    explicit: String,
) -> anyhow::Result<crate::util::Row> {
    todo!("runner::env::current_project")
}

/// Go: func recipeSource(s *store.Store, project row) string
pub(crate) fn recipe_source(s: &pulse_store::Store, project: crate::util::Row) -> String {
    todo!("runner::env::recipe_source")
}

/// Go: func envCheck(s *store.Store, project, executor, now string) (*envReport, error)
pub(crate) fn env_check(
    s: &pulse_store::Store,
    project: serde_json::Value,
    now: String,
) -> anyhow::Result<Option<Box<EnvReport>>> {
    todo!("runner::env::env_check")
}

/// Go: func localToolsMissing(project row, recipe *Recipe) string
pub(crate) fn local_tools_missing(
    project: crate::util::Row,
    recipe: &crate::recipe::Recipe,
) -> String {
    todo!("runner::env::local_tools_missing")
}

/// Go: func onPath(tool string) bool
pub(crate) fn on_path(tool: String) -> bool {
    todo!("runner::env::on_path")
}

/// Go: func envCheckText(rep *envReport) string
pub(crate) fn env_check_text(rep: &EnvReport) -> String {
    todo!("runner::env::env_check_text")
}

/// Go: func envShow(s *store.Store, project row) (string, string, bool)
pub(crate) fn env_show(
    s: &pulse_store::Store,
    project: crate::util::Row,
) -> (String, String, bool) {
    todo!("runner::env::env_show")
}

/// Go: func envSet(s *store.Store, project row, text, author, origin, note string) (string, int, *Recipe, error)
pub(crate) fn env_set(
    s: &pulse_store::Store,
    project: crate::util::Row,
    text: String,
    author: String,
    origin: String,
    note: String,
) -> anyhow::Result<(String, i32, Option<Box<crate::recipe::Recipe>>)> {
    todo!("runner::env::env_set")
}

/// Go: func envImport(s *store.Store, project row, author string) (string, int, *Recipe, error)
pub(crate) fn env_import(
    s: &pulse_store::Store,
    project: crate::util::Row,
    author: String,
) -> anyhow::Result<(String, i32, Option<Box<crate::recipe::Recipe>>)> {
    todo!("runner::env::env_import")
}
