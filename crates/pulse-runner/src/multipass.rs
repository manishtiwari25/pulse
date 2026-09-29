#![allow(dead_code)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
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
    #[serde(rename = "CPUs")]
    pub cpus: String,
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
        RwLock::new(crate::provider::cli_command(
            "multipass".into(),
            "multipass is not installed on this machine (https://canonical.com/multipass)".into(),
            &[],
        ))
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
    let mut cfg = MultipassConfig {
        image: s
            .setting("compute.multipass.image", MULTIPASS_IMAGE)
            .trim()
            .into(),
        ..Default::default()
    };
    if cfg.image.is_empty() {
        cfg.image = MULTIPASS_IMAGE.into();
    }
    let mut size = size.trim().to_string();
    if size.is_empty() {
        size = s
            .setting("compute.multipass.size", MULTIPASS_SIZE)
            .trim()
            .into();
    }
    if size.is_empty() {
        size = MULTIPASS_SIZE.into();
    }
    cfg.parse_size(size)?;
    Ok(cfg)
}

/// Go: func multipassBytes(v string) bool
pub(crate) fn multipass_bytes(v: String) -> bool {
    let mut i = 0;
    for b in v.as_bytes() {
        if b.is_ascii_digit() || *b == b'.' {
            i += 1;
        } else {
            break;
        }
    }
    if i == 0 || v[..i].parse::<f64>().is_err() {
        return false;
    }
    matches!(
        v[i..].to_ascii_uppercase().as_str(),
        "" | "B" | "K" | "M" | "G" | "KB" | "MB" | "GB" | "KIB" | "MIB" | "GIB"
    )
}

/// Go: func multipassErr(what string, rc int, stdout, stderr string, err error) error
pub(crate) fn multipass_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    crate::provider::cli_err(what, rc, stdout, stderr, err)
}

/// Go: func multipassDescribe(ctx context.Context, name string) (*multipassInfo, error)
pub(crate) fn multipass_describe(
    ctx: &pulse_bus::CancellationToken,
    name: String,
) -> anyhow::Result<Option<Box<MultipassInfo>>> {
    let (rc, out, err_text) = (RUN_MULTIPASS.read().unwrap())(
        ctx,
        vec![
            "info".into(),
            name.clone(),
            "--format".into(),
            "json".into(),
        ],
        MULTIPASS_SHORT,
    )?;
    if rc != 0 {
        if crate::provider::not_found_answer(out.clone(), err_text.clone()) {
            return Ok(None);
        }
        return multipass_err(
            format!("multipass info {name}"),
            rc,
            out,
            err_text,
            anyhow!(""),
        )
        .map(|_| None);
    }
    #[derive(Deserialize, Default)]
    struct Res {
        #[serde(default)]
        info: BTreeMap<String, MultipassInfo>,
    }
    let res: Res = serde_json::from_str(out.trim()).map_err(|_| {
        anyhow!(
            "multipass info {}: unreadable answer: {}",
            name,
            crate::run::squeeze(out.trim(), 200)
        )
    })?;
    Ok(res.info.get(&name).cloned().map(Box::new))
}

impl MultipassProvider {
    /// Go: func (multipassProvider) Name() string
    pub(crate) fn name(&self) -> String {
        "multipass".into()
    }
    /// Go: func (multipassProvider) Kind() string
    pub(crate) fn kind(&self) -> String {
        "Multipass instance".into()
    }
    /// Go: func (p multipassProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error)
    pub(crate) fn provision(
        &self,
        ctx: &pulse_bus::CancellationToken,
        req: crate::provider::ProvisionRequest,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        let (s, hub_home, log) = crate::provider::provision_start("multipass".into(), req.clone())?;
        let cfg = multipass_settings(&s, req.size.clone())?;
        let (rc, out, err_text) =
            (RUN_MULTIPASS.read().unwrap())(ctx, vec!["version".into()], MULTIPASS_SHORT)?;
        if rc != 0 {
            return multipass_err(
                "multipass is not answering (is multipassd running?)".into(),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| None);
        }
        log(
            "multipass: %s",
            &[json!(crate::run::squeeze(
                &out.trim().replace('\n', "; "),
                120
            ))],
        );
        let mut lines = Vec::new();
        let (_, pubkey) = crate::pair::pair_ensure_hub_key(hub_home.clone(), &mut lines)?;
        let name = crate::provider::machine_name(req.id.clone());
        let mut m = crate::provider::Machine {
            provider: "multipass".into(),
            name: name.clone(),
            executor: req.executor.clone(),
            tags: crate::provider::machine_tags(req.clone()),
            detail: BTreeMap::from([
                ("image".into(), cfg.image.clone()),
                ("cpus".into(), cfg.cpus.clone()),
                ("memory".into(), cfg.memory.clone()),
                ("disk".into(), cfg.disk.clone()),
            ]),
            ..Default::default()
        };
        let info = multipass_describe(ctx, name.clone())?;
        if let Some(info) = info {
            log("instance %s exists; not recreated", &[json!(name)]);
            if info.state != "Running" {
                let (rc, out, err_text) = (RUN_MULTIPASS.read().unwrap())(
                    ctx,
                    vec!["start".into(), name.clone()],
                    MULTIPASS_LONG,
                )?;
                if rc != 0 {
                    return multipass_err(
                        format!("multipass start {name}"),
                        rc,
                        out,
                        err_text,
                        anyhow!(""),
                    )
                    .map(|_| Some(Box::new(m)));
                }
            }
        } else {
            let rendered = crate::provider::write_cloud_init(pubkey, !s.harness_native())?;
            let mut args = vec![
                "launch".into(),
                cfg.image.clone(),
                "--name".into(),
                name.clone(),
                "--cloud-init".into(),
                rendered.clone(),
                "--timeout".into(),
                "1200".into(),
            ];
            if !cfg.cpus.is_empty() {
                args.extend(["--cpus".into(), cfg.cpus.clone()]);
            }
            if !cfg.memory.is_empty() {
                args.extend(["--memory".into(), cfg.memory.clone()]);
            }
            if !cfg.disk.is_empty() {
                args.extend(["--disk".into(), cfg.disk.clone()]);
            }
            log(
                "multipass launch %s (%s; cpus=%s mem=%s disk=%s)",
                &[
                    json!(name),
                    json!(cfg.image),
                    json!(crate::token::or_dash(&cfg.cpus)),
                    json!(crate::token::or_dash(&cfg.memory)),
                    json!(crate::token::or_dash(&cfg.disk)),
                ],
            );
            let (rc, out, err_text) = (RUN_MULTIPASS.read().unwrap())(ctx, args, MULTIPASS_LONG)?;
            let _ = std::fs::remove_file(rendered);
            if rc != 0 {
                return multipass_err(
                    format!("multipass launch {name}"),
                    rc,
                    out,
                    err_text,
                    anyhow!(""),
                )
                .map(|_| Some(Box::new(m)));
            }
        }
        let mut ip = String::new();
        crate::provider::poll_until(ctx, format!("the address of {name}"), || {
            let got = multipass_describe(ctx, name.clone())?;
            let Some(got) = got else {
                return Err(anyhow!("instance {} vanished while booting", name));
            };
            ip = got.ip();
            match got.state.as_str() {
                "Starting" | "Running" | "Restarting" | "Unknown" => {
                    Ok(!ip.is_empty() && got.state == "Running")
                }
                _ => Err(anyhow!("instance {} is {}", name, got.state)),
            }
        })?;
        m.detail.insert("public_ip".into(), ip.clone());
        let mut hostkeys = Vec::new();
        crate::provider::poll_until(ctx, format!("the host key of {name}"), || {
            let (rc, out, err_text) = (RUN_MULTIPASS.read().unwrap())(
                ctx,
                vec![
                    "exec".into(),
                    name.clone(),
                    "--".into(),
                    "cat".into(),
                    "/etc/ssh/ssh_host_ed25519_key.pub".into(),
                ],
                MULTIPASS_SHORT,
            )?;
            if rc != 0 {
                return Ok(false);
            }
            if !err_text.is_empty() {
                return Ok(false);
            }
            hostkeys = crate::provider::host_key_lines(out);
            Ok(!hostkeys.is_empty())
        })?;
        crate::provider::provision_finish(ctx, req, &m, hub_home, ip, hostkeys, log)?;
        Ok(Some(Box::new(m)))
    }
    /// Go: func (p multipassProvider) Release(ctx context.Context, m *Machine) error
    pub(crate) fn release(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        if m.name.is_empty() {
            return Ok(());
        }
        let (rc, out, err_text) = (RUN_MULTIPASS.read().unwrap())(
            ctx,
            vec!["delete".into(), "--purge".into(), m.name.clone()],
            MULTIPASS_LONG,
        )?;
        if rc != 0 {
            if crate::provider::not_found_answer(out.clone(), err_text.clone()) {
                return Ok(());
            }
            return multipass_err(
                format!("multipass delete --purge {}", m.name),
                rc,
                out,
                err_text,
                anyhow!(""),
            );
        }
        Ok(())
    }
    /// Go: func (p multipassProvider) Probe(ctx context.Context, m *Machine) error
    pub(crate) fn probe(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        if m.name.is_empty() {
            return Err(anyhow!("multipass: no machine"));
        }
        let info = multipass_describe(ctx, m.name.clone())?;
        let Some(info) = info else {
            return Err(anyhow!("{} is gone", m.name));
        };
        if info.state != "Running" {
            return Err(anyhow!(
                "{} is {}",
                m.name,
                crate::board::py_repr(info.state.clone())
            ));
        }
        Ok(())
    }
    /// Go: func (p multipassProvider) Find(ctx context.Context, _ *store.Store, executor, host string) (*Machine, error)
    pub(crate) fn find(
        &self,
        ctx: &pulse_bus::CancellationToken,
        _unused: &pulse_store::Store,
        executor: String,
        host: String,
    ) -> anyhow::Result<Option<Box<crate::provider::Machine>>> {
        let (rc, out, err_text) = (RUN_MULTIPASS.read().unwrap())(
            ctx,
            vec!["list".into(), "--format".into(), "json".into()],
            MULTIPASS_SHORT,
        )?;
        if rc != 0 {
            return Err(anyhow!(
                "multipass could not answer: {}",
                crate::run::err_message(
                    &multipass_err("multipass list".into(), rc, out, err_text, anyhow!(""))
                        .unwrap_err()
                )
            ));
        }
        #[derive(Deserialize, Default)]
        struct Res {
            #[serde(default)]
            list: Vec<ListItem>,
        }
        #[derive(Deserialize, Default)]
        struct ListItem {
            name: String,
            #[serde(default)]
            ipv4: Vec<String>,
        }
        let res: Res = serde_json::from_str(out.trim()).map_err(|_| {
            anyhow!(
                "multipass could not answer: unreadable list: {}",
                crate::run::squeeze(out.trim(), 200)
            )
        })?;
        for i in res.list {
            if !i.name.starts_with("pulse-") {
                continue;
            }
            let ip = i.ipv4.first().cloned().unwrap_or_default();
            if i.name == executor || (!host.is_empty() && ip == host) {
                return Ok(Some(Box::new(crate::provider::Machine {
                    provider: "multipass".into(),
                    name: i.name,
                    tags: BTreeMap::from([(crate::provider::TAG_HANDS.into(), "hands".into())]),
                    detail: BTreeMap::from([("public_ip".into(), ip)]),
                    ..Default::default()
                })));
            }
        }
        Ok(None)
    }
    /// Go: func (p multipassProvider) Stop(ctx context.Context, m *Machine) error
    pub(crate) fn stop(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<()> {
        let (rc, out, err_text) = (RUN_MULTIPASS.read().unwrap())(
            ctx,
            vec!["stop".into(), m.name.clone()],
            MULTIPASS_LONG,
        )?;
        if rc != 0 {
            return multipass_err(
                format!("multipass stop {}", m.name),
                rc,
                out,
                err_text,
                anyhow!(""),
            );
        }
        Ok(())
    }
    /// Go: func (p multipassProvider) Place(*Machine) string
    pub(crate) fn place(&self, _value: &crate::provider::Machine) -> String {
        "this machine".into()
    }
    /// Go: func (p multipassProvider) StartHint(m *Machine) string
    pub(crate) fn start_hint(&self, m: &crate::provider::Machine) -> String {
        format!("multipass start {}", m.name)
    }
    /// Go: func (p multipassProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error)
    pub(crate) fn delete(
        &self,
        ctx: &pulse_bus::CancellationToken,
        m: &crate::provider::Machine,
    ) -> anyhow::Result<(bool, Vec<String>)> {
        if m.name.is_empty() {
            return Ok((false, vec![]));
        }
        if multipass_describe(ctx, m.name.clone())?.is_none() {
            return Ok((false, vec![]));
        }
        let (rc, out, err_text) = (RUN_MULTIPASS.read().unwrap())(
            ctx,
            vec!["delete".into(), "--purge".into(), m.name.clone()],
            MULTIPASS_LONG,
        )?;
        if rc != 0 {
            return multipass_err(
                format!("multipass delete --purge {}", m.name),
                rc,
                out,
                err_text,
                anyhow!(""),
            )
            .map(|_| (true, vec![]));
        }
        Ok((true, vec![format!("instance {} with its disk", m.name)]))
    }
}

impl MultipassConfig {
    /// Go: func (c *multipassConfig) parseSize(size string) error
    pub(crate) fn parse_size(&mut self, size: String) -> anyhow::Result<()> {
        for part in size.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some((key, value)) = part.split_once('=') else {
                return Err(anyhow!(
                    "multipass size {}: want key=value pairs like {}",
                    crate::board::py_repr(size),
                    MULTIPASS_SIZE
                ));
            };
            let key = key.trim().to_lowercase();
            let value = value.trim();
            if value.is_empty() {
                return Err(anyhow!(
                    "multipass size {}: want key=value pairs like {}",
                    crate::board::py_repr(size),
                    MULTIPASS_SIZE
                ));
            }
            match key.as_str() {
                "cpus" | "cpu" => {
                    if value.parse::<i32>().ok().filter(|n| *n >= 1).is_none() {
                        return Err(anyhow!("multipass size: cpus={} is not a count", value));
                    }
                    self.cpus = value.into();
                }
                "mem" | "memory" => {
                    if !multipass_bytes(value.into()) {
                        return Err(anyhow!(
                            "multipass size: mem={} is not a size like 4G",
                            value
                        ));
                    }
                    self.memory = value.into();
                }
                "disk" => {
                    if !multipass_bytes(value.into()) {
                        return Err(anyhow!(
                            "multipass size: disk={} is not a size like 32G",
                            value
                        ));
                    }
                    self.disk = value.into();
                }
                _ => {
                    return Err(anyhow!(
                        "multipass size: unknown key {} (cpus, mem, disk)",
                        crate::board::py_repr(key)
                    ));
                }
            }
        }
        Ok(())
    }
}

impl MultipassInfo {
    /// Go: func (i multipassInfo) ip() string
    pub(crate) fn ip(&self) -> String {
        self.i_pv4.first().cloned().unwrap_or_default()
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
