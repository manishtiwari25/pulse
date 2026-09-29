#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const AZURE_ADMIN_USER: &str = crate::provider::ADMIN_USER;
pub(crate) const AZURE_WORKDIR: &str = crate::provider::HANDS_WORKDIR;
pub(crate) const AZURE_DISK_GB: &str = crate::provider::HANDS_DISK_GB;
pub(crate) const AZURE_IMAGE: &str = "Ubuntu2404";
pub(crate) const AZURE_VM_SIZE: &str = "Standard_B2ms";
pub(crate) const AZ_SHORT: Duration = Duration::from_secs(60);
pub(crate) const AZ_LONG: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AzureProvider {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AzureConfig {
    pub subscription: String,
    pub resource_group: String,
    pub location: String,
    pub vm_size: String,
    pub image: String,
}

pub(crate) static RUN_AZ: LazyLock<RwLock<Box<crate::provider::CliRunner>>> = LazyLock::new(|| {
    RwLock::new(Box::new(|cancel, args, timeout| {
        todo!("runner::azure::run_az")
    }))
});

pub(crate) fn set_run_az(
    runner: Box<crate::provider::CliRunner>,
) -> Box<crate::provider::CliRunner> {
    std::mem::replace(&mut *RUN_AZ.write().expect("run_az poisoned"), runner)
}

pub(crate) fn init() {
    crate::provider::register_provider(AzureProvider {});
}

/// Go: func azureSettings(s *store.Store, size string) (azureConfig, error)
pub(crate) fn azure_settings(s: &pulse_store::Store, size: String) -> anyhow::Result<AzureConfig> {
    todo!("runner::azure::azure_settings")
}

/// Go: func azureVMName(requestID string) string
pub(crate) fn azure_vm_name(request_id: String) -> String {
    todo!("runner::azure::azure_vm_name")
}

/// Go: func azErr(what string, rc int, stdout, stderr string, err error) error
pub(crate) fn az_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    todo!("runner::azure::az_err")
}

impl AzureProvider {
    /// Go: func (azureProvider) Name() string
    pub(crate) fn name(&self) -> String {
        todo!("runner::azure::name")
    }
    /// Go: func (azureProvider) Kind() string
    pub(crate) fn kind(&self) -> String {
        todo!("runner::azure::kind")
    }
    /// Go: func (p azureProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error)
    pub(crate) fn provision(
        &self,
        ctx: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        todo!("runner::azure::provision")
    }
    /// Go: func (p azureProvider) config(m *Machine) azureConfig
    pub(crate) fn config(&self, m: &crate::provider::Machine) -> AzureConfig {
        todo!("runner::azure::config")
    }
    /// Go: func (p azureProvider) Release(ctx context.Context, m *Machine) error
    pub(crate) fn release(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::azure::release")
    }
    /// Go: func (p azureProvider) Probe(ctx context.Context, m *Machine) error
    pub(crate) fn probe(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::azure::probe")
    }
    /// Go: func (p azureProvider) Find(ctx context.Context, _ *store.Store, executor, host string) (*Machine, error)
    pub(crate) fn find(
        &self,
        ctx: &pulse_bus::CancellationToken,
        _unused: &pulse_store::Store,
        _value: serde_json::Value,
        host: String,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        todo!("runner::azure::find")
    }
    /// Go: func (p azureProvider) Stop(ctx context.Context, m *Machine) error
    pub(crate) fn stop(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::azure::stop")
    }
    /// Go: func (p azureProvider) Place(m *Machine) string
    pub(crate) fn place(&self, m: &crate::provider::Machine) -> String {
        todo!("runner::azure::place")
    }
    /// Go: func (p azureProvider) StartHint(m *Machine) string
    pub(crate) fn start_hint(&self, m: &crate::provider::Machine) -> String {
        todo!("runner::azure::start_hint")
    }
    /// Go: func (p azureProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error)
    pub(crate) fn delete(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        todo!("runner::azure::delete")
    }
}

impl AzureConfig {
    /// Go: func (c azureConfig) args(a ...string) []string
    pub(crate) fn args(&self, a: &[String]) -> Vec<String> {
        todo!("runner::azure::args")
    }
}

impl crate::provider::Provider for AzureProvider {
    fn name(&self) -> String {
        "azure".to_string()
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
        todo!("runner::azure::find")
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
