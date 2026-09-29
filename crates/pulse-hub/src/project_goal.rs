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
    /// Go: func (h *Handler) additionProjectGoal(w http.ResponseWriter, r *http.Request, slug string)
    pub(crate) fn addition_project_goal(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        slug: String,
    ) -> () {
        todo!("hub::project_goal::addition_project_goal")
    }
}
/// Go: func readProjectGoalPayload(w http.ResponseWriter, r *http.Request) (string, bool, bool)
pub(crate) fn read_project_goal_payload(
    w: &mut crate::httpx::ResponseWriter,
    r: &crate::httpx::Request,
) -> (String, bool, bool) {
    todo!("hub::project_goal::read_project_goal_payload")
}
