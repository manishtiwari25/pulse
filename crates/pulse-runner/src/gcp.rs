#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const GCP_MACHINE_TYPE: &str = "e2-medium";
pub(crate) const GCP_IMAGE: &str = "ubuntu-2404-lts-amd64";
pub(crate) const GCP_IMAGE_PROJECT: &str = "ubuntu-os-cloud";
pub(crate) const GCP_FIREWALL: &str = "pulse-hands-ssh";
pub(crate) const GCP_NETWORK_TAG: &str = "pulse-hands";
pub(crate) const GCP_SHORT: Duration = Duration::from_secs(60);
pub(crate) const GCP_LONG: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GcpProvider {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GcpConfig {
    pub project: String,
    pub zone: String,
    pub machine_type: String,
    pub image: String,
    pub network: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GcpInstance {
    pub name: String,
    pub status: String,
    pub zone: String,
    #[serde(rename = "machineType")]
    pub machine_type: String,
    pub labels: BTreeMap<String, String>,
    pub network_interfaces: Vec<serde_json::Value>,
    pub access_configs: Vec<serde_json::Value>,
    #[serde(rename = "natIP")]
    pub nat_ip: String,
}

pub(crate) static RUN_GCLOUD: LazyLock<RwLock<Box<crate::provider::CliRunner>>> =
    LazyLock::new(|| {
        RwLock::new(Box::new(|cancel, args, timeout| {
            todo!("runner::gcp::run_gcloud")
        }))
    });

pub(crate) fn set_run_gcloud(
    runner: Box<crate::provider::CliRunner>,
) -> Box<crate::provider::CliRunner> {
    std::mem::replace(
        &mut *RUN_GCLOUD.write().expect("run_gcloud poisoned"),
        runner,
    )
}

pub(crate) fn init() {
    crate::provider::register_provider(GcpProvider {});
}

/// Go: func gcpSettings(s *store.Store, size string) gcpConfig
pub(crate) fn gcp_settings(s: &pulse_store::Store, size: String) -> GcpConfig {
    todo!("runner::gcp::gcp_settings")
}

/// Go: func gcpErr(what string, rc int, stdout, stderr string, err error) error
pub(crate) fn gcp_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    todo!("runner::gcp::gcp_err")
}

/// Go: func gcpJSON(ctx context.Context, what string, args []string, timeout time.Duration, v any) error
pub(crate) fn gcp_json(
    ctx: &pulse_bus::CancellationToken,
    what: String,
    args: Vec<String>,
    timeout: std::time::Duration,
    v: serde_json::Value,
) -> anyhow::Result<()> {
    todo!("runner::gcp::gcp_json")
}

/// Go: func gcpLabel(v string) string
pub(crate) fn gcp_label(v: String) -> String {
    todo!("runner::gcp::gcp_label")
}

/// Go: func gcpMachine(cfg gcpConfig, i gcpInstance) *Machine
pub(crate) fn gcp_machine(cfg: GcpConfig, i: GcpInstance) -> Option<Box<crate::provider::Machine>> {
    todo!("runner::gcp::gcp_machine")
}

/// Go: func gcpDescribe(ctx context.Context, cfg gcpConfig, name string) (*gcpInstance, error)
pub(crate) fn gcp_describe(
    ctx: &pulse_bus::CancellationToken,
    cfg: GcpConfig,
    name: String,
) -> anyhow::Result<Option<Box<GcpInstance>>> {
    todo!("runner::gcp::gcp_describe")
}

/// Go: func gcpFirewallRule(ctx context.Context, cfg gcpConfig, log func(string, ...any)) error
pub(crate) fn gcp_firewall_rule(
    ctx: &pulse_bus::CancellationToken,
    cfg: GcpConfig,
    log: Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
) -> anyhow::Result<()> {
    todo!("runner::gcp::gcp_firewall_rule")
}

impl GcpProvider {
    /// Go: func (gcpProvider) Name() string
    pub(crate) fn name(&self) -> String {
        todo!("runner::gcp::name")
    }
    /// Go: func (gcpProvider) Kind() string
    pub(crate) fn kind(&self) -> String {
        todo!("runner::gcp::kind")
    }
    /// Go: func (p gcpProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error)
    pub(crate) fn provision(
        &self,
        ctx: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        todo!("runner::gcp::provision")
    }
    /// Go: func (p gcpProvider) config(m *Machine) gcpConfig
    pub(crate) fn config(&self, m: &crate::provider::Machine) -> GcpConfig {
        todo!("runner::gcp::config")
    }
    /// Go: func (p gcpProvider) Release(ctx context.Context, m *Machine) error
    pub(crate) fn release(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::gcp::release")
    }
    /// Go: func (p gcpProvider) Probe(ctx context.Context, m *Machine) error
    pub(crate) fn probe(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::gcp::probe")
    }
    /// Go: func (p gcpProvider) Find(ctx context.Context, s *store.Store, executor, host string) (*Machine, error)
    pub(crate) fn find(
        &self,
        ctx: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        _value: serde_json::Value,
        host: String,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        todo!("runner::gcp::find")
    }
    /// Go: func (p gcpProvider) Stop(ctx context.Context, m *Machine) error
    pub(crate) fn stop(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::gcp::stop")
    }
    /// Go: func (p gcpProvider) Place(m *Machine) string
    pub(crate) fn place(&self, m: &crate::provider::Machine) -> String {
        todo!("runner::gcp::place")
    }
    /// Go: func (p gcpProvider) StartHint(m *Machine) string
    pub(crate) fn start_hint(&self, m: &crate::provider::Machine) -> String {
        todo!("runner::gcp::start_hint")
    }
    /// Go: func (p gcpProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error)
    pub(crate) fn delete(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        todo!("runner::gcp::delete")
    }
}

impl GcpConfig {
    /// Go: func (c gcpConfig) args(a ...string) []string
    pub(crate) fn args(&self, a: &[String]) -> Vec<String> {
        todo!("runner::gcp::args")
    }
    /// Go: func (c gcpConfig) scope(a []string) []string
    pub(crate) fn scope(&self, a: Vec<String>) -> Vec<String> {
        todo!("runner::gcp::scope")
    }
    /// Go: func (c gcpConfig) imageArgs() []string
    pub(crate) fn image_args(&self) -> Vec<String> {
        todo!("runner::gcp::image_args")
    }
}

impl GcpInstance {
    /// Go: func (i gcpInstance) publicIP() string
    pub(crate) fn public_ip(&self) -> String {
        todo!("runner::gcp::public_ip")
    }
    /// Go: func (i gcpInstance) zoneName() string
    pub(crate) fn zone_name(&self) -> String {
        todo!("runner::gcp::zone_name")
    }
}

impl crate::provider::Provider for GcpProvider {
    fn name(&self) -> String {
        "gcp".to_string()
    }
    fn kind(&self) -> String {
        self.kind()
    }
    fn provision(
        &self,
        cancel: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        self.provision(cancel, req)
    }
    fn release(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        self.release(cancel, m)
    }
    fn probe(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        self.probe(cancel, m)
    }
    fn find(
        &self,
        cancel: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        executor: &str,
        host: &str,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        todo!("runner::gcp::find")
    }
    fn stop(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        self.stop(cancel, m)
    }
    fn delete(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        self.delete(cancel, m)
    }
    fn place(&self, m: &crate::provider::Machine) -> String {
        self.place(m)
    }
    fn start_hint(&self, m: &crate::provider::Machine) -> String {
        self.start_hint(m)
    }
}
