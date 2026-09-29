#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const MULTIPASS_SIZE: &str = "cpus=2,mem=4G,disk=32G";
pub(crate) const MULTIPASS_IMAGE: &str = "24.04";
pub(crate) const MULTIPASS_SHORT: Duration = Duration::from_secs(60);
pub(crate) const MULTIPASS_LONG: Duration = Duration::from_secs(20 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MultipassProvider {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MultipassConfig {
    pub cp_us: String,
    pub memory: String,
    pub disk: String,
    pub image: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MultipassInfo {
    pub state: String,
    #[serde(rename = "ipv4")]
    pub i_pv4: Vec<String>,
    #[serde(rename = "image_release")]
    pub image: String,
}

pub(crate) static RUN_MULTIPASS: LazyLock<RwLock<Box<crate::provider::CliRunner>>> =
    LazyLock::new(|| {
        RwLock::new(Box::new(|cancel, args, timeout| {
            todo!("runner::multipass::run_multipass")
        }))
    });

pub(crate) fn set_run_multipass(
    runner: Box<crate::provider::CliRunner>,
) -> Box<crate::provider::CliRunner> {
    std::mem::replace(
        &mut *RUN_MULTIPASS.write().expect("run_multipass poisoned"),
        runner,
    )
}

pub(crate) fn init() {
    crate::provider::register_provider(MultipassProvider {});
}

/// Go: func multipassSettings(s *store.Store, size string) (multipassConfig, error)
pub(crate) fn multipass_settings(
    s: &pulse_store::Store,
    size: String,
) -> anyhow::Result<MultipassConfig> {
    todo!("runner::multipass::multipass_settings")
}

/// Go: func multipassBytes(v string) bool
pub(crate) fn multipass_bytes(v: String) -> bool {
    todo!("runner::multipass::multipass_bytes")
}

/// Go: func multipassErr(what string, rc int, stdout, stderr string, err error) error
pub(crate) fn multipass_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    todo!("runner::multipass::multipass_err")
}

/// Go: func multipassDescribe(ctx context.Context, name string) (*multipassInfo, error)
pub(crate) fn multipass_describe(
    ctx: &pulse_bus::CancellationToken,
    name: String,
) -> anyhow::Result<Option<Box<MultipassInfo>>> {
    todo!("runner::multipass::multipass_describe")
}

impl MultipassProvider {
    /// Go: func (multipassProvider) Name() string
    pub(crate) fn name(&self) -> String {
        todo!("runner::multipass::name")
    }
    /// Go: func (multipassProvider) Kind() string
    pub(crate) fn kind(&self) -> String {
        todo!("runner::multipass::kind")
    }
    /// Go: func (p multipassProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error)
    pub(crate) fn provision(
        &self,
        ctx: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        todo!("runner::multipass::provision")
    }
    /// Go: func (p multipassProvider) Release(ctx context.Context, m *Machine) error
    pub(crate) fn release(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::multipass::release")
    }
    /// Go: func (p multipassProvider) Probe(ctx context.Context, m *Machine) error
    pub(crate) fn probe(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::multipass::probe")
    }
    /// Go: func (p multipassProvider) Find(ctx context.Context, _ *store.Store, executor, host string) (*Machine, error)
    pub(crate) fn find(
        &self,
        ctx: &pulse_bus::CancellationToken,
        _unused: &pulse_store::Store,
        _value: serde_json::Value,
        host: String,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        todo!("runner::multipass::find")
    }
    /// Go: func (p multipassProvider) Stop(ctx context.Context, m *Machine) error
    pub(crate) fn stop(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::multipass::stop")
    }
    /// Go: func (p multipassProvider) Place(*Machine) string
    pub(crate) fn place(&self, _value: &crate::provider::Machine) -> String {
        todo!("runner::multipass::place")
    }
    /// Go: func (p multipassProvider) StartHint(m *Machine) string
    pub(crate) fn start_hint(&self, m: &crate::provider::Machine) -> String {
        todo!("runner::multipass::start_hint")
    }
    /// Go: func (p multipassProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error)
    pub(crate) fn delete(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        todo!("runner::multipass::delete")
    }
}

impl MultipassConfig {
    /// Go: func (c *multipassConfig) parseSize(size string) error
    pub(crate) fn parse_size(&mut self, size: String) -> anyhow::Result<()> {
        todo!("runner::multipass::parse_size")
    }
}

impl MultipassInfo {
    /// Go: func (i multipassInfo) ip() string
    pub(crate) fn ip(&self) -> String {
        todo!("runner::multipass::ip")
    }
}

impl crate::provider::Provider for MultipassProvider {
    fn name(&self) -> String {
        "multipass".to_string()
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
        todo!("runner::multipass::find")
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
