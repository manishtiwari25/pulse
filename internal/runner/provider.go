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

// What every compute provider shares (D-D, ADR-020, ADR-034). A provider is a thin port of the provider's own
// CLI: the login is the CLI's login on the hub machine (az login, aws configure or aws sso login, gcloud auth
// login; multipass needs none), every argument is an identifier and never a credential, every error is
// redacted before it reaches the board. The machine is always one Ubuntu host prepared by cloudInitConfig,
// reached over the admin user adminUser with the hub key, whose host key arrived over a channel the provider
// vouches for (run-command, the serial console, a local exec) and never over ssh's first connection - and it
// ends as an executor exactly the way `pulse pair` does (provisionFinish).

const (
	adminUser       = "pulseadmin"     // the admin login on every machine PULSE makes
	handsWorkdir    = "/srv/pulse"     // the work directory the forced command roots every path in
	handsDiskGB     = "64"             // the OS disk of a provisioned hand
	hostkeyMarker   = "pulse-hostkey " // cloud-init prints the host key behind this on the serial console
	hostkeyBlockTop = "-----BEGIN SSH HOST KEY KEYS-----"
	hostkeyBlockEnd = "-----END SSH HOST KEY KEYS-----"
	tagHands        = "pulse" // every machine PULSE made carries pulse=hands (ADR-034)
	tagRequest      = "pulse-request"
	tagProject      = "pulse-project"
)

// cliRunner runs one provider CLI; tests swap a provider's runner for a fake.
// provisionWait is the pause between attempts while a machine boots and cloud-init runs; tests shorten it.
var provisionWait = 20 * time.Second

type cliRunner func(ctx context.Context, args []string, timeout time.Duration) (rc int, stdout, stderr string, err error)

// cliCommand is the cliRunner for program: a non-zero exit is reported through rc (err stays nil), a missing
// program as the missing message, and a timeout by name so the finding says which call hung.
func cliCommand(program, missing string, env ...string) cliRunner {
	return func(ctx context.Context, args []string, timeout time.Duration) (int, string, string, error) {
		if ctx == nil {
			ctx = context.Background()
		}
		cctx, cancel := context.WithTimeout(ctx, timeout)
		defer cancel()
		cmd := exec.CommandContext(cctx, program, args...)
		var out, errb bytes.Buffer
		cmd.Stdout, cmd.Stderr = &out, &errb
		cmd.Env = append(environ(), env...)
		err := cmd.Run()
		if err != nil {
			var ee *exec.ExitError
			if errors.As(err, &ee) && cctx.Err() == nil {
				return ee.ExitCode(), out.String(), errb.String(), nil
			}
			if errors.Is(err, exec.ErrNotFound) {
				return -1, "", "", errors.New(missing)
			}
			if cctx.Err() != nil {
				return -1, out.String(), errb.String(), fmt.Errorf("%s %s timed out after %s", program, strings.Join(args[:min(2, len(args))], " "), timeout)
			}
			return -1, out.String(), errb.String(), err
		}
		return 0, out.String(), errb.String(), nil
	}
}

// cliErr turns a failed CLI call into one redacted line.
func cliErr(what string, rc int, stdout, stderr string, err error) error {
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

// notFoundAnswer says whether a CLI's failure means the resource is gone rather than the call broke.
func notFoundAnswer(stdout, stderr string) bool {
	low := strings.ToLower(stderr + stdout)
	for _, w := range []string{"notfound", "not found", "could not be found", "does not exist", "was not found", "no such", "instance does not exist"} {
		if strings.Contains(low, w) {
			return true
		}
	}
	return false
}

// cloudInitConfig is the cloud-config every provisioned hand boots with: git and the harness prerequisites
// (Node.js 22 and the copilot CLI), the work directory, and its host key printed on the serial console behind
// hostkeyMarker so a provider without a run-command channel (aws, gcp) still learns it without trusting the
// first ssh connection. adminKey, when given, creates the admin user with that key for the providers whose CLI
// does not place one itself (aws, gcp, multipass); Azure passes the key to `az vm create` and leaves it empty.
// The hands user, its forced-command key, the linux pulse binary and the sshd drop-in are placed by the
// pairing step, so nothing secret is rendered here.
func cloudInitConfig(adminKey string) string {
	var b strings.Builder
	b.WriteString(`#cloud-config
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
`)
	if key := strings.TrimSpace(adminKey); key != "" {
		b.WriteString("  - name: " + adminUser + "\n")
		b.WriteString("    shell: /bin/bash\n")
		b.WriteString("    sudo: ALL=(ALL) NOPASSWD:ALL\n")
		b.WriteString("    lock_passwd: true\n")
		b.WriteString("    ssh_authorized_keys:\n")
		b.WriteString("      - " + key + "\n")
	}
	b.WriteString(`runcmd:
  # The host key on the serial console: the channel the provider vouches for (aws, gcp read it back).
  - sh -c 'echo "` + hostkeyMarker + `$(cat /etc/ssh/ssh_host_ed25519_key.pub)" > /dev/console'
  # Node.js 22 and the copilot CLI, for every user (the harness prerequisites).
  - mkdir -p /etc/apt/keyrings
  - curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg
  - echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main" > /etc/apt/sources.list.d/nodesource.list
  - apt-get update
  - apt-get install -y nodejs
  - npm install -g @github/copilot
  # The work directory the forced command roots every path in; pairing sets the owner.
  - mkdir -p ` + handsWorkdir + `
final_message: "pulse compute machine ready after $UPTIME seconds"
`)
	return b.String()
}

// writeCloudInit renders the cloud-config into a temp file the CLI reads; the caller removes it.
func writeCloudInit(adminKey string) (string, error) {
	f, err := os.CreateTemp("", "pulse-cloud-init-*.yaml")
	if err != nil {
		return "", err
	}
	if _, err := f.WriteString(cloudInitConfig(adminKey)); err != nil {
		f.Close()
		os.Remove(f.Name())
		return "", err
	}
	f.Close()
	return f.Name(), nil
}

// hostKeyLines picks the ed25519 host key lines out of text: whole `ssh-ed25519 ...` lines (a run-command or a
// local exec answer), the marker line cloud-init prints on the serial console, and the block cloud-init's
// keys_to_console prints on its own. Order kept, duplicates dropped.
func hostKeyLines(text string) []string {
	var out []string
	seen := map[string]bool{}
	add := func(line string) {
		f := strings.Fields(line)
		if len(f) >= 2 && f[0] == "ssh-ed25519" {
			key := f[0] + " " + f[1]
			if !seen[key] {
				seen[key] = true
				out = append(out, key)
			}
		}
	}
	inBlock := false
	for _, line := range strings.Split(strings.ReplaceAll(text, "\r", ""), "\n") {
		line = strings.TrimSpace(line)
		switch {
		case strings.Contains(line, hostkeyMarker):
			add(line[strings.Index(line, hostkeyMarker)+len(hostkeyMarker):])
		case line == hostkeyBlockTop:
			inBlock = true
		case line == hostkeyBlockEnd:
			inBlock = false
		case inBlock || strings.HasPrefix(line, "ssh-ed25519 "):
			add(line)
		}
	}
	return out
}

// provisionStart is the checking every Provision begins with: a store, a valid executor name, the hub home
// and a logger; it returns the pieces so a provider reads like a script.
func provisionStart(provider string, req ProvisionRequest) (*store.Store, string, func(string, ...any), error) {
	if req.Store == nil {
		return nil, "", nil, fmt.Errorf("%s: a store is needed", provider)
	}
	if req.Executor == "" || !validName(req.Executor) {
		return nil, "", nil, fmt.Errorf("%s: executor name %s is not lowercase words with dashes", provider, pyRepr(req.Executor))
	}
	log := req.Log
	if log == nil {
		log = func(string, ...any) {}
	}
	hubHome := req.HubHome
	if hubHome == "" {
		hubHome = homeDir()
	}
	return req.Store, hubHome, log, nil
}

// machineTags are the tags every machine PULSE makes carries (ADR-034: pulse=hands names it ours).
func machineTags(req ProvisionRequest) map[string]string {
	return map[string]string{tagHands: "hands", tagRequest: req.ID, tagProject: req.Project}
}

// machineName is the provider-side name of a request's machine, e.g. pulse-cr-007.
func machineName(requestID string) string { return slugify("pulse-"+strings.ToLower(requestID), 40) }

// writeAdminAlias writes the ssh alias the admin session uses so the hub key opens the admin user: Host
// <alias>, HostName <ip>, User pulseadmin, IdentityFile ~/.ssh/pulse_hands_ed25519.
func writeAdminAlias(hubHome, alias, host string) error {
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
	block := strings.Join([]string{"Host " + alias, "    HostName " + host, "    Port 22", "    User " + adminUser,
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

// waitCloudInit polls the admin session until cloud-init is done (git present); ctx bounds the wait.
func waitCloudInit(ctx context.Context, admin string, log func(string, ...any)) error {
	probe := "cloud-init status --wait >/dev/null 2>&1; command -v git >/dev/null 2>&1 && echo pulse-ready"
	for try := 1; ; try++ {
		rc, out, _, err := adminSSHRun(admin, probe, nil, 15*time.Minute, "")
		if err == nil && rc == 0 && strings.Contains(out, "pulse-ready") {
			return nil
		}
		if err != nil {
			log("ssh attempt %d: %s", try, errMessage(err))
		}
		if !sleepCtx(ctx, provisionWait) {
			return fmt.Errorf("gave up waiting for %s: %s", admin, ctx.Err())
		}
	}
}

// pairHands is the pairing step (pair.go); tests swap it for a registration without ssh.
var pairHands = pairExecutor

// provisionFinish is the half of Provision every provider shares once its machine answers at ip and its host
// key arrived over the provider's channel: known_hosts, the admin alias <name>-admin, the wait for cloud-init,
// the pairing (workdir, one worker, the requesting project, copilot installed), the admin route the daemon
// heals over later, and the login when a source exists (ADR-034 rule 6: the machine is born logged in;
// without a source the caps gate should have held the request). m.Address becomes pulseadmin@ip.
func provisionFinish(ctx context.Context, req ProvisionRequest, m *Machine, hubHome, ip string, hostkeys []string, log func(string, ...any)) error {
	s := req.Store
	if ip == "" {
		return fmt.Errorf("%s has no public address", m.Name)
	}
	if len(hostkeys) == 0 {
		return fmt.Errorf("%s gave no ssh host key", m.Name)
	}
	m.Address = adminUser + "@" + ip
	if m.Detail == nil {
		m.Detail = map[string]string{}
	}
	m.Detail["public_ip"] = ip
	pairWriteKnownHosts(hubHome, ip, 22, hostkeys[:1])
	alias := m.Name + "-admin"
	if err := writeAdminAlias(hubHome, alias, ip); err != nil {
		return err
	}
	admin := adminUser + "@" + alias
	log("%s is up at %s; waiting for cloud-init", m.Name, ip)
	if err := waitCloudInit(ctx, admin, log); err != nil {
		return err
	}
	rep, err := pairHands(s, admin, req.Executor, s.Now(), pairOpts{Workdir: handsWorkdir, Workers: 1, Projects: req.Project,
		InstallCopilot: true, Actor: humanAgent, HubHome: hubHome})
	if err != nil {
		return fmt.Errorf("pairing %s: %s", req.Executor, redactSecrets(errMessage(err)))
	}
	for _, l := range rep.Lines {
		log("pair: %s", l)
	}
	if !rep.OK {
		last := ""
		if n := len(rep.Lines); n > 0 {
			last = rep.Lines[n-1]
		}
		return fmt.Errorf("pairing %s did not complete: %s", req.Executor, redactSecrets(last))
	}
	setExecutorAdmin(s, req.Executor, m.Address)
	if tok, origin, err := loginSource(s); err != nil {
		log("login source: %s", errMessage(err))
	} else if len(tok) > 0 {
		res, err := placeLogin(s, s.Now(), req.Executor, m.Address, tokenReader(tok), false, false, req.Actor)
		if err != nil {
			return fmt.Errorf("placing the %s on %s: %s", origin, req.Executor, redactSecrets(errMessage(err)))
		}
		for _, l := range res.Lines {
			log("login: %s", redactSecrets(l))
		}
		if !res.OK {
			return fmt.Errorf("the %s was refused on %s; the machine cannot work", origin, req.Executor)
		}
	} else {
		log("no login source: %s starts without a copilot login", req.Executor)
	}
	return nil
}

func sleepCtx(ctx context.Context, d time.Duration) bool {
	select {
	case <-ctx.Done():
		return false
	case <-time.After(d):
		return true
	}
}

// pollUntil calls step every provisionWait until it returns done, an error, or ctx ends; what names the wait
// in the error. It is the loop every provider's "wait for the address / the host key" is.
func pollUntil(ctx context.Context, what string, step func() (bool, error)) error {
	for try := 0; ; try++ {
		if try > 0 && !sleepCtx(ctx, provisionWait) {
			return fmt.Errorf("gave up waiting for %s: %s", what, ctx.Err())
		}
		done, err := step()
		if err != nil {
			return err
		}
		if done {
			return nil
		}
	}
}

// cliList splits one CLI answer of whitespace-separated words.
func cliList(out string) []string { return strings.Fields(out) }

// machineFromDetail rebuilds a Machine from a compute_machines or retired_vms row's provider, name and detail
// JSON; a legacy retired_vms row without detail gets its resource group back from the column.
func machineFromDetail(provider, name, detailJSON, resourceGroup string) *Machine {
	m := &Machine{Provider: provider, Name: name, Tags: map[string]string{}, Detail: map[string]string{}}
	var detail struct {
		Tags   map[string]string `json:"tags"`
		Detail map[string]string `json:"detail"`
	}
	if err := json.Unmarshal([]byte(detailJSON), &detail); err == nil {
		if detail.Tags != nil {
			m.Tags = detail.Tags
		}
		if detail.Detail != nil {
			m.Detail = detail.Detail
		}
	}
	if m.Detail["resource_group"] == "" && resourceGroup != "" {
		m.Detail["resource_group"] = resourceGroup
	}
	return m
}
