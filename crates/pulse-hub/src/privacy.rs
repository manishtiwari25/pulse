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
pub(crate) const HELDOUT_SETTING: &str = "evolve.heldout_dir";
pub(crate) const PRIVATE_VALUE: &str = pulse_bus::PRIVATE_VALUE;
/// Go: func hiddenSettingValue(value string) string
pub(crate) fn hidden_setting_value(value: String) -> String {
    todo!("hub::privacy::hidden_setting_value")
}
/// Go: func privateSettingEvent(event additionEvent) additionEvent
pub(crate) fn private_setting_event(event: serde_json::Value) -> serde_json::Value {
    todo!("hub::privacy::private_setting_event")
}
