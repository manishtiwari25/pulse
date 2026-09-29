#![allow(unused_variables, dead_code, unused_imports)]
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, UiFs};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub(crate) type JsonMap = BTreeMap<String, serde_json::Value>;
/// Go: type TallySource interface {
pub trait TallySource: Send + Sync {
    fn records(&self, project: &str, area: &str) -> anyhow::Result<String>;
}
/// Go: type nativeTally struct{ store *store.Store }
pub(crate) struct NativeTally {
    pub(crate) store: Option<std::sync::Arc<pulse_store::Store>>,
}
/// Go: type boardData struct {
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct BoardData {
    pub(crate) project: pulse_store::Project,
    pub(crate) meta: JsonMap,
    pub(crate) params: JsonMap,
    pub(crate) posts: Vec<JsonMap>,
    pub(crate) by_id: JsonMap,
    pub(crate) claims: Vec<JsonMap>,
    pub(crate) threads: Vec<JsonMap>,
    pub(crate) proposals: Vec<JsonMap>,
    pub(crate) suggestions: Vec<JsonMap>,
    pub(crate) amendments: Vec<JsonMap>,
    pub(crate) evolution: JsonMap,
}
impl Handler {
    /// Go: func (n nativeTally) Records(project, area string) (string, error)
    pub fn records(&self, area: String) -> anyhow::Result<String> {
        todo!("hub::board::records")
    }
    /// Go: func (h *Handler) entry(slug string) (*store.Project, error)
    pub(crate) fn entry(&self, slug: String) -> anyhow::Result<Option<pulse_store::Project>> {
        todo!("hub::board::entry")
    }
    /// Go: func (h *Handler) snapshot(project, area string) (*boardData, error)
    pub(crate) fn snapshot(&self, area: String) -> anyhow::Result<BoardData> {
        todo!("hub::board::snapshot")
    }
}
impl BoardData {
    /// Go: func (d *boardData) thread(key string) map[string]any
    pub(crate) fn thread(&self, key: String) -> JsonMap {
        todo!("hub::board::thread")
    }
    /// Go: func (d *boardData) enrichThreads()
    pub(crate) fn enrich_threads(&self) -> () {
        todo!("hub::board::enrich_threads")
    }
    /// Go: func (d *boardData) post(id string) map[string]any
    pub(crate) fn post(&self, id: String) -> JsonMap {
        todo!("hub::board::post")
    }
    /// Go: func (d *boardData) overview() map[string]any
    pub(crate) fn overview(&self) -> JsonMap {
        todo!("hub::board::overview")
    }
    /// Go: func (d *boardData) threads() []map[string]any
    pub(crate) fn threads(&self) -> Vec<JsonMap> {
        todo!("hub::board::threads")
    }
    /// Go: func (d *boardData) notices() map[string]any
    pub(crate) fn notices(&self) -> JsonMap {
        todo!("hub::board::notices")
    }
}
/// Go: func str(v any) string
pub(crate) fn str(v: serde_json::Value) -> String {
    todo!("hub::board::str")
}
/// Go: func num(v any) int
pub(crate) fn num(v: serde_json::Value) -> i64 {
    todo!("hub::board::num")
}
/// Go: func flagInt(b bool) int
pub(crate) fn flag_int(b: bool) -> i64 {
    todo!("hub::board::flag_int")
}
/// Go: func object(keys string, values ...any) map[string]any
pub(crate) fn object(keys: String, values: Vec<serde_json::Value>) -> JsonMap {
    todo!("hub::board::object")
}
/// Go: func pick(row map[string]any, keys string) map[string]any
pub(crate) fn pick(row: JsonMap, keys: String) -> JsonMap {
    todo!("hub::board::pick")
}
/// Go: func clone(row map[string]any) map[string]any
pub(crate) fn clone(row: JsonMap) -> JsonMap {
    todo!("hub::board::clone")
}
/// Go: func order(rows []map[string]any, fields ...string)
pub(crate) fn order(rows: Vec<JsonMap>, fields: Vec<String>) -> () {
    todo!("hub::board::order")
}
/// Go: func gitValue(repo string, args ...string) string
pub(crate) fn git_value(repo: String, args: Vec<String>) -> String {
    todo!("hub::board::git_value")
}
/// Go: func present(p *store.Project) bool
pub(crate) fn present(p: &pulse_store::Project) -> bool {
    todo!("hub::board::present")
}
/// Go: func truncate(text string, n int) string
pub(crate) fn truncate(text: String, n: i64) -> String {
    todo!("hub::board::truncate")
}
/// Go: func postFrontmatter(p tally.Post) map[string]any
pub(crate) fn post_frontmatter(p: serde_json::Value) -> JsonMap {
    todo!("hub::board::post_frontmatter")
}
/// Go: func proposalText(body string) string
pub(crate) fn proposal_text(body: String) -> String {
    todo!("hub::board::proposal_text")
}
/// Go: func laterHubKind(posts []map[string]any, index int, kinds ...string) bool
pub(crate) fn later_hub_kind(posts: Vec<JsonMap>, index: i64, kinds: Vec<String>) -> bool {
    todo!("hub::board::later_hub_kind")
}
/// Go: func laterHubAnswerDate(posts []map[string]any, index int) bool
pub(crate) fn later_hub_answer_date(posts: Vec<JsonMap>, index: i64) -> bool {
    todo!("hub::board::later_hub_answer_date")
}
/// Go: func countStatus(rows []map[string]any, statuses ...string) int
pub(crate) fn count_status(rows: Vec<JsonMap>, statuses: Vec<String>) -> i64 {
    todo!("hub::board::count_status")
}
/// Go: func countPlannable(rows []map[string]any) int
pub(crate) fn count_plannable(rows: Vec<JsonMap>) -> i64 {
    todo!("hub::board::count_plannable")
}

/// Go: func sortedKeys[V any](m map[string]V) []string
pub(crate) fn sorted_keys<V>(m: BTreeMap<String, V>) -> Vec<String> {
    todo!("hub::board::sorted_keys")
}
