#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) type AmendRefusals = BTreeMap<String, String>;

/// Go: func autoApplyAmendments(s *store.Store, slug, now string, refused amendRefusals) []string
pub(crate) fn auto_apply_amendments(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    refused: AmendRefusals,
) -> Vec<String> {
    todo!("runner::amend::auto_apply_amendments")
}
