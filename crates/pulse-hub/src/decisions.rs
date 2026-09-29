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
pub(crate) const CONSOLE_NOTE: &str = " (via console)";
impl Handler {
    /// Go: func (h *Handler) additionRecordDecision(w http.ResponseWriter, r *http.Request, id, action string)
    pub(crate) fn addition_record_decision(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        action: String,
    ) -> () {
        todo!("hub::decisions::addition_record_decision")
    }
    /// Go: func (h *Handler) additionRecordDraft(w http.ResponseWriter, r *http.Request, id, action string)
    pub(crate) fn addition_record_draft(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        action: String,
    ) -> () {
        todo!("hub::decisions::addition_record_draft")
    }
}
