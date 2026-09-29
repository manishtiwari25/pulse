#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const ADMIN_USER: &str = "pulseadmin";
pub(crate) const HANDS_WORKDIR: &str = "/srv/pulse";
pub(crate) const HANDS_DISK_GB: &str = "64";
pub(crate) const HOSTKEY_MARKER: &str = "pulse-hostkey ";
pub(crate) const HOSTKEY_BLOCK_TOP: &str = "-----BEGIN SSH HOST KEY KEYS-----";
pub(crate) const HOSTKEY_BLOCK_END: &str = "-----END SSH HOST KEY KEYS-----";
pub(crate) const TAG_HANDS: &str = "pulse";
pub(crate) const TAG_REQUEST: &str = "pulse-request";
pub(crate) const TAG_PROJECT: &str = "pulse-project";
pub(crate) static PROVISION_WAIT: LazyLock<RwLock<Duration>> =
    LazyLock::new(|| RwLock::new(Duration::from_secs(20)));
pub(crate) fn set_provision_wait(value: Duration) -> Duration {
    std::mem::replace(
        &mut *PROVISION_WAIT.write().expect("provision_wait poisoned"),
        value,
    )
}
pub(crate) type CliRunner = dyn Fn(
        &pulse_bus::CancellationToken,
        Vec<String>,
        Duration,
    ) -> anyhow::Result<(i32, String, String)>
    + Send
    + Sync;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvisionRequest {
    pub id: String,
    pub project: String,
    pub size: String,
    pub hours: i32,
    pub reason: String,
    pub executor: String,
    #[serde(skip)]
    pub store: Option<pulse_store::Store>,
    pub actor: String,
    pub hub_home: String,
    #[serde(skip)]
    pub log: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Machine {
    #[serde(rename = "provider")]
    pub provider: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "executor")]
    pub executor: String,
    #[serde(rename = "address")]
    pub address: String,
    #[serde(rename = "tags", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
    #[serde(rename = "detail", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub detail: BTreeMap<String, String>,
}

pub trait Provider: Send + Sync {
    fn name(&self) -> String;
    fn kind(&self) -> String;
    fn provision(
        &self,
        cancel: &pulse_bus::CancellationToken,
        req: ProvisionRequest,
    ) -> anyhow::Result<Option<Box<Machine>>>;
    fn release(&self, cancel: &pulse_bus::CancellationToken, m: &Machine) -> anyhow::Result<()>;
    fn probe(&self, cancel: &pulse_bus::CancellationToken, m: &Machine) -> anyhow::Result<()>;
    fn find(
        &self,
        cancel: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        executor: &str,
        host: &str,
    ) -> anyhow::Result<Option<Box<Machine>>>;
    fn stop(&self, cancel: &pulse_bus::CancellationToken, m: &Machine) -> anyhow::Result<()>;
    fn delete(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &Machine,
    ) -> anyhow::Result<(bool, Vec<String>)>;
    fn place(&self, m: &Machine) -> String;
    fn start_hint(&self, m: &Machine) -> String;
}

pub static PROVIDERS: LazyLock<RwLock<Vec<Box<dyn Provider + Send + Sync>>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));
pub fn register_provider<P: Provider + Send + Sync + 'static>(p: P) {
    PROVIDERS
        .write()
        .expect("providers poisoned")
        .push(Box::new(p));
}

pub(crate) static PAIR_HANDS: LazyLock<
    RwLock<
        Box<
            dyn Fn(
                    &pulse_store::Store,
                    &str,
                    &str,
                    &str,
                    crate::pair::PairOpts,
                ) -> anyhow::Result<Option<Box<crate::pair::PairReport>>>
                + Send
                + Sync,
        >,
    >,
> = LazyLock::new(|| {
    RwLock::new(Box::new(|s, admin, name, now, o| {
        crate::pair::pair_executor(s, admin.to_string(), name.to_string(), now.to_string(), o)
    }))
});

pub(crate) fn set_pair_hands(
    f: Box<
        dyn Fn(
                &pulse_store::Store,
                &str,
                &str,
                &str,
                crate::pair::PairOpts,
            ) -> anyhow::Result<Option<Box<crate::pair::PairReport>>>
            + Send
            + Sync,
    >,
) -> Box<
    dyn Fn(
            &pulse_store::Store,
            &str,
            &str,
            &str,
            crate::pair::PairOpts,
        ) -> anyhow::Result<Option<Box<crate::pair::PairReport>>>
        + Send
        + Sync,
> {
    std::mem::replace(&mut *PAIR_HANDS.write().expect("pair_hands poisoned"), f)
}

/// Go: func cliCommand(program, missing string, env ...string) cliRunner
pub(crate) fn cli_command(program: String, missing: String, env: &[String]) -> Box<CliRunner> {
    todo!("runner::provider::cli_command")
}

/// Go: func cliErr(what string, rc int, stdout, stderr string, err error) error
pub(crate) fn cli_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    todo!("runner::provider::cli_err")
}

/// Go: func notFoundAnswer(stdout, stderr string) bool
pub(crate) fn not_found_answer(stdout: String, stderr: String) -> bool {
    todo!("runner::provider::not_found_answer")
}

/// Go: func cloudInitConfig(adminKey string, installCopilot bool) string
pub(crate) fn cloud_init_config(admin_key: String, install_copilot: bool) -> String {
    todo!("runner::provider::cloud_init_config")
}

/// Go: func writeCloudInit(adminKey string, installCopilot bool) (string, error)
pub(crate) fn write_cloud_init(admin_key: String, install_copilot: bool) -> anyhow::Result<String> {
    todo!("runner::provider::write_cloud_init")
}

/// Go: func hostKeyLines(text string) []string
pub(crate) fn host_key_lines(text: String) -> Vec<String> {
    todo!("runner::provider::host_key_lines")
}

/// Go: func provisionStart(provider string, req ProvisionRequest) (*store.Store, string, func(string, ...any), error)
pub(crate) fn provision_start(
    provider: String,
    req: crate::provider::ProvisionRequest,
) -> anyhow::Result<(
    Option<Box<pulse_store::Store>>,
    String,
    Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
)> {
    todo!("runner::provider::provision_start")
}

/// Go: func machineTags(req ProvisionRequest) map[string]string
pub(crate) fn machine_tags(req: crate::provider::ProvisionRequest) -> BTreeMap<String, String> {
    todo!("runner::provider::machine_tags")
}

/// Go: func machineName(requestID string) string
pub(crate) fn machine_name(request_id: String) -> String {
    todo!("runner::provider::machine_name")
}

/// Go: func writeAdminAlias(hubHome, alias, host string) error
pub(crate) fn write_admin_alias(
    hub_home: String,
    alias: String,
    host: String,
) -> anyhow::Result<()> {
    todo!("runner::provider::write_admin_alias")
}

/// Go: func waitCloudInit(ctx context.Context, admin string, log func(string, ...any)) error
pub(crate) fn wait_cloud_init(
    ctx: &pulse_bus::CancellationToken,
    admin: String,
    log: Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
) -> anyhow::Result<()> {
    todo!("runner::provider::wait_cloud_init")
}

/// Go: func provisionFinish(ctx context.Context, req ProvisionRequest, m *Machine, hubHome, ip string, hostkeys []string, log func(string, ...any)) error
pub(crate) fn provision_finish(
    ctx: &pulse_bus::CancellationToken,
    req: crate::provider::ProvisionRequest,
    m: &crate::provider::Machine,
    hub_home: String,
    ip: String,
    hostkeys: Vec<String>,
    log: Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
) -> anyhow::Result<()> {
    todo!("runner::provider::provision_finish")
}

/// Go: func sleepCtx(ctx context.Context, d time.Duration) bool
pub(crate) fn sleep_ctx(ctx: &pulse_bus::CancellationToken, d: std::time::Duration) -> bool {
    todo!("runner::provider::sleep_ctx")
}

/// Go: func pollUntil(ctx context.Context, what string, step func() (bool, error)) error
pub(crate) fn poll_until(
    ctx: &pulse_bus::CancellationToken,
    what: String,
    step: Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
) -> anyhow::Result<()> {
    todo!("runner::provider::poll_until")
}

/// Go: func cliList(out string) []string
pub(crate) fn cli_list(out: String) -> Vec<String> {
    todo!("runner::provider::cli_list")
}

/// Go: func machineFromDetail(provider, name, detailJSON, resourceGroup string) *Machine
pub(crate) fn machine_from_detail(
    provider: String,
    name: String,
    detail_json: String,
    resource_group: String,
) -> Option<Box<crate::provider::Machine>> {
    todo!("runner::provider::machine_from_detail")
}
