#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const HEAL_THREAD: &str = "executors";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RetiredVm {
    pub request: String,
}

pub(crate) static SSH_HOSTNAME: LazyLock<RwLock<Box<dyn Fn(&str) -> String + Send + Sync>>> =
    LazyLock::new(|| RwLock::new(Box::new(|host| todo!("runner::retire::ssh_hostname"))));

pub(crate) fn set_ssh_hostname(
    f: Box<dyn Fn(&str) -> String + Send + Sync>,
) -> Box<dyn Fn(&str) -> String + Send + Sync> {
    std::mem::replace(
        &mut *SSH_HOSTNAME.write().expect("ssh_hostname poisoned"),
        f,
    )
}

/// Go: func retireCandidates(s *store.Store) []row
pub(crate) fn retire_candidates(s: &pulse_store::Store) -> Vec<crate::util::Row> {
    todo!("runner::retire::retire_candidates")
}

/// Go: func retireProvider(s *store.Store) Provider
pub(crate) fn retire_provider(s: &pulse_store::Store) -> Box<dyn crate::provider::Provider> {
    todo!("runner::retire::retire_provider")
}

/// Go: func identifyVM(s *store.Store, r row) (*retiredVM, bool, string)
pub(crate) fn identify_vm(
    s: &pulse_store::Store,
    r: crate::util::Row,
) -> (Option<Box<RetiredVm>>, bool, String) {
    todo!("runner::retire::identify_vm")
}

/// Go: func tendExecutors(s *store.Store, now, actor string) []string
pub(crate) fn tend_executors(s: &pulse_store::Store, now: String, actor: String) -> Vec<String> {
    todo!("runner::retire::tend_executors")
}

/// Go: func healExecutor(s *store.Store, now, actor string, r row, identify func() (*retiredVM, bool)) (bool, string)
pub(crate) fn heal_executor(
    s: &pulse_store::Store,
    now: String,
    actor: String,
    r: crate::util::Row,
    identify: Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
) -> (bool, String) {
    todo!("runner::retire::heal_executor")
}

/// Go: func adminRoute(address, ip string) string
pub(crate) fn admin_route(address: String, ip: String) -> String {
    todo!("runner::retire::admin_route")
}

/// Go: func machineDetail(m *Machine, extra map[string]any) map[string]any
pub(crate) fn machine_detail(
    m: &crate::provider::Machine,
    extra: BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    todo!("runner::retire::machine_detail")
}

/// Go: func saveRetired(s *store.Store, executor string, m *Machine, now, deleteAfter string)
pub(crate) fn save_retired(
    s: &pulse_store::Store,
    executor: String,
    m: &crate::provider::Machine,
    now: String,
    delete_after: String,
) -> () {
    todo!("runner::retire::save_retired")
}

/// Go: func retiredMachine(r row) *Machine
pub(crate) fn retired_machine(r: crate::util::Row) -> Option<Box<crate::provider::Machine>> {
    todo!("runner::retire::retired_machine")
}

/// Go: func retireExecutor(s *store.Store, now, actor string, r row, vm *retiredVM, ours bool, note string) []string
pub(crate) fn retire_executor(
    s: &pulse_store::Store,
    now: String,
    actor: String,
    r: crate::util::Row,
    vm: &RetiredVm,
    ours: bool,
    note: String,
) -> Vec<String> {
    todo!("runner::retire::retire_executor")
}

/// Go: func deleteRetired(s *store.Store, now, actor string) []string
pub(crate) fn delete_retired(s: &pulse_store::Store, now: String, actor: String) -> Vec<String> {
    todo!("runner::retire::delete_retired")
}
