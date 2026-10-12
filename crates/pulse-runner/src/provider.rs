#![allow(dead_code)]

use anyhow::anyhow;
use serde::Deserialize;
use serde_json::json;
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
pub(crate) type PairHandsFn = dyn Fn(
        &pulse_store::Store,
        &str,
        &str,
        &str,
        crate::pair::PairOpts,
    ) -> anyhow::Result<Option<Box<crate::pair::PairReport>>>
    + Send
    + Sync;
pub(crate) type ProviderLogFn = dyn Fn(&str, &[serde_json::Value]) + Send + Sync;

pub(crate) use crate::compute::provider_named;
pub use crate::compute::{Machine, Provider, ProvisionRequest, provider_names, register_provider};

pub(crate) static PAIR_HANDS: LazyLock<RwLock<Box<PairHandsFn>>> = LazyLock::new(|| {
    RwLock::new(Box::new(|s, admin, name, now, o| {
        crate::pair::pair_executor(s, admin.to_string(), name.to_string(), now.to_string(), o)
    }))
});

pub(crate) fn set_pair_hands(f: Box<PairHandsFn>) -> Box<PairHandsFn> {
    std::mem::replace(&mut *PAIR_HANDS.write().expect("pair_hands poisoned"), f)
}

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

pub(crate) fn write_cloud_init(admin_key: String, install_copilot: bool) -> anyhow::Result<String> {
    let mut path = std::env::temp_dir();
    path.push(format!("pulse-cloud-init-{}.yaml", uuid::Uuid::new_v4()));
    std::fs::write(&path, cloud_init_config(admin_key, install_copilot))?;
    Ok(path.to_string_lossy().to_string())
}

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

pub(crate) fn provision_start(
    provider: String,
    req: crate::provider::ProvisionRequest,
) -> anyhow::Result<(pulse_store::Store, String, Box<ProviderLogFn>)> {
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

pub(crate) fn machine_tags(req: crate::provider::ProvisionRequest) -> BTreeMap<String, String> {
    BTreeMap::from([
        (TAG_HANDS.to_string(), "hands".to_string()),
        (TAG_REQUEST.to_string(), req.id),
        (TAG_PROJECT.to_string(), req.project),
    ])
}

pub(crate) fn machine_name(request_id: String) -> String {
    crate::util::slugify(&format!("pulse-{}", request_id.to_lowercase()), 40)
}

/// Remembers a machine the moment the provider made it, before the address, host key and pairing
/// steps that can still fail: whatever happens after this, the compute worker finds the row and
/// releases the machine instead of leaving it running (CR-002 ran for a day because the Rust port
/// dropped the release the Go daemon did on error).
pub(crate) fn machine_created(req: &crate::provider::ProvisionRequest, m: &Machine) {
    if let Some(s) = &req.store {
        crate::compute::save_machine(s, &req.id, m, &s.now());
    }
}

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

pub(crate) fn provision_finish(
    ctx: &pulse_bus::CancellationToken,
    req: crate::provider::ProvisionRequest,
    m: &mut crate::provider::Machine,
    hub_home: String,
    ip: String,
    hostkeys: Vec<String>,
    log: Box<ProviderLogFn>,
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

pub(crate) fn cli_list(out: String) -> Vec<String> {
    out.split_whitespace().map(ToOwned::to_owned).collect()
}

pub(crate) fn machine_from_detail(
    provider: String,
    name: String,
    detail_json: String,
    resource_group: String,
) -> Option<crate::provider::Machine> {
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
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, new_bare_repo, new_store};
    use crate::test_helpers_cloud::{TEST_DAEMON, install_ssh_hostname};
    use crate::test_helpers_daemon::{exec1, must_one, set_setting};
    use crate::test_helpers_environment::drained_by_login;
    use crate::test_helpers_provider::*;

    const TEST_HOST_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKB root@pulse";

    #[test]
    fn providers_registered() {
        let _serial = crate::test_helpers::serial();
        crate::azure::init();
        crate::aws::init();
        crate::gcp::init();
        crate::multipass::init();
        let names = provider_names();
        let mut want = pulse_store::COMPUTE_PROVIDERS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        want.sort();
        assert_eq!(names, want);
        let mut kinds = std::collections::BTreeSet::new();
        for n in names {
            let p = provider_named(&n).unwrap_or_else(|| panic!("provider {n} missing"));
            assert_eq!(p.name(), n);
            assert!(!p.kind().is_empty(), "provider {n}: empty kind");
            assert!(kinds.insert(p.kind()), "provider {n}: duplicate kind");
            assert!(
                crate::commands::PROVIDER_AUTH
                    .get(n.as_str())
                    .is_some_and(|v| !v.is_empty()),
                "provider {n}: no auth line"
            );
            let size = pulse_store::compute_size_setting(&n);
            assert!(!size.is_empty(), "provider {n}: no size setting");
            assert!(
                pulse_store::COMPUTE_RULES.contains_key(size.as_str()),
                "provider {n}: no size setting default"
            );
        }
    }

    #[test]
    fn host_key_lines() {
        let _serial = crate::test_helpers::serial();
        let console = "[   12.345] cloud-init[812]: -----BEGIN SSH HOST KEY KEYS-----\n\
ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBFAKE root@pulse\n\
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKB root@pulse\n\
-----END SSH HOST KEY KEYS-----\n\
some other noise\n\
pulse-hostkey ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKBLOCKB root@pulse\n";
        let keys = super::host_key_lines(console.to_string());
        assert_eq!(keys.len(), 1, "keys: {keys:?}");
        assert!(
            keys[0].starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBLOCK"),
            "keys: {keys:?}"
        );
        let keys = super::host_key_lines(format!(
            "Enable succeeded: \n[stdout]\n{TEST_HOST_KEY}\n[stderr]\n"
        ));
        assert_eq!(
            keys,
            vec![
                TEST_HOST_KEY
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ")
            ],
            "run-command shape"
        );
        assert!(super::host_key_lines("nothing yet\n".to_string()).is_empty());
    }

    #[test]
    fn cloud_init_config_admin_key() {
        let _serial = crate::test_helpers::serial();
        let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUB pulse-hub";
        let with = cloud_init_config(key.to_string(), true);
        for want in [
            "#cloud-config",
            &format!("name: {ADMIN_USER}"),
            &format!("- {key}"),
            "ALL=(ALL) NOPASSWD:ALL",
            HOSTKEY_MARKER,
            "/dev/console",
            &format!("mkdir -p {HANDS_WORKDIR}"),
            "@github/copilot",
        ] {
            assert!(with.contains(want), "cloud-init lacks {want:?}:\n{with}");
        }
        let without = cloud_init_config(String::new(), true);
        assert!(
            !without.contains(&format!("name: {ADMIN_USER}"))
                && !without.contains("ssh_authorized_keys"),
            "cloud-init without a key still adds a user:\n{without}"
        );
        assert!(
            without.contains(HOSTKEY_MARKER),
            "host key line missing without a key"
        );
        let native = cloud_init_config(key.to_string(), false);
        for absent in [
            "deb.nodesource.com",
            "apt-get install -y nodejs",
            "npm install -g @github/copilot",
        ] {
            assert!(
                !native.contains(absent),
                "native cloud-init has legacy install line {absent:?}:\n{native}"
            );
        }
        assert!(
            native.contains(&format!("mkdir -p {HANDS_WORKDIR}"))
                && native.contains(HOSTKEY_MARKER),
            "native cloud-init lacks base setup:\n{native}"
        );
        let path = write_cloud_init(key.to_string(), true).expect("write cloud-init");
        let data = std::fs::read_to_string(&path).expect("read cloud-init");
        let _ = std::fs::remove_file(path);
        assert_eq!(data, with);
    }

    #[test]
    fn daemon_once_compute_aws() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        let h = new_compute_harness(
            &s,
            &[
                ("compute.provider", "aws"),
                ("compute.aws.region", "eu-west-1"),
                ("compute.aws.profile", "pulse"),
            ],
        );
        let aws = new_fake_aws();
        let _fake = install_fake(crate::aws::set_run_aws, {
            let aws = aws.clone();
            Box::new(move |ctx, args, timeout| aws.run(ctx, args, timeout))
        });
        h.provision_and_release(&s, "aws", "203.0.113.9");

        let run = aws.first("ec2 run-instances");
        for want in [
            &format!("--image-id resolve:ssm:{}", crate::aws::AWS_IMAGE),
            "--instance-type t3.medium",
            "--subnet-id subnet-1",
            "--security-group-ids sg-pulse",
            "--associate-public-ip-address",
            "--user-data file://",
            &format!("VolumeSize={}", HANDS_DISK_GB),
            "HttpTokens=required",
            "Tags=[{Key=Name,Value=pulse-cr-001},{Key=pulse,Value=hands},{Key=pulse-request,Value=CR-001},{Key=pulse-project,Value=proj}]",
            "--profile pulse --region eu-west-1 --output json",
        ] {
            assert!(run.contains(want), "run-instances lacks {want:?}: {run}");
        }
        assert!(
            aws.has("ec2 create-security-group --group-name pulse-hands")
                && aws.has("ec2 authorize-security-group-ingress --group-id sg-pulse --protocol tcp --port 22 --cidr 0.0.0.0/0"),
            "security group: {:?}",
            aws.joined()
        );
        assert_eq!(
            aws.0.lock().unwrap().consoles,
            2,
            "console reads: {:?}",
            aws.joined()
        );
        assert!(
            aws.has("ec2 get-console-output --instance-id i-0abc --latest --query Output --profile pulse --region eu-west-1 --output text"),
            "console reads: {:?}",
            aws.joined()
        );
        assert_eq!(aws.0.lock().unwrap().terminate, vec!["i-0abc"]);
        let (m, _) = crate::compute::load_machine(&s, "CR-001");
        let m = m.expect("machine");
        assert_eq!(
            m.detail.get("instance_id").map(String::as_str),
            Some("i-0abc")
        );
        assert_eq!(
            m.detail.get("region").map(String::as_str),
            Some("eu-west-1")
        );
        assert_eq!(
            m.detail.get("security_group").map(String::as_str),
            Some("sg-pulse")
        );
        assert_eq!(m.detail.get("subnet").map(String::as_str), Some("subnet-1"));
    }

    #[test]
    fn daemon_once_compute_gcp() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        let h = new_compute_harness(
            &s,
            &[
                ("compute.provider", "gcp"),
                ("compute.gcp.zone", "europe-west1-b"),
            ],
        );
        let gcp = new_fake_gcloud();
        let _fake = install_fake(crate::gcp::set_run_gcloud, {
            let gcp = gcp.clone();
            Box::new(move |ctx, args, timeout| gcp.run(ctx, args, timeout))
        });
        h.provision_and_release(&s, "gcp", "203.0.113.11");

        let create = gcp.first("compute instances create");
        for want in [
            "compute instances create pulse-cr-001 --zone europe-west1-b --machine-type e2-medium --image-family ubuntu-2404-lts-amd64 --image-project ubuntu-os-cloud",
            &format!("--boot-disk-size {}GB", HANDS_DISK_GB),
            "--boot-disk-auto-delete",
            "--network default",
            "--labels pulse=hands,pulse-request=cr-001,pulse-project=proj",
            "--tags pulse-hands",
            "--metadata-from-file user-data=",
            "--project pulse-proj --format json",
        ] {
            assert!(create.contains(want), "create lacks {want:?}: {create}");
        }
        assert!(
            gcp.has("compute firewall-rules create pulse-hands-ssh --network default --direction INGRESS --allow tcp:22 --source-ranges 0.0.0.0/0 --target-tags pulse-hands"),
            "firewall: {:?}",
            gcp.joined()
        );
        assert_eq!(
            gcp.0.lock().unwrap().serials,
            2,
            "serial reads: {:?}",
            gcp.joined()
        );
        assert!(
            gcp.has("compute instances get-serial-port-output pulse-cr-001 --zone europe-west1-b --project pulse-proj"),
            "serial reads: {:?}",
            gcp.joined()
        );
        assert_eq!(gcp.0.lock().unwrap().deleted, vec!["pulse-cr-001"]);
        assert!(
            gcp.has("compute instances delete pulse-cr-001 --zone europe-west1-b --quiet --project pulse-proj"),
            "deleted: {:?} {:?}",
            gcp.0.lock().unwrap().deleted,
            gcp.joined()
        );
        let (m, _) = crate::compute::load_machine(&s, "CR-001");
        let m = m.expect("machine");
        assert_eq!(
            m.detail.get("project").map(String::as_str),
            Some("pulse-proj")
        );
        assert_eq!(
            m.detail.get("zone").map(String::as_str),
            Some("europe-west1-b")
        );
        assert_eq!(m.detail.get("type").map(String::as_str), Some("e2-medium"));
    }

    #[test]
    fn daemon_once_compute_multipass() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        let h = new_compute_harness(&s, &[("compute.provider", "multipass")]);
        let mp = new_fake_multipass();
        let _fake = install_fake(crate::multipass::set_run_multipass, {
            let mp = mp.clone();
            Box::new(move |ctx, args, timeout| mp.run(ctx, args, timeout))
        });
        h.provision_and_release(&s, "multipass", "192.168.64.7");

        let launch = mp.first("launch ");
        assert!(
            launch.starts_with("launch 24.04 --name pulse-cr-001 --cloud-init ")
                && launch.ends_with("--timeout 1200 --cpus 2 --memory 4G --disk 32G"),
            "launch: {launch}"
        );
        assert_eq!(
            mp.0.lock().unwrap().execs,
            2,
            "exec reads: {:?}",
            mp.joined()
        );
        assert!(
            mp.has("exec pulse-cr-001 -- cat /etc/ssh/ssh_host_ed25519_key.pub"),
            "exec reads: {:?}",
            mp.joined()
        );
        assert_eq!(mp.0.lock().unwrap().deleted, vec!["pulse-cr-001"]);
        let (m, _) = crate::compute::load_machine(&s, "CR-001");
        let m = m.expect("machine");
        assert_eq!(m.detail.get("image").map(String::as_str), Some("24.04"));
        assert_eq!(m.detail.get("cpus").map(String::as_str), Some("2"));
        assert_eq!(m.detail.get("memory").map(String::as_str), Some("4G"));
        assert_eq!(m.detail.get("disk").map(String::as_str), Some("32G"));
    }

    #[test]
    fn compute_fails_when_the_provider_cli_is_missing() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        let h = new_compute_harness(
            &s,
            &[
                ("compute.provider", "gcp"),
                ("compute.gcp.zone", "europe-west1-b"),
            ],
        );
        let _fake = install_fake(
            crate::gcp::set_run_gcloud,
            Box::new(|_, _, _| {
                Err(anyhow::anyhow!(
                    "the Google Cloud CLI (gcloud) is not installed on this machine"
                ))
            }),
        );
        let req = pulse_tasks::request_compute(
            &s,
            "proj",
            "aurora",
            "",
            2,
            "the fixtures take an hour here",
        )
        .unwrap_or_else(|err| panic!("{err}"));
        pulse_tasks::approve(&s, &req.id, "go ahead", crate::util::HUMAN_AGENT)
            .unwrap_or_else(|err| panic!("{err}"));
        let log = crate::test_helpers::once_daemon(
            &s,
            crate::daemon::DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: nothing'".into(),
                hub_home: h.hub_home.clone(),
                ..Default::default()
            },
        );
        let got = pulse_tasks::get_compute(&s, &req.id).unwrap_or_default();
        if got.status != "failed" || !log.contains("gcloud) is not installed") {
            panic!("request: {got:?}\n{log}");
        }
        if !crate::executor::executor_row(&s, "hands-cr-001")
            .0
            .is_empty()
        {
            panic!("an executor was registered without a machine");
        }
    }

    #[test]
    fn daemon_once_compute_slow_provision() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        let h = new_compute_harness(&s, &[("compute.provider", "multipass")]);
        let _old_wait = set_provision_wait(std::time::Duration::from_millis(1500));
        let mp = new_fake_multipass();
        let _fake = install_fake(crate::multipass::set_run_multipass, {
            let mp = mp.clone();
            Box::new(move |ctx, args, timeout| mp.run(ctx, args, timeout))
        });
        let req = pulse_tasks::request_compute(
            &s,
            "proj",
            "aurora",
            "",
            2,
            "the fixtures take an hour here",
        )
        .expect("request compute");
        pulse_tasks::approve(&s, &req.id, "go ahead", pulse_store::HUMAN_AGENT).expect("approve");
        let log = crate::test_helpers::once_daemon(
            &s,
            crate::daemon::DaemonOpts {
                hub_home: h.hub_home.clone(),
                ..Default::default()
            },
        );
        let got = pulse_tasks::get_compute(&s, &req.id).expect("get compute");
        assert_eq!(
            (got.status.as_str(), got.executor.as_str()),
            ("ready", "hands-cr-001"),
            "slow provision after once-daemon: {got:?}\n{log}"
        );
    }

    const AZURE_SETTINGS: &[(&str, &str)] = &[
        ("compute.provider", "azure"),
        ("compute.azure.resource_group", "pulse-hands"),
        ("compute.azure.location", "westeurope"),
        ("compute.azure.vm_size", "Standard_B2ms"),
    ];

    fn approved_request(s: &pulse_store::Store) -> pulse_tasks::ComputeRequest {
        let req =
            pulse_tasks::request_compute(s, "proj", "aurora", "", 2, "the fixtures take an hour")
                .unwrap_or_else(|err| panic!("{err}"));
        pulse_tasks::approve(s, &req.id, "go ahead", crate::util::HUMAN_AGENT)
            .unwrap_or_else(|err| panic!("{err}"))
    }

    fn once(s: &pulse_store::Store, h: &ComputeHarness) -> String {
        crate::test_helpers::once_daemon(
            s,
            crate::daemon::DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: nothing'".into(),
                hub_home: h.hub_home.clone(),
                ..Default::default()
            },
        )
    }

    fn findings_like(s: &pulse_store::Store, pattern: &str) -> Vec<String> {
        let conn = s.conn().unwrap();
        crate::util::must_rows(
            &conn,
            "SELECT summary FROM posts WHERE kind = 'finding' AND summary LIKE ? ORDER BY id",
            &[&pattern],
        )
        .iter()
        .map(|r| r.str("summary"))
        .collect()
    }

    /// CR-002 on the live hub: the VM was bought, then the request failed for want of a
    /// linux/amd64 pulse binary. The binary is now checked first, so nothing is bought.
    #[test]
    fn compute_buys_nothing_without_a_hands_binary() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        let h = new_compute_harness(&s, AZURE_SETTINGS);
        let az = new_fake_az();
        let _fake = install_fake(crate::azure::set_run_az, {
            let az = az.clone();
            Box::new(move |ctx, args, timeout| az.run(ctx, args, timeout))
        });
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let harness_binary = crate::pair::set_hands_binary(Box::new({
            let asked = asked.clone();
            move |arch| {
                asked.lock().unwrap().push(arch.to_string());
                Err(anyhow::anyhow!(
                    "no linux/{arch} pulse binary to ship: set PULSE_HANDS_BINARY"
                ))
            }
        }));
        let req = approved_request(&s);
        let log = once(&s, &h);
        let _ = crate::pair::set_hands_binary(harness_binary);
        let got = pulse_tasks::get_compute(&s, &req.id).unwrap_or_default();
        assert_eq!(got.status, "failed", "request: {got:?}\n{log}");
        assert_eq!(
            *asked.lock().unwrap(),
            vec!["amd64"],
            "Standard_B2ms is amd64"
        );
        assert!(
            !az.has("vm create") && !az.has("group create") && az.deleted().is_empty(),
            "az was asked to make something: {:?}",
            az.calls()
        );
        let found = findings_like(&s, "compute CR-001 failed: no linux/amd64 pulse binary%");
        assert!(
            found.len() == 1 && found[0].ends_with("; no machine was bought"),
            "findings: {found:?}\n{log}"
        );
        assert!(
            crate::executor::executor_row(&s, "hands-cr-001")
                .0
                .is_empty(),
            "an executor was registered without a machine"
        );
    }

    /// A failure after the machine exists releases it (VM and its siblings) before the request
    /// is failed, as the Go runner did; the machine row keeps the released time.
    #[test]
    fn compute_releases_what_a_failed_provisioning_created() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        let h = new_compute_harness(&s, AZURE_SETTINGS);
        let az = new_fake_az();
        let _fake = install_fake(crate::azure::set_run_az, {
            let az = az.clone();
            Box::new(move |ctx, args, timeout| {
                if args.join(" ").starts_with("vm run-command invoke") {
                    return Ok((1, String::new(), "RunCommand is disabled here".into()));
                }
                az.run(ctx, args, timeout)
            })
        });
        let req = approved_request(&s);
        let log = once(&s, &h);
        let got = pulse_tasks::get_compute(&s, &req.id).unwrap_or_default();
        assert_eq!(got.status, "failed", "request: {got:?}\n{log}");
        assert_eq!(az.deleted(), vec!["pulse-cr-001"], "{log}");
        let tags = az.0.lock().unwrap().tags.get("pulse-cr-001").cloned();
        assert_eq!(
            tags.as_ref().and_then(|t| t.get(TAG_REQUEST).cloned()),
            Some("CR-001".to_string()),
            "the VM carries its request tag: {tags:?}"
        );
        let (m, row) = crate::compute::load_machine(&s, &req.id);
        assert!(
            m.is_some_and(|m| m.name == "pulse-cr-001") && !row.str("released_at").is_empty(),
            "machine row after the failure: {row:?}"
        );
        let found = findings_like(&s, "compute CR-001 failed:%");
        assert!(
            found.len() == 1
                && found[0].contains("RunCommand is disabled here")
                && found[0].ends_with("; what was created is released"),
            "findings: {found:?}\n{log}"
        );
        assert!(
            crate::executor::executor_row(&s, "hands-cr-001")
                .0
                .is_empty(),
            "an executor was registered for a failed machine"
        );
    }

    /// Two ways a daemon restart mid-create leaves a request `provisioning`: the machine was
    /// recorded (released at once) or not yet (the next sweep finds it by its tag).
    #[test]
    fn compute_stale_provisioning_is_released_now_or_by_the_sweep() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        let h = new_compute_harness(&s, AZURE_SETTINGS);
        let az = new_fake_az();
        let _fake = install_fake(crate::azure::set_run_az, {
            let az = az.clone();
            Box::new(move |ctx, args, timeout| az.run(ctx, args, timeout))
        });
        let recorded = approved_request(&s);
        let forgotten = approved_request(&s);
        for r in [&recorded, &forgotten] {
            pulse_tasks::set_compute_status(&s, &r.id, "provisioning", "", TEST_DAEMON)
                .unwrap_or_else(|err| panic!("{err}"));
            az.leak(&format!("pulse-{}", r.id.to_lowercase()), &r.id, "proj");
        }
        crate::compute::save_machine(
            &s,
            &recorded.id,
            &Machine {
                provider: "azure".into(),
                name: "pulse-cr-001".into(),
                executor: "hands-cr-001".into(),
                detail: BTreeMap::from([("resource_group".to_string(), "pulse-hands".to_string())]),
                ..Default::default()
            },
            TEST_NOW,
        );
        let log = once(&s, &h);
        for r in [&recorded, &forgotten] {
            let got = pulse_tasks::get_compute(&s, &r.id).unwrap_or_default();
            assert_eq!(got.status, "failed", "{}: {got:?}\n{log}", r.id);
        }
        assert_eq!(
            az.deleted(),
            vec!["pulse-cr-001"],
            "first daemon run\n{log}"
        );
        let found = findings_like(&s, "compute CR-00_ failed: the daemon stopped%");
        assert!(
            found.len() == 2
                && found.iter().any(|f| f.starts_with("compute CR-001") && f.ends_with("; what was created is released"))
                && found.iter().any(|f| f.starts_with("compute CR-002") && f.contains("released by the sweep")),
            "findings: {found:?}\n{log}"
        );
        // the sweep of that run saw CR-002 still provisioning; the next one releases it
        let log = once(&s, &h);
        assert_eq!(
            az.deleted(),
            vec!["pulse-cr-001", "pulse-cr-002"],
            "second run\n{log}"
        );
        let conn = s.conn().unwrap();
        let swept = crate::util::must_rows(
            &conn,
            "SELECT subject, detail FROM events WHERE action = 'compute.leftover-released' ORDER BY seq",
            &[],
        );
        assert!(
            swept.len() == 1
                && swept[0].str("subject") == "CR-002"
                && swept[0]
                    .str("detail")
                    .contains("\"name\": \"pulse-cr-002\""),
            "leftover events: {swept:?}\n{log}"
        );
    }

    /// The sweep never calls the provider on a hub that has not bought a machine, releases only
    /// machines tagged with a request of this store that is over, and leaves the rest alone.
    #[test]
    fn compute_sweep_releases_only_what_is_over() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        let h = new_compute_harness(&s, AZURE_SETTINGS);
        let az = new_fake_az();
        let _fake = install_fake(crate::azure::set_run_az, {
            let az = az.clone();
            Box::new(move |ctx, args, timeout| az.run(ctx, args, timeout))
        });
        az.leak("pulse-cr-001", "CR-001", "proj");
        az.leak("pulse-cr-002", "CR-002", "proj");
        az.leak("pulse-cr-009", "CR-009", "other-store");
        let over = pulse_tasks::request_compute(&s, "proj", "aurora", "", 2, "done long ago")
            .unwrap_or_else(|err| panic!("{err}"));
        let live = pulse_tasks::request_compute(&s, "proj", "aurora", "", 2, "in use")
            .unwrap_or_else(|err| panic!("{err}"));
        let log = once(&s, &h);
        assert!(
            !az.has("vm list") && az.deleted().is_empty(),
            "nothing was ever bought here, yet az was asked: {:?}\n{log}",
            az.calls()
        );
        pulse_tasks::set_compute_status(&s, &over.id, "failed", "", TEST_DAEMON)
            .unwrap_or_else(|err| panic!("{err}"));
        pulse_tasks::set_compute_status(&s, &live.id, "ready", "hands-cr-002", TEST_DAEMON)
            .unwrap_or_else(|err| panic!("{err}"));
        crate::compute::save_machine(
            &s,
            &live.id,
            &Machine {
                provider: "azure".into(),
                name: "pulse-cr-002".into(),
                executor: "hands-cr-002".into(),
                ..Default::default()
            },
            &s.now(),
        );
        let log = once(&s, &h);
        assert!(
            az.has("vm list -d --query"),
            "no sweep: {:?}\n{log}",
            az.calls()
        );
        assert_eq!(az.deleted(), vec!["pulse-cr-001"], "{log}");
        let found = findings_like(&s, "compute CR-001: released pulse-cr-001 (azure)%");
        assert_eq!(found.len(), 1, "findings: {found:?}\n{log}");
        let alive: Vec<String> = az.0.lock().unwrap().created.keys().cloned().collect();
        assert_eq!(alive, vec!["pulse-cr-002", "pulse-cr-009"], "left alone");
    }

    #[test]
    fn hands_arch_follows_the_size_family() {
        for (provider, size, want) in [
            ("azure", "Standard_B2ms", "amd64"),
            ("azure", "Standard_D2s_v5", "amd64"),
            ("azure", "Standard_D2ps_v5", "arm64"),
            ("azure", "Standard_B2pts_v2", "arm64"),
            ("azure", "Standard_E4pds_v5", "arm64"),
            ("azure", "Standard_NP10s", "amd64"),
            ("azure", "", "amd64"),
            ("aws", "t3.medium", "amd64"),
            ("aws", "t4g.medium", "arm64"),
            ("aws", "c6gn.large", "arm64"),
            ("aws", "a1.large", "arm64"),
            ("aws", "g4dn.xlarge", "amd64"),
            ("gcp", "e2-medium", "amd64"),
            ("gcp", "t2a-standard-2", "arm64"),
            ("gcp", "c4a-standard-4", "arm64"),
            ("nowhere", "anything", "amd64"),
        ] {
            assert_eq!(
                crate::compute::hands_arch_for_size(provider, size),
                want,
                "{provider} {size}"
            );
        }
    }

    #[test]
    fn tend_retires_and_deletes_our_ec2_instance() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        let s = new_store();
        new_bare_repo(&s);
        set_setting(&s, "compute.provider", "aws");
        set_setting(&s, "compute.aws.region", "eu-west-1");
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        let aws = new_fake_aws();
        aws.0.lock().unwrap().instances.insert(
            "i-0777".into(),
            AwsFake {
                id: "i-0777".into(),
                name: "pulse-cr-007".into(),
                request: "CR-007".into(),
                project: "proj".into(),
                state: "running".into(),
                ip: "203.0.113.7".into(),
            },
        );
        aws.0.lock().unwrap().describes = 5;
        let _fake = install_fake(crate::aws::set_run_aws, {
            let aws = aws.clone();
            Box::new(move |ctx, args, timeout| aws.run(ctx, args, timeout))
        });
        let _host = install_ssh_hostname(|_host| "203.0.113.7".into());
        let _stub = stub_gh("");

        let lines = crate::retire::tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        let calls = aws.joined();
        if calls.len() != 2
            || calls[0]
                != "ec2 describe-instances --filters Name=tag:pulse,Values=hands Name=instance-state-name,Values=pending,running,stopping,stopped --region eu-west-1 --output json"
            || calls[1]
                != "ec2 stop-instances --instance-ids i-0777 --region eu-west-1 --output json"
        {
            panic!("aws calls: {calls:?}");
        }
        let r = must_one(
            &s,
            "SELECT * FROM retired_vms WHERE executor = 'vm-seven'",
            &[],
        );
        if r.0.is_empty()
            || r.str("provider") != "aws"
            || r.str("name") != "pulse-cr-007"
            || r.str("delete_after") != crate::util::plus_hours(TEST_NOW, 24.0)
            || !r.str("detail").contains(r#""instance_id":"i-0777""#)
        {
            panic!("retired_vms: {r:?}");
        }
        let ev = must_one(
            &s,
            "SELECT * FROM events WHERE action = 'executor.retired' AND subject = 'vm-seven'",
            &[],
        );
        let d = crate::util::json_map(&ev.str("detail"));
        if d["provider"] != "aws" || d["instance_id"] != "i-0777" || d["place"] != "eu-west-1" {
            panic!("event: {ev:?}");
        }
        let p = must_one(
            &s,
            "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'",
            &[],
        );
        if p.str("summary")
            != format!(
                "retired vm-seven: login refused; EC2 instance pulse-cr-007 (eu-west-1) stopped now, deleted at {} unless kept",
                crate::util::plus_hours(TEST_NOW, 24.0)
            )
        {
            panic!("summary: {:?}", p.str("summary"));
        }
        if !p.str("body").contains("Undo: `aws ec2 start-instances --instance-ids i-0777 --region eu-west-1` then `pulse executor activate vm-seven`") {
            panic!("body:\n{}", p.str("body"));
        }
        if !lines
            .join("\n")
            .contains("retired vm-seven: stopped pulse-cr-007 (eu-west-1)")
        {
            panic!("lines: {lines:?}");
        }

        // At the grace the instance is terminated through the row's provider, whatever compute.provider says now.
        set_setting(&s, "compute.provider", "azure");
        aws.reset();
        let later = crate::util::plus_hours(TEST_NOW, 24.0);
        let lines = crate::retire::tend_executors(&s, later.clone(), TEST_DAEMON.into());
        let calls = aws.joined();
        if calls.len() != 2
            || !calls[0].starts_with("ec2 describe-instances --filters Name=instance-id,Values=i-0777 --region eu-west-1")
            || calls[1] != "ec2 terminate-instances --instance-ids i-0777 --region eu-west-1 --output json"
        {
            panic!("delete calls: {calls:?}");
        }
        let r = must_one(
            &s,
            "SELECT * FROM retired_vms WHERE executor = 'vm-seven'",
            &[],
        );
        if r.str("deleted_at") != later || !r.str("note").is_empty() {
            panic!("row: {r:?}");
        }
        if must_one(
            &s,
            "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute' AND summary = 'deleted pulse-cr-007 (eu-west-1)'",
            &[],
        )
        .0
        .is_empty()
        {
            panic!(
                "delete finding missing: {:?}",
                crate::test_helpers_daemon::must_rows(&s, "SELECT summary FROM posts", &[])
            );
        }
        if !lines
            .join("\n")
            .contains("deleted pulse-cr-007 (eu-west-1): instance i-0777 with its root volume")
        {
            panic!("lines: {lines:?}");
        }
        // Already gone: closed without a terminate.
        aws.reset();
        exec1(
            &s,
            "INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after, detail) VALUES ('vm-gone', 'aws', '', 'pulse-cr-gone', ?, ?, ?)",
            &[
                &TEST_NOW,
                &TEST_NOW,
                &r#"{"detail":{"instance_id":"i-gone","region":"eu-west-1"}}"#,
            ],
        );
        let lines = crate::retire::tend_executors(&s, later.clone(), TEST_DAEMON.into());
        let calls = aws.joined();
        if calls.len() != 1
            || !calls[0]
                .starts_with("ec2 describe-instances --filters Name=instance-id,Values=i-gone")
        {
            panic!("missing instance calls: {calls:?}");
        }
        let r = must_one(
            &s,
            "SELECT * FROM retired_vms WHERE executor = 'vm-gone'",
            &[],
        );
        if r.str("note") != "vm not found at delete time" {
            panic!("row: {r:?}");
        }
        let _ = lines;
    }

    #[test]
    fn tend_retires_and_deletes_our_gce_instance() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        crate::commands::register();
        let s = new_store();
        new_bare_repo(&s);
        s.set_setting(crate::util::HUMAN_AGENT, "compute.provider", "gcp")
            .unwrap();
        s.set_setting(
            crate::util::HUMAN_AGENT,
            "compute.gcp.project",
            "pulse-proj",
        )
        .unwrap();
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        let gcp = new_fake_gcloud();
        gcp.0.lock().unwrap().instances.insert(
            "pulse-cr-007".into(),
            GcpFake {
                name: "pulse-cr-007".into(),
                status: "RUNNING".into(),
                ip: "203.0.113.7".into(),
                request: "cr-007".into(),
                project: "proj".into(),
            },
        );
        let _fake = install_fake(crate::gcp::set_run_gcloud, {
            let gcp = gcp.clone();
            Box::new(move |ctx, args, timeout| gcp.run(ctx, args, timeout))
        });
        let _host = install_ssh_hostname(|_| "203.0.113.7".into());
        let lines = crate::retire::tend_executors(&s, TEST_NOW.into(), "daemon".into());
        let calls = gcp.joined();
        assert_eq!(
            calls,
            vec![
                "compute instances list --filter=labels.pulse=hands --project pulse-proj --format json",
                "compute instances stop pulse-cr-007 --zone europe-west1-b --quiet --project pulse-proj",
            ],
            "gcloud calls"
        );
        assert!(
            lines
                .join("\n")
                .contains("retired vm-seven: stopped pulse-cr-007 (europe-west1-b)"),
            "lines: {lines:?}"
        );
        gcp.reset();
        let later = crate::util::plus_hours(TEST_NOW, 24.0);
        let lines = crate::retire::tend_executors(&s, later, "daemon".into());
        let calls = gcp.joined();
        assert_eq!(
            calls,
            vec![
                "compute instances describe pulse-cr-007 --zone europe-west1-b --project pulse-proj --format json",
                "compute instances delete pulse-cr-007 --zone europe-west1-b --quiet --project pulse-proj",
            ],
            "delete calls"
        );
        assert!(
            lines.join("\n").contains(
                "deleted pulse-cr-007 (europe-west1-b): instance pulse-cr-007 with its boot disk"
            ),
            "lines: {lines:?}"
        );
    }

    #[test]
    fn tend_retires_and_deletes_our_multipass_instance() {
        let _serial = crate::test_helpers::serial();
        let _lock = provider_test_lock();
        crate::commands::register();
        let s = new_store();
        new_bare_repo(&s);
        s.set_setting(crate::util::HUMAN_AGENT, "compute.provider", "multipass")
            .unwrap();
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        drained_by_login(&s, "vm-mine", "pulse@192.168.64.3");
        let mp = new_fake_multipass();
        mp.0.lock().unwrap().instances.insert(
            "pulse-cr-007".into(),
            MpFake {
                name: "pulse-cr-007".into(),
                state: "Running".into(),
                ip: "192.168.64.7".into(),
            },
        );
        mp.0.lock().unwrap().instances.insert(
            "primary".into(),
            MpFake {
                name: "primary".into(),
                state: "Running".into(),
                ip: "192.168.64.3".into(),
            },
        );
        let _fake = install_fake(crate::multipass::set_run_multipass, {
            let mp = mp.clone();
            Box::new(move |ctx, args, timeout| mp.run(ctx, args, timeout))
        });
        let _host = install_ssh_hostname(|host| {
            if host == "vm-seven" {
                "192.168.64.7".into()
            } else {
                host.into()
            }
        });
        let lines = crate::retire::tend_executors(&s, TEST_NOW.into(), "daemon".into());
        let calls = mp.joined();
        assert_eq!(
            calls,
            vec![
                "list --format json",
                "list --format json",
                "stop pulse-cr-007"
            ],
            "multipass calls"
        );
        let text = lines.join("\n");
        assert!(
            text.contains("retired vm-seven: stopped pulse-cr-007 (this machine)")
                && text.contains("left running because PULSE did not create it (no Multipass instance tagged pulse=hands is named vm-mine or answers at 192.168.64.3)"),
            "lines: {lines:?}"
        );
        mp.reset();
        let later = crate::util::plus_hours(TEST_NOW, 24.0);
        let lines = crate::retire::tend_executors(&s, later, "daemon".into());
        assert_eq!(
            mp.joined(),
            vec![
                "info pulse-cr-007 --format json",
                "delete --purge pulse-cr-007"
            ],
            "delete calls"
        );
        assert!(
            lines.join("\n").contains(
                "deleted pulse-cr-007 (this machine): instance pulse-cr-007 with its disk"
            ),
            "lines: {lines:?}"
        );
        assert!(
            mp.0.lock().unwrap().instances.contains_key("primary"),
            "the owner's own VM was deleted"
        );
    }
}
