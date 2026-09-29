#![allow(dead_code)]

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
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
    #[serde(rename = "networkInterfaces", default)]
    pub network_interfaces: Vec<GcpNetworkInterface>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GcpNetworkInterface {
    #[serde(rename = "accessConfigs", default)]
    pub access_configs: Vec<GcpAccessConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GcpAccessConfig {
    #[serde(rename = "natIP", default)]
    pub nat_ip: String,
}

pub(crate) static RUN_GCLOUD: LazyLock<RwLock<Box<crate::provider::CliRunner>>> =
    LazyLock::new(|| {
        RwLock::new(crate::provider::cli_command(
            "gcloud".into(),
            "the Google Cloud CLI (gcloud) is not installed on this machine".into(),
            &[
                "CLOUDSDK_CORE_DISABLE_PROMPTS=1".into(),
                "CLOUDSDK_CORE_DISABLE_COLOR=true".into(),
            ],
        ))
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
    let mut cfg = GcpConfig {
        project: s.setting("compute.gcp.project", "").trim().into(),
        zone: s.setting("compute.gcp.zone", "").trim().into(),
        machine_type: size.trim().into(),
        image: s.setting("compute.gcp.image", GCP_IMAGE).trim().into(),
        network: s.setting("compute.gcp.network", "").trim().into(),
    };
    if cfg.machine_type.is_empty() {
        cfg.machine_type = s
            .setting("compute.gcp.machine_type", GCP_MACHINE_TYPE)
            .trim()
            .into();
    }
    if cfg.machine_type.is_empty() {
        cfg.machine_type = GCP_MACHINE_TYPE.into();
    }
    if cfg.image.is_empty() {
        cfg.image = GCP_IMAGE.into();
    }
    if cfg.network.is_empty() {
        cfg.network = "default".into();
    }
    cfg
}

/// Go: func gcpErr(what string, rc int, stdout, stderr string, err error) error
pub(crate) fn gcp_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    crate::provider::cli_err(what, rc, stdout, stderr, err)
}

/// Go: func gcpJSON(ctx context.Context, what string, args []string, timeout time.Duration, v any) error
pub(crate) fn gcp_json(
    ctx: &pulse_bus::CancellationToken,
    what: String,
    args: Vec<String>,
    timeout: std::time::Duration,
    _v: serde_json::Value,
) -> anyhow::Result<()> {
    let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(ctx, args, timeout)?;
    if rc != 0 {
        return gcp_err(what, rc, out, err_text, anyhow!(""));
    }
    let _: serde_json::Value = serde_json::from_str(out.trim()).map_err(|_| {
        anyhow!(
            "{}: unreadable answer: {}",
            what,
            crate::run::squeeze(out.trim(), 200)
        )
    })?;
    Ok(())
}

/// Go: func gcpLabel(v string) string
pub(crate) fn gcp_label(v: String) -> String {
    let mut out = String::new();
    for ch in v.to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    if out.len() > 63 {
        out.truncate(63);
    }
    out
}

/// Go: func gcpMachine(cfg gcpConfig, i gcpInstance) *Machine
pub(crate) fn gcp_machine(cfg: GcpConfig, i: GcpInstance) -> Option<crate::provider::Machine> {
    let mt = i
        .machine_type
        .rsplit('/')
        .next()
        .unwrap_or(&i.machine_type)
        .to_string();
    let mut tags = BTreeMap::from([(crate::provider::TAG_HANDS.into(), "hands".into())]);
    for k in [crate::provider::TAG_REQUEST, crate::provider::TAG_PROJECT] {
        if let Some(v) = i.labels.get(k).filter(|v| !v.is_empty()) {
            tags.insert(k.into(), v.clone());
        }
    }
    Some(crate::provider::Machine {
        provider: "gcp".into(),
        name: i.name.clone(),
        tags,
        detail: BTreeMap::from([
            ("project".into(), cfg.project),
            ("zone".into(), i.zone_name()),
            ("type".into(), mt),
            ("public_ip".into(), i.public_ip()),
        ]),
        ..Default::default()
    })
}

/// Go: func gcpDescribe(ctx context.Context, cfg gcpConfig, name string) (*gcpInstance, error)
pub(crate) fn gcp_describe(
    ctx: &pulse_bus::CancellationToken,
    cfg: GcpConfig,
    name: String,
) -> anyhow::Result<Option<Box<GcpInstance>>> {
    let args = cfg.args(&[
        "compute".into(),
        "instances".into(),
        "describe".into(),
        name.clone(),
        "--zone".into(),
        cfg.zone.clone(),
    ]);
    let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(ctx, args, GCP_SHORT)?;
    if rc != 0 {
        if crate::provider::not_found_answer(out.clone(), err_text.clone()) {
            return Ok(None);
        }
        return gcp_err(
            format!("gcloud compute instances describe {name}"),
            rc,
            out,
            err_text,
            anyhow!(""),
        )
        .map(|_| unreachable!());
    }
    let i: GcpInstance = serde_json::from_str(out.trim()).map_err(|_| {
        anyhow!(
            "gcloud compute instances describe {}: unreadable answer: {}",
            name,
            crate::run::squeeze(out.trim(), 200)
        )
    })?;
    Ok(Some(Box::new(i)))
}

/// Go: func gcpFirewallRule(ctx context.Context, cfg gcpConfig, log func(string, ...any)) error
pub(crate) fn gcp_firewall_rule(
    ctx: &pulse_bus::CancellationToken,
    cfg: GcpConfig,
    log: &(dyn Fn(&str, &[serde_json::Value]) + Send + Sync),
) -> anyhow::Result<()> {
    let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(
        ctx,
        cfg.args(&[
            "compute".into(),
            "firewall-rules".into(),
            "describe".into(),
            GCP_FIREWALL.into(),
        ]),
        GCP_SHORT,
    )?;
    if rc == 0 {
        return Ok(());
    }
    if !crate::provider::not_found_answer(out.clone(), err_text.clone()) {
        return gcp_err(
            format!("gcloud compute firewall-rules describe {GCP_FIREWALL}"),
            rc,
            out,
            err_text,
            anyhow!(""),
        );
    }
    let args = cfg.args(&[
        "compute".into(),
        "firewall-rules".into(),
        "create".into(),
        GCP_FIREWALL.into(),
        "--network".into(),
        cfg.network.clone(),
        "--direction".into(),
        "INGRESS".into(),
        "--allow".into(),
        "tcp:22".into(),
        "--source-ranges".into(),
        "0.0.0.0/0".into(),
        "--target-tags".into(),
        GCP_NETWORK_TAG.into(),
        "--description".into(),
        "PULSE compute machines: ssh in".into(),
    ]);
    let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(ctx, args, Duration::from_secs(120))?;
    if rc != 0 {
        return gcp_err(
            format!("gcloud compute firewall-rules create {GCP_FIREWALL}"),
            rc,
            out,
            err_text,
            anyhow!(""),
        );
    }
    log(
        "created firewall rule %s in network %s with port 22 open to tag %s",
        &[
            json!(GCP_FIREWALL),
            json!(cfg.network),
            json!(GCP_NETWORK_TAG),
        ],
    );
    Ok(())
}

impl GcpProvider {
    /// Go: func (gcpProvider) Name() string
    pub(crate) fn name(&self) -> String {
        "gcp".into()
    }
    /// Go: func (gcpProvider) Kind() string
    pub(crate) fn kind(&self) -> String {
        "Compute Engine instance".into()
    }
    /// Go: func (p gcpProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error)
    pub(crate) fn provision(
        &self,
        ctx: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<crate::provider::Machine> {
        let (s, hub_home, log) = crate::provider::provision_start("gcp".into(), req.clone())?;
        let mut cfg = gcp_settings(&s, req.size.clone());
        let (rc, out, mut err_text) = (RUN_GCLOUD.read().unwrap())(
            ctx,
            vec![
                "auth".into(),
                "list".into(),
                "--filter=status:ACTIVE".into(),
                "--format=value(account)".into(),
            ],
            GCP_SHORT,
        )?;
        if rc != 0 || out.trim().is_empty() {
            if rc == 0 {
                err_text = "no active account".into();
            }
            return gcp_err("not logged in to Google Cloud (run `gcloud auth login` on this machine as the daemon's user)".into(), rc, out, err_text, anyhow!("")).map(|_| unreachable!());
        }
        let account = out.trim().to_string();
        if cfg.project.is_empty()
            && let Ok((0, out, _)) = (RUN_GCLOUD.read().unwrap())(
                ctx,
                vec!["config".into(), "get-value".into(), "project".into()],
                GCP_SHORT,
            )
        {
            cfg.project = out.trim().into();
        }
        let missing = [
            ("compute.gcp.project", cfg.project.as_str()),
            ("compute.gcp.zone", cfg.zone.as_str()),
        ]
        .into_iter()
        .filter_map(|(k, v)| if v.is_empty() { Some(k) } else { None })
        .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(anyhow!(
                "gcp is not configured: set {} (pulse settings set <name> <value>)",
                missing.join(" and ")
            ));
        }
        log(
            "gcp project %s as %s in %s",
            &[json!(cfg.project), json!(account), json!(cfg.zone)],
        );
        let mut lines = Vec::new();
        let (_, pubkey) = crate::pair::pair_ensure_hub_key(hub_home.clone(), &mut lines)?;
        let name = crate::provider::machine_name(req.id.clone());
        let mut m = crate::provider::Machine {
            provider: "gcp".into(),
            name: name.clone(),
            executor: req.executor.clone(),
            tags: crate::provider::machine_tags(req.clone()),
            detail: BTreeMap::from([
                ("project".into(), cfg.project.clone()),
                ("zone".into(), cfg.zone.clone()),
                ("type".into(), cfg.machine_type.clone()),
                ("image".into(), cfg.image.clone()),
                ("network".into(), cfg.network.clone()),
            ]),
            ..Default::default()
        };
        let mut inst = if let Some(i) = gcp_describe(ctx, cfg.clone(), name.clone())? {
            log("instance %s exists; not recreated", &[json!(name)]);
            *i
        } else {
            gcp_firewall_rule(ctx, cfg.clone(), &*log)?;
            let rendered = crate::provider::write_cloud_init(pubkey, !s.harness_native())?;
            log(
                "gcloud compute instances create %s (%s, %s, %s GiB)",
                &[
                    json!(name),
                    json!(cfg.machine_type),
                    json!(cfg.image),
                    json!(crate::provider::HANDS_DISK_GB),
                ],
            );
            let labels = format!(
                "{}=hands,{}={},{}={}",
                crate::provider::TAG_HANDS,
                crate::provider::TAG_REQUEST,
                gcp_label(req.id.clone()),
                crate::provider::TAG_PROJECT,
                gcp_label(req.project.clone())
            );
            let mut args = vec![
                "compute".into(),
                "instances".into(),
                "create".into(),
                name.clone(),
                "--zone".into(),
                cfg.zone.clone(),
                "--machine-type".into(),
                cfg.machine_type.clone(),
            ];
            args.extend(cfg.image_args());
            args.extend([
                "--boot-disk-size".into(),
                format!("{}GB", crate::provider::HANDS_DISK_GB),
                "--boot-disk-type".into(),
                "pd-balanced".into(),
                "--boot-disk-auto-delete".into(),
                "--network".into(),
                cfg.network.clone(),
                "--labels".into(),
                labels,
                "--tags".into(),
                GCP_NETWORK_TAG.into(),
                "--metadata-from-file".into(),
                format!("user-data={rendered}"),
            ]);
            let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(ctx, cfg.args(&args), GCP_LONG)?;
            let _ = std::fs::remove_file(rendered);
            if rc != 0 {
                return gcp_err(
                    format!("gcloud compute instances create {name}"),
                    rc,
                    out,
                    err_text,
                    anyhow!(""),
                )
                .map(|_| m);
            }
            let created: Vec<GcpInstance> = serde_json::from_str(out.trim()).unwrap_or_default();
            if created.is_empty() {
                return Err(anyhow!(
                    "gcloud compute instances create {} made no instance",
                    name
                ));
            }
            created[0].clone()
        };
        let mut ip = inst.public_ip();
        crate::provider::poll_until(ctx, format!("the address of {name}"), || {
            if !ip.is_empty() && inst.status == "RUNNING" {
                return Ok(true);
            }
            let got = gcp_describe(ctx, cfg.clone(), name.clone())?;
            let Some(got) = got else {
                return Err(anyhow!("instance {} vanished while booting", name));
            };
            inst = *got;
            ip = inst.public_ip();
            match inst.status.as_str() {
                "PROVISIONING" | "STAGING" | "RUNNING" => {
                    Ok(!ip.is_empty() && inst.status == "RUNNING")
                }
                _ => Err(anyhow!("instance {} is {}", name, inst.status)),
            }
        })?;
        m.detail.insert("public_ip".into(), ip.clone());
        let mut hostkeys = Vec::new();
        crate::provider::poll_until(
            ctx,
            format!("the host key of {name} on its serial console"),
            || {
                let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(
                    ctx,
                    cfg.scope(vec![
                        "compute".into(),
                        "instances".into(),
                        "get-serial-port-output".into(),
                        name.clone(),
                        "--zone".into(),
                        cfg.zone.clone(),
                    ]),
                    GCP_SHORT,
                )?;
                if rc != 0 {
                    return gcp_err(
                        format!("gcloud compute instances get-serial-port-output {name}"),
                        rc,
                        out,
                        err_text,
                        anyhow!(""),
                    )
                    .map(|_| false);
                }
                hostkeys = crate::provider::host_key_lines(out);
                Ok(!hostkeys.is_empty())
            },
        )?;
        crate::provider::provision_finish(ctx, req, &mut m, hub_home, ip, hostkeys, log)?;
        Ok(m)
    }
    /// Go: func (p gcpProvider) config(m *Machine) gcpConfig
    pub(crate) fn config(&self, m: &crate::provider::Machine) -> GcpConfig {
        GcpConfig {
            project: m.detail.get("project").cloned().unwrap_or_default(),
            zone: m.detail.get("zone").cloned().unwrap_or_default(),
            ..Default::default()
        }
    }
    /// Go: func (p gcpProvider) Release(ctx context.Context, m *Machine) error
    pub(crate) fn release(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        if m.name.is_empty() {
            return Ok(());
        }
        let cfg = self.config(m);
        if cfg.zone.is_empty() {
            return Err(anyhow!("gcp: the machine's zone is unknown"));
        }
        let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(
            ctx,
            cfg.scope(vec![
                "compute".into(),
                "instances".into(),
                "delete".into(),
                m.name.clone(),
                "--zone".into(),
                cfg.zone.clone(),
                "--quiet".into(),
            ]),
            GCP_LONG,
        )?;
        if rc != 0 {
            if crate::provider::not_found_answer(out.clone(), err_text.clone()) {
                return Ok(());
            }
            return gcp_err(
                format!("gcloud compute instances delete {}", m.name),
                rc,
                out,
                err_text,
                anyhow!(""),
            );
        }
        Ok(())
    }
    /// Go: func (p gcpProvider) Probe(ctx context.Context, m *Machine) error
    pub(crate) fn probe(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        if m.name.is_empty() {
            return Err(anyhow!("gcp: no machine"));
        }
        let inst = gcp_describe(ctx, self.config(m), m.name.clone())?;
        let Some(inst) = inst else {
            return Err(anyhow!("{} is gone", m.name));
        };
        if inst.status != "RUNNING" {
            return Err(anyhow!(
                "{} is {}",
                m.name,
                crate::board::py_repr(inst.status.clone())
            ));
        }
        Ok(())
    }
    /// Go: func (p gcpProvider) Find(ctx context.Context, s *store.Store, executor, host string) (*Machine, error)
    pub(crate) fn find(
        &self,
        ctx: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        executor: String,
        host: String,
    ) -> anyhow::Result<Option<crate::provider::Machine>> {
        let cfg = gcp_settings(s, String::new());
        let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(
            ctx,
            cfg.args(&[
                "compute".into(),
                "instances".into(),
                "list".into(),
                format!("--filter=labels.{}=hands", crate::provider::TAG_HANDS),
            ]),
            GCP_SHORT,
        )?;
        if rc != 0 {
            return Err(anyhow!(
                "the Google Cloud CLI could not answer: {}",
                crate::run::err_message(&anyhow!(
                    gcp_err(
                        "gcloud compute instances list".into(),
                        rc,
                        out,
                        err_text,
                        anyhow!("")
                    )
                    .unwrap_err()
                ))
            ));
        }
        let got: Vec<GcpInstance> = serde_json::from_str(out.trim()).map_err(|_| {
            anyhow!(
                "the Google Cloud CLI could not answer: unreadable answer: {}",
                crate::run::squeeze(out.trim(), 200)
            )
        })?;
        for i in got {
            if i.name == executor || (!host.is_empty() && i.public_ip() == host) {
                return Ok(gcp_machine(cfg, i));
            }
        }
        Ok(None)
    }
    /// Go: func (p gcpProvider) Stop(ctx context.Context, m *Machine) error
    pub(crate) fn stop(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        let cfg = self.config(m);
        let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(
            ctx,
            cfg.scope(vec![
                "compute".into(),
                "instances".into(),
                "stop".into(),
                m.name.clone(),
                "--zone".into(),
                cfg.zone.clone(),
                "--quiet".into(),
            ]),
            GCP_LONG,
        )?;
        if rc != 0 {
            return gcp_err(
                format!("gcloud compute instances stop {}", m.name),
                rc,
                out,
                err_text,
                anyhow!(""),
            );
        }
        Ok(())
    }
    /// Go: func (p gcpProvider) Place(m *Machine) string
    pub(crate) fn place(&self, m: &crate::provider::Machine) -> String {
        m.detail.get("zone").cloned().unwrap_or_default()
    }
    /// Go: func (p gcpProvider) StartHint(m *Machine) string
    pub(crate) fn start_hint(&self, m: &crate::provider::Machine) -> String {
        let mut hint = format!(
            "gcloud compute instances start {} --zone {}",
            m.name,
            m.detail.get("zone").cloned().unwrap_or_default()
        );
        if let Some(project) = m.detail.get("project").filter(|s| !s.is_empty()) {
            hint.push_str(&format!(" --project {project}"));
        }
        hint
    }
    /// Go: func (p gcpProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error)
    pub(crate) fn delete(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        let cfg = self.config(m);
        if m.name.is_empty() || cfg.zone.is_empty() {
            return Ok((false, vec![]));
        }
        if gcp_describe(ctx, cfg.clone(), m.name.clone())?.is_none() {
            return Ok((false, vec![]));
        }
        let (rc, out, err_text) = (RUN_GCLOUD.read().unwrap())(
            ctx,
            cfg.scope(vec![
                "compute".into(),
                "instances".into(),
                "delete".into(),
                m.name.clone(),
                "--zone".into(),
                cfg.zone.clone(),
                "--quiet".into(),
            ]),
            GCP_LONG,
        )?;
        if rc != 0 {
            return gcp_err(
                format!("gcloud compute instances delete {}", m.name),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| (true, vec![]));
        }
        Ok((
            true,
            vec![format!("instance {} with its boot disk", m.name)],
        ))
    }
}

impl GcpConfig {
    /// Go: func (c gcpConfig) args(a ...string) []string
    pub(crate) fn args(&self, a: &[String]) -> Vec<String> {
        let mut out = self.scope(a.to_vec());
        out.extend(["--format".into(), "json".into()]);
        out
    }
    /// Go: func (c gcpConfig) scope(a []string) []string
    pub(crate) fn scope(&self, a: Vec<String>) -> Vec<String> {
        let mut out = a;
        if !self.project.is_empty() {
            out.extend(["--project".into(), self.project.clone()]);
        }
        out
    }
    /// Go: func (c gcpConfig) imageArgs() []string
    pub(crate) fn image_args(&self) -> Vec<String> {
        if self.image.contains('/') {
            vec!["--image".into(), self.image.clone()]
        } else {
            vec![
                "--image-family".into(),
                self.image.clone(),
                "--image-project".into(),
                GCP_IMAGE_PROJECT.into(),
            ]
        }
    }
}

impl GcpInstance {
    /// Go: func (i gcpInstance) publicIP() string
    pub(crate) fn public_ip(&self) -> String {
        for n in &self.network_interfaces {
            for a in &n.access_configs {
                if !a.nat_ip.is_empty() {
                    return a.nat_ip.clone();
                }
            }
        }
        String::new()
    }
    /// Go: func (i gcpInstance) zoneName() string
    pub(crate) fn zone_name(&self) -> String {
        self.zone
            .rsplit('/')
            .next()
            .unwrap_or(&self.zone)
            .to_string()
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
    ) -> anyhow::Result<crate::provider::Machine> {
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
    ) -> anyhow::Result<Option<crate::provider::Machine>> {
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

#[cfg(test)]
mod tests {
    #[test]
    fn gcp_label() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(super::gcp_label("CR-001".to_string()), "cr-001");
        assert_eq!(super::gcp_label("Proj.Name/x".to_string()), "proj-name-x");
        assert_eq!(super::gcp_label("a".repeat(70)).len(), 63);
    }
}
