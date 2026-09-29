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
pub(crate) const CODE_DELIVERY_DEFAULT_LIMIT: i64 = 100;
pub(crate) const CODE_DELIVERY_MAX_LIMIT: i64 = 200;
impl Handler {
    /// Go: func (h *Handler) additionCodeDeliveries(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_code_deliveries(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::code_deliveries::addition_code_deliveries")
    }
    /// Go: func (h *Handler) validCodeDeliveryProject(w http.ResponseWriter, project string) bool
    pub(crate) fn valid_code_delivery_project(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        project: String,
    ) -> bool {
        todo!("hub::code_deliveries::valid_code_delivery_project")
    }
}
/// Go: func codeDeliveryPositiveInt(w http.ResponseWriter, value, name string, fallback, maximum int) (int, bool)
pub(crate) fn code_delivery_positive_int(
    w: &mut crate::httpx::ResponseWriter,
    name: String,
    maximum: i64,
) -> (i64, bool) {
    todo!("hub::code_deliveries::code_delivery_positive_int")
}
/// Go: func codeDeliveryBefore(w http.ResponseWriter, value string) (int64, bool)
pub(crate) fn code_delivery_before(
    w: &mut crate::httpx::ResponseWriter,
    value: String,
) -> (i64, bool) {
    todo!("hub::code_deliveries::code_delivery_before")
}
