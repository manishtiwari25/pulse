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
impl Handler {
    /// Go: func (h *Handler) running(r *http.Request) (map[string]any, error)
    pub(crate) fn running(&self, r: &crate::httpx::Request) -> anyhow::Result<JsonMap> {
        todo!("hub::running::running")
    }
    /// Go: func (h *Handler) additionRunning(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_running(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::running::addition_running")
    }
}
