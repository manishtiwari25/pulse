package runner

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// `pulse pair` (ADR-021 section 2): turn an existing Linux machine into a pair of hands over the admin session.
// The hands it ships are the linux `pulse` binary itself (`pulse hands` is the forced command), streamed
// base64-encoded inside the apply script instead of the old shell script.

const (
	pairDropinPath = "/etc/ssh/sshd_config.d/60-pulse-hands.conf"
	pairHandsPath  = "/usr/local/bin/pulse-hands"
	pairPulsePath  = "/usr/local/bin/pulse"
)

func validName(name string) bool { return validNameRe.MatchString(name) }

func pairDropinText() string {
	return strings.Join([]string{
		"# The hands' user gets exactly what its authorized_keys line allows and nothing else.",
		"Match User pulse",
		"    PasswordAuthentication no",
		"    AllowAgentForwarding no",
		"    AllowTcpForwarding no",
		"    X11Forwarding no",
		"    PermitTTY no",
		"    PermitTunnel no",
		"",
	}, "\n")
}

// pairAuthorizedKeyLine pins the key to `pulse-hands hands`: the binary's hands verb is the forced command.
func pairAuthorizedKeyLine(workdir, pubkey string) string {
	return fmt.Sprintf(`command="PULSE_HANDS_WORKDIR=%s %s hands",restrict %s`, workdir, pairHandsPath, strings.TrimSpace(pubkey))
}

func pairParseInspection(text string) (map[string]string, []string) {
	fields := parseProbe(text)
	var hostkeys []string
	for _, line := range strings.Split(text, "\n") {
		if strings.HasPrefix(line, "hostkey ") {
			hostkeys = append(hostkeys, strings.TrimSpace(line[len("hostkey "):]))
		}
	}
	return fields, hostkeys
}

func pairTail(text string) string { return tail(strings.Join(strings.Fields(text), " "), 500) }

func pairRefuse(format string, args ...any) error { return fail(2, format, args...) }

func pairValidate(name, admin, workdir string, workers int) (string, string, string, int, string, error) {
	name = slugify(name, 40)
	if name == "" || name == localExecutor || name == "x" || !validName(name) {
		return "", "", "", 0, "", pairRefuse("Executor name must be lowercase words with dashes and not %s", pyRepr(localExecutor))
	}
	user, host, port, err := parseSSHAddress(admin)
	if err != nil {
		return "", "", "", 0, "", err
	}
	workdir = strings.TrimRight(strings.TrimSpace(workdir), "/")
	if workdir == "" {
		workdir = "/"
	}
	if !strings.HasPrefix(workdir, "/") || contains(strings.Split(workdir, "/"), "..") {
		return "", "", "", 0, "", pairRefuse("--workdir must be an absolute path on the executor (e.g. /srv/pulse)")
	}
	if workers < 1 {
		return "", "", "", 0, "", pairRefuse("--workers must be at least 1")
	}
	return name, user, host, port, workdir, nil
}

type sshBlock struct {
	names    []string
	hostname string
}

func pairSSHConfigBlocks(text string) []sshBlock {
	var blocks []sshBlock
	var cur *sshBlock
	for _, line := range strings.Split(text, "\n") {
		words := strings.Fields(line)
		if len(words) > 0 && strings.EqualFold(words[0], "host") {
			if cur != nil {
				blocks = append(blocks, *cur)
			}
			cur = &sshBlock{names: words[1:]}
			continue
		}
		if cur != nil && len(words) >= 2 && strings.EqualFold(words[0], "hostname") {
			cur.hostname = words[1]
		}
	}
	if cur != nil {
		blocks = append(blocks, *cur)
	}
	return blocks
}

func pairAliasState(config, name string) *sshBlock {
	for _, b := range pairSSHConfigBlocks(config) {
		if contains(b.names, name) {
			bb := b
			return &bb
		}
	}
	return nil
}

func pairHostBlock(name, host string, port int) string {
	return strings.Join([]string{
		"Host " + name,
		"    HostName " + host,
		fmt.Sprintf("    Port %d", port),
		"    User pulse",
		"    IdentityFile ~/.ssh/pulse_hands_ed25519",
		"    IdentitiesOnly yes",
		"",
	}, "\n")
}

func pairKnownHostPrefix(host string, port int) string {
	if port == 22 {
		return host
	}
	return fmt.Sprintf("[%s]:%d", host, port)
}

func pairManualDropinLines() []string {
	return append([]string{"manual sshd Match block for " + pairDropinPath + ":"}, strings.Split(strings.TrimRight(pairDropinText(), "\n"), "\n")...)
}

func pairManualCopilotLines() []string {
	return []string{
		"manual copilot install steps for this machine:",
		"  mkdir -p /etc/apt/keyrings",
		"  curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg",
		`  echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main" > /etc/apt/sources.list.d/nodesource.list`,
		"  apt-get update",
		"  apt-get install -y nodejs",
		"  npm install -g @github/copilot",
	}
}

// pairInspectScript: read-only remote inspection; prints key=value lines and never changes the machine.
// It also reads hands metadata for comparison once the linux binary is built.
func pairInspectScript(workdir, pubkey, handsHash, dropinHash string) string {
	return `#!/bin/sh
set -u
WORKDIR=` + shellQuote(workdir) + `
PUBKEY=` + shellQuote(pubkey) + `
DROPIN_HASH=` + shellQuote(dropinHash) + `
DROPIN=` + shellQuote(pairDropinPath) + `
printf 'uname=%s\n' "$(uname -s 2>/dev/null || echo unknown)"
printf 'arch=%s\n' "$(uname -m 2>/dev/null || echo unknown)"
if sudo -n true 2>/dev/null; then echo sudo=yes; else echo sudo=no; fi
if getent passwd pulse >/dev/null 2>&1; then echo user=present; else echo user=absent; fi
` + pairInspectHandsScript(pairHandsPath, handsHash) + `if [ -d "$WORKDIR" ]; then
  echo workdir=present
  if command -v stat >/dev/null 2>&1; then
    owner="$(stat -c '%U:%G:%a' "$WORKDIR" 2>/dev/null || ls -ld "$WORKDIR" | awk '{print $3 ":" $4}')"
  else
    owner="$(ls -ld "$WORKDIR" | awk '{print $3 ":" $4}')"
  fi
  printf 'workdir_owner=%s\n' "$owner"
else
  echo workdir=absent
  echo workdir_owner=
fi
auth=absent
auth_lines=0
home="$(getent passwd pulse 2>/dev/null | awk -F: '{print $6}')"
ak="$home/.ssh/authorized_keys"
if [ -n "$home" ] && [ -f "$ak" ]; then
  auth_lines="$(wc -l < "$ak" | tr -d ' ')"
  if [ "$auth_lines" = "1" ] && grep -F "$PUBKEY" "$ak" >/dev/null 2>&1; then auth=ours; else auth=other; fi
fi
printf 'authorized_keys=%s\n' "$auth"
printf 'authorized_keys_lines=%s\n' "$auth_lines"
if [ -f "$DROPIN" ]; then
  if [ -n "$DROPIN_HASH" ] && command -v sha256sum >/dev/null 2>&1 && [ "$(sha256sum "$DROPIN" | awk '{print $1}')" = "$DROPIN_HASH" ]; then
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
if [ -n "${COPILOT_GITHUB_TOKEN:-}${GH_TOKEN:-}${GITHUB_TOKEN:-}" ] || su -s /bin/sh pulse -c 'test -f "$HOME/.pulse/private/providers/copilot.json"' >/dev/null 2>&1; then echo harness=yes; else echo harness=no; fi
if command -v git >/dev/null 2>&1; then echo git=present; else echo git=missing; fi
if su -s /bin/sh pulse -c 'command -v mise >/dev/null 2>&1 || test -x "$HOME/.local/bin/mise"' >/dev/null 2>&1; then
  printf 'mise=%s\n' "$(su -s /bin/sh pulse -c 'PATH="$HOME/.local/bin:$PATH" mise version 2>/dev/null' | awk '{print $1}' | head -1)"
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
`
}

// miseInstallBlock puts the pinned mise release under the executor user's ~/.local/bin, checksum-verified.
func miseInstallBlock(user, homeVar string) string {
	bare := strings.TrimPrefix(miseVersion, "v")
	return fmt.Sprintf(`
mise_dst="%[1]s/.local/bin/mise"
if [ -x "$mise_dst" ] && [ "$(sudo -u %[2]s -H "$mise_dst" version 2>/dev/null | awk '{print $1}' | head -1)" = "%[3]s" ]; then
  step mise kept
else
  case "$(uname -m)" in
    x86_64) mise_arch=x64; mise_sum=%[4]s ;;
    aarch64|arm64) mise_arch=arm64; mise_sum=%[5]s ;;
    *) mise_arch=""; mise_sum="" ;;
  esac
  if [ -z "$mise_arch" ]; then
    step mise skipped
  else
    mise_tmp="$(mktemp)"
    if curl -fsSL -o "$mise_tmp" "https://github.com/jdx/mise/releases/download/%[6]s/mise-%[6]s-linux-$mise_arch" &&
       [ "$(sha256sum "$mise_tmp" | awk '{print $1}')" = "$mise_sum" ]; then
      install -d -m 0755 -o %[2]s -g %[2]s "%[1]s/.local" "%[1]s/.local/bin"
      install -m 0755 -o %[2]s -g %[2]s "$mise_tmp" "$mise_dst"
      rm -f "$mise_tmp"
      step mise done
    else
      rm -f "$mise_tmp"
      step mise failed
    fi
  fi
fi
`, homeVar, user, bare, miseSHA256["x86_64"], miseSHA256["aarch64"], miseVersion)
}

// pairApplyScript is the idempotent root script sent on stdin to the admin session. `hands` is the linux
// pulse binary, shipped base64 in a quoted heredoc (no expansion) and decoded on the machine.
func pairApplyScript(name, workdir, pubkey string, hands []byte, installCopilot bool) string {
	installBlock := "step copilot skipped\n"
	if installCopilot {
		installBlock = `
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
`
	}
	return `#!/bin/sh
set -eu
NAME=` + shellQuote(name) + `
WORKDIR=` + shellQuote(workdir) + `
AUTH_LINE=` + shellQuote(pairAuthorizedKeyLine(workdir, pubkey)) + `
DROPIN=` + shellQuote(pairDropinPath) + `
step() { printf 'step=%s result=%s\n' "$1" "$2"; }
if getent passwd pulse >/dev/null 2>&1; then
  step user kept
else
  useradd -m -s /bin/bash pulse
  passwd -l pulse >/dev/null 2>&1 || true
  step user done
fi
home="$(getent passwd pulse | awk -F: '{print $6}')"
base64 -d > ` + pairHandsPath + `.tmp <<'PULSE_HANDS_EOF'
` + wrapBase64(hands) + `
PULSE_HANDS_EOF
if [ -f ` + pairHandsPath + ` ] && cmp -s ` + pairHandsPath + `.tmp ` + pairHandsPath + `; then
  rm -f ` + pairHandsPath + `.tmp
  step hands kept
else
  install -m 0755 -o root -g root ` + pairHandsPath + `.tmp ` + pairHandsPath + `
  rm -f ` + pairHandsPath + `.tmp
  step hands done
fi
ln -sf ` + pairHandsPath + ` ` + pairPulsePath + `
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
` + installBlock + miseInstallBlock("pulse", "$home") + `install -d -m 0755 -o root -g root /etc/ssh/sshd_config.d
cat > "$DROPIN.tmp" <<'PULSE_DROPIN_EOF'
` + pairDropinText() + `PULSE_DROPIN_EOF
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
`
}

func wrapBase64(data []byte) string {
	enc := base64.StdEncoding.EncodeToString(data)
	var b strings.Builder
	for len(enc) > 76 {
		b.WriteString(enc[:76])
		b.WriteByte('\n')
		enc = enc[76:]
	}
	b.WriteString(enc)
	return b.String()
}

func pairUndoScript() string {
	return `#!/bin/sh
set -eu
DROPIN=` + shellQuote(pairDropinPath) + `
if [ -f "$DROPIN.prev" ]; then
  mv -f "$DROPIN.prev" "$DROPIN"
else
  rm -f "$DROPIN"
fi
systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || systemctl restart ssh 2>/dev/null || systemctl restart sshd
printf 'step=dropin result=restored-or-removed\n'
`
}

func pairCommitScript() string {
	return `#!/bin/sh
set -eu
rm -f ` + shellQuote(pairDropinPath) + `.prev
printf 'step=dropin-prev result=removed\n'
`
}

var stepRe = regexp.MustCompile(`^step=([A-Za-z0-9_-]+)[ \t]+result=([A-Za-z0-9_-]+)`)

func pairStepResults(text string) map[string]string {
	steps := map[string]string{}
	for _, line := range strings.Split(text, "\n") {
		if m := stepRe.FindStringSubmatch(strings.TrimSpace(line)); m != nil {
			steps[m[1]] = m[2]
		}
	}
	return steps
}

func pairEnsureHubKey(hubHome string, lines *[]string) (string, string, error) {
	sshDir := filepath.Join(hubHome, ".ssh")
	key := filepath.Join(sshDir, "pulse_hands_ed25519")
	pub := key + ".pub"
	if !fileExists(key) {
		_ = os.MkdirAll(sshDir, 0o700)
		_ = os.Chmod(sshDir, 0o700)
		out, err := exec.Command("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "pulse-hub", "-f", key).CombinedOutput()
		if err != nil {
			return "", "", fail(1, "ssh-keygen failed: %s", pairTail(string(out)))
		}
		_ = os.Chmod(key, 0o600)
		*lines = append(*lines, "generated hub key: "+key)
	}
	data, err := os.ReadFile(pub)
	if err != nil {
		return "", "", fail(1, "Missing public key beside %s", key)
	}
	first, _, _ := strings.Cut(strings.TrimSpace(string(data)), "\n")
	return key, first, nil
}

func pairInspectionPubkey(hubHome string, dryRun bool, lines *[]string) (string, string, bool) {
	key := filepath.Join(hubHome, ".ssh", "pulse_hands_ed25519")
	pub := key + ".pub"
	if fileExists(key) && fileExists(pub) {
		data, _ := os.ReadFile(pub)
		first, _, _ := strings.Cut(strings.TrimSpace(string(data)), "\n")
		return key, first, true
	}
	if dryRun {
		*lines = append(*lines, "would generate hub key: "+key)
	}
	return key, "ssh-ed25519 PULSEPAIRINSPECTIONONLY pulse-hub", false
}

func pairCheckAlias(hubHome, name, host string) (string, string, *sshBlock, error) {
	config := filepath.Join(hubHome, ".ssh", "config")
	text := ""
	if data, err := os.ReadFile(config); err == nil {
		text = string(data)
	}
	block := pairAliasState(text, name)
	if block != nil && host != name {
		target := block.hostname
		if target == "" {
			target = name
		}
		if target != host {
			return "", "", nil, pairRefuse("alias %s already points elsewhere", name)
		}
	}
	return config, text, block, nil
}

func pairWriteAlias(config, text string, block *sshBlock, name, host string, port int) bool {
	if block != nil {
		return false
	}
	_ = os.MkdirAll(filepath.Dir(config), 0o700)
	_ = os.Chmod(filepath.Dir(config), 0o700)
	prefix := ""
	if text != "" {
		prefix = "\n\n"
		if strings.HasSuffix(text, "\n") {
			prefix = "\n"
		}
	}
	f, err := os.OpenFile(config, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o600)
	if err != nil {
		return false
	}
	_, _ = f.WriteString(prefix + pairHostBlock(name, host, port))
	f.Close()
	_ = os.Chmod(config, 0o600)
	return true
}

func pairWriteKnownHosts(hubHome, host string, port int, hostkeys []string) int {
	if len(hostkeys) == 0 {
		return 0
	}
	path := filepath.Join(hubHome, ".ssh", "known_hosts")
	_ = os.MkdirAll(filepath.Dir(path), 0o700)
	existing := ""
	if data, err := os.ReadFile(path); err == nil {
		existing = string(data)
	}
	have := map[string]bool{}
	for _, l := range strings.Split(existing, "\n") {
		have[l] = true
	}
	prefix := pairKnownHostPrefix(host, port)
	f, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o600)
	if err != nil {
		return 0
	}
	defer f.Close()
	added := 0
	for _, hk := range hostkeys {
		hk = strings.TrimSpace(hk)
		line := prefix + " " + hk
		if hk != "" && !have[line] {
			fmt.Fprintln(f, line)
			have[line] = true
			added++
		}
	}
	_ = os.Chmod(path, 0o600)
	return added
}

func pairOpenControlMaster(admin, controlPath string) (int, string, error) {
	user, host, port, err := parseSSHAddress(admin)
	if err != nil {
		return 0, "", err
	}
	cmd := append([]string{"ssh"}, sshOptions...)
	cmd = append(cmd, "-o", "ControlMaster=yes", "-o", "ControlPersist=120", "-o", "ControlPath="+controlPath, "-p", strconv.Itoa(port), user+"@"+host, "true")
	rc, out, errText, err := runSSH(context.Background(), cmd, nil, 60*time.Second)
	if err != nil {
		if err == exec.ErrNotFound {
			return 0, "", &ExecutorUnreachable{"ssh is not installed on this machine"}
		}
		return 0, "", &ExecutorUnreachable{"ssh control master timed out after 60s"}
	}
	return rc, errText + out, nil
}

func pairCloseControlMaster(admin, controlPath string) {
	user, host, port, err := parseSSHAddress(admin)
	if err != nil {
		return
	}
	_, _, _, _ = runSSH(context.Background(), []string{"ssh", "-O", "exit", "-o", "ControlPath=" + controlPath, "-p", strconv.Itoa(port), user + "@" + host}, nil, 30*time.Second)
}

func pairInspectionSummary(insp map[string]string, hands []byte) []string {
	keys := []string{"uname", "arch", "sudo", "user", "hands", "workdir", "workdir_owner", "authorized_keys",
		"authorized_keys_lines", "dropin", "dropin_dir", "remotes", "apt", "copilot", "node", "pulse", "harness", "git", "gitconfig"}
	out := []string{"inspection:"}
	for _, k := range keys {
		value := insp[k]
		if k == "hands" {
			value = pairHandsSummary(insp, hands)
		}
		out = append(out, fmt.Sprintf("  %s=%s", k, value))
	}
	return out
}

func pairRefusalsFromInspection(insp map[string]string, installCopilot bool) []string {
	if insp["uname"] != "Linux" {
		u := insp["uname"]
		if u == "" {
			u = "unknown"
		}
		return []string{fmt.Sprintf("remote uname is %s, not Linux", u)}
	}
	if insp["sudo"] != "yes" {
		return []string{"sudo without a password is required"}
	}
	if insp["authorized_keys"] == "other" {
		return []string{"pulse authorized_keys has other content"}
	}
	if insp["dropin_dir"] == "no" {
		return append([]string{"this sshd does not include sshd_config.d - the manual Match block is printed"}, pairManualDropinLines()...)
	}
	rem := insp["remotes"]
	if rem == "" {
		rem = "0"
	}
	n, err := strconv.Atoi(rem)
	if err != nil {
		return []string{"remote remotes count is not readable: " + insp["remotes"]}
	}
	if n > 0 {
		return []string{fmt.Sprintf("%s clone(s) with git remotes exist under the workdir", insp["remotes"])}
	}
	if installCopilot && insp["apt"] != "yes" {
		return append([]string{"--install-copilot needs apt on the remote machine"}, pairManualCopilotLines()...)
	}
	return nil
}

// goArchFor maps `uname -m` on the machine to GOARCH.
func goArchFor(uname string) string {
	switch uname {
	case "x86_64", "amd64":
		return "amd64"
	case "aarch64", "arm64":
		return "arm64"
	}
	return ""
}

// moduleRoot finds the pulse module (go.mod) from the working directory or the binary's directory.
func moduleRoot() string {
	starts := []string{}
	if wd, err := os.Getwd(); err == nil {
		starts = append(starts, wd)
	}
	if exe, err := os.Executable(); err == nil {
		starts = append(starts, filepath.Dir(exe))
	}
	for _, start := range starts {
		for dir := start; ; dir = filepath.Dir(dir) {
			if data, err := os.ReadFile(filepath.Join(dir, "go.mod")); err == nil && strings.Contains(string(data), "module github.com/manishtiwari25/pulse") {
				return dir
			}
			if filepath.Dir(dir) == dir {
				break
			}
		}
	}
	return ""
}

// handsBinary is the linux `pulse` for the machine's architecture: PULSE_HANDS_BINARY, a prebuilt
// `pulse-linux-<arch>` beside this binary, or a `go build` of the module (GOOS=linux GOARCH=<arch>, CGO off).
var handsBinary = func(arch string) ([]byte, string, error) {
	if p := os.Getenv("PULSE_HANDS_BINARY"); p != "" {
		data, err := os.ReadFile(p)
		return data, "PULSE_HANDS_BINARY=" + p, err
	}
	if exe, err := os.Executable(); err == nil {
		sibling := filepath.Join(filepath.Dir(exe), "pulse-linux-"+arch)
		if data, err := os.ReadFile(sibling); err == nil {
			return data, sibling, nil
		}
		if runtime.GOOS == "linux" && runtime.GOARCH == arch {
			data, err := os.ReadFile(exe)
			return data, exe, err
		}
	}
	root := moduleRoot()
	if root == "" {
		return nil, "", fail(1, "no linux/%s pulse binary to ship: set PULSE_HANDS_BINARY, place pulse-linux-%s beside this binary, or run from the pulse source tree", arch, arch)
	}
	tmp, err := os.CreateTemp("", "pulse-linux-"+arch+"-*")
	if err != nil {
		return nil, "", err
	}
	tmp.Close()
	defer os.Remove(tmp.Name())
	cmd := exec.Command("go", "build", "-trimpath", "-o", tmp.Name(), "./cmd/pulse")
	cmd.Dir = root
	cmd.Env = envWith(os.Environ(), map[string]string{"GOOS": "linux", "GOARCH": arch, "CGO_ENABLED": "0"})
	if out, err := cmd.CombinedOutput(); err != nil {
		return nil, "", fail(1, "go build for linux/%s failed: %s", arch, pairTail(string(out)))
	}
	data, err := os.ReadFile(tmp.Name())
	return data, fmt.Sprintf("go build GOOS=linux GOARCH=%s ./cmd/pulse", arch), err
}

func sha256Bytes(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

type pairOpts struct {
	Workdir        string
	Workers        int
	Projects       string
	InstallCopilot bool
	DryRun         bool
	Actor          string
	HubHome        string
}

type pairReport struct {
	OK      bool
	Lines   []string
	Scripts map[string]string
	Changed []string
}

// pairExecutor is pair_executor(): hub key, inspection before any change, one idempotent apply script carrying
// the linux pulse binary, the sshd drop-in guarded by `sshd -t` and a second admin connection, the ssh alias
// and host key, `executor add`, `probe`, one `executor.pair` event.
func pairExecutor(s *store.Store, admin, name, now string, o pairOpts) (*pairReport, error) {
	name, _, host, port, workdir, err := pairValidate(name, admin, o.Workdir, o.Workers)
	if err != nil {
		return nil, err
	}
	actor := o.Actor
	if actor == "" {
		actor = humanAgent
	}
	decision, err := agentGate(s.DB, "pair: "+name, actor, now)
	if err != nil {
		return nil, err
	}
	existing := executorRow(s, name)
	if existing != nil && existing.str("address") != "pulse@"+name {
		return nil, pairRefuse("executor %s exists with another address", name)
	}
	hubHome := o.HubHome
	if hubHome == "" {
		hubHome = homeDir()
	}
	rep := &pairReport{Lines: []string{"pairing executor " + name}, Scripts: map[string]string{}}
	lines := &rep.Lines
	keyPath, pubkey, keyReady := pairInspectionPubkey(hubHome, o.DryRun, lines)
	config, configText, aliasBlock, err := pairCheckAlias(hubHome, name, host)
	if err != nil {
		return nil, err
	}
	dropin := pairDropinText()
	inspectScript := pairInspectScript(workdir, pubkey, "", sha256Text(dropin))
	rc, out, errText, err := adminSSHRun(admin, "sudo -n sh -s", []byte(inspectScript), 120*time.Second, "")
	if err != nil {
		return nil, pairRefuse("inspection failed: %s", errMessage(err))
	}
	if rc != 0 {
		t := pairTail(errText)
		if t == "" {
			t = pairTail(out)
		}
		if strings.Contains(strings.ToLower(t), "password") {
			return nil, pairRefuse("sudo without a password is required")
		}
		if t == "" {
			t = fmt.Sprintf("exit %d", rc)
		}
		return nil, pairRefuse("inspection failed: %s", t)
	}
	insp, hostkeys := pairParseInspection(out)
	arch := goArchFor(insp["arch"])
	var hands []byte
	handsFrom := ""
	if insp["uname"] == "Linux" && (!o.DryRun || insp["hands"] != "absent") {
		if arch == "" {
			return nil, pairRefuse("remote architecture %s has no pulse build (x86_64 and aarch64 do)", pyRepr(insp["arch"]))
		}
		hands, handsFrom, err = handsBinary(arch)
		if err != nil {
			return nil, err
		}
	}
	*lines = append(*lines, pairInspectionSummary(insp, hands)...)
	if handsFrom != "" {
		*lines = append(*lines, fmt.Sprintf("hands binary: linux/%s, %d bytes, from %s", arch, len(hands), handsFrom))
	}
	if insp["copilot"] == "missing" && !o.InstallCopilot && !s.HarnessNative() {
		*lines = append(*lines, "hint: copilot is missing; the final probe may leave this executor drained until a human installs or logs it in")
	}
	if refusal := pairRefusalsFromInspection(insp, o.InstallCopilot); len(refusal) > 0 {
		*lines = append(*lines, refusal...)
		return nil, fail(2, "%s", strings.Join(*lines, "\n"))
	}
	placeholder := fmt.Sprintf("<pulse linux/%s binary, base64>", arch)
	applyScript := pairApplyScript(name, workdir, pubkey, hands, o.InstallCopilot)
	undoScript, commitScript := pairUndoScript(), pairCommitScript()
	rep.Scripts["inspect"], rep.Scripts["undo"], rep.Scripts["commit"] = inspectScript, undoScript, commitScript
	if o.DryRun {
		shown := strings.Replace(pairApplyScript(name, workdir, pubkey, nil, o.InstallCopilot), "<<'PULSE_HANDS_EOF'\n\nPULSE_HANDS_EOF", "<<'PULSE_HANDS_EOF'\n"+placeholder+"\nPULSE_HANDS_EOF", 1)
		rep.Scripts["apply"] = shown
		*lines = append(*lines, "remote script (would run as root via sudo -n sh -s; the hands heredoc carries the linux pulse binary):",
			strings.TrimRight(shown, "\n"), "undo script (the exact inverse):", strings.TrimRight(undoScript, "\n"),
			"commit script (after successful admin re-check):", strings.TrimRight(commitScript, "\n"), "local changes that would be made:")
		if !fileExists(keyPath) {
			*lines = append(*lines, "  generate "+keyPath)
		}
		if aliasBlock != nil {
			*lines = append(*lines, fmt.Sprintf("  keep existing Host %s block", name))
		} else {
			*lines = append(*lines, strings.TrimRight(pairHostBlock(name, host, port), "\n"))
		}
		for _, hk := range hostkeys {
			*lines = append(*lines, fmt.Sprintf("  known_hosts: %s %s", pairKnownHostPrefix(host, port), hk))
		}
		if existing != nil {
			*lines = append(*lines, fmt.Sprintf("  executor %s already registered as pulse@%s: add skipped", name, name))
		} else {
			*lines = append(*lines, fmt.Sprintf("  executor add %s --ssh pulse@%s --workdir %s --workers %d --projects %s", name, name, workdir, o.Workers, normalizeProjects(o.Projects)))
		}
		*lines = append(*lines, "  probe "+name)
		rep.OK = true
		return rep, nil
	}
	rep.Scripts["apply"] = applyScript
	if !keyReady {
		keyPath, pubkey, err = pairEnsureHubKey(hubHome, lines)
		if err != nil {
			return nil, err
		}
		_ = keyPath
		applyScript = pairApplyScript(name, workdir, pubkey, hands, o.InstallCopilot)
		rep.Scripts["apply"] = applyScript
		rep.Scripts["inspect"] = pairInspectScript(workdir, pubkey, sha256Bytes(hands), sha256Text(dropin))
	}
	sockDir, err := os.MkdirTemp("", "pulse-pair-")
	if err != nil {
		return nil, err
	}
	sock := filepath.Join(sockDir, "cm")
	defer func() {
		pairCloseControlMaster(admin, sock)
		_ = os.RemoveAll(sockDir)
	}()
	steps := map[string]string{}
	mrc, mout, err := pairOpenControlMaster(admin, sock)
	if err != nil || mrc != 0 {
		if err != nil {
			mout = errMessage(err)
		}
		*lines = append(*lines, "admin control master failed: "+pairTail(mout))
		return rep, nil
	}
	rc, out, errText, err = adminSSHRun(admin, "sudo -n sh -s", []byte(applyScript), 600*time.Second, sock)
	if err != nil {
		*lines = append(*lines, "remote apply failed: "+errMessage(err))
		return rep, nil
	}
	steps = pairStepResults(out)
	for _, line := range strings.Split(out, "\n") {
		if strings.HasPrefix(line, "step=") {
			*lines = append(*lines, line)
		}
	}
	firstNonEmpty := func(a, b string) string {
		if t := pairTail(a); t != "" {
			return t
		}
		return pairTail(b)
	}
	if rc == 3 {
		*lines = append(*lines, "sshd -t failed; drop-in was removed and ssh was not reloaded: "+firstNonEmpty(errText, out))
		return rep, nil
	}
	if rc != 0 {
		var done []string
		for k, v := range steps {
			if v == "done" {
				done = append(done, k)
			}
		}
		sort.Strings(done)
		doneText := strings.Join(done, ", ")
		if doneText == "" {
			doneText = "none"
		}
		why := firstNonEmpty(errText, out)
		if why == "" {
			why = fmt.Sprintf("exit %d", rc)
		}
		*lines = append(*lines, fmt.Sprintf("remote apply failed after steps %s: %s", doneText, why))
		return rep, nil
	}
	crc, cout, cerr, err := adminSSHRun(admin, "true", nil, 60*time.Second, "")
	if err != nil {
		crc, cout, cerr = 255, "", errMessage(err)
	}
	if crc != 0 {
		_, _, _, _ = adminSSHRun(admin, "sudo -n sh -s", []byte(undoScript), 120*time.Second, sock)
		*lines = append(*lines, "admin path failed after reload; drop-in removed and sshd reloaded through the held session")
		if t := firstNonEmpty(cerr, cout); t != "" {
			*lines = append(*lines, t)
		}
		return rep, nil
	}
	krc, kout, kerr, err := adminSSHRun(admin, "sudo -n sh -s # pulse-pair-commit", []byte(commitScript), 60*time.Second, sock)
	if err != nil || krc != 0 {
		why := firstNonEmpty(kerr, kout)
		if err != nil {
			why = errMessage(err)
		}
		if why == "" {
			why = fmt.Sprintf("exit %d", krc)
		}
		*lines = append(*lines, "remote commit cleanup failed: "+why)
		return rep, nil
	}
	for _, line := range strings.Split(kout, "\n") {
		if strings.HasPrefix(line, "step=") {
			*lines = append(*lines, line)
		}
	}
	if pairWriteAlias(config, configText, aliasBlock, name, host, port) {
		rep.Changed = append(rep.Changed, "ssh-config")
		*lines = append(*lines, fmt.Sprintf("wrote Host %s alias for the executor", name))
	}
	if added := pairWriteKnownHosts(hubHome, host, port, hostkeys); added > 0 {
		rep.Changed = append(rep.Changed, "known-hosts")
		*lines = append(*lines, fmt.Sprintf("trusted %d host key line(s) from the admin session", added))
	}
	if existing == nil {
		if _, err := addExecutor(s, now, name, "pulse@"+name, workdir, o.Workers, o.Projects, actor); err != nil {
			return nil, err
		}
		rep.Changed = append(rep.Changed, "executor")
		*lines = append(*lines, fmt.Sprintf("added executor %s with alias-only address", name))
	} else {
		*lines = append(*lines, fmt.Sprintf("kept existing executor %s", name))
	}
	setExecutorAdmin(s, name, admin)
	probeOK, note, err := probeExecutor(s, name, actor, now)
	if err != nil {
		return nil, err
	}
	if probeOK {
		*lines = append(*lines, fmt.Sprintf("%s: ok, active: %s", name, note))
	} else {
		*lines = append(*lines, fmt.Sprintf("%s: paired; probe drained it: %s", name, note))
	}
	eventAt(s.DB, now, actor, "executor.pair", name, map[string]any{"decision": decision, "steps": steps, "probe_ok": probeOK,
		"workdir": workdir, "install_copilot": o.InstallCopilot, "hands": handsFrom})
	rep.OK = true
	return rep, nil
}
