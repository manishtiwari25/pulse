#![allow(dead_code)]

use anyhow::anyhow;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
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

/// Go: func validName(name string) bool
pub(crate) fn valid_name(name: String) -> bool {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^[a-z][a-z0-9-]*$").unwrap());
    RE.is_match(&name)
}

/// Go: func pairDropinText() string
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

/// Go: func pairAuthorizedKeyLine(workdir, pubkey string) string
pub(crate) fn pair_authorized_key_line(workdir: String, pubkey: String) -> String {
    format!(
        "command=\"PULSE_HANDS_WORKDIR={} {} hands\",restrict {}",
        workdir,
        PAIR_HANDS_PATH,
        pubkey.trim()
    )
}

/// Go: func pairParseInspection(text string) (map[string]string, []string)
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

/// Go: func pairTail(text string) string
pub(crate) fn pair_tail(text: String) -> String {
    crate::executor::tail(&text.split_whitespace().collect::<Vec<_>>().join(" "), 500)
}

/// Go: func pairRefuse(format string, args ...any) error
pub(crate) fn pair_refuse(format: String, args: &[serde_json::Value]) -> anyhow::Result<()> {
    let mut msg = format;
    for a in args {
        msg = msg.replacen("%s", a.as_str().unwrap_or(&a.to_string()), 1);
    }
    Err(crate::util::fail(2, msg).into())
}

/// Go: func pairValidate(name, admin, workdir string, workers int) (string, string, string, int, string, error)
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

/// Go: func pairSSHConfigBlocks(text string) []sshBlock
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

/// Go: func pairAliasState(config, name string) *sshBlock
pub(crate) fn pair_alias_state(config: String, name: String) -> Option<Box<SshBlock>> {
    pair_ssh_config_blocks(config)
        .into_iter()
        .find(|b| b.names.iter().any(|n| n == &name))
        .map(Box::new)
}

/// Go: func pairHostBlock(name, host string, port int) string
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

/// Go: func pairKnownHostPrefix(host string, port int) string
pub(crate) fn pair_known_host_prefix(host: String, port: i32) -> String {
    if port == 22 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

/// Go: func pairManualDropinLines() []string
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

/// Go: func pairManualCopilotLines() []string
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

/// Go: func pairInspectScript(workdir, pubkey, handsHash, dropinHash string) string
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

/// Go: func miseInstallBlock(user, homeVar string) string
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

/// Go: func pairApplyScript(name, workdir, pubkey string, hands []byte, installCopilot bool) string
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

/// Go: func wrapBase64(data []byte) string
pub(crate) fn wrap_base64(data: Vec<u8>) -> String {
    let enc = base64::engine::general_purpose::STANDARD.encode(data);
    enc.as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Go: func pairUndoScript() string
pub(crate) fn pair_undo_script() -> String {
    format!(
        "#!/bin/sh\nset -eu\nDROPIN={}\nif [ -f \"$DROPIN.prev\" ]; then\n  mv -f \"$DROPIN.prev\" \"$DROPIN\"\nelse\n  rm -f \"$DROPIN\"\nfi\nsystemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || systemctl restart ssh 2>/dev/null || systemctl restart sshd\nprintf 'step=dropin result=restored-or-removed\\n'\n",
        crate::util::shell_quote(PAIR_DROPIN_PATH)
    )
}

/// Go: func pairCommitScript() string
pub(crate) fn pair_commit_script() -> String {
    format!(
        "#!/bin/sh\nset -eu\nrm -f {}.prev\nprintf 'step=dropin-prev result=removed\\n'\n",
        crate::util::shell_quote(PAIR_DROPIN_PATH)
    )
}

/// Go: func pairStepResults(text string) map[string]string
pub(crate) fn pair_step_results(text: String) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if let Some(c) = STEP_RE.captures(line.trim()) {
            out.insert(c[1].to_string(), c[2].to_string());
        }
    }
    out
}

/// Go: func pairEnsureHubKey(hubHome string, lines *[]string) (string, string, error)
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

/// Go: func pairInspectionPubkey(hubHome string, dryRun bool, lines *[]string) (string, string, bool)
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

/// Go: func pairCheckAlias(hubHome, name, host string) (string, string, *sshBlock, error)
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

/// Go: func pairWriteAlias(config, text string, block *sshBlock, name, host string, port int) bool
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

/// Go: func pairWriteKnownHosts(hubHome, host string, port int, hostkeys []string) int
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

/// Go: func pairOpenControlMaster(admin, controlPath string) (int, string, error)
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

/// Go: func pairCloseControlMaster(admin, controlPath string)
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

/// Go: func pairInspectionSummary(insp map[string]string, hands []byte) []string
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

/// Go: func pairRefusalsFromInspection(insp map[string]string, installCopilot bool) []string
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

/// Go: func goArchFor(uname string) string
pub(crate) fn go_arch_for(uname: String) -> String {
    match uname.as_str() {
        "x86_64" | "amd64" => "amd64".into(),
        "aarch64" | "arm64" => "arm64".into(),
        _ => String::new(),
    }
}

/// Go: func moduleRoot() string
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
    for start in starts {
        for dir in start.ancestors() {
            let go_mod = dir.join("go.mod");
            if std::fs::read_to_string(&go_mod)
                .is_ok_and(|d| d.contains("module github.com/manishtiwari25/pulse"))
            {
                return dir.to_string_lossy().to_string();
            }
        }
    }
    String::new()
}

/// Go: func sha256Bytes(data []byte) string
pub(crate) fn sha256_bytes(data: Vec<u8>) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(data))
}

/// Go: func pairExecutor(s *store.Store, admin, name, now string, o pairOpts) (*pairReport, error)
pub(crate) fn pair_executor(
    s: &pulse_store::Store,
    admin: String,
    name: String,
    now: String,
    o: PairOpts,
) -> anyhow::Result<Option<Box<PairReport>>> {
    let (name, _user, host, port, workdir) =
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
    let apply_script = pair_apply_script(
        name.clone(),
        workdir.clone(),
        pubkey.clone(),
        hands.clone(),
        o.install_copilot,
    );
    let undo_script = pair_undo_script();
    let commit_script = pair_commit_script();
    rep.scripts.insert("inspect".into(), inspect_script);
    rep.scripts.insert("undo".into(), undo_script.clone());
    rep.scripts.insert("commit".into(), commit_script.clone());
    if o.dry_run {
        let placeholder = format!("<pulse linux/{arch} binary, base64>");
        let shown = pair_apply_script(
            name.clone(),
            workdir.clone(),
            pubkey,
            Vec::new(),
            o.install_copilot,
        )
        .replace(
            "<<'PULSE_HANDS_EOF'\n\nPULSE_HANDS_EOF",
            &format!("<<'PULSE_HANDS_EOF'\n{placeholder}\nPULSE_HANDS_EOF"),
        );
        rep.scripts.insert("apply".into(), shown.clone());
        rep.lines.extend([
            "remote script (would run as root via sudo -n sh -s; the hands heredoc carries the linux pulse binary):".into(),
            shown.trim_end().into(),
            "undo script (the exact inverse):".into(),
            undo_script.trim_end().into(),
            "commit script (after successful admin re-check):".into(),
            commit_script.trim_end().into(),
            "local changes that would be made:".into(),
        ]);
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

    if !key_ready {
        let (_, pubkey) = pair_ensure_hub_key(hub_home, &mut rep.lines)?;
        rep.scripts.insert(
            "apply".into(),
            pair_apply_script(name, workdir, pubkey, hands, o.install_copilot),
        );
    } else {
        rep.scripts.insert("apply".into(), apply_script);
    }
    let _ = (decision, actor, config, config_text, alias_block);
    Err(anyhow!(
        "pair live apply is waiting on cross-slice executor/store implementations"
    ))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, count, new_store};
    use crate::test_helpers_f6::invoke;
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
            let go_arch = if cfg!(target_arch = "x86_64") {
                "amd64"
            } else if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                ""
            };
            if go_arch == arch {
                return Ok((std::fs::read(&exe)?, exe.to_string_lossy().to_string()));
            }
        }
    }
    let root = module_root();
    if root.is_empty() {
        return Err(crate::util::fail(1, format!("no linux/{arch} pulse binary to ship: set PULSE_HANDS_BINARY, place pulse-linux-{arch} beside this binary, or run from the pulse source tree")).into());
    }
    let tmp = std::env::temp_dir().join(format!("pulse-linux-{arch}-{}", uuid::Uuid::new_v4()));
    let out = Command::new("go")
        .args([
            "build",
            "-trimpath",
            "-o",
            &tmp.to_string_lossy(),
            "./cmd/pulse",
        ])
        .current_dir(&root)
        .env("GOOS", "linux")
        .env("GOARCH", arch)
        .env("CGO_ENABLED", "0")
        .output()?;
    if !out.status.success() {
        return Err(crate::util::fail(
            1,
            format!(
                "go build for linux/{arch} failed: {}",
                pair_tail(String::from_utf8_lossy(&out.stderr).to_string())
            ),
        )
        .into());
    }
    let data = std::fs::read(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Ok((
        data,
        format!("go build GOOS=linux GOARCH={arch} ./cmd/pulse"),
    ))
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
