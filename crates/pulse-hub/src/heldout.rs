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
pub(crate) const HELDOUT_HEADER: &str = "X-Pulse-Heldout";
pub(crate) const HELDOUT_SLOT_LIMIT: i64 = 1000;
impl Handler {
    /// Go: func (h *Handler) additionHeldout(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_heldout(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::heldout::addition_heldout")
    }
}
/// Go: func heldoutWrite(w http.ResponseWriter, payload map[string]any) (evolve.HeldoutWrite, bool)
pub(crate) fn heldout_write(
    w: &mut crate::httpx::ResponseWriter,
    payload: JsonMap,
) -> (serde_json::Value, bool) {
    todo!("hub::heldout::heldout_write")
}
/// Go: func heldoutSlot(w http.ResponseWriter, payload map[string]any) (int, bool)
pub(crate) fn heldout_slot(w: &mut crate::httpx::ResponseWriter, payload: JsonMap) -> (i64, bool) {
    todo!("hub::heldout::heldout_slot")
}
