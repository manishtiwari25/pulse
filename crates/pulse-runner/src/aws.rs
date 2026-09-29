#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const AWS_INSTANCE_TYPE: &str = "t3.medium";
pub(crate) const AWS_IMAGE: &str =
    "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id";
pub(crate) const AWS_GROUP_NAME: &str = "pulse-hands";
pub(crate) const AWS_SHORT: Duration = Duration::from_secs(60);
pub(crate) const AWS_LONG: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AwsProvider {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AwsConfig {
    pub profile: String,
    pub region: String,
    pub instance_type: String,
    pub image: String,
    pub subnet: String,
    pub security_group: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AwsInstance {
    #[serde(rename = "InstanceId")]
    pub instance_id: String,
    #[serde(rename = "PublicIpAddress")]
    pub public_ip: String,
    #[serde(rename = "InstanceType")]
    pub r#type: String,
    #[serde(rename = "SubnetId")]
    pub subnet_id: String,
    pub state: serde_json::Value,
    #[serde(rename = "Name")]
    pub name: String,
    pub tags: Vec<serde_json::Value>,
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(rename = "Value")]
    pub value: String,
    pub security_groups: Vec<serde_json::Value>,
    #[serde(rename = "GroupId")]
    pub group_id: String,
}

pub(crate) static RUN_AWS: LazyLock<RwLock<Box<crate::provider::CliRunner>>> =
    LazyLock::new(|| {
        RwLock::new(Box::new(|cancel, args, timeout| {
            todo!("runner::aws::run_aws")
        }))
    });

pub(crate) fn set_run_aws(
    runner: Box<crate::provider::CliRunner>,
) -> Box<crate::provider::CliRunner> {
    std::mem::replace(&mut *RUN_AWS.write().expect("run_aws poisoned"), runner)
}

pub(crate) fn init() {
    crate::provider::register_provider(AwsProvider {});
}

/// Go: func awsSettings(s *store.Store, size string) awsConfig
pub(crate) fn aws_settings(s: &pulse_store::Store, size: String) -> AwsConfig {
    todo!("runner::aws::aws_settings")
}

/// Go: func awsErr(what string, rc int, stdout, stderr string, err error) error
pub(crate) fn aws_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    todo!("runner::aws::aws_err")
}

/// Go: func awsJSON(ctx context.Context, what string, args []string, timeout time.Duration, v any) error
pub(crate) fn aws_json(
    ctx: &pulse_bus::CancellationToken,
    what: String,
    args: Vec<String>,
    timeout: std::time::Duration,
    v: serde_json::Value,
) -> anyhow::Result<()> {
    todo!("runner::aws::aws_json")
}

/// Go: func awsDescribe(ctx context.Context, cfg awsConfig, filters ...string) ([]awsInstance, error)
pub(crate) fn aws_describe(
    ctx: &pulse_bus::CancellationToken,
    cfg: AwsConfig,
    filters: &[String],
) -> anyhow::Result<Vec<AwsInstance>> {
    todo!("runner::aws::aws_describe")
}

/// Go: func awsMachine(cfg awsConfig, i awsInstance) *Machine
pub(crate) fn aws_machine(cfg: AwsConfig, i: AwsInstance) -> Option<Box<crate::provider::Machine>> {
    todo!("runner::aws::aws_machine")
}

/// Go: func awsNetwork(ctx context.Context, cfg awsConfig, log func(string, ...any)) (subnet, group string, err error)
pub(crate) fn aws_network(
    ctx: &pulse_bus::CancellationToken,
    cfg: AwsConfig,
    log: Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
) -> anyhow::Result<(String, String)> {
    todo!("runner::aws::aws_network")
}

impl AwsProvider {
    /// Go: func (awsProvider) Name() string
    pub(crate) fn name(&self) -> String {
        todo!("runner::aws::name")
    }
    /// Go: func (awsProvider) Kind() string
    pub(crate) fn kind(&self) -> String {
        todo!("runner::aws::kind")
    }
    /// Go: func (p awsProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error)
    pub(crate) fn provision(
        &self,
        ctx: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        todo!("runner::aws::provision")
    }
    /// Go: func (p awsProvider) config(m *Machine) awsConfig
    pub(crate) fn config(&self, m: &crate::provider::Machine) -> AwsConfig {
        todo!("runner::aws::config")
    }
    /// Go: func (p awsProvider) Release(ctx context.Context, m *Machine) error
    pub(crate) fn release(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::aws::release")
    }
    /// Go: func (p awsProvider) Probe(ctx context.Context, m *Machine) error
    pub(crate) fn probe(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::aws::probe")
    }
    /// Go: func (p awsProvider) Find(ctx context.Context, s *store.Store, executor, host string) (*Machine, error)
    pub(crate) fn find(
        &self,
        ctx: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        _value: serde_json::Value,
        host: String,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        todo!("runner::aws::find")
    }
    /// Go: func (p awsProvider) Stop(ctx context.Context, m *Machine) error
    pub(crate) fn stop(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        todo!("runner::aws::stop")
    }
    /// Go: func (p awsProvider) Place(m *Machine) string
    pub(crate) fn place(&self, m: &crate::provider::Machine) -> String {
        todo!("runner::aws::place")
    }
    /// Go: func (p awsProvider) StartHint(m *Machine) string
    pub(crate) fn start_hint(&self, m: &crate::provider::Machine) -> String {
        todo!("runner::aws::start_hint")
    }
    /// Go: func (p awsProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error)
    pub(crate) fn delete(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        todo!("runner::aws::delete")
    }
}

impl AwsConfig {
    /// Go: func (c awsConfig) scope(a []string) []string
    pub(crate) fn scope(&self, a: Vec<String>) -> Vec<String> {
        todo!("runner::aws::scope")
    }
    /// Go: func (c awsConfig) args(a ...string) []string
    pub(crate) fn args(&self, a: &[String]) -> Vec<String> {
        todo!("runner::aws::args")
    }
    /// Go: func (c awsConfig) imageArg() string
    pub(crate) fn image_arg(&self) -> String {
        todo!("runner::aws::image_arg")
    }
}

impl AwsInstance {
    /// Go: func (i awsInstance) tag(key string) string
    pub(crate) fn tag(&self, key: String) -> String {
        todo!("runner::aws::tag")
    }
}

impl crate::provider::Provider for AwsProvider {
    fn name(&self) -> String {
        "aws".to_string()
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
        todo!("runner::aws::find")
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
