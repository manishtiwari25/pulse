#![allow(dead_code)]

use anyhow::anyhow;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const PAIR_DROPIN_PATH: &str = "/etc/ssh/sshd_config.d/60-pulse-hands.conf";
pub(crate) const PAIR_HANDS_PATH: &str = "/usr/local/bin/pulse-hands";
pub(crate) const PAIR_PULSE_PATH: &str = "/usr/local/bin/pulse";
pub(crate) static STEP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^step=([A-Za-z0-9_-]+)[ \t]+result=([A-Za-z0-9_-]+)").unwrap()
});

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SshBlock {
    pub(crate) names: Vec<String>,
    pub(crate) hostname: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PairOpts {
    pub workdir: String,
    pub workers: i32,
    pub projects: String,
    pub install_copilot: bool,
    pub dry_run: bool,
    pub actor: String,
    pub hub_home: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PairReport {
    pub ok: bool,
    pub lines: Vec<String>,
    pub scripts: BTreeMap<String, String>,
    pub changed: Vec<String>,
}

pub(crate) type HandsBinaryFn = dyn Fn(&str) -> anyhow::Result<(Vec<u8>, String)> + Send + Sync;
pub(crate) static HANDS_BINARY: LazyLock<RwLock<Box<HandsBinaryFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(default_hands_binary)));

pub(crate) fn set_hands_binary(f: Box<HandsBinaryFn>) -> Box<HandsBinaryFn> {
    std::mem::replace(
        &mut *HANDS_BINARY.write().expect("hands_binary poisoned"),
        f,
    )
}

/// A checkout to cross-build the hands binary from when neither the working directory nor the
/// binary's own folder is inside one: the daemon sets it to the brain project's path, the
/// checkout it builds itself from.
pub(crate) static HANDS_SOURCE_ROOT: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new(String::new()));

pub(crate) fn set_hands_source_root(root: String) {
    *HANDS_SOURCE_ROOT
        .write()
        .expect("hands_source_root poisoned") = root;
}

pub(crate) fn valid_name(name: String) -> bool {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^[a-z][a-z0-9-]*$").unwrap());
    RE.is_match(&name)
}

pub(crate) fn pair_dropin_text() -> String {
    [
        "# The hands' user gets exactly what its authorized_keys line allows and nothing else.",
        "Match User pulse",
        "    PasswordAuthentication no",
        "    AllowAgentForwarding no",
        "    AllowTcpForwarding no",
        "    X11Forwarding no",
        "    PermitTTY no",
        "    PermitTunnel no",
        "",
    ]
    .join("\n")
}

pub(crate) fn pair_authorized_key_line(workdir: String, pubkey: String) -> String {
    format!(
        "command=\"PULSE_HANDS_WORKDIR={} {} hands\",restrict {}",
        workdir,
        PAIR_HANDS_PATH,
        pubkey.trim()
    )
}

pub(crate) fn pair_parse_inspection(text: String) -> (BTreeMap<String, String>, Vec<String>) {
    let mut fields = BTreeMap::new();
    let mut hostkeys = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("hostkey ") {
            hostkeys.push(rest.trim().to_string());
        } else if let Some((k, v)) = line.split_once('=') {
            fields.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    (fields, hostkeys)
}

pub(crate) fn pair_tail(text: String) -> String {
    crate::executor::tail(&text.split_whitespace().collect::<Vec<_>>().join(" "), 500)
}

pub(crate) fn pair_refuse(format: String, args: &[serde_json::Value]) -> anyhow::Result<()> {
    let mut msg = format;
    for a in args {
        msg = msg.replacen("%s", a.as_str().unwrap_or(&a.to_string()), 1);
    }
    Err(crate::util::fail(2, msg).into())
}

pub(crate) fn pair_validate(
    name: String,
    admin: String,
    workdir: String,
    workers: i32,
) -> anyhow::Result<(String, String, String, i32, String)> {
    let name = crate::util::slugify(&name, 40);
    if name.is_empty()
        || name == crate::executor::LOCAL_EXECUTOR
        || name == "x"
        || !valid_name(name.clone())
    {
        return Err(crate::util::fail(
            2,
            format!(
                "Executor name must be lowercase words with dashes and not {}",
                crate::board::py_repr(crate::executor::LOCAL_EXECUTOR.into())
            ),
        )
        .into());
    }
    let (user, host, port) = parse_pair_ssh_address(&admin)?;
    let mut workdir = workdir.trim().trim_end_matches('/').to_string();
    if workdir.is_empty() {
        workdir = "/".into();
    }
    if !workdir.starts_with('/') || workdir.split('/').any(|p| p == "..") {
        return Err(crate::util::fail(
            2,
            "--workdir must be an absolute path on the executor (e.g. /srv/pulse)",
        )
        .into());
    }
    if workers < 1 {
        return Err(crate::util::fail(2, "--workers must be at least 1").into());
    }
    Ok((name, user, host, port, workdir))
}

pub(crate) fn pair_ssh_config_blocks(text: String) -> Vec<SshBlock> {
    let mut blocks = Vec::new();
    let mut cur: Option<SshBlock> = None;
    for line in text.lines() {
        let words = line.split_whitespace().collect::<Vec<_>>();
        if !words.is_empty() && words[0].eq_ignore_ascii_case("host") {
            if let Some(b) = cur.take() {
                blocks.push(b);
            }
            cur = Some(SshBlock {
                names: words[1..].iter().map(|s| s.to_string()).collect(),
                hostname: String::new(),
            });
        } else if let Some(c) = cur.as_mut()
            && words.len() >= 2
            && words[0].eq_ignore_ascii_case("hostname")
        {
            c.hostname = words[1].into();
        }
    }
    if let Some(b) = cur {
        blocks.push(b);
    }
    blocks
}

pub(crate) fn pair_alias_state(config: String, name: String) -> Option<Box<SshBlock>> {
    pair_ssh_config_blocks(config)
        .into_iter()
        .find(|b| b.names.iter().any(|n| n == &name))
        .map(Box::new)
}

pub(crate) fn pair_host_block(name: String, host: String, port: i32) -> String {
    [
        format!("Host {name}"),
        format!("    HostName {host}"),
        format!("    Port {port}"),
        "    User pulse".into(),
        "    IdentityFile ~/.ssh/pulse_hands_ed25519".into(),
        "    IdentitiesOnly yes".into(),
        String::new(),
    ]
    .join("\n")
}

pub(crate) fn pair_known_host_prefix(host: String, port: i32) -> String {
    if port == 22 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

pub(crate) fn pair_manual_dropin_lines() -> Vec<String> {
    let mut out = vec![format!("manual sshd Match block for {PAIR_DROPIN_PATH}:")];
    out.extend(
        pair_dropin_text()
            .trim_end_matches('\n')
            .lines()
            .map(ToOwned::to_owned),
    );
    out
}

pub(crate) fn pair_manual_copilot_lines() -> Vec<String> {
    vec![
        "manual copilot install steps for this machine:".into(),
        "  mkdir -p /etc/apt/keyrings".into(),
        "  curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg".into(),
        "  echo \"deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main\" > /etc/apt/sources.list.d/nodesource.list".into(),
        "  apt-get update".into(),
        "  apt-get install -y nodejs".into(),
        "  npm install -g @github/copilot".into(),
    ]
}

pub(crate) fn pair_inspect_script(
    workdir: String,
    pubkey: String,
    hands_hash: String,
    dropin_hash: String,
) -> String {
    format!(
        r#"#!/bin/sh
set -u
WORKDIR={workdir_q}
PUBKEY={pubkey_q}
DROPIN_HASH={dropin_hash_q}
DROPIN={dropin_q}
printf 'uname=%s\n' "$(uname -s 2>/dev/null || echo unknown)"
printf 'arch=%s\n' "$(uname -m 2>/dev/null || echo unknown)"
if sudo -n true 2>/dev/null; then echo sudo=yes; else echo sudo=no; fi
if getent passwd pulse >/dev/null 2>&1; then echo user=present; else echo user=absent; fi
{hands_script}if [ -d "$WORKDIR" ]; then
  echo workdir=present
  if command -v stat >/dev/null 2>&1; then
    owner="$(stat -c '%U:%G:%a' "$WORKDIR" 2>/dev/null || ls -ld "$WORKDIR" | awk '{{print $3 ":" $4}}')"
  else
    owner="$(ls -ld "$WORKDIR" | awk '{{print $3 ":" $4}}')"
  fi
  printf 'workdir_owner=%s\n' "$owner"
else
  echo workdir=absent
  echo workdir_owner=
fi
auth=absent
auth_lines=0
home="$(getent passwd pulse 2>/dev/null | awk -F: '{{print $6}}')"
ak="$home/.ssh/authorized_keys"
if [ -n "$home" ] && [ -f "$ak" ]; then
  auth_lines="$(wc -l < "$ak" | tr -d ' ')"
  if [ "$auth_lines" = "1" ] && grep -F "$PUBKEY" "$ak" >/dev/null 2>&1; then auth=ours; else auth=other; fi
fi
printf 'authorized_keys=%s\n' "$auth"
printf 'authorized_keys_lines=%s\n' "$auth_lines"
if [ -f "$DROPIN" ]; then
  if [ -n "$DROPIN_HASH" ] && command -v sha256sum >/dev/null 2>&1 && [ "$(sha256sum "$DROPIN" | awk '{{print $1}}')" = "$DROPIN_HASH" ]; then
    echo dropin=present
  else
    echo dropin=differs
  fi
else
  echo dropin=absent
fi
if [ -d /etc/ssh/sshd_config.d ] && grep -E '^[[:space:]]*Include[[:space:]].*sshd_config\.d/\*\.conf' /etc/ssh/sshd_config >/dev/null 2>&1; then
  echo dropin_dir=yes
else
  echo dropin_dir=no
fi
remotes=0
if [ -d "$WORKDIR" ]; then
  remotes="$(find "$WORKDIR" -mindepth 2 -maxdepth 3 -type d -name .git 2>/dev/null | while read d; do git --git-dir="$d" remote 2>/dev/null | grep -q . && echo x; done | wc -l | tr -d ' ')"
fi
printf 'remotes=%s\n' "$remotes"
if command -v apt-get >/dev/null 2>&1; then echo apt=yes; else echo apt=no; fi
if command -v copilot >/dev/null 2>&1; then echo copilot=present; else echo copilot=missing; fi
if command -v node >/dev/null 2>&1; then echo node=present; else echo node=missing; fi
if command -v pulse >/dev/null 2>&1; then echo pulse=present; else echo pulse=missing; fi
if [ -n "${{COPILOT_GITHUB_TOKEN:-}}${{GH_TOKEN:-}}${{GITHUB_TOKEN:-}}" ] || su -s /bin/sh pulse -c 'test -f "$HOME/.pulse/private/providers/copilot.json"' >/dev/null 2>&1; then echo harness=yes; else echo harness=no; fi
if command -v git >/dev/null 2>&1; then echo git=present; else echo git=missing; fi
if su -s /bin/sh pulse -c 'command -v mise >/dev/null 2>&1 || test -x "$HOME/.local/bin/mise"' >/dev/null 2>&1; then
  printf 'mise=%s\n' "$(su -s /bin/sh pulse -c 'PATH="$HOME/.local/bin:$PATH" mise version 2>/dev/null' | awk '{{print $1}}' | head -1)"
else
  echo mise=missing
fi
if su -s /bin/sh pulse -c 'test "$(git config --global --get commit.gpgsign 2>/dev/null)" = false && test "$(git config --global --get core.askPass 2>/dev/null)" = ""' >/dev/null 2>&1; then
  echo gitconfig=ok
else
  echo gitconfig=missing
fi
echo hostkeys=
for key in /etc/ssh/ssh_host_*_key.pub; do
  [ -f "$key" ] || continue
  printf 'hostkey %s\n' "$(cat "$key")"
done
"#,
        workdir_q = crate::util::shell_quote(&workdir),
        pubkey_q = crate::util::shell_quote(&pubkey),
        dropin_hash_q = crate::util::shell_quote(&dropin_hash),
        dropin_q = crate::util::shell_quote(PAIR_DROPIN_PATH),
        hands_script =
            crate::pair_inspect::pair_inspect_hands_script(PAIR_HANDS_PATH.into(), hands_hash),
    )
}

pub(crate) fn mise_install_block(user: String, home_var: String) -> String {
    let bare = crate::recipe::MISE_VERSION.trim_start_matches('v');
    format!(
        r#"
mise_dst="{home_var}/.local/bin/mise"
if [ -x "$mise_dst" ] && [ "$(sudo -u {user} -H "$mise_dst" version 2>/dev/null | awk '{{print $1}}' | head -1)" = "{bare}" ]; then
  step mise kept
else
  case "$(uname -m)" in
    x86_64) mise_arch=x64; mise_sum=c98423c8470d6dc416d9f7036d0646d8ef5ae92ad9186907f8fcc84cbe7db4ea ;;
    aarch64|arm64) mise_arch=arm64; mise_sum=0ef0a778eaa8599f3e90a8a0979c9fc3f79922cafb5fa6d39f366d974da33bba ;;
    *) mise_arch=""; mise_sum="" ;;
  esac
  if [ -z "$mise_arch" ]; then
    step mise skipped
  else
    mise_tmp="$(mktemp)"
    if curl -fsSL -o "$mise_tmp" "https://github.com/jdx/mise/releases/download/{ver}/mise-{ver}-linux-$mise_arch" &&
       [ "$(sha256sum "$mise_tmp" | awk '{{print $1}}')" = "$mise_sum" ]; then
      install -d -m 0755 -o {user} -g {user} "{home_var}/.local" "{home_var}/.local/bin"
      install -m 0755 -o {user} -g {user} "$mise_tmp" "$mise_dst"
      rm -f "$mise_tmp"
      step mise done
    else
      rm -f "$mise_tmp"
      step mise failed
    fi
  fi
fi
"#,
        ver = crate::recipe::MISE_VERSION
    )
}

pub(crate) fn pair_apply_script(
    name: String,
    workdir: String,
    pubkey: String,
    hands: Vec<u8>,
    install_copilot: bool,
) -> String {
    let install_block = if install_copilot {
        r#"
if command -v copilot >/dev/null 2>&1; then
  step copilot kept
else
  mkdir -p /etc/apt/keyrings
  if [ ! -f /etc/apt/keyrings/nodesource.gpg ]; then
    curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg
  fi
  echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main" > /etc/apt/sources.list.d/nodesource.list
  apt-get update
  apt-get install -y nodejs
  npm install -g @github/copilot
  step copilot done
fi
"#.to_string()
    } else {
        "step copilot skipped\n".into()
    };
    format!(
        r#"#!/bin/sh
set -eu
NAME={name_q}
WORKDIR={workdir_q}
AUTH_LINE={auth_q}
DROPIN={dropin_q}
step() {{ printf 'step=%s result=%s\n' "$1" "$2"; }}
if getent passwd pulse >/dev/null 2>&1; then
  step user kept
else
  useradd -m -s /bin/bash pulse
  passwd -l pulse >/dev/null 2>&1 || true
  step user done
fi
home="$(getent passwd pulse | awk -F: '{{print $6}}')"
base64 -d > {hands_path}.tmp <<'PULSE_HANDS_EOF'
{hands_b64}
PULSE_HANDS_EOF
if [ -f {hands_path} ] && cmp -s {hands_path}.tmp {hands_path}; then
  rm -f {hands_path}.tmp
  step hands kept
else
  install -m 0755 -o root -g root {hands_path}.tmp {hands_path}
  rm -f {hands_path}.tmp
  step hands done
fi
ln -sf {hands_path} {pulse_path}
step pulse done
if [ -d "$WORKDIR" ]; then
  chown pulse:pulse "$WORKDIR"
  chmod 0750 "$WORKDIR"
  step workdir kept
else
  install -d -m 0750 -o pulse -g pulse "$WORKDIR"
  step workdir done
fi
install -d -m 0700 -o pulse -g pulse "$home/.ssh"
tmp_auth="$home/.ssh/authorized_keys.tmp"
printf '%s\n' "$AUTH_LINE" > "$tmp_auth"
if [ -f "$home/.ssh/authorized_keys" ] && cmp -s "$tmp_auth" "$home/.ssh/authorized_keys"; then
  rm -f "$tmp_auth"
  step authorized_keys kept
else
  install -m 0600 -o pulse -g pulse "$tmp_auth" "$home/.ssh/authorized_keys"
  rm -f "$tmp_auth"
  step authorized_keys done
fi
sudo -u pulse -H git config --global commit.gpgsign false
sudo -u pulse -H git config --global core.askPass ""
step gitconfig done
{install_block}{mise}install -d -m 0755 -o root -g root /etc/ssh/sshd_config.d
cat > "$DROPIN.tmp" <<'PULSE_DROPIN_EOF'
{dropin_text}PULSE_DROPIN_EOF
dropin_changed=0
if [ -f "$DROPIN" ] && cmp -s "$DROPIN.tmp" "$DROPIN"; then
  rm -f "$DROPIN.tmp"
  step dropin kept
else
  if [ -f "$DROPIN" ]; then
    cp -p "$DROPIN" "$DROPIN.prev"
  else
    rm -f "$DROPIN.prev"
  fi
  install -m 0644 -o root -g root "$DROPIN.tmp" "$DROPIN"
  rm -f "$DROPIN.tmp"
  dropin_changed=1
  step dropin done
fi
if [ "$dropin_changed" = "1" ]; then
  if ! sshd_out="$(sshd -t 2>&1)"; then
    printf 'sshd -t failed: %s\n' "$sshd_out" >&2
    if [ -f "$DROPIN.prev" ]; then
      mv -f "$DROPIN.prev" "$DROPIN"
    else
      rm -f "$DROPIN"
    fi
    exit 3
  fi
  systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || systemctl restart ssh 2>/dev/null || systemctl restart sshd
  step sshd_reload done
else
  step sshd_reload skipped
fi
"#,
        name_q = crate::util::shell_quote(&name),
        workdir_q = crate::util::shell_quote(&workdir),
        auth_q = crate::util::shell_quote(&pair_authorized_key_line(workdir, pubkey)),
        dropin_q = crate::util::shell_quote(PAIR_DROPIN_PATH),
        hands_b64 = wrap_base64(hands),
        hands_path = PAIR_HANDS_PATH,
        pulse_path = PAIR_PULSE_PATH,
        mise = mise_install_block("pulse".into(), "$home".into()),
        dropin_text = pair_dropin_text(),
    )
}

pub(crate) fn wrap_base64(data: Vec<u8>) -> String {
    let enc = base64::engine::general_purpose::STANDARD.encode(data);
    enc.as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn pair_undo_script() -> String {
    format!(
        "#!/bin/sh\nset -eu\nDROPIN={}\nif [ -f \"$DROPIN.prev\" ]; then\n  mv -f \"$DROPIN.prev\" \"$DROPIN\"\nelse\n  rm -f \"$DROPIN\"\nfi\nsystemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || systemctl restart ssh 2>/dev/null || systemctl restart sshd\nprintf 'step=dropin result=restored-or-removed\\n'\n",
        crate::util::shell_quote(PAIR_DROPIN_PATH)
    )
}

pub(crate) fn pair_commit_script() -> String {
    format!(
        "#!/bin/sh\nset -eu\nrm -f {}.prev\nprintf 'step=dropin-prev result=removed\\n'\n",
        crate::util::shell_quote(PAIR_DROPIN_PATH)
    )
}

pub(crate) fn pair_step_results(text: String) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if let Some(c) = STEP_RE.captures(line.trim()) {
            out.insert(c[1].to_string(), c[2].to_string());
        }
    }
    out
}

pub(crate) fn pair_ensure_hub_key(
    hub_home: String,
    lines: &mut Vec<String>,
) -> anyhow::Result<(String, String)> {
    let ssh_dir = Path::new(&hub_home).join(".ssh");
    let key = ssh_dir.join("pulse_hands_ed25519");
    let pubp = key.with_extension("pub");
    if !crate::util::file_exists(&key.to_string_lossy()) {
        std::fs::create_dir_all(&ssh_dir)?;
        let out = Command::new("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "pulse-hub",
                "-f",
                &key.to_string_lossy(),
            ])
            .output()?;
        if !out.status.success() {
            return Err(crate::util::fail(
                1,
                format!(
                    "ssh-keygen failed: {}",
                    pair_tail(String::from_utf8_lossy(&out.stderr).to_string())
                ),
            )
            .into());
        }
        lines.push(format!("generated hub key: {}", key.to_string_lossy()));
    }
    let data = std::fs::read_to_string(&pubp).map_err(|_| {
        crate::util::fail(
            1,
            format!("Missing public key beside {}", key.to_string_lossy()),
        )
    })?;
    Ok((
        key.to_string_lossy().to_string(),
        data.trim().lines().next().unwrap_or("").to_string(),
    ))
}

pub(crate) fn pair_inspection_pubkey(
    hub_home: String,
    dry_run: bool,
    lines: &mut Vec<String>,
) -> (String, String, bool) {
    let key = Path::new(&hub_home)
        .join(".ssh")
        .join("pulse_hands_ed25519");
    let pubp = key.with_extension("pub");
    if key.exists() && pubp.exists() {
        let data = std::fs::read_to_string(&pubp).unwrap_or_default();
        return (
            key.to_string_lossy().to_string(),
            data.trim().lines().next().unwrap_or("").to_string(),
            true,
        );
    }
    if dry_run {
        lines.push(format!("would generate hub key: {}", key.to_string_lossy()));
    }
    (
        key.to_string_lossy().to_string(),
        "ssh-ed25519 PULSEPAIRINSPECTIONONLY pulse-hub".into(),
        false,
    )
}

pub(crate) fn pair_check_alias(
    hub_home: String,
    name: String,
    host: String,
) -> anyhow::Result<(String, String, Option<Box<SshBlock>>)> {
    let config = Path::new(&hub_home).join(".ssh").join("config");
    let text = std::fs::read_to_string(&config).unwrap_or_default();
    let block = pair_alias_state(text.clone(), name.clone());
    if let Some(b) = &block
        && host != name
    {
        let target = if b.hostname.is_empty() {
            name.clone()
        } else {
            b.hostname.clone()
        };
        if target != host {
            return Err(
                crate::util::fail(2, format!("alias {} already points elsewhere", name)).into(),
            );
        }
    }
    Ok((config.to_string_lossy().to_string(), text, block))
}

pub(crate) fn pair_write_alias(
    config: String,
    text: String,
    block: Option<&SshBlock>,
    name: String,
    host: String,
    port: i32,
) -> bool {
    if block.is_some() {
        return false;
    }
    let path = Path::new(&config);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let prefix = if text.is_empty() {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let mut f = match std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    if f.write_all(format!("{}{}", prefix, pair_host_block(name, host, port)).as_bytes())
        .is_err()
    {
        return false;
    }
    true
}

pub(crate) fn pair_write_known_hosts(
    hub_home: String,
    host: String,
    port: i32,
    hostkeys: Vec<String>,
) -> i32 {
    if hostkeys.is_empty() {
        return 0;
    }
    let path = Path::new(&hub_home).join(".ssh").join("known_hosts");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut have = existing
        .lines()
        .map(ToOwned::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let prefix = pair_known_host_prefix(host, port);
    let mut f = match std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let mut added = 0;
    for hk in hostkeys {
        let hk = hk.trim();
        let line = format!("{prefix} {hk}");
        if !hk.is_empty() && have.insert(line.clone()) {
            let _ = writeln!(f, "{line}");
            added += 1;
        }
    }
    added
}

pub(crate) fn pair_open_control_master(
    admin: String,
    control_path: String,
) -> anyhow::Result<(i32, String)> {
    let (user, host, port) = parse_pair_ssh_address(&admin)?;
    let args = vec![
        "ssh".to_string(),
        "-o".into(),
        "ControlMaster=yes".into(),
        "-o".into(),
        "ControlPersist=120".into(),
        "-o".into(),
        format!("ControlPath={control_path}"),
        "-p".into(),
        port.to_string(),
        format!("{user}@{host}"),
        "true".into(),
    ];
    let out = Command::new(&args[0]).args(&args[1..]).output();
    match out {
        Ok(o) => Ok((
            o.status.code().unwrap_or(-1),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            ),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(anyhow!(crate::executor::ExecutorUnreachable {
                msg: "ssh is not installed on this machine".into()
            }))
        }
        Err(_) => Err(anyhow!(crate::executor::ExecutorUnreachable {
            msg: "ssh control master timed out after 60s".into()
        })),
    }
}

pub(crate) fn pair_close_control_master(admin: String, control_path: String) {
    if let Ok((user, host, port)) = parse_pair_ssh_address(&admin) {
        let _ = Command::new("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={control_path}"),
                "-p",
                &port.to_string(),
                &format!("{user}@{host}"),
            ])
            .output();
    }
}

pub(crate) fn pair_inspection_summary(
    insp: BTreeMap<String, String>,
    hands: Vec<u8>,
) -> Vec<String> {
    let keys = [
        "uname",
        "arch",
        "sudo",
        "user",
        "hands",
        "workdir",
        "workdir_owner",
        "authorized_keys",
        "authorized_keys_lines",
        "dropin",
        "dropin_dir",
        "remotes",
        "apt",
        "copilot",
        "node",
        "pulse",
        "harness",
        "git",
        "gitconfig",
    ];
    let mut out = vec!["inspection:".into()];
    for k in keys {
        let value = if k == "hands" {
            crate::pair_inspect::pair_hands_summary(insp.clone(), hands.clone())
        } else {
            insp.get(k).cloned().unwrap_or_default()
        };
        out.push(format!("  {k}={value}"));
    }
    out
}

pub(crate) fn pair_refusals_from_inspection(
    insp: BTreeMap<String, String>,
    install_copilot: bool,
) -> Vec<String> {
    if insp.get("uname").map(String::as_str) != Some("Linux") {
        return vec![format!(
            "remote uname is {}, not Linux",
            insp.get("uname")
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| "unknown".into())
        )];
    }
    if insp.get("sudo").map(String::as_str) != Some("yes") {
        return vec!["sudo without a password is required".into()];
    }
    if insp.get("authorized_keys").map(String::as_str) == Some("other") {
        return vec!["pulse authorized_keys has other content".into()];
    }
    if insp.get("dropin_dir").map(String::as_str) == Some("no") {
        let mut out = vec![
            "this sshd does not include sshd_config.d - the manual Match block is printed".into(),
        ];
        out.extend(pair_manual_dropin_lines());
        return out;
    }
    let rem = insp.get("remotes").cloned().unwrap_or_else(|| "0".into());
    let n = rem.parse::<i32>().unwrap_or(-1);
    if n < 0 {
        return vec![format!("remote remotes count is not readable: {}", rem)];
    }
    if n > 0 {
        return vec![format!(
            "{} clone(s) with git remotes exist under the workdir",
            rem
        )];
    }
    if install_copilot && insp.get("apt").map(String::as_str) != Some("yes") {
        let mut out = vec!["--install-copilot needs apt on the remote machine".into()];
        out.extend(pair_manual_copilot_lines());
        return out;
    }
    Vec::new()
}

pub(crate) fn go_arch_for(uname: String) -> String {
    match uname.as_str() {
        "x86_64" | "amd64" => "amd64".into(),
        "aarch64" | "arm64" => "arm64".into(),
        _ => String::new(),
    }
}

pub(crate) fn module_root() -> String {
    let mut starts = Vec::new();
    if let Ok(wd) = std::env::current_dir() {
        starts.push(wd);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(p) = exe.parent()
    {
        starts.push(p.to_path_buf());
    }
    let hinted = HANDS_SOURCE_ROOT
        .read()
        .expect("hands_source_root poisoned")
        .clone();
    if !hinted.is_empty() {
        starts.push(PathBuf::from(hinted));
    }
    module_root_from(&starts)
}

/// The first pulse workspace at or above any of the starts, in order.
pub(crate) fn module_root_from(starts: &[PathBuf]) -> String {
    for start in starts {
        for dir in start.ancestors() {
            if is_pulse_workspace(dir) {
                return dir.to_string_lossy().to_string();
            }
        }
    }
    String::new()
}

fn is_pulse_workspace(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|d| d.contains("[workspace]"))
        && dir.join("crates/pulse/Cargo.toml").is_file()
}

pub(crate) fn sha256_bytes(data: Vec<u8>) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(data))
}

/// The remote apply may install node, copilot and mise; the undo and commit scripts only touch one file.
pub(crate) const PAIR_APPLY_TIMEOUT: Duration = Duration::from_secs(600);
pub(crate) const PAIR_FINISH_TIMEOUT: Duration = Duration::from_secs(120);
pub(crate) const PAIR_REMOTE_COMMAND: &str = "sudo -n sh -s";

/// The apply script as the dry run shows it: the hands heredoc carries a placeholder instead of the binary.
pub(crate) fn pair_shown_apply_script(
    name: String,
    workdir: String,
    pubkey: String,
    arch: String,
    install_copilot: bool,
) -> String {
    let placeholder = format!("<pulse linux/{arch} binary, base64>");
    pair_apply_script(name, workdir, pubkey, Vec::new(), install_copilot).replace(
        "<<'PULSE_HANDS_EOF'\n\nPULSE_HANDS_EOF",
        &format!("<<'PULSE_HANDS_EOF'\n{placeholder}\nPULSE_HANDS_EOF"),
    )
}

pub(crate) fn pair_script_lines(shown: &str, undo: &str, commit: &str) -> Vec<String> {
    vec![
        "remote script (would run as root via sudo -n sh -s; the hands heredoc carries the linux pulse binary):".into(),
        shown.trim_end().into(),
        "undo script (the exact inverse):".into(),
        undo.trim_end().into(),
        "commit script (after successful admin re-check):".into(),
        commit.trim_end().into(),
    ]
}

/// `step=NAME result=R` lines in script order, as one `NAME=R` word each.
pub(crate) fn pair_step_summary(text: &str) -> String {
    text.lines()
        .filter_map(|l| STEP_RE.captures(l.trim()))
        .map(|c| format!("{}={}", &c[1], &c[2]))
        .collect::<Vec<_>>()
        .join(" ")
}

fn pair_output_tail(out: &str, err_text: &str) -> String {
    let mut t = pair_tail(err_text.to_string());
    if t.is_empty() {
        t = pair_tail(out.to_string());
    }
    t
}

/// A probe note that means the pairing itself does not work (transport, key, forced command,
/// workdir), as opposed to hands that answer but are not fit yet (login, copilot, git, disk).
pub(crate) fn pair_probe_broken(note: &str) -> bool {
    note.starts_with("unreachable:")
        || note.contains("pulse-hands did not answer")
        || note.contains("authorized key lacks command=/restrict")
        || note.contains("workdir is ")
        || note.contains("hands predate ADR-022")
}

pub(crate) fn pair_next_step(name: &str, admin: &str, note: &str) -> String {
    if note.contains("copilot missing") {
        format!(
            "install copilot there (`pulse pair {admin} --name {name} --install-copilot` on an apt machine), then `pulse executor probe {name}`"
        )
    } else if note.contains("harness login missing") || note.contains("not logged in") {
        format!(
            "`pulse executor login {name} --admin {admin}` (add --with-token to paste a token), then `pulse executor probe {name}`"
        )
    } else {
        format!("fix that on the machine, then `pulse executor probe {name}`")
    }
}

fn pair_ensure_ssh_dir(hub_home: &str) -> std::path::PathBuf {
    let dir = Path::new(hub_home).join(".ssh");
    if !dir.is_dir() {
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    dir
}

/// The local side of a live pair: the `Host NAME` block and the machine's host keys. Both are
/// idempotent; a second run with the same machine changes neither file.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pair_apply_local(
    hub_home: String,
    config: String,
    config_text: String,
    block: Option<&SshBlock>,
    name: String,
    host: String,
    port: i32,
    hostkeys: Vec<String>,
) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let mut lines = Vec::new();
    let mut changed = Vec::new();
    pair_ensure_ssh_dir(&hub_home);
    if block.is_some() {
        lines.push(format!("kept existing Host {name} block in {config}"));
    } else {
        let bak = format!("{config}.bak");
        let backed_up = Path::new(&config).is_file();
        if backed_up {
            std::fs::copy(&config, &bak).map_err(|e| {
                crate::util::fail(1, format!("cannot back up {config} to {bak}: {e}"))
            })?;
        }
        if !pair_write_alias(
            config.clone(),
            config_text,
            block,
            name.clone(),
            host.clone(),
            port,
        ) {
            return Err(crate::util::fail(
                1,
                format!("cannot write the Host {name} block to {config}"),
            )
            .into());
        }
        changed.push(config.clone());
        if backed_up {
            lines.push(format!(
                "added Host {name} block to {config} (the previous file is {bak})"
            ));
        } else {
            lines.push(format!("added Host {name} block to {config}"));
        }
    }
    let known = Path::new(&hub_home)
        .join(".ssh")
        .join("known_hosts")
        .to_string_lossy()
        .to_string();
    let prefix = pair_known_host_prefix(host.clone(), port);
    if hostkeys.is_empty() {
        lines.push(format!(
            "known_hosts: the machine reported no host keys; {known} is unchanged"
        ));
    } else {
        let added = pair_write_known_hosts(hub_home, host, port, hostkeys.clone());
        if added > 0 {
            changed.push(known.clone());
            lines.push(format!(
                "known_hosts: added {added} of {} line(s) for {prefix} to {known}",
                hostkeys.len()
            ));
        } else {
            lines.push(format!(
                "known_hosts: all {} line(s) for {prefix} already in {known}",
                hostkeys.len()
            ));
        }
    }
    Ok((lines, changed))
}

/// A re-pair updates the row `pulse executor add` would refuse to create twice; its status is
/// left to the probe that follows.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pair_update_executor(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    address: &str,
    workdir: &str,
    workers: i32,
    projects: &str,
    actor: &str,
) -> anyhow::Result<crate::util::Row> {
    let conn = s.conn()?;
    let args: [&dyn rusqlite::ToSql; 5] = [&address, &workdir, &workers, &projects, &name];
    crate::util::exec1(
        &conn,
        "UPDATE executors SET kind = 'ssh', address = ?, workdir = ?, workers = ?, projects = ? WHERE name = ?",
        &args,
    );
    crate::util::event_at(
        &conn,
        now,
        actor,
        "executor.update",
        name,
        serde_json::json!({"address": address, "workdir": workdir, "workers": workers, "projects": projects, "by": "pair"}),
    );
    Ok(crate::executor::executor_row(s, name))
}

struct PairLive<'a> {
    s: &'a pulse_store::Store,
    admin: &'a str,
    user: &'a str,
    name: &'a str,
    host: &'a str,
    port: i32,
    workdir: &'a str,
    now: &'a str,
    actor: &'a str,
    decision: &'a str,
    opts: &'a PairOpts,
    hub_home: &'a str,
    key_ready: bool,
    pubkey: String,
    hands: Vec<u8>,
    arch: &'a str,
    hostkeys: Vec<String>,
    config: String,
    config_text: String,
    alias_block: Option<Box<SshBlock>>,
    undo_script: String,
    commit_script: String,
}

fn pair_run_admin(
    admin: &str,
    script: &str,
    timeout: Duration,
) -> anyhow::Result<(i32, String, String)> {
    crate::executor::admin_ssh_run(admin, PAIR_REMOTE_COMMAND, script.as_bytes(), timeout, "")
}

/// Everything the dry run prints, done: the hub key and local ssh files, the apply script as
/// root over the admin session, the executor row, the probe, then the commit script or the
/// undo script. Returns the report on an active or a drained executor; fails (after undoing)
/// when the hands cannot be reached through the pairing it just made.
fn pair_apply_live(
    live: PairLive<'_>,
    mut rep: PairReport,
) -> anyhow::Result<Option<Box<PairReport>>> {
    let PairLive {
        s,
        admin,
        user,
        name,
        host,
        port,
        workdir,
        now,
        actor,
        decision,
        opts,
        hub_home,
        key_ready,
        pubkey,
        hands,
        arch,
        hostkeys,
        config,
        config_text,
        alias_block,
        undo_script,
        commit_script,
    } = live;
    let projects = crate::executor::normalize_projects(&opts.projects);
    let workers = opts.workers;
    let pubkey = if key_ready {
        pubkey
    } else {
        let (key_path, pubkey) = pair_ensure_hub_key(hub_home.to_string(), &mut rep.lines)?;
        rep.changed.push(key_path);
        pubkey
    };
    let apply_script = pair_apply_script(
        name.to_string(),
        workdir.to_string(),
        pubkey.clone(),
        hands,
        opts.install_copilot,
    );
    rep.scripts.insert("apply".into(), apply_script.clone());

    let (local_lines, local_changed) = pair_apply_local(
        hub_home.to_string(),
        config,
        config_text,
        alias_block.as_deref(),
        name.to_string(),
        host.to_string(),
        port,
        hostkeys,
    )?;
    rep.lines.extend(local_lines);
    rep.changed.extend(local_changed);

    let mins = PAIR_APPLY_TIMEOUT.as_secs() / 60;
    rep.lines.push(format!(
        "applying on {host} as root (ssh -p {port} {user}@{host} {PAIR_REMOTE_COMMAND}, up to {mins} min)"
    ));
    let (rc, out, err_text) = match pair_run_admin(admin, &apply_script, PAIR_APPLY_TIMEOUT) {
        Ok(v) => v,
        Err(e) => {
            rep.lines
                .push(format!("apply failed: {}", crate::run::err_message(&e)));
            rep.lines.push(format!(
                "next: once {user}@{host} answers again, run `pulse pair {admin} --name {name}` again; the apply script is idempotent"
            ));
            return Err(crate::util::fail(1, rep.lines.join("\n")).into());
        }
    };
    let steps = pair_step_results(out.clone());
    let step_line = pair_step_summary(&out);
    if rc != 0 {
        let tail = pair_output_tail(&out, &err_text);
        if tail.to_lowercase().contains("password") {
            rep.lines.push(format!(
                "sudo asked {user}@{host} for a password: the apply script did not run"
            ));
            rep.lines.push(format!(
                "next: allow passwordless sudo for {user} there (`echo '{user} ALL=(ALL) NOPASSWD: ALL' | sudo tee /etc/sudoers.d/{user}`) and run `pulse pair {admin} --name {name}` again, or run the script below as root yourself, then `pulse executor add {name} --ssh pulse@{name} --workdir {workdir} --workers {workers} --projects {projects}` and `pulse executor probe {name}`"
            ));
            rep.lines.extend(pair_script_lines(
                &pair_shown_apply_script(
                    name.to_string(),
                    workdir.to_string(),
                    pubkey,
                    arch.to_string(),
                    opts.install_copilot,
                ),
                &undo_script,
                &commit_script,
            ));
            return Err(crate::util::fail(2, rep.lines.join("\n")).into());
        }
        rep.lines.push(format!(
            "apply failed on {host} (exit {rc}): {}",
            if tail.is_empty() {
                "no output".to_string()
            } else {
                tail
            }
        ));
        if !step_line.is_empty() {
            rep.lines.push(format!("  steps: {step_line}"));
        }
        pair_remote_undo(admin, host, &steps, &undo_script, &mut rep.lines);
        rep.lines.push(format!(
            "next: fix that on the machine, then run `pulse pair {admin} --name {name}` again; the apply script is idempotent"
        ));
        return Err(crate::util::fail(1, rep.lines.join("\n")).into());
    }
    rep.lines.push(format!(
        "applied on {host} as root: {}",
        if step_line.is_empty() {
            "no step results".to_string()
        } else {
            step_line
        }
    ));
    if !err_text.trim().is_empty() {
        rep.lines
            .push(format!("  remote stderr: {}", pair_tail(err_text)));
    }

    let address = format!("pulse@{name}");
    let existing = crate::executor::executor_row(s, name);
    let created = crate::util::is_nil(&existing);
    if created {
        crate::executor::add_executor(
            s,
            now,
            name,
            &address,
            workdir,
            workers,
            &opts.projects,
            actor,
        )?;
        rep.lines.push(format!(
            "registered executor {name} ({address}, {workdir}, {workers} worker(s), projects {projects})"
        ));
    } else {
        pair_update_executor(s, now, name, &address, workdir, workers, &projects, actor)?;
        rep.lines.push(format!(
            "updated executor {name} ({address}, {workdir}, {workers} worker(s), projects {projects}; status was {})",
            existing.str("status")
        ));
    }

    let (ok, note) = match crate::executor::probe_executor(s, name, actor, now) {
        Ok(v) => v,
        Err(e) => (
            false,
            format!("unreachable: {}", crate::run::err_message(&e)),
        ),
    };
    let broken = !ok && pair_probe_broken(&note);
    if ok {
        let row = crate::executor::executor_row(s, name);
        if row.str("status") != "active" && row.str("probe_note").starts_with("ok") {
            crate::executor::set_executor_status(s, now, name, "active", actor, None)?;
        }
    } else if !broken {
        let row = crate::executor::executor_row(s, name);
        if row.str("status") == "active" {
            crate::executor::set_executor_status(s, now, name, "drained", actor, None)?;
        }
    }

    let outcome = if broken {
        "undone"
    } else if ok {
        "active"
    } else {
        "drained"
    };
    if broken {
        rep.lines.push(format!("probe FAILED: {note}"));
        pair_remote_undo(admin, host, &steps, &undo_script, &mut rep.lines);
        if created {
            match pulse_board::remove_executor(s, name, actor, false) {
                Ok(_) => rep
                    .lines
                    .push(format!("removed executor {name} from the store")),
                Err(e) => rep.lines.push(format!(
                    "could not remove executor {name}: {}",
                    crate::run::err_message(&e)
                )),
            }
        } else {
            let row = crate::executor::executor_row(s, name);
            if !crate::util::is_nil(&row) && row.str("status") == "active" {
                crate::executor::set_executor_status(s, now, name, "drained", actor, None)?;
            }
            rep.lines
                .push(format!("drained executor {name}; its row and history stay"));
        }
    } else {
        match pair_run_admin(admin, &commit_script, PAIR_FINISH_TIMEOUT) {
            Ok((0, out, _)) => rep.lines.push(format!(
                "committed on {host}: {}",
                pair_step_summary(&out)
            )),
            Ok((rc, out, err_text)) => rep.lines.push(format!(
                "commit script failed on {host} (exit {rc}): {}; {PAIR_DROPIN_PATH}.prev stays there, which is harmless",
                pair_output_tail(&out, &err_text)
            )),
            Err(e) => rep.lines.push(format!(
                "commit script did not run on {host}: {}; {PAIR_DROPIN_PATH}.prev stays there, which is harmless",
                crate::run::err_message(&e)
            )),
        }
    }

    let mut detail = serde_json::json!({
        "host": host,
        "port": port,
        "admin": admin,
        "executor": name,
        "workdir": workdir,
        "workers": workers,
        "projects": projects,
        "created": created,
        "probe_ok": ok,
        "probe": crate::util::trunc(&note, 300),
        "outcome": outcome,
    });
    if !decision.is_empty() {
        detail["decision"] = serde_json::Value::String(decision.to_string());
    }
    crate::util::event_at(&s.conn()?, now, actor, "pair.applied", name, detail);

    if broken {
        rep.lines.push(format!(
            "next: check the connection from this account (`ssh -v {name}` uses the Host {name} block and the hub key), then run `pulse pair {admin} --name {name}` again"
        ));
        return Err(crate::util::fail(1, rep.lines.join("\n")).into());
    }
    let row = crate::executor::executor_row(s, name);
    let status = row.str("status");
    rep.lines.push(String::new());
    rep.lines.push(if ok {
        format!("paired {name}: {status}")
    } else {
        format!("paired {name}: {status} (the probe did not pass)")
    });
    rep.lines.extend([
        format!("  host      {host}:{port} (admin {user}@{host})"),
        format!("  user      pulse (ssh alias {name}, key ~/.ssh/pulse_hands_ed25519)"),
        format!("  workdir   {workdir}"),
        format!("  workers   {workers}"),
        format!("  projects  {projects}"),
        if ok {
            format!("  probe     {note}")
        } else {
            format!("  probe     FAILED: {note}")
        },
    ]);
    if !ok {
        rep.lines.push(format!(
            "  next      {}",
            pair_next_step(name, admin, &note)
        ));
    }
    rep.lines
        .push(format!("  undo      pulse remove executor {name}"));
    rep.ok = ok;
    Ok(Some(Box::new(rep)))
}

/// The undo script is the inverse of this run only when this run changed the sshd drop-in;
/// a re-pair that kept an earlier drop-in has nothing to undo there.
fn pair_remote_undo(
    admin: &str,
    host: &str,
    steps: &BTreeMap<String, String>,
    undo_script: &str,
    lines: &mut Vec<String>,
) {
    if steps.get("dropin").map(String::as_str) != Some("done") {
        lines.push(format!(
            "left {PAIR_DROPIN_PATH} on {host} as this run found it (nothing to undo)"
        ));
        return;
    }
    match pair_run_admin(admin, undo_script, PAIR_FINISH_TIMEOUT) {
        Ok((0, out, _)) => lines.push(format!(
            "undid the sshd drop-in on {host}: {}",
            pair_step_summary(&out)
        )),
        Ok((rc, out, err_text)) => lines.push(format!(
            "undo script failed on {host} (exit {rc}): {}; run it yourself as root",
            pair_output_tail(&out, &err_text)
        )),
        Err(e) => lines.push(format!(
            "undo script did not run on {host}: {}; run it yourself as root",
            crate::run::err_message(&e)
        )),
    }
}

pub(crate) fn pair_executor(
    s: &pulse_store::Store,
    admin: String,
    name: String,
    now: String,
    o: PairOpts,
) -> anyhow::Result<Option<Box<PairReport>>> {
    let (name, user, host, port, workdir) =
        pair_validate(name, admin.clone(), o.workdir.clone(), o.workers)?;
    let actor = if o.actor.is_empty() {
        crate::util::HUMAN_AGENT.to_string()
    } else {
        o.actor.clone()
    };
    let decision = crate::onboard::agent_gate(
        &s.conn()?,
        format!("pair: {name}"),
        actor.clone(),
        now.clone(),
    )?;
    let hub_home = if o.hub_home.is_empty() {
        crate::util::home_dir()
    } else {
        o.hub_home.clone()
    };
    let mut rep = PairReport {
        lines: vec![format!("pairing executor {name}")],
        scripts: BTreeMap::new(),
        ..Default::default()
    };
    let (key_path, pubkey, key_ready) =
        pair_inspection_pubkey(hub_home.clone(), o.dry_run, &mut rep.lines);
    let (config, config_text, alias_block) =
        pair_check_alias(hub_home.clone(), name.clone(), host.clone())?;
    let dropin = pair_dropin_text();
    let inspect_script = pair_inspect_script(
        workdir.clone(),
        pubkey.clone(),
        String::new(),
        crate::util::sha256_text(&dropin),
    );
    let (rc, out, err_text) = crate::executor::admin_ssh_run(
        &admin,
        "sudo -n sh -s",
        inspect_script.as_bytes(),
        Duration::from_secs(120),
        "",
    )
    .map_err(|e| {
        crate::util::fail(
            2,
            format!("inspection failed: {}", crate::run::err_message(&e)),
        )
    })?;
    if rc != 0 {
        let mut t = pair_tail(err_text);
        if t.is_empty() {
            t = pair_tail(out);
        }
        if t.to_lowercase().contains("password") {
            return Err(crate::util::fail(2, "sudo without a password is required").into());
        }
        if t.is_empty() {
            t = format!("exit {rc}");
        }
        return Err(crate::util::fail(2, format!("inspection failed: {t}")).into());
    }
    let (insp, hostkeys) = pair_parse_inspection(out);
    let arch = go_arch_for(insp.get("arch").cloned().unwrap_or_default());
    let mut hands = Vec::new();
    let mut hands_from = String::new();
    if insp.get("uname").map(String::as_str) == Some("Linux")
        && (!o.dry_run || insp.get("hands").map(String::as_str) != Some("absent"))
    {
        if arch.is_empty() {
            return Err(crate::util::fail(
                2,
                format!(
                    "remote architecture {} has no pulse build (x86_64 and aarch64 do)",
                    crate::board::py_repr(insp.get("arch").cloned().unwrap_or_default())
                ),
            )
            .into());
        }
        let got = (HANDS_BINARY.read().unwrap())(&arch)?;
        hands = got.0;
        hands_from = got.1;
    }
    rep.lines
        .extend(pair_inspection_summary(insp.clone(), hands.clone()));
    if !hands_from.is_empty() {
        rep.lines.push(format!(
            "hands binary: linux/{arch}, {} bytes, from {hands_from}",
            hands.len()
        ));
    }
    if insp.get("copilot").map(String::as_str) == Some("missing")
        && !o.install_copilot
        && !s.harness_native()
    {
        rep.lines.push("hint: copilot is missing; the final probe may leave this executor drained until a human installs or logs it in".into());
    }
    let refusal = pair_refusals_from_inspection(insp, o.install_copilot);
    if !refusal.is_empty() {
        rep.lines.extend(refusal);
        return Err(crate::util::fail(2, rep.lines.join("\n")).into());
    }
    let undo_script = pair_undo_script();
    let commit_script = pair_commit_script();
    rep.scripts.insert("inspect".into(), inspect_script);
    rep.scripts.insert("undo".into(), undo_script.clone());
    rep.scripts.insert("commit".into(), commit_script.clone());
    if o.dry_run {
        let shown = pair_shown_apply_script(
            name.clone(),
            workdir.clone(),
            pubkey,
            arch.clone(),
            o.install_copilot,
        );
        rep.scripts.insert("apply".into(), shown.clone());
        rep.lines
            .extend(pair_script_lines(&shown, &undo_script, &commit_script));
        rep.lines.push("local changes that would be made:".into());
        if !Path::new(&key_path).exists() {
            rep.lines.push(format!("  generate {key_path}"));
        }
        if alias_block.is_some() {
            rep.lines.push(format!("  keep existing Host {name} block"));
        } else {
            rep.lines.push(
                pair_host_block(name.clone(), host.clone(), port)
                    .trim_end()
                    .into(),
            );
        }
        for hk in hostkeys {
            rep.lines.push(format!(
                "  known_hosts: {} {}",
                pair_known_host_prefix(host.clone(), port),
                hk
            ));
        }
        rep.lines.push(format!("  executor add {name} --ssh pulse@{name} --workdir {workdir} --workers {} --projects {}", o.workers, crate::executor::normalize_projects(&o.projects)));
        rep.lines.push(format!("  probe {name}"));
        rep.ok = true;
        return Ok(Some(Box::new(rep)));
    }

    pair_apply_live(
        PairLive {
            s,
            admin: &admin,
            user: &user,
            name: &name,
            host: &host,
            port,
            workdir: &workdir,
            now: &now,
            actor: &actor,
            decision: &decision,
            opts: &o,
            hub_home: &hub_home,
            key_ready,
            pubkey,
            hands,
            arch: &arch,
            hostkeys,
            config,
            config_text,
            alias_block,
            undo_script,
            commit_script,
        },
        rep,
    )
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, count, new_store};
    use crate::test_helpers_provider::invoke;
    use std::sync::{LazyLock, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct FakeSsh {
        calls: usize,
        out: String,
        require_metadata: bool,
    }

    static PAIR_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
    static FAKE_SSH: LazyLock<Mutex<FakeSsh>> = LazyLock::new(|| Mutex::new(FakeSsh::default()));

    fn fake_run_ssh(
        _cancel: &pulse_bus::CancellationToken,
        _argv: &[String],
        stdin: &[u8],
        _timeout: Duration,
    ) -> anyhow::Result<(i32, String, String)> {
        let mut fake = FAKE_SSH.lock().expect("fake ssh");
        fake.calls += 1;
        let script = String::from_utf8_lossy(stdin);
        if !script.contains("printf 'uname=%s\\n'") {
            panic!("dry run sent a non-inspection script");
        }
        if fake.require_metadata && !script.contains("hands_buildinfo=") {
            panic!("missing metadata in the read-only inspection script");
        }
        Ok((0, fake.out.clone(), String::new()))
    }

    #[test]
    fn pair_dry_run_hands_diagnostics() {
        let _serial = crate::test_helpers::serial();
        let _guard = PAIR_TEST_LOCK.lock().expect("pair test lock");
        let template = b"#!/bin/sh\n# pulse-hands v2.0.0\nexit 0\n".to_vec();
        let template_hash = sha256_bytes(template.clone());
        let modified_hash = sha256_bytes(b"#!/bin/sh\n# pulse-hands v2.0.0\nexit 1\n".to_vec());
        for (name, version, hash, want) in [
            (
                "same",
                "v2.0.0".to_string(),
                template_hash.clone(),
                "present".to_string(),
            ),
            (
                "same-version-edited",
                "v2.0.0".to_string(),
                modified_hash.clone(),
                format!(
                    "differs (installed v2.0.0 {}, template v2.0.0 {})",
                    &modified_hash[..7],
                    &template_hash[..7]
                ),
            ),
            (
                "unrecognized-old-version",
                "v1.0.0".to_string(),
                modified_hash.clone(),
                format!(
                    "differs (installed v1.0.0 {}, template v2.0.0 {})",
                    &modified_hash[..7],
                    &template_hash[..7]
                ),
            ),
            (
                "shipped-legacy",
                String::new(),
                "c75aba6a43bc7db6f640fc145358e5f41304ca2975946e86f0ada9bb79da5118".to_string(),
                format!(
                    "older (installed legacy-ccb0790 c75aba6, template v2.0.0 {})",
                    &template_hash[..7]
                ),
            ),
            (
                "edited-legacy",
                "legacy-ccb0790".to_string(),
                "c75aba6a43bc7db6f640fc145358e5f41304ca2975946e86f0ada9bb79da5119".to_string(),
                format!(
                    "differs (installed legacy-ccb0790 c75aba6, template v2.0.0 {})",
                    &template_hash[..7]
                ),
            ),
            (
                "unversioned",
                String::new(),
                modified_hash.clone(),
                format!(
                    "differs (installed unknown {}, template v2.0.0 {})",
                    &modified_hash[..7],
                    &template_hash[..7]
                ),
            ),
            (
                "missing-hash",
                "v1.0.0".to_string(),
                String::new(),
                format!(
                    "differs (installed v1.0.0 unknown, template v2.0.0 {})",
                    &template_hash[..7]
                ),
            ),
            (
                "short-hash",
                "v1.0.0".to_string(),
                "c75aba6".to_string(),
                format!(
                    "differs (installed v1.0.0 unknown, template v2.0.0 {})",
                    &template_hash[..7]
                ),
            ),
        ] {
            let s = new_store();
            let hub = tempfile::tempdir().expect("hub");
            let events = count(&s, "SELECT COUNT(*) AS n FROM events", &[]);
            let old_ssh = crate::executor::run_ssh_hook();
            let old_binary = set_hands_binary(Box::new({
                let template = template.clone();
                move |arch| {
                    assert_eq!(arch, "amd64", "{name}");
                    Ok((template.clone(), "fixture".to_string()))
                }
            }));
            crate::executor::set_run_ssh(fake_run_ssh);
            {
                let mut fake = FAKE_SSH.lock().expect("fake ssh");
                *fake = FakeSsh {
                    calls: 0,
                    out: format!(
                        "uname=Linux\narch=x86_64\nsudo=yes\nhands=differs\ndropin_dir=yes\nremotes=0\nhands_version={version}\nhands_sha256={hash}\n"
                    ),
                    require_metadata: false,
                };
            }
            let rep = pair_executor(
                &s,
                "admin@example.invalid".into(),
                "hands-two".into(),
                TEST_NOW.into(),
                PairOpts {
                    workdir: "/srv/pulse".into(),
                    workers: 1,
                    projects: "*".into(),
                    dry_run: true,
                    hub_home: hub.path().display().to_string(),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .expect("report");
            crate::executor::restore_run_ssh(old_ssh);
            let _ = set_hands_binary(old_binary);
            let hands_line = rep
                .lines
                .iter()
                .find(|l| l.starts_with("  hands="))
                .cloned()
                .unwrap_or_default();
            assert_eq!(hands_line, format!("  hands={want}"), "{name}");
            assert!(rep.ok, "{name}: report not ok");
            assert_eq!(FAKE_SSH.lock().expect("fake ssh").calls, 1, "{name}");
            assert!(
                crate::executor::executor_row(&s, "hands-two").is_empty()
                    && count(&s, "SELECT COUNT(*) AS n FROM events", &[]) == events,
                "inspection changed the store"
            );
            assert!(
                std::fs::read_dir(hub.path())
                    .expect("hub read")
                    .next()
                    .is_none(),
                "inspection changed the hub home"
            );
        }
    }

    #[test]
    fn pair_inspect_alias() {
        let _serial = crate::test_helpers::serial();
        let _guard = PAIR_TEST_LOCK.lock().expect("pair test lock");
        let s = new_store();
        let home = tempfile::tempdir().expect("home");
        let old_home = std::env::var("HOME").ok();
        let _env = pulse_testutil::setenv("HOME", home.path());
        let old_ssh = crate::executor::run_ssh_hook();
        let old_binary = set_hands_binary(Box::new(|_| {
            Ok((b"#!/bin/sh\n# pulse-hands v2\n".to_vec(), "fixture".into()))
        }));
        crate::executor::set_run_ssh(fake_run_ssh);
        {
            let mut fake = FAKE_SSH.lock().expect("fake ssh");
            *fake = FakeSsh {
                    calls: 0,
                    out: "uname=Linux\narch=x86_64\nsudo=yes\nhands=differs\ndropin_dir=yes\nremotes=0\nhands_version=v1\n".into(),
                    require_metadata: true,
                };
        }
        let events = count(&s, "SELECT COUNT(*) AS n FROM events", &[]);
        let (out, err_text, code) = invoke(
            &s,
            "pair",
            "",
            &["admin@example.invalid", "--name", "hands-two", "--inspect"],
        );
        assert!(
            code == 0
                && out.contains("hands=differs (installed v1 unknown, template v2 ")
                && FAKE_SSH.lock().expect("fake ssh").calls == 1,
            "inspect alias: code={code}, stderr={err_text:?}\n{out}"
        );
        assert!(
            count(&s, "SELECT COUNT(*) AS n FROM events", &[]) == events
                && crate::executor::executor_row(&s, "hands-two").is_empty(),
            "inspect alias changed the store"
        );
        let _ = set_hands_binary(Box::new(|_| anyhow::bail!("template unavailable")));
        let err = pair_executor(
            &s,
            "admin@example.invalid".into(),
            "hands-two".into(),
            TEST_NOW.into(),
            PairOpts {
                workdir: "/srv/pulse".into(),
                workers: 1,
                dry_run: true,
                ..Default::default()
            },
        )
        .expect_err("template failure");
        assert!(
            err.to_string().contains("template unavailable")
                && FAKE_SSH.lock().expect("fake ssh").calls == 2,
            "template failure: {err}"
        );
        let err = pair_executor(
            &s,
            "admin@example.invalid".into(),
            "hands-two".into(),
            TEST_NOW.into(),
            PairOpts {
                workdir: "/srv/pulse".into(),
                workers: 1,
                dry_run: true,
                actor: "worker".into(),
                ..Default::default()
            },
        )
        .expect_err("agent gate");
        assert!(
            !err.to_string().is_empty() && FAKE_SSH.lock().expect("fake ssh").calls == 2,
            "inspection bypassed the agent gate: {err}"
        );
        crate::executor::restore_run_ssh(old_ssh);
        let _ = set_hands_binary(old_binary);
        if let Some(home) = old_home {
            let _env = pulse_testutil::setenv("HOME", home);
        }
    }

    // ---- the live path, against a recording fake ssh ----

    const PROBE_B64: &str = "b64 cHJvYmUA";
    const INSPECTION_OK: &str = "uname=Linux\narch=x86_64\nsudo=yes\nuser=absent\nhands=absent\nworkdir=absent\nauthorized_keys=absent\ndropin=absent\ndropin_dir=yes\nremotes=0\napt=yes\ncopilot=present\nnode=present\npulse=missing\nharness=no\ngit=present\ngitconfig=missing\nhostkeys=\nhostkey ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFAKEHOSTKEY root@example\n";
    const APPLY_OK: &str = "step=user result=done\nstep=hands result=done\nstep=pulse result=done\nstep=workdir result=done\nstep=authorized_keys result=done\nstep=gitconfig result=done\nstep=copilot result=skipped\nstep=mise result=done\nstep=dropin result=done\nstep=sshd_reload result=done\n";
    const PROBE_OK: &str = "hands=ok user=pulse workdir=/srv/pulse git=2.43 copilot=present logged_in=yes harness=yes pulse=/usr/local/bin/pulse remotes=0 keys_ok=yes disk_free_mb=200000 jobs=- keys=- heartbeat=1\n";

    #[derive(Clone, Debug, PartialEq)]
    struct SshCall {
        argv: Vec<String>,
        kind: &'static str,
    }

    #[derive(Clone)]
    struct LiveReplies {
        inspect: (i32, String, String),
        apply: (i32, String, String),
        probe: (i32, String, String),
        undo: (i32, String, String),
        commit: (i32, String, String),
    }

    impl Default for LiveReplies {
        fn default() -> Self {
            Self {
                inspect: (0, INSPECTION_OK.into(), String::new()),
                apply: (0, APPLY_OK.into(), String::new()),
                probe: (0, PROBE_OK.into(), String::new()),
                undo: (
                    0,
                    "step=dropin result=restored-or-removed\n".into(),
                    String::new(),
                ),
                commit: (0, "step=dropin-prev result=removed\n".into(), String::new()),
            }
        }
    }

    fn script_kind(argv: &[String], stdin: &[u8]) -> &'static str {
        let script = String::from_utf8_lossy(stdin);
        if argv.last().is_some_and(|a| a == PROBE_B64) {
            "probe"
        } else if script.contains("printf 'uname=%s\\n'") {
            "inspect"
        } else if script.contains("PULSE_HANDS_EOF") {
            "apply"
        } else if script.contains("result=restored-or-removed") {
            "undo"
        } else if script.contains("result=removed") {
            "commit"
        } else {
            "other"
        }
    }

    fn install_live_fake(
        replies: LiveReplies,
    ) -> (
        crate::test_helpers_hands::RunSshGuard,
        std::sync::Arc<Mutex<Vec<SshCall>>>,
    ) {
        let calls = std::sync::Arc::new(Mutex::new(Vec::<SshCall>::new()));
        let seen = calls.clone();
        let guard = crate::test_helpers_hands::install_run_ssh(move |_, argv, stdin, timeout| {
            let kind = script_kind(argv, stdin);
            seen.lock().expect("calls").push(SshCall {
                argv: argv.to_vec(),
                kind,
            });
            let remote = argv.last().cloned().unwrap_or_default();
            if kind == "probe" {
                assert!(
                    argv.iter().any(|a| a == "pulse@hands-two"),
                    "probe went to {argv:?}"
                );
            } else {
                assert_eq!(
                    remote, "sudo -n sh -s",
                    "{kind} did not run as root through sudo -n sh -s: {argv:?}"
                );
            }
            if kind == "apply" {
                assert_eq!(timeout, PAIR_APPLY_TIMEOUT, "apply timeout");
                let script = String::from_utf8_lossy(stdin);
                assert!(
                    script.starts_with("#!/bin/sh\nset -eu\nNAME=hands-two\n")
                        && script.contains("ssh-ed25519 AAAATESTKEY pulse-hub"),
                    "apply script carries the executor name and the hub key"
                );
            }
            let r = match kind {
                "inspect" => &replies.inspect,
                "apply" => &replies.apply,
                "probe" => &replies.probe,
                "undo" => &replies.undo,
                "commit" => &replies.commit,
                other => panic!("unexpected ssh call {other}: {argv:?}"),
            };
            Ok(r.clone())
        });
        (guard, calls)
    }

    fn admin_argv() -> Vec<String> {
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ClearAllForwardings=yes",
            "-p",
            "22",
            "admin@example.invalid",
            "sudo -n sh -s",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn hands_argv() -> Vec<String> {
        let mut v = admin_argv();
        v.truncate(v.len() - 2);
        v.push("pulse@hands-two".into());
        v.push(PROBE_B64.into());
        v
    }

    /// A hub home with the hands key already generated, so no test shells out to ssh-keygen.
    fn hub_with_key() -> tempfile::TempDir {
        let hub = tempfile::tempdir().expect("hub");
        let ssh = hub.path().join(".ssh");
        std::fs::create_dir_all(&ssh).expect("ssh dir");
        std::fs::write(ssh.join("pulse_hands_ed25519"), "fake private key\n").expect("key");
        std::fs::write(
            ssh.join("pulse_hands_ed25519.pub"),
            "ssh-ed25519 AAAATESTKEY pulse-hub\n",
        )
        .expect("pub");
        hub
    }

    fn live_opts(hub: &tempfile::TempDir, workers: i32) -> PairOpts {
        PairOpts {
            workdir: "/srv/pulse".into(),
            workers,
            projects: "*".into(),
            hub_home: hub.path().display().to_string(),
            ..Default::default()
        }
    }

    fn live_pair(
        s: &pulse_store::Store,
        hub: &tempfile::TempDir,
        workers: i32,
    ) -> anyhow::Result<Option<Box<PairReport>>> {
        pair_executor(
            s,
            "admin@example.invalid".into(),
            "hands-two".into(),
            TEST_NOW.into(),
            live_opts(hub, workers),
        )
    }

    fn fake_hands_binary() -> Box<HandsBinaryFn> {
        Box::new(|arch| {
            assert_eq!(arch, "amd64");
            Ok((
                b"#!/bin/sh\n# pulse-hands v2\nexit 0\n".to_vec(),
                "fixture".into(),
            ))
        })
    }

    fn pair_applied_events(s: &pulse_store::Store) -> Vec<serde_json::Value> {
        let conn = s.conn().expect("conn");
        crate::util::must_rows(
            &conn,
            "SELECT detail FROM events WHERE action = 'pair.applied' AND subject = 'hands-two' ORDER BY seq",
            &[],
        )
        .into_iter()
        .map(|r| serde_json::from_str(&r.str("detail")).expect("detail json"))
        .collect()
    }

    #[test]
    fn pair_live_applies_registers_probes_and_commits() {
        let _serial = crate::test_helpers::serial();
        let _guard = PAIR_TEST_LOCK.lock().expect("pair test lock");
        let s = new_store();
        let hub = hub_with_key();
        let old_binary = set_hands_binary(fake_hands_binary());
        let (ssh, calls) = install_live_fake(LiveReplies::default());
        let rep = live_pair(&s, &hub, 2)
            .unwrap_or_else(|e| panic!("live pair: {e}"))
            .expect("report");
        drop(ssh);
        let _ = set_hands_binary(old_binary);
        let got = calls.lock().expect("calls").clone();
        assert_eq!(
            got,
            vec![
                SshCall {
                    argv: admin_argv(),
                    kind: "inspect"
                },
                SshCall {
                    argv: admin_argv(),
                    kind: "apply"
                },
                SshCall {
                    argv: hands_argv(),
                    kind: "probe"
                },
                SshCall {
                    argv: admin_argv(),
                    kind: "commit"
                },
            ],
            "ssh sequence"
        );
        let text = rep.lines.join("\n");
        assert!(rep.ok, "report not ok:\n{text}");
        let row = crate::executor::executor_row(&s, "hands-two");
        assert!(
            !crate::util::is_nil(&row)
                && row.str("status") == "active"
                && row.str("address") == "pulse@hands-two"
                && row.str("workdir") == "/srv/pulse"
                && row.int("workers") == 2
                && row.str("projects") == "*"
                && row.str("probe_note").starts_with("ok:"),
            "executor row: {row:?}"
        );
        for want in [
            "added Host hands-two block to",
            "known_hosts: added 1 of 1 line(s) for example.invalid to",
            "applied on example.invalid as root: user=done hands=done pulse=done workdir=done authorized_keys=done gitconfig=done copilot=skipped mise=done dropin=done sshd_reload=done",
            "registered executor hands-two (pulse@hands-two, /srv/pulse, 2 worker(s), projects *)",
            "committed on example.invalid: dropin-prev=removed",
            "paired hands-two: active",
            "  host      example.invalid:22 (admin admin@example.invalid)",
            "  workers   2",
            "  probe     ok:",
            "  undo      pulse remove executor hands-two",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
        assert!(
            rep.scripts["apply"].contains("PULSE_HANDS_EOF")
                && !rep.scripts["apply"].contains("<pulse linux/"),
            "the live apply script carries the real binary"
        );
        let config = std::fs::read_to_string(hub.path().join(".ssh/config")).expect("config");
        assert_eq!(
            config,
            pair_host_block("hands-two".into(), "example.invalid".into(), 22),
            "ssh config"
        );
        let known =
            std::fs::read_to_string(hub.path().join(".ssh/known_hosts")).expect("known_hosts");
        assert_eq!(
            known,
            "example.invalid ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFAKEHOSTKEY root@example\n"
        );
        assert!(
            !hub.path().join(".ssh/config.bak").exists(),
            "no .bak when the config did not exist before"
        );
        let events = pair_applied_events(&s);
        assert_eq!(events.len(), 1, "pair.applied events: {events:?}");
        let d = &events[0];
        assert!(
            d["host"] == "example.invalid"
                && d["executor"] == "hands-two"
                && d["probe_ok"] == true
                && d["outcome"] == "active"
                && d["created"] == true
                && d["workers"] == 2,
            "pair.applied detail: {d}"
        );
        for action in ["executor.add", "executor.probe"] {
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = 'hands-two'",
                    &[&action],
                ),
                1,
                "{action} events"
            );
        }
    }

    #[test]
    fn pair_live_refusal_stops_before_apply() {
        let _serial = crate::test_helpers::serial();
        let _guard = PAIR_TEST_LOCK.lock().expect("pair test lock");
        let s = new_store();
        let hub = hub_with_key();
        let before = std::fs::read_dir(hub.path().join(".ssh"))
            .expect("ssh dir")
            .count();
        let old_binary = set_hands_binary(fake_hands_binary());
        let (ssh, calls) = install_live_fake(LiveReplies {
            inspect: (
                0,
                INSPECTION_OK.replace("remotes=0", "remotes=2"),
                String::new(),
            ),
            ..Default::default()
        });
        let events = count(&s, "SELECT COUNT(*) AS n FROM events", &[]);
        let err = live_pair(&s, &hub, 1).expect_err("refusal");
        drop(ssh);
        let _ = set_hands_binary(old_binary);
        let msg = err.to_string();
        let code = err
            .downcast_ref::<pulse_cli::Error>()
            .map(|e| e.exit_code())
            .unwrap_or(0);
        assert!(
            code == 2 && msg.contains("2 clone(s) with git remotes exist under the workdir"),
            "refusal: code {code}: {msg}"
        );
        let kinds = calls
            .lock()
            .expect("calls")
            .iter()
            .map(|c| c.kind)
            .collect::<Vec<_>>();
        assert_eq!(kinds, vec!["inspect"], "ssh calls after a refusal");
        assert!(
            crate::executor::executor_row(&s, "hands-two").is_empty()
                && count(&s, "SELECT COUNT(*) AS n FROM events", &[]) == events,
            "a refusal changed the store"
        );
        assert_eq!(
            std::fs::read_dir(hub.path().join(".ssh"))
                .expect("ssh dir")
                .count(),
            before,
            "a refusal changed the hub home"
        );
    }

    #[test]
    fn pair_live_failed_probe_undoes_and_removes() {
        let _serial = crate::test_helpers::serial();
        let _guard = PAIR_TEST_LOCK.lock().expect("pair test lock");
        let s = new_store();
        let hub = hub_with_key();
        let old_binary = set_hands_binary(fake_hands_binary());
        let (ssh, calls) = install_live_fake(LiveReplies {
            probe: (
                crate::executor::SSH_EXIT_UNREACH,
                String::new(),
                "ssh: connect to host example.invalid port 22: Connection refused".into(),
            ),
            ..Default::default()
        });
        let err = live_pair(&s, &hub, 1).expect_err("failed probe");
        drop(ssh);
        let _ = set_hands_binary(old_binary);
        let msg = err.to_string();
        let code = err
            .downcast_ref::<pulse_cli::Error>()
            .map(|e| e.exit_code())
            .unwrap_or(0);
        assert_eq!(code, 1, "exit code: {msg}");
        let kinds = calls
            .lock()
            .expect("calls")
            .iter()
            .map(|c| c.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec!["inspect", "apply", "probe", "undo"],
            "ssh calls after a failed probe"
        );
        for want in [
            "probe FAILED: unreachable: ssh: connect to host example.invalid port 22: Connection refused",
            "undid the sshd drop-in on example.invalid: dropin=restored-or-removed",
            "removed executor hands-two from the store",
            "next: check the connection from this account (`ssh -v hands-two`",
            "run `pulse pair admin@example.invalid --name hands-two` again",
        ] {
            assert!(msg.contains(want), "missing {want:?} in:\n{msg}");
        }
        assert!(
            crate::executor::executor_row(&s, "hands-two").is_empty(),
            "an executor row survived the failed probe"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM executors WHERE name = 'hands-two' AND status = 'removed'",
                &[],
            ),
            1,
            "the row is kept as removed"
        );
        let events = pair_applied_events(&s);
        assert!(
            events.len() == 1 && events[0]["probe_ok"] == false && events[0]["outcome"] == "undone",
            "pair.applied: {events:?}"
        );
        assert!(
            hub.path().join(".ssh/config").is_file(),
            "the local Host block stays (harmless)"
        );
    }

    #[test]
    fn pair_live_unfit_probe_keeps_the_executor_drained() {
        let _serial = crate::test_helpers::serial();
        let _guard = PAIR_TEST_LOCK.lock().expect("pair test lock");
        let s = new_store();
        let hub = hub_with_key();
        let old_binary = set_hands_binary(fake_hands_binary());
        let (ssh, calls) = install_live_fake(LiveReplies {
            probe: (
                0,
                PROBE_OK.replace("harness=yes", "harness=no"),
                String::new(),
            ),
            ..Default::default()
        });
        let rep = live_pair(&s, &hub, 1)
            .unwrap_or_else(|e| panic!("live pair: {e}"))
            .expect("report");
        drop(ssh);
        let _ = set_hands_binary(old_binary);
        let kinds = calls
            .lock()
            .expect("calls")
            .iter()
            .map(|c| c.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec!["inspect", "apply", "probe", "commit"],
            "unfit hands are paired, not undone"
        );
        let text = rep.lines.join("\n");
        assert!(!rep.ok, "report ok although the probe failed:\n{text}");
        for want in [
            "paired hands-two: drained (the probe did not pass)",
            "  probe     FAILED: harness login missing",
            "  next      `pulse executor login hands-two --admin admin@example.invalid` (add --with-token to paste a token), then `pulse executor probe hands-two`",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
        let row = crate::executor::executor_row(&s, "hands-two");
        assert!(
            !crate::util::is_nil(&row)
                && row.str("status") == "drained"
                && row.str("probe_note").starts_with("harness login missing"),
            "executor row: {row:?}"
        );
        let events = pair_applied_events(&s);
        assert!(
            events.len() == 1
                && events[0]["probe_ok"] == false
                && events[0]["outcome"] == "drained",
            "pair.applied: {events:?}"
        );
    }

    #[test]
    fn pair_live_local_ssh_files_are_idempotent() {
        let _serial = crate::test_helpers::serial();
        let _guard = PAIR_TEST_LOCK.lock().expect("pair test lock");
        let s = new_store();
        let hub = hub_with_key();
        let config_path = hub.path().join(".ssh/config");
        let known_path = hub.path().join(".ssh/known_hosts");
        let original = "Host other\n    HostName other.example\n";
        std::fs::write(&config_path, original).expect("config");
        let old_binary = set_hands_binary(fake_hands_binary());
        let (ssh, calls) = install_live_fake(LiveReplies::default());

        let first = live_pair(&s, &hub, 1)
            .unwrap_or_else(|e| panic!("first pair: {e}"))
            .expect("report");
        assert!(first.ok, "first pair:\n{}", first.lines.join("\n"));
        let config1 = std::fs::read_to_string(&config_path).expect("config");
        let known1 = std::fs::read_to_string(&known_path).expect("known_hosts");
        assert_eq!(
            config1,
            format!(
                "{original}\n{}",
                pair_host_block("hands-two".into(), "example.invalid".into(), 22)
            ),
            "first config"
        );
        assert_eq!(
            std::fs::read_to_string(hub.path().join(".ssh/config.bak")).expect("bak"),
            original,
            "the .bak keeps the file as it was before the first change"
        );
        assert_eq!(known1.lines().count(), 1, "first known_hosts: {known1:?}");
        let bak_mtime = std::fs::metadata(hub.path().join(".ssh/config.bak"))
            .expect("bak")
            .modified()
            .expect("mtime");

        // A re-pair with different sizing: the files stay as they are, the row is updated.
        let second = live_pair(&s, &hub, 3)
            .unwrap_or_else(|e| panic!("second pair: {e}"))
            .expect("report");
        drop(ssh);
        let _ = set_hands_binary(old_binary);
        let text = second.lines.join("\n");
        assert!(second.ok, "second pair:\n{text}");
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("config"),
            config1,
            "second run changed the ssh config"
        );
        assert_eq!(
            std::fs::read_to_string(&known_path).expect("known_hosts"),
            known1,
            "second run changed known_hosts"
        );
        assert_eq!(
            std::fs::metadata(hub.path().join(".ssh/config.bak"))
                .expect("bak")
                .modified()
                .expect("mtime"),
            bak_mtime,
            "second run rewrote the .bak"
        );
        for want in [
            "kept existing Host hands-two block in",
            "known_hosts: all 1 line(s) for example.invalid already in",
            "updated executor hands-two (pulse@hands-two, /srv/pulse, 3 worker(s), projects *; status was active)",
            "paired hands-two: active",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
        let row = crate::executor::executor_row(&s, "hands-two");
        assert!(
            row.str("status") == "active" && row.int("workers") == 3,
            "re-paired row: {row:?}"
        );
        let kinds = calls
            .lock()
            .expect("calls")
            .iter()
            .map(|c| c.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                "inspect", "apply", "probe", "commit", "inspect", "apply", "probe", "commit"
            ]
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.update' AND subject = 'hands-two'",
                &[],
            ),
            1,
            "executor.update events"
        );
        assert_eq!(pair_applied_events(&s).len(), 2, "pair.applied events");
    }

    #[test]
    fn module_root_finds_the_hinted_checkout() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("brain");
        std::fs::create_dir_all(root.join("crates/pulse")).expect("crates");
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").expect("manifest");
        std::fs::write(root.join("crates/pulse/Cargo.toml"), "[package]\n").expect("pulse");
        let nowhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&nowhere).expect("elsewhere");
        assert_eq!(module_root_from(std::slice::from_ref(&nowhere)), "");
        // the hint is searched upward like the others, so the daemon may be given a sub-folder
        assert_eq!(
            module_root_from(&[nowhere, root.join("crates/pulse-runner/src")]),
            root.to_string_lossy()
        );
        // a plain crate is not a checkout the hands binary can be built from
        std::fs::remove_file(root.join("crates/pulse/Cargo.toml")).expect("rm");
        assert_eq!(module_root_from(&[root]), "");
    }

    #[test]
    fn cross_build_prefers_zigbuild_when_present() {
        let plain = cross_build_args("x86_64-unknown-linux-musl", "/src/pulse", false);
        assert_eq!(
            plain,
            vec![
                "build",
                "--release",
                "--locked",
                "--bin",
                "pulse",
                "--target",
                "x86_64-unknown-linux-musl",
                "--target-dir",
                "/src/pulse/target"
            ]
        );
        let zig = cross_build_args("aarch64-unknown-linux-musl", "/src/pulse", true);
        assert_eq!(zig[0], "zigbuild");
        assert_eq!(
            zig[1..],
            plain[1..]
                .iter()
                .map(|a| a.replace("x86_64", "aarch64"))
                .collect::<Vec<_>>()[..]
        );
    }
}

fn default_hands_binary(arch: &str) -> anyhow::Result<(Vec<u8>, String)> {
    if let Ok(p) = std::env::var("PULSE_HANDS_BINARY") {
        return Ok((std::fs::read(&p)?, format!("PULSE_HANDS_BINARY={p}")));
    }
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(format!("pulse-linux-{arch}"));
        if let Ok(data) = std::fs::read(&sibling) {
            return Ok((data, sibling.to_string_lossy().to_string()));
        }
        #[cfg(target_os = "linux")]
        {
            let rust_arch = if cfg!(target_arch = "x86_64") {
                "amd64"
            } else if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                ""
            };
            if rust_arch == arch {
                return Ok((std::fs::read(&exe)?, exe.to_string_lossy().to_string()));
            }
        }
    }
    let triple = match arch {
        "amd64" => "x86_64-unknown-linux-musl",
        "arm64" => "aarch64-unknown-linux-musl",
        _ => "",
    };
    let root = module_root();
    if root.is_empty() || triple.is_empty() {
        return Err(crate::util::fail(1, hands_binary_missing(arch, triple)).into());
    }
    if !rustup_target_installed(triple) {
        return Err(crate::util::fail(
            1,
            format!(
                "{}; the checkout at {root} cannot cross-build it: the Rust target {triple} is not installed (rustup target add {triple}; linking it from macOS needs cargo-zigbuild and zig)",
                hands_binary_missing(arch, triple)
            ),
        )
        .into());
    }
    let zig = on_path("cargo-zigbuild");
    let argv = cross_build_args(triple, &root, zig);
    let out = Command::new("cargo")
        .args(&argv)
        .current_dir(&root)
        .output()?;
    if !out.status.success() {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        let mut why = format!(
            "cargo {} for linux/{arch} in {root} failed: {}",
            argv[0],
            pair_tail(combined)
        );
        if !zig && !cfg!(target_os = "linux") {
            why.push_str(
                "; cross-linking from this machine needs cargo-zigbuild (cargo install cargo-zigbuild; brew install zig), or place pulse-linux-",
            );
            why.push_str(arch);
            why.push_str(" beside this binary / set PULSE_HANDS_BINARY");
        }
        return Err(crate::util::fail(1, why).into());
    }
    let artifact = Path::new(&root)
        .join("target")
        .join(triple)
        .join("release")
        .join("pulse");
    let data = std::fs::read(&artifact)?;
    Ok((
        data,
        format!("cargo {} --target {triple} --bin pulse", argv[0]),
    ))
}

fn hands_binary_missing(arch: &str, triple: &str) -> String {
    let mut s = format!(
        "no linux/{arch} pulse binary to ship: set PULSE_HANDS_BINARY, place pulse-linux-{arch} beside this binary, or run from the pulse source tree"
    );
    if !triple.is_empty() {
        s.push_str(&format!(
            " with the {triple} target installed (the daemon builds from the brain project's checkout)"
        ));
    }
    s
}

/// `cargo zigbuild` links a musl target from any host; plain `cargo build` needs a linker for it.
pub(crate) fn cross_build_args(triple: &str, root: &str, zigbuild: bool) -> Vec<String> {
    vec![
        if zigbuild { "zigbuild" } else { "build" }.into(),
        "--release".into(),
        "--locked".into(),
        "--bin".into(),
        "pulse".into(),
        "--target".into(),
        triple.into(),
        "--target-dir".into(),
        format!("{root}/target"),
    ]
}

fn rustup_target_installed(triple: &str) -> bool {
    match Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim() == triple),
        // without rustup (a distro toolchain) cargo itself says whether the target is there
        _ => true,
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

fn parse_pair_ssh_address(text: &str) -> anyhow::Result<(String, String, i32)> {
    let (user, rest) = text.split_once('@').ok_or_else(|| {
        crate::util::fail(
            2,
            format!("bad ssh address {text:?}: want user@host[:port]"),
        )
    })?;
    if user.is_empty() || rest.is_empty() {
        return Err(crate::util::fail(
            2,
            format!("bad ssh address {text:?}: want user@host[:port]"),
        )
        .into());
    }
    let (host, port) = if let Some((h, p)) = rest.rsplit_once(':') {
        if p.chars().all(|c| c.is_ascii_digit()) {
            (h.to_string(), p.parse::<i32>().unwrap_or(22))
        } else {
            (rest.to_string(), 22)
        }
    } else {
        (rest.to_string(), 22)
    };
    if host.is_empty() {
        return Err(crate::util::fail(
            2,
            format!("bad ssh address {text:?}: want user@host[:port]"),
        )
        .into());
    }
    Ok((user.to_string(), host, port))
}
