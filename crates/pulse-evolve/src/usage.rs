#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, Once};
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct UsageOptions {
    pub runner: String,
    pub days: i64,
    pub home: String,
    pub now: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageSession {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "models")]
    pub models: Vec<String>,
    #[serde(rename = "input")]
    pub input: Option<i64>,
    #[serde(rename = "output")]
    pub output: Option<i64>,
    #[serde(rename = "cache_read")]
    pub cache_read: Option<i64>,
    #[serde(rename = "cache_write")]
    pub cache_write: Option<i64>,
    #[serde(rename = "cache")]
    pub cache: Option<i64>,
    #[serde(rename = "subagent_total")]
    pub subagent_total: Option<i64>,
    #[serde(rename = "total")]
    pub total: Option<i64>,
    #[serde(rename = "messages")]
    pub messages: i64,
    #[serde(rename = "mode")]
    pub mode: String,
    #[serde(rename = "updated")]
    pub updated: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerUsage {
    #[serde(rename = "runner")]
    pub runner: String,
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "sessions")]
    pub sessions: Vec<UsageSession>,
    #[serde(rename = "warnings")]
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone)]
pub(crate) struct LocalUsageFile {
    pub(crate) path: String,
    pub(crate) at: DateTime<Utc>,
}
/// Go: func usageFiles(roots []string, extension string, cutoff time.Time) ([]localUsageFile, int)
pub(crate) fn usage_files(
    roots: &[String],
    extension: &str,
    cutoff: DateTime<Utc>,
) -> (Vec<LocalUsageFile>, i64) {
    todo!("evolve::usage::usage_files")
}
/// Go: func object(value any) map[string]any
pub(crate) fn object(value: &Value) -> Option<Map<String, Value>> {
    todo!("evolve::usage::object")
}
/// Go: func text(value any) string
pub(crate) fn text(value: &Value) -> String {
    todo!("evolve::usage::text")
}
/// Go: func objectOrJSON(value any) map[string]any
pub(crate) fn object_or_json(value: &Value) -> Option<Map<String, Value>> {
    todo!("evolve::usage::object_or_json")
}
/// Go: func decodeObject(data []byte) (map[string]any, error)
pub(crate) fn decode_object(data: &[u8]) -> Result<Map<String, Value>> {
    todo!("evolve::usage::decode_object")
}
/// Go: func usageLines(file string, each func(map[string]any)) (bad int)
pub(crate) fn usage_lines<F>(file: &str, each: F) -> i64
where
    F: FnMut(Map<String, Value>),
{
    todo!("evolve::usage::usage_lines")
}
/// Go: func token(value any) *int64
pub(crate) fn token(value: &Value) -> Option<i64> {
    todo!("evolve::usage::token")
}
/// Go: func addToken(dest **int64, value *int64)
pub(crate) fn add_token(dest: &mut Option<i64>, value: Option<i64>) {
    todo!("evolve::usage::add_token")
}
/// Go: func sumTokens(values ...*int64) *int64
pub(crate) fn sum_tokens(values: &[Option<i64>]) -> Option<i64> {
    todo!("evolve::usage::sum_tokens")
}
/// Go: func addModel(session *UsageSession, model string)
pub(crate) fn add_model(session: &mut UsageSession, model: &str) {
    todo!("evolve::usage::add_model")
}
/// Go: func newUsage(id string, at time.Time) *UsageSession
pub(crate) fn new_usage(id: &str, at: DateTime<Utc>) -> UsageSession {
    todo!("evolve::usage::new_usage")
}
/// Go: func hasTokens(s *UsageSession) bool
pub(crate) fn has_tokens(s: &UsageSession) -> bool {
    todo!("evolve::usage::has_tokens")
}
/// Go: func collectCopilot(files []localUsageFile) ([]UsageSession, int)
pub(crate) fn collect_copilot(files: &[LocalUsageFile]) -> (Vec<UsageSession>, i64) {
    todo!("evolve::usage::collect_copilot")
}
/// Go: func collectClaude(files []localUsageFile) ([]UsageSession, int)
pub(crate) fn collect_claude(files: &[LocalUsageFile]) -> (Vec<UsageSession>, i64) {
    todo!("evolve::usage::collect_claude")
}
/// Go: func collectCodex(files []localUsageFile) ([]UsageSession, int)
pub(crate) fn collect_codex(files: &[LocalUsageFile]) -> (Vec<UsageSession>, i64) {
    todo!("evolve::usage::collect_codex")
}
/// Go: func collectOpenCode(files []localUsageFile) ([]UsageSession, int)
pub(crate) fn collect_open_code(files: &[LocalUsageFile]) -> (Vec<UsageSession>, i64) {
    todo!("evolve::usage::collect_open_code")
}
/// Go: func usageSessionList(sessions map[string]*UsageSession) []UsageSession
pub(crate) fn usage_session_list(sessions: BTreeMap<String, UsageSession>) -> Vec<UsageSession> {
    todo!("evolve::usage::usage_session_list")
}
/// Go: func Usage(opts UsageOptions) ([]RunnerUsage, error)
pub fn usage(opts: UsageOptions) -> Result<Vec<RunnerUsage>> {
    todo!("evolve::usage::usage")
}
