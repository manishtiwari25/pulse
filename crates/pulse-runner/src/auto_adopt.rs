#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

/// Go: func autoAdoptDrafts(s *store.Store, now, actor string) []string
pub(crate) fn auto_adopt_drafts(s: &pulse_store::Store, now: String, actor: String) -> Vec<String> {
    todo!("runner::auto_adopt::auto_adopt_drafts")
}
