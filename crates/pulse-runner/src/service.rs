#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/service.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) const SERVICE_LABEL: &str = "pulse.daemon";
pub(crate) const HUB_PORT: i32 = 8765;
pub(crate) const LAUNCHER_NAME: &str = "pulse-daemon.sh";
pub(crate) static SERVICE_SLEEP: LazyLock<RwLock<fn(std::time::Duration)>> =
    LazyLock::new(|| RwLock::new(std::thread::sleep));
pub(crate) fn set_service_sleep(f: fn(std::time::Duration)) {
    *SERVICE_SLEEP.write().expect("service sleep lock") = f;
}
pub(crate) type RunServiceFn = dyn Fn(Vec<String>) -> (i32, String) + Send + Sync;
pub(crate) static RUN_SERVICE: LazyLock<RwLock<Box<RunServiceFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(|argv| todo!("runner::service::run_service"))));
pub(crate) fn set_run_service(f: Box<RunServiceFn>) {
    *RUN_SERVICE.write().expect("run service lock") = f;
}
pub(crate) static PROTECTED_FOLDERS: &[&str] = &["Desktop", "Documents", "Downloads"];

#[derive(Debug, Clone, Default)]
pub(crate) struct ServiceUnit {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) unit: String,
    pub(crate) launcher: String,
    pub(crate) log: String,
    pub(crate) desc: String,
    pub(crate) args: Vec<String>,
    pub(crate) guard: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ServiceInstallOptions {
    pub interval: i32,
    pub hub_port: i32,
    pub no_hub: bool,
    pub dry_run: bool,
    pub defer_hub: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceInstall {
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "daemon_path")]
    pub daemon_path: String,
    #[serde(rename = "hub_path")]
    pub hub_path: String,
    #[serde(rename = "output")]
    pub output: String,
    #[serde(skip)]
    pub load_hub: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnitState {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "path")]
    pub path: String,
    #[serde(rename = "present")]
    pub present: bool,
    #[serde(rename = "loaded")]
    pub loaded: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceState {
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "supported")]
    pub supported: bool,
    #[serde(rename = "installed")]
    pub installed: bool,
    #[serde(rename = "units")]
    pub units: Vec<UnitState>,
    #[serde(rename = "binary")]
    pub binary: String,
    #[serde(rename = "logs")]
    pub logs: String,
}

/// Go: func daemonUnit(interval int) serviceUnit
pub(crate) fn daemon_unit(interval: i32) -> ServiceUnit {
    todo!("runner::service::daemon_unit")
}

/// Go: func hubUnit(port int) serviceUnit
pub(crate) fn hub_unit(port: i32) -> ServiceUnit {
    todo!("runner::service::hub_unit")
}

/// Go: func serviceUnits(interval, port int, noHub bool) []serviceUnit
pub(crate) fn service_units(interval: i32, port: i32, no_hub: bool) -> Vec<ServiceUnit> {
    todo!("runner::service::service_units")
}

/// Go: func servicePaths(system, home string) (string, string)
pub(crate) fn service_paths(system: String, home: String) -> (String, String) {
    todo!("runner::service::service_paths")
}

/// Go: func unitPaths(u serviceUnit, system, home string) (string, string)
pub(crate) fn unit_paths(u: ServiceUnit, system: String, home: String) -> (String, String) {
    todo!("runner::service::unit_paths")
}

/// Go: func xmlEsc(s string) string
pub(crate) fn xml_esc(s: String) -> String {
    todo!("runner::service::xml_esc")
}

/// Go: func serviceFileText(kind string, interval int, binary, logPath, pathEnv, pulseHome string) string
pub(crate) fn service_file_text(
    kind: String,
    interval: i32,
    binary: String,
    log_path: String,
    path_env: String,
    pulse_home: String,
) -> String {
    todo!("runner::service::service_file_text")
}

/// Go: func unitFileText(u serviceUnit, kind string, binary, logPath, pathEnv, pulseHome string) string
pub(crate) fn unit_file_text(
    u: ServiceUnit,
    kind: String,
    binary: String,
    log_path: String,
    path_env: String,
    pulse_home: String,
) -> String {
    todo!("runner::service::unit_file_text")
}

/// Go: func launcherPath(pulseHome string) string
pub(crate) fn launcher_path(pulse_home: String) -> String {
    todo!("runner::service::launcher_path")
}

/// Go: func unitLauncherPath(u serviceUnit, pulseHome string) string
pub(crate) fn unit_launcher_path(u: ServiceUnit, pulse_home: String) -> String {
    todo!("runner::service::unit_launcher_path")
}

/// Go: func launcherText(binary string, interval int) string
pub(crate) fn launcher_text(binary: String, interval: i32) -> String {
    todo!("runner::service::launcher_text")
}

/// Go: func unitLauncherText(u serviceUnit, binary string) string
pub(crate) fn unit_launcher_text(u: ServiceUnit, binary: String) -> String {
    todo!("runner::service::unit_launcher_text")
}

/// Go: func launcherFromPlist(text string) string
pub(crate) fn launcher_from_plist(text: String) -> String {
    todo!("runner::service::launcher_from_plist")
}

/// Go: func serviceCommands(kind, path, action string) [][]string
pub(crate) fn service_commands(kind: String, path: String, action: String) -> Vec<Vec<String>> {
    todo!("runner::service::service_commands")
}

/// Go: func unitCommands(u serviceUnit, kind, path, action string) [][]string
pub(crate) fn unit_commands(
    u: ServiceUnit,
    kind: String,
    path: String,
    action: String,
) -> Vec<Vec<String>> {
    todo!("runner::service::unit_commands")
}

/// Go: func pulseBinary() string
pub(crate) fn pulse_binary() -> String {
    todo!("runner::service::pulse_binary")
}

/// Go: func installService(out io.Writer, interval int, pulseHome, system, userHome string, dryRun bool) (string, error)
pub(crate) fn install_service_inner(
    out: &mut dyn std::io::Write,
    interval: i32,
    pulse_home: String,
    system: String,
    user_home: String,
    dry_run: bool,
) -> anyhow::Result<String> {
    todo!("runner::service::install_service")
}

/// Go: func installUnit(out io.Writer, u serviceUnit, pulseHome, system, userHome string, dryRun bool) (string, error)
pub(crate) fn install_unit(
    out: &mut dyn std::io::Write,
    u: ServiceUnit,
    pulse_home: String,
    system: String,
    user_home: String,
    dry_run: bool,
) -> anyhow::Result<String> {
    todo!("runner::service::install_unit")
}

/// Go: func prepareUnit(out io.Writer, u serviceUnit, pulseHome, system, userHome string, dryRun bool) (string, func() error, error)
pub(crate) fn prepare_unit(
    out: &mut dyn std::io::Write,
    u: ServiceUnit,
    pulse_home: String,
    system: String,
    user_home: String,
    dry_run: bool,
) -> anyhow::Result<(String, serde_json::Value)> {
    todo!("runner::service::prepare_unit")
}

/// Go: func loadUnit(out io.Writer, u serviceUnit, kind, path string) error
pub(crate) fn load_unit(
    out: &mut dyn std::io::Write,
    u: ServiceUnit,
    kind: String,
    path: String,
) -> anyhow::Result<()> {
    todo!("runner::service::load_unit")
}

/// Go: func InstallService(out io.Writer, pulseHome string, o ServiceInstallOptions) (*ServiceInstall, error)
pub fn install_service(
    out: &mut dyn std::io::Write,
    pulse_home: String,
    o: ServiceInstallOptions,
) -> anyhow::Result<Option<ServiceInstall>> {
    todo!("runner::service::install_service")
}

/// Go: func ServiceStatus(pulseHome string) ServiceState
pub fn service_status(pulse_home: String) -> ServiceState {
    todo!("runner::service::service_status")
}

/// Go: func sameFile(path, text string) bool
pub(crate) fn same_file(path: String, text: String) -> bool {
    todo!("runner::service::same_file")
}

/// Go: func UninstallService(out io.Writer) string
pub fn uninstall_service(out: &mut dyn std::io::Write) -> String {
    todo!("runner::service::uninstall_service")
}

/// Go: func uninstallService(out io.Writer, system, userHome string) string
pub(crate) fn uninstall_service_inner(
    out: &mut dyn std::io::Write,
    system: String,
    user_home: String,
) -> String {
    todo!("runner::service::uninstall_service")
}

/// Go: func serviceStatus(out io.Writer, s *store.Store, system, userHome string)
pub(crate) fn service_status_inner(
    out: &mut dyn std::io::Write,
    s: &pulse_store::Store,
    system: String,
    user_home: String,
) {
    todo!("runner::service::service_status")
}

/// Go: func protectedFoldersNote(s *store.Store, system, userHome string) string
pub(crate) fn protected_folders_note(
    s: &pulse_store::Store,
    system: String,
    user_home: String,
) -> String {
    todo!("runner::service::protected_folders_note")
}
