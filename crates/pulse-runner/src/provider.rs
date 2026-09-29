#![allow(dead_code)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::Command;
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
    let env = env.to_vec();
    Box::new(move |cancel, args, timeout| {
        if cancel.is_cancelled() {
            return Err(anyhow!(
                "{program} {} timed out after {:?}",
                args.iter().take(2).cloned().collect::<Vec<_>>().join(" "),
                timeout
            ));
        }
        let mut cmd = Command::new(&program);
        cmd.args(&args);
        cmd.env_clear();
        for kv in crate::proc::environ() {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
        for kv in &env {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
        let out = cmd.output();
        match out {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                Ok((o.status.code().unwrap_or(-1), stdout, stderr))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(anyhow!(missing.clone())),
            Err(e) => Err(anyhow!(e)),
        }
    })
}

/// Go: func cliErr(what string, rc int, stdout, stderr string, err error) error
pub(crate) fn cli_err(
    what: String,
    rc: i32,
    stdout: String,
    stderr: String,
    err: anyhow::Error,
) -> anyhow::Result<()> {
    if !err.to_string().is_empty() {
        return Err(anyhow!(
            "{}: {}",
            what,
            crate::compute::redact_secrets(&crate::run::err_message(&err))
        ));
    }
    let mut text = stderr.trim().to_string();
    if text.is_empty() {
        text = stdout.trim().to_string();
    }
    if text.is_empty() {
        text = format!("exit {rc}");
    }
    Err(anyhow!(
        "{}: {}",
        what,
        crate::compute::redact_secrets(&crate::run::squeeze(&text, 400))
    ))
}

/// Go: func notFoundAnswer(stdout, stderr string) bool
pub(crate) fn not_found_answer(stdout: String, stderr: String) -> bool {
    let low = format!("{stderr}{stdout}").to_lowercase();
    [
        "notfound",
        "not found",
        "could not be found",
        "does not exist",
        "was not found",
        "no such",
        "instance does not exist",
    ]
    .iter()
    .any(|w| low.contains(w))
}

/// Go: func cloudInitConfig(adminKey string, installCopilot bool) string
pub(crate) fn cloud_init_config(admin_key: String, install_copilot: bool) -> String {
    let mut b = String::from(
        "#cloud-config\n# PULSE compute machine (agents' hands) - Ubuntu 24.04 (ADR-020, D-D). Prepared for `pulse pair`, which\n# adds the pulse user, its forced-command key, the linux pulse binary and the sshd drop-in over the admin session.\npackage_update: true\npackage_upgrade: true\npackages:\n  - git\n  - python3\n  - ca-certificates\n  - curl\n  - gnupg\n  - build-essential\nusers:\n  - default\n",
    );
    let key = admin_key.trim();
    if !key.is_empty() {
        b.push_str(&format!(
            "  - name: {ADMIN_USER}\n    shell: /bin/bash\n    sudo: ALL=(ALL) NOPASSWD:ALL\n    lock_passwd: true\n    ssh_authorized_keys:\n      - {key}\n"
        ));
    }
    b.push_str(&format!(
        "runcmd:\n  # The host key on the serial console: the channel the provider vouches for (aws, gcp read it back).\n  - sh -c 'echo \"{HOSTKEY_MARKER}$(cat /etc/ssh/ssh_host_ed25519_key.pub)\" > /dev/console'\n"
    ));
    if install_copilot {
        b.push_str("  # Node.js 22 and the copilot CLI, for every user (the legacy worker prerequisites).\n  - mkdir -p /etc/apt/keyrings\n  - curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg\n  - echo \"deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main\" > /etc/apt/sources.list.d/nodesource.list\n  - apt-get update\n  - apt-get install -y nodejs\n  - npm install -g @github/copilot\n");
    }
    b.push_str(&format!(
        "  # The work directory the forced command roots every path in; pairing sets the owner.\n  - mkdir -p {HANDS_WORKDIR}\nfinal_message: \"pulse compute machine ready after $UPTIME seconds\"\n"
    ));
    b
}

/// Go: func writeCloudInit(adminKey string, installCopilot bool) (string, error)
pub(crate) fn write_cloud_init(admin_key: String, install_copilot: bool) -> anyhow::Result<String> {
    let mut path = std::env::temp_dir();
    path.push(format!("pulse-cloud-init-{}.yaml", uuid::Uuid::new_v4()));
    std::fs::write(&path, cloud_init_config(admin_key, install_copilot))?;
    Ok(path.to_string_lossy().to_string())
}

/// Go: func hostKeyLines(text string) []string
pub(crate) fn host_key_lines(text: String) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let add = |line: &str, out: &mut Vec<String>, seen: &mut std::collections::BTreeSet<String>| {
        let f = line.split_whitespace().collect::<Vec<_>>();
        if f.len() >= 2 && f[0] == "ssh-ed25519" {
            let key = format!("{} {}", f[0], f[1]);
            if seen.insert(key.clone()) {
                out.push(key);
            }
        }
    };
    let mut in_block = false;
    for raw in text.replace('\r', "").lines() {
        let line = raw.trim();
        if let Some(i) = line.find(HOSTKEY_MARKER) {
            add(&line[i + HOSTKEY_MARKER.len()..], &mut out, &mut seen);
        } else if line == HOSTKEY_BLOCK_TOP {
            in_block = true;
        } else if line == HOSTKEY_BLOCK_END {
            in_block = false;
        } else if in_block || line.starts_with("ssh-ed25519 ") {
            add(line, &mut out, &mut seen);
        }
    }
    out
}

/// Go: func provisionStart(provider string, req ProvisionRequest) (*store.Store, string, func(string, ...any), error)
pub(crate) fn provision_start(
    provider: String,
    req: crate::provider::ProvisionRequest,
) -> anyhow::Result<(
    pulse_store::Store,
    String,
    Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
)> {
    let Some(s) = req.store.clone() else {
        return Err(anyhow!("{provider}: a store is needed"));
    };
    if req.executor.is_empty() || !crate::pair::valid_name(req.executor.clone()) {
        return Err(anyhow!(
            "{}: executor name {} is not lowercase words with dashes",
            provider,
            crate::board::py_repr(req.executor)
        ));
    }
    let hub_home = if req.hub_home.is_empty() {
        crate::util::home_dir()
    } else {
        req.hub_home
    };
    Ok((s, hub_home, Box::new(|_, _| {})))
}

/// Go: func machineTags(req ProvisionRequest) map[string]string
pub(crate) fn machine_tags(req: crate::provider::ProvisionRequest) -> BTreeMap<String, String> {
    BTreeMap::from([
        (TAG_HANDS.to_string(), "hands".to_string()),
        (TAG_REQUEST.to_string(), req.id),
        (TAG_PROJECT.to_string(), req.project),
    ])
}

/// Go: func machineName(requestID string) string
pub(crate) fn machine_name(request_id: String) -> String {
    crate::util::slugify(&format!("pulse-{}", request_id.to_lowercase()), 40)
}

/// Go: func writeAdminAlias(hubHome, alias, host string) error
pub(crate) fn write_admin_alias(
    hub_home: String,
    alias: String,
    host: String,
) -> anyhow::Result<()> {
    let config = Path::new(&hub_home).join(".ssh").join("config");
    let text = std::fs::read_to_string(&config).unwrap_or_default();
    if let Some(b) = crate::pair::pair_alias_state(text.clone(), alias.clone()) {
        if b.hostname != host {
            return Err(anyhow!(
                "ssh alias {} already points at {}, not {}",
                alias,
                b.hostname,
                host
            ));
        }
        return Ok(());
    }
    std::fs::create_dir_all(config.parent().unwrap())?;
    let mut prefix = String::new();
    if !text.is_empty() {
        prefix.push('\n');
        if !text.ends_with('\n') {
            prefix.push('\n');
        }
    }
    let block = [
        format!("Host {alias}"),
        format!("    HostName {host}"),
        "    Port 22".to_string(),
        format!("    User {ADMIN_USER}"),
        "    IdentityFile ~/.ssh/pulse_hands_ed25519".to_string(),
        "    IdentitiesOnly yes".to_string(),
        String::new(),
    ]
    .join("\n");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config)?;
    f.write_all(format!("{prefix}{block}").as_bytes())?;
    Ok(())
}

/// Go: func waitCloudInit(ctx context.Context, admin string, log func(string, ...any)) error
pub(crate) fn wait_cloud_init(
    ctx: &pulse_bus::CancellationToken,
    admin: String,
    log: &(dyn Fn(&str, &[serde_json::Value]) + Send + Sync),
) -> anyhow::Result<()> {
    let probe = "cloud-init status --wait >/dev/null 2>&1; command -v git >/dev/null 2>&1 && echo pulse-ready";
    let mut try_no = 1;
    loop {
        let res =
            crate::executor::admin_ssh_run(&admin, probe, &[], Duration::from_secs(15 * 60), "");
        if let Ok((0, ref out, _)) = res
            && out.contains("pulse-ready")
        {
            return Ok(());
        }
        if let Err(e) = res {
            log(
                "ssh attempt %d: %s",
                &[json!(try_no), json!(crate::run::err_message(&e))],
            );
        }
        if !sleep_ctx(
            ctx,
            *PROVISION_WAIT.read().expect("provision_wait poisoned"),
        ) {
            return Err(anyhow!("gave up waiting for {}: context canceled", admin));
        }
        try_no += 1;
    }
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
    let Some(s) = req.store.clone() else {
        return Err(anyhow!("{}: a store is needed", m.provider));
    };
    if ip.is_empty() {
        return Err(anyhow!("{} has no public address", m.name));
    }
    if hostkeys.is_empty() {
        return Err(anyhow!("{} gave no ssh host key", m.name));
    }
    let mut m = m.clone();
    m.address = format!("{ADMIN_USER}@{ip}");
    m.detail.insert("public_ip".into(), ip.clone());
    crate::pair::pair_write_known_hosts(hub_home.clone(), ip.clone(), 22, hostkeys[..1].to_vec());
    let alias = format!("{}-admin", m.name);
    write_admin_alias(hub_home.clone(), alias.clone(), ip.clone())?;
    let admin = format!("{ADMIN_USER}@{alias}");
    log(
        "%s is up at %s; waiting for cloud-init",
        &[json!(m.name), json!(ip)],
    );
    wait_cloud_init(ctx, admin.clone(), &*log)?;
    let rep = (PAIR_HANDS.read().expect("pair_hands poisoned"))(
        &s,
        &admin,
        &req.executor,
        &s.now(),
        crate::pair::PairOpts {
            workdir: HANDS_WORKDIR.into(),
            workers: 1,
            projects: req.project.clone(),
            install_copilot: !s.harness_native(),
            actor: crate::util::HUMAN_AGENT.into(),
            hub_home: hub_home.clone(),
            ..Default::default()
        },
    )?
    .ok_or_else(|| anyhow!("pairing {} did not complete", req.executor))?;
    for l in &rep.lines {
        log("pair: %s", &[json!(l)]);
    }
    if !rep.ok {
        let last = rep.lines.last().cloned().unwrap_or_default();
        return Err(anyhow!(
            "pairing {} did not complete: {}",
            req.executor,
            crate::compute::redact_secrets(&last)
        ));
    }
    crate::executor::set_executor_admin(&s, &req.executor, &m.address);
    if let Ok((tok, origin)) = crate::token::login_source(&s) {
        if !tok.is_empty() {
            let mut rdr = crate::token::token_reader(tok);
            let res = (*crate::executor::PLACE_LOGIN
                .read()
                .expect("place_login poisoned"))(
                &s,
                &s.now(),
                &req.executor,
                &m.address,
                &mut rdr,
                false,
                false,
                &req.actor,
            )?;
            for l in &res.lines {
                log("login: %s", &[json!(crate::compute::redact_secrets(l))]);
            }
            if !res.ok {
                return Err(anyhow!(
                    "the {} was refused on {}; the machine cannot work",
                    origin,
                    req.executor
                ));
            }
        } else {
            log(
                "no login source: %s starts without a worker login",
                &[json!(req.executor)],
            );
        }
    }
    Ok(())
}

/// Go: func sleepCtx(ctx context.Context, d time.Duration) bool
pub(crate) fn sleep_ctx(ctx: &pulse_bus::CancellationToken, d: std::time::Duration) -> bool {
    let step = Duration::from_millis(10);
    let start = std::time::Instant::now();
    while start.elapsed() < d {
        if ctx.is_cancelled() {
            return false;
        }
        std::thread::sleep(step.min(d.saturating_sub(start.elapsed())));
    }
    !ctx.is_cancelled()
}

/// Go: func pollUntil(ctx context.Context, what string, step func() (bool, error)) error
pub(crate) fn poll_until<F>(
    ctx: &pulse_bus::CancellationToken,
    what: String,
    mut step: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> anyhow::Result<bool>,
{
    let mut try_no = 0;
    loop {
        if try_no > 0
            && !sleep_ctx(
                ctx,
                *PROVISION_WAIT.read().expect("provision_wait poisoned"),
            )
        {
            return Err(anyhow!("gave up waiting for {}: context canceled", what));
        }
        if step()? {
            return Ok(());
        }
        try_no += 1;
    }
}

/// Go: func cliList(out string) []string
pub(crate) fn cli_list(out: String) -> Vec<String> {
    out.split_whitespace().map(ToOwned::to_owned).collect()
}

/// Go: func machineFromDetail(provider, name, detailJSON, resourceGroup string) *Machine
pub(crate) fn machine_from_detail(
    provider: String,
    name: String,
    detail_json: String,
    resource_group: String,
) -> Option<Box<crate::provider::Machine>> {
    #[derive(Deserialize)]
    struct Detail {
        #[serde(default)]
        tags: BTreeMap<String, String>,
        #[serde(default)]
        detail: BTreeMap<String, String>,
    }
    let mut m = crate::provider::Machine {
        provider,
        name,
        tags: BTreeMap::new(),
        detail: BTreeMap::new(),
        ..Default::default()
    };
    if let Ok(d) = serde_json::from_str::<Detail>(&detail_json) {
        m.tags = d.tags;
        m.detail = d.detail;
    }
    if m.detail.get("resource_group").is_none_or(String::is_empty) && !resource_group.is_empty() {
        m.detail.insert("resource_group".into(), resource_group);
    }
    Some(Box::new(m))
}
