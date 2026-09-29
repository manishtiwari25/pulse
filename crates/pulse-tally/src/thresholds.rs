use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use crate::{amend::constitution_text, amend::stop_present, text::table_row};

pub const PROTECTED_DEFAULT: &str = "Hard stops, Kill switch, Amendments, frontmatter";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thresholds {
    pub quorum: i32,
    pub expiry_days: i32,
    pub stale_days: i32,
    pub amend_quorum: i32,
    pub amend_majority: String,
    pub now: String,
    pub amend_num: i32,
    pub amend_den: i32,
    pub protected: String,
    pub constitution_present: bool,
    pub stop_present: bool,
    #[serde(skip)]
    pub constitution: String,
}

static AMEND_QUORUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]+").unwrap());
static AMEND_MAJORITY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]+/[0-9]+").unwrap());

pub fn load_thresholds(s: &pulse_store::Store, project: &str, override_: Thresholds) -> Thresholds {
    let mut th = override_;
    let mut text = String::new();
    if !project.is_empty() && project != pulse_store::COMMONS_SLUG {
        text = constitution_text(s, project);
    }
    let mut protected = String::new();
    if !text.is_empty() {
        if th.amend_quorum == 0
            && let Some(cell) = table_row(&text, "Amendment quorum")
            && let Some(m) = AMEND_QUORUM_RE.find(&cell)
        {
            th.amend_quorum = m.as_str().parse().unwrap_or(0);
        }
        if th.amend_majority.is_empty()
            && let Some(cell) = table_row(&text, "Amendment supermajority")
            && let Some(m) = AMEND_MAJORITY_RE.find(&cell)
        {
            th.amend_majority = m.as_str().to_string();
        }
        protected = table_row(&text, "Not amendable by vote").unwrap_or_default();
    }
    if th.quorum == 0 {
        th.quorum = s.setting_int("quorum") as i32;
    }
    if th.expiry_days == 0 {
        th.expiry_days = s.setting_int("expiry_days") as i32;
    }
    if th.stale_days == 0 {
        th.stale_days = s.setting_int("stale_days") as i32;
    }
    if th.amend_quorum == 0 {
        th.amend_quorum = s.setting_int("amend_quorum") as i32;
    }
    if th.amend_majority.is_empty() {
        th.amend_majority = s.setting("amend_majority", "");
    }
    if protected.is_empty() {
        protected = PROTECTED_DEFAULT.to_string();
    }
    th.protected = protected;
    th.constitution_present = !text.is_empty();
    th.constitution = text;
    th.stop_present = stop_present(s, project);
    let (num, den) = parse_majority(&th.amend_majority).unwrap_or_else(|_| {
        th.amend_majority = pulse_store::DEFAULT_SETTINGS["amend_majority"].clone();
        parse_majority(&th.amend_majority).unwrap_or((2, 3))
    });
    th.amend_num = num;
    th.amend_den = den;
    th
}

pub fn parse_majority(value: &str) -> anyhow::Result<(i32, i32)> {
    let (n, d) = pulse_store::majority(value)?;
    Ok((n as i32, d as i32))
}
