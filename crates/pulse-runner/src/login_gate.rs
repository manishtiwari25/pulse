#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const LOGIN_GATE_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) static LOGIN_GATE: LazyLock<
    RwLock<Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync>>,
> = LazyLock::new(|| RwLock::new(Box::new(local_login_gate)));
pub(crate) static LOCAL_PROVIDERS: LazyLock<RwLock<BTreeMap<String, serde_json::Value>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

pub(crate) fn set_login_gate(
    f: Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync>,
) -> Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync> {
    std::mem::replace(&mut *LOGIN_GATE.write().expect("login_gate poisoned"), f)
}

/// Go: func localLoginGate(s *store.Store) string
pub(crate) fn local_login_gate(s: &pulse_store::Store) -> String {
    todo!("runner::login_gate::local_login_gate")
}

/// Go: func localProvider(home, name string, settings map[string]string) (provider.Provider, error)
pub(crate) fn local_provider(
    home: String,
    name: String,
    settings: BTreeMap<String, String>,
) -> anyhow::Result<Box<dyn pulse_provider::Provider>> {
    todo!("runner::login_gate::local_provider")
}

/// Go: func providerSettingsKey(settings map[string]string) string
pub(crate) fn provider_settings_key(settings: BTreeMap<String, String>) -> String {
    todo!("runner::login_gate::provider_settings_key")
}

/// Go: func loginWanted(s *store.Store, brain row, queued int, gate, now, actor string) row
pub(crate) fn login_wanted(
    s: &pulse_store::Store,
    brain: crate::util::Row,
    queued: i32,
    gate: String,
    now: String,
    actor: String,
) -> crate::util::Row {
    todo!("runner::login_gate::login_wanted")
}
