#![allow(dead_code)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
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
    RwLock::new(crate::provider::cli_command(
        "az".into(),
        "the Azure CLI (az) is not installed on this machine".into(),
        &[
            "AZURE_CORE_NO_COLOR=true".into(),
            "AZURE_CORE_ONLY_SHOW_ERRORS=true".into(),
        ],
    ))
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
    let mut cfg = AzureConfig {
        subscription: s.setting("compute.azure.subscription", "").trim().into(),
        resource_group: s.setting("compute.azure.resource_group", "").trim().into(),
        location: s.setting("compute.azure.location", "").trim().into(),
        vm_size: size.trim().into(),
        image: s.setting("compute.azure.image", AZURE_IMAGE).trim().into(),
    };
    if cfg.vm_size.is_empty() {
        cfg.vm_size = s
            .setting("compute.azure.vm_size", AZURE_VM_SIZE)
            .trim()
            .into();
    }
    if cfg.image.is_empty() {
        cfg.image = AZURE_IMAGE.into();
    }
    let missing = [
        ("compute.azure.resource_group", cfg.resource_group.as_str()),
        ("compute.azure.location", cfg.location.as_str()),
    ]
    .into_iter()
    .filter_map(|(k, v)| if v.is_empty() { Some(k) } else { None })
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(anyhow!(
            "azure is not configured: set {} (pulse settings set <name> <value>)",
            missing.join(" and ")
        ));
    }
    Ok(cfg)
}

/// Go: func azureVMName(requestID string) string
pub(crate) fn azure_vm_name(request_id: String) -> String {
    crate::provider::machine_name(request_id)
}

/// Go: func azErr(what string, rc int, stdout, stderr string, err error) error
pub(crate) fn az_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    crate::provider::cli_err(what, rc, stdout, stderr, err)
}

impl AzureProvider {
    /// Go: func (azureProvider) Name() string
    pub(crate) fn name(&self) -> String {
        "azure".into()
    }
    /// Go: func (azureProvider) Kind() string
    pub(crate) fn kind(&self) -> String {
        "Azure VM".into()
    }
    /// Go: func (p azureProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error)
    pub(crate) fn provision(
        &self,
        ctx: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        let (s, hub_home, log) = crate::provider::provision_start("azure".into(), req.clone())?;
        let cfg = azure_settings(&s, req.size.clone())?;
        let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
            ctx,
            cfg.args(&[
                "account".into(),
                "show".into(),
                "--query".into(),
                "name".into(),
                "-o".into(),
                "tsv".into(),
            ]),
            AZ_SHORT,
        )?;
        if rc != 0 {
            return az_err(
                "not logged in to Azure (run `az login` on this machine as the daemon's user)"
                    .into(),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| None);
        }
        log("azure subscription: %s", &[json!(out.trim())]);
        let mut lines = Vec::new();
        crate::pair::pair_ensure_hub_key(hub_home.clone(), &mut lines)?;
        let pub_path = format!("{hub_home}/.ssh/pulse_hands_ed25519.pub");
        let vm = azure_vm_name(req.id.clone());
        let mut m = crate::provider::Machine {
            provider: "azure".into(),
            name: vm.clone(),
            executor: req.executor.clone(),
            tags: crate::provider::machine_tags(req.clone()),
            detail: BTreeMap::from([
                ("resource_group".into(), cfg.resource_group.clone()),
                ("location".into(), cfg.location.clone()),
                ("size".into(), cfg.vm_size.clone()),
                ("image".into(), cfg.image.clone()),
                ("subscription".into(), cfg.subscription.clone()),
            ]),
            ..Default::default()
        };
        if let Ok((rc, _, _)) = (RUN_AZ.read().unwrap())(
            ctx,
            cfg.args(&[
                "group".into(),
                "show".into(),
                "--name".into(),
                cfg.resource_group.clone(),
                "-o".into(),
                "none".into(),
            ]),
            AZ_SHORT,
        ) {
            if rc != 0 {
                let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
                    ctx,
                    cfg.args(&[
                        "group".into(),
                        "create".into(),
                        "--name".into(),
                        cfg.resource_group.clone(),
                        "--location".into(),
                        cfg.location.clone(),
                        "--tags".into(),
                        format!("{}=hands", crate::provider::TAG_HANDS),
                        "-o".into(),
                        "none".into(),
                    ]),
                    Duration::from_secs(120),
                )?;
                if rc != 0 {
                    return az_err(
                        format!("az group create {}", cfg.resource_group),
                        rc,
                        out,
                        err_text,
                        anyhow!(""),
                    )
                    .map(|_| None);
                }
                log(
                    "created resource group %s in %s",
                    &[json!(cfg.resource_group), json!(cfg.location)],
                );
            }
        }
        let rendered = crate::provider::write_cloud_init(String::new(), !s.harness_native())?;
        let show = (RUN_AZ.read().unwrap())(
            ctx,
            cfg.args(&[
                "vm".into(),
                "show".into(),
                "--resource-group".into(),
                cfg.resource_group.clone(),
                "--name".into(),
                vm.clone(),
                "-o".into(),
                "none".into(),
            ]),
            AZ_SHORT,
        )?;
        if show.0 != 0 {
            log(
                "az vm create %s (%s, %s, %s GiB)",
                &[
                    json!(vm),
                    json!(cfg.vm_size),
                    json!(cfg.image),
                    json!(AZURE_DISK_GB),
                ],
            );
            let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
                ctx,
                cfg.args(&[
                    "vm".into(),
                    "create".into(),
                    "--resource-group".into(),
                    cfg.resource_group.clone(),
                    "--name".into(),
                    vm.clone(),
                    "--location".into(),
                    cfg.location.clone(),
                    "--image".into(),
                    cfg.image.clone(),
                    "--size".into(),
                    cfg.vm_size.clone(),
                    "--admin-username".into(),
                    AZURE_ADMIN_USER.into(),
                    "--ssh-key-values".into(),
                    pub_path,
                    "--custom-data".into(),
                    rendered.clone(),
                    "--public-ip-sku".into(),
                    "Standard".into(),
                    "--os-disk-size-gb".into(),
                    AZURE_DISK_GB.into(),
                    "--storage-sku".into(),
                    "StandardSSD_LRS".into(),
                    "--os-disk-delete-option".into(),
                    "Delete".into(),
                    "--nic-delete-option".into(),
                    "Delete".into(),
                    "--tags".into(),
                    format!("{}=hands", crate::provider::TAG_HANDS),
                    format!("{}={}", crate::provider::TAG_REQUEST, req.id),
                    format!("{}={}", crate::provider::TAG_PROJECT, req.project),
                    "-o".into(),
                    "json".into(),
                ]),
                Duration::from_secs(20 * 60),
            )?;
            if rc != 0 {
                return az_err(format!("az vm create {vm}"), rc, out, err_text, anyhow!(""))
                    .map(|_| Some(Box::new(m)));
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&out) {
                if let Some(ip) = v.get("publicIpAddress").and_then(|v| v.as_str()) {
                    m.address = format!("{AZURE_ADMIN_USER}@{ip}");
                }
            }
        } else {
            log("vm %s exists; not recreated", &[json!(vm)]);
        }
        let mut ip = m
            .address
            .trim_start_matches(&format!("{AZURE_ADMIN_USER}@"))
            .to_string();
        for try_no in 0..6 {
            if !ip.is_empty() {
                break;
            }
            if try_no > 0
                && !crate::provider::sleep_ctx(
                    ctx,
                    *crate::provider::PROVISION_WAIT.read().unwrap(),
                )
            {
                return Err(anyhow!("context canceled"));
            }
            let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
                ctx,
                cfg.args(&[
                    "vm".into(),
                    "show".into(),
                    "--resource-group".into(),
                    cfg.resource_group.clone(),
                    "--name".into(),
                    vm.clone(),
                    "-d".into(),
                    "--query".into(),
                    "publicIps".into(),
                    "-o".into(),
                    "tsv".into(),
                ]),
                AZ_SHORT,
            )?;
            if rc != 0 {
                return az_err(format!("az vm show {vm}"), rc, out, err_text, anyhow!(""))
                    .map(|_| Some(Box::new(m)));
            }
            ip = out.trim().split(',').next().unwrap_or("").to_string();
        }
        if ip.is_empty() {
            return Err(anyhow!("vm {} has no public address", vm));
        }
        let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
            ctx,
            cfg.args(&[
                "vm".into(),
                "run-command".into(),
                "invoke".into(),
                "--resource-group".into(),
                cfg.resource_group.clone(),
                "--name".into(),
                vm.clone(),
                "--command-id".into(),
                "RunShellScript".into(),
                "--scripts".into(),
                "cat /etc/ssh/ssh_host_ed25519_key.pub".into(),
                "--query".into(),
                "value[0].message".into(),
                "-o".into(),
                "tsv".into(),
            ]),
            Duration::from_secs(10 * 60),
        )?;
        let _ = std::fs::remove_file(rendered);
        if rc != 0 {
            return az_err(
                format!("az vm run-command invoke (host key) {vm}"),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| Some(Box::new(m)));
        }
        let hostkeys = crate::provider::host_key_lines(out);
        if hostkeys.is_empty() {
            return Err(anyhow!("vm {} gave no ssh host key", vm));
        }
        crate::provider::provision_finish(ctx, req, &m, hub_home, ip, hostkeys, log)?;
        Ok(Some(Box::new(m)))
    }
    /// Go: func (p azureProvider) config(m *Machine) azureConfig
    pub(crate) fn config(&self, m: &crate::provider::Machine) -> AzureConfig {
        AzureConfig {
            subscription: m.detail.get("subscription").cloned().unwrap_or_default(),
            resource_group: m.detail.get("resource_group").cloned().unwrap_or_default(),
            location: m.detail.get("location").cloned().unwrap_or_default(),
            ..Default::default()
        }
    }
    /// Go: func (p azureProvider) Release(ctx context.Context, m *Machine) error
    pub(crate) fn release(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        if m.name.is_empty() {
            return Ok(());
        }
        let cfg = self.config(m);
        if cfg.resource_group.is_empty() {
            return Err(anyhow!("azure: the machine's resource group is unknown"));
        }
        if let Ok((0, _, _)) = (RUN_AZ.read().unwrap())(
            ctx,
            cfg.args(&[
                "vm".into(),
                "show".into(),
                "--resource-group".into(),
                cfg.resource_group.clone(),
                "--name".into(),
                m.name.clone(),
                "-o".into(),
                "none".into(),
            ]),
            AZ_SHORT,
        ) {
            let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
                ctx,
                cfg.args(&[
                    "vm".into(),
                    "delete".into(),
                    "--resource-group".into(),
                    cfg.resource_group.clone(),
                    "--name".into(),
                    m.name.clone(),
                    "--yes".into(),
                    "-o".into(),
                    "none".into(),
                ]),
                AZ_LONG,
            )?;
            if rc != 0 {
                return az_err(
                    format!("az vm delete {}", m.name),
                    rc,
                    out,
                    err_text,
                    anyhow!(""),
                );
            }
        }
        Ok(())
    }
    /// Go: func (p azureProvider) Probe(ctx context.Context, m *Machine) error
    pub(crate) fn probe(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        if m.name.is_empty() {
            return Err(anyhow!("azure: no machine"));
        }
        let cfg = self.config(m);
        let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
            ctx,
            cfg.args(&[
                "vm".into(),
                "show".into(),
                "--resource-group".into(),
                cfg.resource_group.clone(),
                "--name".into(),
                m.name.clone(),
                "-d".into(),
                "--query".into(),
                "powerState".into(),
                "-o".into(),
                "tsv".into(),
            ]),
            AZ_SHORT,
        )?;
        if rc != 0 {
            return az_err(
                format!("az vm show {}", m.name),
                rc,
                out,
                err_text,
                anyhow!(""),
            );
        }
        if out.trim() != "VM running" {
            return Err(anyhow!(
                "{} is {}",
                m.name,
                crate::board::py_repr(out.trim().into())
            ));
        }
        Ok(())
    }
    /// Go: func (p azureProvider) Find(ctx context.Context, _ *store.Store, executor, host string) (*Machine, error)
    pub(crate) fn find(
        &self,
        ctx: &pulse_bus::CancellationToken,
        _unused: &pulse_store::Store,
        executor: String,
        host: String,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        let query = format!(
            "[?tags.pulse=='hands' && (name=='{}' || publicIps=='{}')].{{name:name,rg:resourceGroup,ip:publicIps}}",
            executor, host
        );
        let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
            ctx,
            vec![
                "vm".into(),
                "list".into(),
                "-d".into(),
                "--query".into(),
                query,
                "-o".into(),
                "json".into(),
            ],
            AZ_SHORT,
        )?;
        if rc != 0 {
            return Err(anyhow!(
                "the Azure CLI could not answer: {}",
                crate::run::err_message(
                    &az_err("az vm list".into(), rc, out, err_text, anyhow!("")).unwrap_err()
                )
            ));
        }
        #[derive(Deserialize, Default)]
        struct Found {
            name: String,
            rg: String,
            ip: String,
        }
        let found: Vec<Found> = serde_json::from_str(out.trim()).unwrap_or_default();
        let Some(f) = found.first() else {
            return Ok(None);
        };
        Ok(Some(Box::new(crate::provider::Machine {
            provider: "azure".into(),
            name: f.name.clone(),
            tags: BTreeMap::from([(crate::provider::TAG_HANDS.into(), "hands".into())]),
            detail: BTreeMap::from([
                ("resource_group".into(), f.rg.clone()),
                ("public_ip".into(), f.ip.clone()),
            ]),
            ..Default::default()
        })))
    }
    /// Go: func (p azureProvider) Stop(ctx context.Context, m *Machine) error
    pub(crate) fn stop(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
            ctx,
            vec![
                "vm".into(),
                "deallocate".into(),
                "-g".into(),
                m.detail.get("resource_group").cloned().unwrap_or_default(),
                "-n".into(),
                m.name.clone(),
                "-o".into(),
                "none".into(),
            ],
            AZ_LONG,
        )?;
        if rc != 0 {
            return az_err(
                format!("az vm deallocate {}", m.name),
                rc,
                out,
                err_text,
                anyhow!(""),
            );
        }
        Ok(())
    }
    /// Go: func (p azureProvider) Place(m *Machine) string
    pub(crate) fn place(&self, m: &crate::provider::Machine) -> String {
        m.detail.get("resource_group").cloned().unwrap_or_default()
    }
    /// Go: func (p azureProvider) StartHint(m *Machine) string
    pub(crate) fn start_hint(&self, m: &crate::provider::Machine) -> String {
        format!(
            "az vm start -g {} -n {}",
            m.detail.get("resource_group").cloned().unwrap_or_default(),
            m.name
        )
    }
    /// Go: func (p azureProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error)
    pub(crate) fn delete(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        let rg = m.detail.get("resource_group").cloned().unwrap_or_default();
        let name = m.name.clone();
        let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
            ctx,
            vec![
                "vm".into(),
                "show".into(),
                "-g".into(),
                rg.clone(),
                "-n".into(),
                name.clone(),
                "-o".into(),
                "json".into(),
            ],
            AZ_SHORT,
        )?;
        if rc != 0 {
            if crate::provider::not_found_answer(out.clone(), err_text.clone()) {
                return Ok((false, vec![]));
            }
            return az_err(format!("az vm show {name}"), rc, out, err_text, anyhow!(""))
                .map(|_| (false, vec![]));
        }
        let (rc, out, err_text) = (RUN_AZ.read().unwrap())(
            ctx,
            vec![
                "vm".into(),
                "delete".into(),
                "-g".into(),
                rg,
                "-n".into(),
                name.clone(),
                "--yes".into(),
                "-o".into(),
                "none".into(),
            ],
            AZ_LONG,
        )?;
        if rc != 0 {
            return az_err(
                format!("az vm delete {name}"),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| (true, vec![]));
        }
        Ok((true, vec![format!("vm {name}")]))
    }
}

impl AzureConfig {
    /// Go: func (c azureConfig) args(a ...string) []string
    pub(crate) fn args(&self, a: &[String]) -> Vec<String> {
        let mut out = a.to_vec();
        if !self.subscription.is_empty() {
            out.extend(["--subscription".into(), self.subscription.clone()]);
        }
        out
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
        self.find(cancel, s, executor.to_string(), host.to_string())
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
