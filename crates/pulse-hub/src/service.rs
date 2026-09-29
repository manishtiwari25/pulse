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
pub(crate) const SERVICE_HUB_PORT: i64 = 8765;
impl Handler {
    /// Go: func (h *Handler) additionService(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_service(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::service::addition_service")
    }
}
