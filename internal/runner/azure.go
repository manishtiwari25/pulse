package runner

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// The azure provider ports legacy/hosted/azure-provision.sh and cloud-init.yaml (ADR-020): one Ubuntu VM in the
// resource group compute.azure.resource_group, created with `az vm create` under the login `az login` left on
// this machine (never a key in the store), prepared by cloud-init (git and the harness prerequisites), then
// registered as an executor exactly the way `pulse pair` does - the linux pulse binary travels over the admin
// ssh session, the hands user gets the forced command. The VM carries the tags pulse=hands (every machine PULSE
// made, ADR-034) and pulse-request=<request id>. Release is
// `az vm delete --yes` (disk and nic go with it through the delete options) and the leftovers the CLI creates
// beside a VM (public ip, vnet, nsg), best effort.

const (
	azureAdminUser = "pulseadmin"
	azureWorkdir   = "/srv/pulse"
	azureDiskGB    = "64"
	azureImage     = "Ubuntu2404"
	azureVMSize    = "Standard_B2ms"
	azureSSHWait   = 20 * time.Second // between attempts while the VM boots and cloud-init runs
)

// azureCloudInit is cloud-init.yaml without its two placeholders: the hands user, its key, the pulse binary
// and the sshd drop-in are placed by the pairing step, so nothing secret or machine-specific is rendered here.
const azureCloudInit = `#cloud-config
# PULSE compute machine (agents' hands) - Ubuntu 24.04 (ADR-020, D-D). Prepared for ` + "`pulse pair`" + `, which
# adds the pulse user, its forced-command key, the linux pulse binary and the sshd drop-in over the admin session.
package_update: true
package_upgrade: true
packages:
  - git
  - python3
  - ca-certificates
  - curl
  - gnupg
  - build-essential
users:
  - default
runcmd:
  # Node.js 22 and the copilot CLI, for every user (the harness prerequisites).
  - mkdir -p /etc/apt/keyrings
  - curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg
  - echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main" > /etc/apt/sources.list.d/nodesource.list
  - apt-get update
  - apt-get install -y nodejs
  - npm install -g @github/copilot
  # The work directory the forced command roots every path in; pairing sets the owner.
  - mkdir -p ` + azureWorkdir + `
final_message: "pulse compute machine ready after $UPTIME seconds"
`

type azureProvider struct{}

func init() { RegisterProvider(azureProvider{}) }

func (azureProvider) Name() string { return "azure" }

// azRunner runs the Azure CLI; tests swap runAz for a fake `az`.
type azRunner func(ctx context.Context, args []string, timeout time.Duration) (rc int, stdout, stderr string, err error)

var runAz azRunner = func(ctx context.Context, args []string, timeout time.Duration) (int, string, string, error) {
	if ctx == nil {
		ctx = context.Background()
	}
	cctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	cmd := exec.CommandContext(cctx, "az", args...)
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	cmd.Env = append(environ(), "AZURE_CORE_NO_COLOR=true", "AZURE_CORE_ONLY_SHOW_ERRORS=true")
	err := cmd.Run()
	if err != nil {
		var ee *exec.ExitError
		if errors.As(err, &ee) && cctx.Err() == nil {
			return ee.ExitCode(), out.String(), errb.String(), nil
		}
		if errors.Is(err, exec.ErrNotFound) {
			return -1, "", "", errors.New("the Azure CLI (az) is not installed on this machine")
		}
		if cctx.Err() != nil {
			return -1, out.String(), errb.String(), fmt.Errorf("az %s timed out after %s", strings.Join(args[:min(2, len(args))], " "), timeout)
		}
		return -1, out.String(), errb.String(), err
	}
	return 0, out.String(), errb.String(), nil
}

// pairHands is the pairing step (pair.go); tests swap it for a registration without ssh.
var pairHands = pairExecutor

type azureConfig struct {
	Subscription, ResourceGroup, Location, VMSize, Image string
}

func azureSettings(s *store.Store, size string) (azureConfig, error) {
	cfg := azureConfig{
		Subscription:  strings.TrimSpace(s.Setting("compute.azure.subscription", "")),
		ResourceGroup: strings.TrimSpace(s.Setting("compute.azure.resource_group", "")),
		Location:      strings.TrimSpace(s.Setting("compute.azure.location", "")),
		VMSize:        strings.TrimSpace(size),
		Image:         strings.TrimSpace(s.Setting("compute.azure.image", azureImage)),
	}
	if cfg.VMSize == "" {
		cfg.VMSize = strings.TrimSpace(s.Setting("compute.azure.vm_size", azureVMSize))
	}
	if cfg.Image == "" {
		cfg.Image = azureImage
	}
	var missing []string
	for _, kv := range [][2]string{{"compute.azure.resource_group", cfg.ResourceGroup}, {"compute.azure.location", cfg.Location}} {
		if kv[1] == "" {
			missing = append(missing, kv[0])
		}
	}
	if len(missing) > 0 {
		return cfg, fmt.Errorf("azure is not configured: set %s (pulse settings set <name> <value>)", strings.Join(missing, " and "))
	}
	return cfg, nil
}

// args prefixes the subscription when one is set; every call asks for a machine-readable answer.
func (c azureConfig) args(a ...string) []string {
	out := append([]string{}, a...)
	if c.Subscription != "" {
		out = append(out, "--subscription", c.Subscription)
	}
	return out
}

func azureVMName(requestID string) string { return slugify("pulse-"+strings.ToLower(requestID), 40) }

// azErr turns a failed az call into one redacted line.
func azErr(what string, rc int, stdout, stderr string, err error) error {
	if err != nil {
		return fmt.Errorf("%s: %s", what, redactSecrets(errMessage(err)))
	}
	text := strings.TrimSpace(stderr)
	if text == "" {
		text = strings.TrimSpace(stdout)
	}
	if text == "" {
		text = fmt.Sprintf("exit %d", rc)
	}
	return fmt.Errorf("%s: %s", what, redactSecrets(squeeze(text, 400)))
}

// azureAdminAlias writes the ssh alias the admin session uses so the hub key opens the admin user: Host
// <alias>, HostName <ip>, User pulseadmin, IdentityFile ~/.ssh/pulse_hands_ed25519.
func azureAdminAlias(hubHome, alias, host string) error {
	config := filepath.Join(hubHome, ".ssh", "config")
	text := ""
	if data, err := os.ReadFile(config); err == nil {
		text = string(data)
	}
	if b := pairAliasState(text, alias); b != nil {
		if b.hostname != host {
			return fmt.Errorf("ssh alias %s already points at %s, not %s", alias, b.hostname, host)
		}
		return nil
	}
	_ = os.MkdirAll(filepath.Dir(config), 0o700)
	prefix := ""
	if text != "" && !strings.HasSuffix(text, "\n") {
		prefix = "\n"
	}
	block := strings.Join([]string{"Host " + alias, "    HostName " + host, "    Port 22", "    User " + azureAdminUser,
		"    IdentityFile ~/.ssh/pulse_hands_ed25519", "    IdentitiesOnly yes", ""}, "\n")
	f, err := os.OpenFile(config, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	defer f.Close()
	if text != "" {
		prefix += "\n"
	}
	_, err = f.WriteString(prefix + block)
	return err
}

// Provision is azure-provision.sh: login check, resource group, `az vm create` with cloud-init and the hub
// key as the admin key, the host key through run-command into known_hosts, wait for cloud-init, then pair.
func (p azureProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error) {
	s := req.Store
	if s == nil {
		return nil, errors.New("azure: a store is needed")
	}
	log := req.Log
	if log == nil {
		log = func(string, ...any) {}
	}
	cfg, err := azureSettings(s, req.Size)
	if err != nil {
		return nil, err
	}
	if req.Executor == "" || !validName(req.Executor) {
		return nil, fmt.Errorf("azure: executor name %s is not lowercase words with dashes", pyRepr(req.Executor))
	}
	hubHome := req.HubHome
	if hubHome == "" {
		hubHome = homeDir()
	}
	rc, out, errText, err := runAz(ctx, cfg.args("account", "show", "--query", "name", "-o", "tsv"), 60*time.Second)
	if err != nil || rc != 0 {
		return nil, azErr("not logged in to Azure (run `az login` on this machine as the daemon's user)", rc, out, errText, err)
	}
	log("azure subscription: %s", strings.TrimSpace(out))
	var lines []string
	_, _, err = pairEnsureHubKey(hubHome, &lines)
	if err != nil {
		return nil, err
	}
	pubPath := filepath.Join(hubHome, ".ssh", "pulse_hands_ed25519.pub")
	vm := azureVMName(req.ID)
	tags := map[string]string{"pulse": "hands", "pulse-request": req.ID, "pulse-project": req.Project}
	m := &Machine{Provider: "azure", Name: vm, Executor: req.Executor, Tags: tags,
		Detail: map[string]string{"resource_group": cfg.ResourceGroup, "location": cfg.Location, "size": cfg.VMSize, "image": cfg.Image, "subscription": cfg.Subscription}}
	if rc, _, _, err := runAz(ctx, cfg.args("group", "show", "--name", cfg.ResourceGroup, "-o", "none"), 60*time.Second); err != nil || rc != 0 {
		rc, out, errText, err := runAz(ctx, cfg.args("group", "create", "--name", cfg.ResourceGroup, "--location", cfg.Location, "--tags", "pulse=hands", "-o", "none"), 120*time.Second)
		if err != nil || rc != 0 {
			return nil, azErr("az group create "+cfg.ResourceGroup, rc, out, errText, err)
		}
		log("created resource group %s in %s", cfg.ResourceGroup, cfg.Location)
	}
	rendered, err := os.CreateTemp("", "pulse-cloud-init-*.yaml")
	if err != nil {
		return nil, err
	}
	defer os.Remove(rendered.Name())
	if _, err := rendered.WriteString(azureCloudInit); err != nil {
		rendered.Close()
		return nil, err
	}
	rendered.Close()
	if rc, _, _, err := runAz(ctx, cfg.args("vm", "show", "--resource-group", cfg.ResourceGroup, "--name", vm, "-o", "none"), 60*time.Second); err != nil || rc != 0 {
		log("az vm create %s (%s, %s, %s GiB)", vm, cfg.VMSize, cfg.Image, azureDiskGB)
		rc, out, errText, err := runAz(ctx, cfg.args("vm", "create", "--resource-group", cfg.ResourceGroup, "--name", vm, "--location", cfg.Location,
			"--image", cfg.Image, "--size", cfg.VMSize, "--admin-username", azureAdminUser, "--ssh-key-values", pubPath,
			"--custom-data", rendered.Name(), "--public-ip-sku", "Standard", "--os-disk-size-gb", azureDiskGB, "--storage-sku", "StandardSSD_LRS",
			"--os-disk-delete-option", "Delete", "--nic-delete-option", "Delete",
			"--tags", "pulse=hands", "pulse-request="+req.ID, "pulse-project="+req.Project, "-o", "json"), 20*time.Minute)
		if err != nil || rc != 0 {
			return m, azErr("az vm create "+vm, rc, out, errText, err)
		}
		var created struct {
			PublicIP string `json:"publicIpAddress"`
		}
		if json.Unmarshal([]byte(out), &created) == nil && created.PublicIP != "" {
			m.Address = azureAdminUser + "@" + created.PublicIP
		}
	} else {
		log("vm %s exists; not recreated", vm)
	}
	ip := strings.TrimPrefix(m.Address, azureAdminUser+"@")
	for try := 0; ip == "" && try < 6; try++ {
		if try > 0 {
			if !sleepCtx(ctx, azureSSHWait) {
				return m, ctx.Err()
			}
		}
		rc, out, errText, err := runAz(ctx, cfg.args("vm", "show", "--resource-group", cfg.ResourceGroup, "--name", vm, "-d", "--query", "publicIps", "-o", "tsv"), 60*time.Second)
		if err != nil || rc != 0 {
			return m, azErr("az vm show "+vm, rc, out, errText, err)
		}
		ip = strings.TrimSpace(strings.Split(out, ",")[0])
	}
	if ip == "" {
		return m, fmt.Errorf("vm %s has no public address", vm)
	}
	m.Address = azureAdminUser + "@" + ip
	m.Detail["public_ip"] = ip
	rc, out, errText, err = runAz(ctx, cfg.args("vm", "run-command", "invoke", "--resource-group", cfg.ResourceGroup, "--name", vm, "--command-id", "RunShellScript",
		"--scripts", "cat /etc/ssh/ssh_host_ed25519_key.pub", "--query", "value[0].message", "-o", "tsv"), 10*time.Minute)
	if err != nil || rc != 0 {
		return m, azErr("az vm run-command invoke (host key) "+vm, rc, out, errText, err)
	}
	var hostkeys []string
	for _, line := range strings.Split(out, "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "ssh-ed25519 ") {
			hostkeys = append(hostkeys, line)
		}
	}
	if len(hostkeys) == 0 {
		return m, fmt.Errorf("vm %s gave no ssh host key", vm)
	}
	pairWriteKnownHosts(hubHome, ip, 22, hostkeys[:1])
	alias := vm + "-admin"
	if err := azureAdminAlias(hubHome, alias, ip); err != nil {
		return m, err
	}
	admin := azureAdminUser + "@" + alias
	log("vm %s is up at %s; waiting for cloud-init", vm, ip)
	if err := azureWaitReady(ctx, admin, log); err != nil {
		return m, err
	}
	rep, err := pairHands(s, admin, req.Executor, s.Now(), pairOpts{Workdir: azureWorkdir, Workers: 1, Projects: req.Project,
		InstallCopilot: true, Actor: humanAgent, HubHome: hubHome})
	if err != nil {
		return m, fmt.Errorf("pairing %s: %s", req.Executor, redactSecrets(errMessage(err)))
	}
	for _, l := range rep.Lines {
		log("pair: %s", l)
	}
	if !rep.OK {
		last := ""
		if n := len(rep.Lines); n > 0 {
			last = rep.Lines[n-1]
		}
		return m, fmt.Errorf("pairing %s did not complete: %s", req.Executor, redactSecrets(last))
	}
	// ADR-034 rule 6: the machine is born logged in when a login source exists; the admin route is remembered
	// so the daemon can heal it later. Without a source the caps gate should have held the request.
	setExecutorAdmin(s, req.Executor, m.Address)
	if tok, origin, err := loginSource(s); err != nil {
		log("login source: %s", errMessage(err))
	} else if len(tok) > 0 {
		res, err := placeLogin(s, s.Now(), req.Executor, m.Address, tokenReader(tok), false, false, req.Actor)
		if err != nil {
			return m, fmt.Errorf("placing the %s on %s: %s", origin, req.Executor, redactSecrets(errMessage(err)))
		}
		for _, l := range res.Lines {
			log("login: %s", redactSecrets(l))
		}
		if !res.OK {
			return m, fmt.Errorf("the %s was refused on %s; the machine cannot work", origin, req.Executor)
		}
	} else {
		log("no login source: %s starts without a copilot login", req.Executor)
	}
	return m, nil
}

// azureWaitReady polls the admin session until cloud-init is done (git present); ctx bounds the wait.
func azureWaitReady(ctx context.Context, admin string, log func(string, ...any)) error {
	probe := "cloud-init status --wait >/dev/null 2>&1; command -v git >/dev/null 2>&1 && echo pulse-ready"
	for try := 1; ; try++ {
		rc, out, _, err := adminSSHRun(admin, probe, nil, 15*time.Minute, "")
		if err == nil && rc == 0 && strings.Contains(out, "pulse-ready") {
			return nil
		}
		if err != nil {
			log("ssh attempt %d: %s", try, errMessage(err))
		}
		if !sleepCtx(ctx, azureSSHWait) {
			return fmt.Errorf("gave up waiting for %s: %s", admin, ctx.Err())
		}
	}
}

func sleepCtx(ctx context.Context, d time.Duration) bool {
	select {
	case <-ctx.Done():
		return false
	case <-time.After(d):
		return true
	}
}

func (p azureProvider) config(m *Machine) azureConfig {
	return azureConfig{Subscription: m.Detail["subscription"], ResourceGroup: m.Detail["resource_group"], Location: m.Detail["location"]}
}

// Release is the --delete half of azure-provision.sh: the VM (disk and nic go with it), then the public ip,
// vnet and nsg the CLI created beside it, best effort.
func (p azureProvider) Release(ctx context.Context, m *Machine) error {
	if m == nil || m.Name == "" {
		return nil
	}
	cfg := p.config(m)
	if cfg.ResourceGroup == "" {
		return errors.New("azure: the machine's resource group is unknown")
	}
	if rc, _, _, err := runAz(ctx, cfg.args("vm", "show", "--resource-group", cfg.ResourceGroup, "--name", m.Name, "-o", "none"), 60*time.Second); err == nil && rc == 0 {
		rc, out, errText, err := runAz(ctx, cfg.args("vm", "delete", "--resource-group", cfg.ResourceGroup, "--name", m.Name, "--yes", "-o", "none"), 15*time.Minute)
		if err != nil || rc != 0 {
			return azErr("az vm delete "+m.Name, rc, out, errText, err)
		}
	} else if err != nil {
		return azErr("az vm show "+m.Name, rc, "", "", err)
	}
	for _, res := range [][2]string{{"public-ip", m.Name + "PublicIP"}, {"vnet", m.Name + "VNET"}, {"nsg", m.Name + "NSG"}} {
		if rc, _, _, err := runAz(ctx, cfg.args("network", res[0], "show", "--resource-group", cfg.ResourceGroup, "--name", res[1], "-o", "none"), 60*time.Second); err == nil && rc == 0 {
			_, _, _, _ = runAz(ctx, cfg.args("network", res[0], "delete", "--resource-group", cfg.ResourceGroup, "--name", res[1], "-o", "none"), 5*time.Minute)
		}
	}
	return nil
}

// Probe: the VM still runs.
func (p azureProvider) Probe(ctx context.Context, m *Machine) error {
	if m == nil || m.Name == "" {
		return errors.New("azure: no machine")
	}
	cfg := p.config(m)
	rc, out, errText, err := runAz(ctx, cfg.args("vm", "show", "--resource-group", cfg.ResourceGroup, "--name", m.Name, "-d", "--query", "powerState", "-o", "tsv"), 60*time.Second)
	if err != nil || rc != 0 {
		return azErr("az vm show "+m.Name, rc, out, errText, err)
	}
	if state := strings.TrimSpace(out); state != "VM running" {
		return fmt.Errorf("%s is %s", m.Name, pyRepr(state))
	}
	return nil
}
