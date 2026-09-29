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
pub(crate) static ADDITION_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub(crate) const ADDITION_TASK_COLUMNS: &str = "t.id,t.project,t.title,t.status,t.priority,t.lead,t.job_id,t.created_by,t.created_at,t.updated_at,t.current_rev,rv.brief";
pub(crate) const ADDITION_TASK_FROM: &str =
    " FROM tasks t JOIN task_revisions rv ON rv.task_id=t.id AND rv.rev=t.current_rev";
/// Go: type additionEvent struct {
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AdditionEvent {
    pub(crate) id: i64,
    pub(crate) at: String,
    pub(crate) actor: String,
    pub(crate) action: String,
    pub(crate) subject: String,
    pub(crate) detail: serde_json::Value,
}
impl Handler {
    /// Go: func (h *Handler) additions(w http.ResponseWriter, r *http.Request, route string) bool
    pub(crate) fn additions(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        route: String,
    ) -> bool {
        todo!("hub::additions::additions")
    }
    /// Go: func (h *Handler) additionProject(r *http.Request) string
    pub(crate) fn addition_project(&self, r: &crate::httpx::Request) -> String {
        todo!("hub::additions::addition_project")
    }
    /// Go: func (h *Handler) additionAvailable(w http.ResponseWriter, r *http.Request, tables ...string) bool
    pub(crate) fn addition_available(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        tables: Vec<String>,
    ) -> bool {
        todo!("hub::additions::addition_available")
    }
    /// Go: func (h *Handler) additionRows(r *http.Request, query string, args ...any) ([]map[string]any, error)
    pub(crate) fn addition_rows(
        &self,
        r: &crate::httpx::Request,
        query: String,
        args: Vec<serde_json::Value>,
    ) -> anyhow::Result<Vec<JsonMap>> {
        todo!("hub::additions::addition_rows")
    }
    /// Go: func (h *Handler) queryRows(ctx context.Context, query string, args ...any) ([]map[string]any, error)
    pub(crate) fn query_rows(
        &self,
        ctx: &pulse_bus::CancellationToken,
        query: String,
        args: Vec<serde_json::Value>,
    ) -> anyhow::Result<Vec<JsonMap>> {
        todo!("hub::additions::query_rows")
    }
    /// Go: func (h *Handler) additionTask(r *http.Request, id string) (map[string]any, error)
    pub(crate) fn addition_task(
        &self,
        r: &crate::httpx::Request,
        id: String,
    ) -> anyhow::Result<JsonMap> {
        todo!("hub::additions::addition_task")
    }
    /// Go: func (h *Handler) additionTaskWork(rows []map[string]any) error
    pub(crate) fn addition_task_work(&self, rows: Vec<JsonMap>) -> String {
        todo!("hub::additions::addition_task_work")
    }
    /// Go: func (h *Handler) additionTasks(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_tasks(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::additions::addition_tasks")
    }
    /// Go: func (h *Handler) additionMessages(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_messages(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::additions::addition_messages")
    }
    /// Go: func (h *Handler) additionCompute(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_compute(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::additions::addition_compute")
    }
    /// Go: func (h *Handler) additionTaskWrite(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_task_write(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::additions::addition_task_write")
    }
    /// Go: func (h *Handler) additionMessageWrite(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_message_write(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::additions::addition_message_write")
    }
    /// Go: func (h *Handler) additionComputeWrite(w http.ResponseWriter, r *http.Request, id, action string)
    pub(crate) fn addition_compute_write(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        action: String,
    ) -> () {
        todo!("hub::additions::addition_compute_write")
    }
    /// Go: func (h *Handler) additionRecords(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_records(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::additions::addition_records")
    }
    /// Go: func (h *Handler) additionConstitution(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_constitution(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::additions::addition_constitution")
    }
    /// Go: func (h *Handler) additionReadEvents(r *http.Request, since int64, limit int64) ([]additionEvent, error)
    pub(crate) fn addition_read_events(
        &self,
        r: &crate::httpx::Request,
        since: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        todo!("hub::additions::addition_read_events")
    }
    /// Go: func (h *Handler) additionEventFilter(r *http.Request) bus.Filter
    pub(crate) fn addition_event_filter(&self, r: &crate::httpx::Request) -> serde_json::Value {
        todo!("hub::additions::addition_event_filter")
    }
    /// Go: func (h *Handler) additionEvents(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_events(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::additions::addition_events")
    }
}
/// Go: func additionNotFound(w http.ResponseWriter)
pub(crate) fn addition_not_found(w: &mut crate::httpx::ResponseWriter) -> () {
    todo!("hub::additions::addition_not_found")
}
/// Go: func additionMethod(w http.ResponseWriter, r *http.Request, methods ...string) bool
pub(crate) fn addition_method(
    w: &mut crate::httpx::ResponseWriter,
    r: &crate::httpx::Request,
    methods: Vec<String>,
) -> bool {
    todo!("hub::additions::addition_method")
}
/// Go: func additionFailure(w http.ResponseWriter, err error)
pub(crate) fn addition_failure(w: &mut crate::httpx::ResponseWriter, err: String) -> () {
    todo!("hub::additions::addition_failure")
}
/// Go: func additionString(w http.ResponseWriter, payload map[string]any, key string, required bool) (string, bool)
pub(crate) fn addition_string(
    w: &mut crate::httpx::ResponseWriter,
    payload: JsonMap,
    key: String,
    required: bool,
) -> (String, bool) {
    todo!("hub::additions::addition_string")
}
/// Go: func additionPriority(w http.ResponseWriter, payload map[string]any) (int, bool)
pub(crate) fn addition_priority(
    w: &mut crate::httpx::ResponseWriter,
    payload: JsonMap,
) -> (i64, bool) {
    todo!("hub::additions::addition_priority")
}
/// Go: func additionRowString(row map[string]any, key string) string
pub(crate) fn addition_row_string(row: JsonMap, key: String) -> String {
    todo!("hub::additions::addition_row_string")
}
/// Go: func additionInteger(w http.ResponseWriter, value, name string, fallback, maximum int64) (int64, bool)
pub(crate) fn addition_integer(
    w: &mut crate::httpx::ResponseWriter,
    name: String,
    maximum: i64,
) -> (i64, bool) {
    todo!("hub::additions::addition_integer")
}
/// Go: func additionRecord(rec store.Record) map[string]any
pub(crate) fn addition_record(rec: pulse_store::Record) -> JsonMap {
    todo!("hub::additions::addition_record")
}
/// Go: func additionProtected(amendment map[string]any, constitution string) bool
pub(crate) fn addition_protected(amendment: JsonMap, constitution: String) -> bool {
    todo!("hub::additions::addition_protected")
}
/// Go: func additionBusEvent(event bus.Event) additionEvent
pub(crate) fn addition_bus_event(event: serde_json::Value) -> serde_json::Value {
    todo!("hub::additions::addition_bus_event")
}
