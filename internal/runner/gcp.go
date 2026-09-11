package runner

// GCP compute provider (ADR-020 D-D, ADR-034): a Compute Engine instance made through the gcloud CLI the
// daemon's user logged in with (`gcloud auth login`), never a stored key. The machine is Ubuntu 24.04 from
// the ubuntu-os-cloud image family (or a full image path), booted with the shared cloud-init as user-data
// metadata so it creates the admin user with the hub key and prints the host key on the serial console;
// PULSE reads it back with `gcloud compute instances get-serial-port-output`, the channel that lets the
// first ssh connection be checked instead of trusted. One firewall rule, pulse-hands-ssh, opens port 22 to
// instances carrying the pulse-hands network tag; instances carry the pulse=hands label every provider uses
// so retire and delete recognise them.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

type gcpProvider struct{}

func init() { RegisterProvider(gcpProvider{}) }

func (gcpProvider) Name() string { return "gcp" }
func (gcpProvider) Kind() string { return "Compute Engine instance" }

const (
	gcpMachineType  = "e2-medium"
	gcpImage        = "ubuntu-2404-lts-amd64"
	gcpImageProject = "ubuntu-os-cloud"
	gcpFirewall     = "pulse-hands-ssh" // the rule that opens port 22 to the pulse-hands network tag
	gcpNetworkTag   = "pulse-hands"
	gcpShort        = 60 * time.Second
	gcpLong         = 10 * time.Minute
)

// runGcloud runs the Google Cloud CLI; tests swap it for a fake `gcloud`.
var runGcloud = cliCommand("gcloud", "the Google Cloud CLI (gcloud) is not installed on this machine", "CLOUDSDK_CORE_DISABLE_PROMPTS=1", "CLOUDSDK_CORE_DISABLE_COLOR=true")

type gcpConfig struct {
	Project, Zone, MachineType, Image, Network string
}

func gcpSettings(s *store.Store, size string) gcpConfig {
	cfg := gcpConfig{
		Project:     strings.TrimSpace(s.Setting("compute.gcp.project", "")),
		Zone:        strings.TrimSpace(s.Setting("compute.gcp.zone", "")),
		MachineType: strings.TrimSpace(size),
		Image:       strings.TrimSpace(s.Setting("compute.gcp.image", gcpImage)),
		Network:     strings.TrimSpace(s.Setting("compute.gcp.network", "")),
	}
	if cfg.MachineType == "" {
		cfg.MachineType = strings.TrimSpace(s.Setting("compute.gcp.machine_type", gcpMachineType))
	}
	if cfg.MachineType == "" {
		cfg.MachineType = gcpMachineType
	}
	if cfg.Image == "" {
		cfg.Image = gcpImage
	}
	if cfg.Network == "" {
		cfg.Network = "default"
	}
	return cfg
}

// args is `gcloud <a...>` in the project when one is set, answering JSON.
func (c gcpConfig) args(a ...string) []string {
	return append(c.scope(a), "--format", "json")
}

func (c gcpConfig) scope(a []string) []string {
	out := append([]string{}, a...)
	if c.Project != "" {
		out = append(out, "--project", c.Project)
	}
	return out
}

// imageArgs picks the image: a family in ubuntu-os-cloud by default, a full path (projects/.../images/...)
// or a "project/image" pair as given.
func (c gcpConfig) imageArgs() []string {
	if strings.Contains(c.Image, "/") {
		return []string{"--image", c.Image}
	}
	return []string{"--image-family", c.Image, "--image-project", gcpImageProject}
}

func gcpErr(what string, rc int, stdout, stderr string, err error) error {
	return cliErr(what, rc, stdout, stderr, err)
}

// gcpJSON runs one call that must answer JSON into v.
func gcpJSON(ctx context.Context, what string, args []string, timeout time.Duration, v any) error {
	rc, out, errText, err := runGcloud(ctx, args, timeout)
	if err != nil || rc != 0 {
		return gcpErr(what, rc, out, errText, err)
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), v); err != nil {
		return fmt.Errorf("%s: unreadable answer: %s", what, squeeze(strings.TrimSpace(out), 200))
	}
	return nil
}

type gcpInstance struct {
	Name              string            `json:"name"`
	Status            string            `json:"status"`
	Zone              string            `json:"zone"`
	MachineType       string            `json:"machineType"`
	Labels            map[string]string `json:"labels"`
	NetworkInterfaces []struct {
		AccessConfigs []struct {
			NatIP string `json:"natIP"`
		} `json:"accessConfigs"`
	} `json:"networkInterfaces"`
}

func (i gcpInstance) publicIP() string {
	for _, n := range i.NetworkInterfaces {
		for _, a := range n.AccessConfigs {
			if a.NatIP != "" {
				return a.NatIP
			}
		}
	}
	return ""
}

// zoneName is the zone's short name out of the URL gcloud answers (.../zones/europe-west1-b).
func (i gcpInstance) zoneName() string {
	return i.Zone[strings.LastIndex(i.Zone, "/")+1:]
}

// gcpLabel is a value GCP accepts as a label: lowercase letters, digits, dash and underscore, 63 at most.
func gcpLabel(v string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(v) {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9', r == '-', r == '_':
			b.WriteRune(r)
		default:
			b.WriteRune('-')
		}
	}
	out := b.String()
	if len(out) > 63 {
		out = out[:63]
	}
	return out
}

// gcpMachine is the Machine an instance PULSE made maps to.
func gcpMachine(cfg gcpConfig, i gcpInstance) *Machine {
	m := &Machine{Provider: "gcp", Name: i.Name, Tags: map[string]string{tagHands: "hands"},
		Detail: map[string]string{"project": cfg.Project, "zone": i.zoneName(), "type": i.MachineType[strings.LastIndex(i.MachineType, "/")+1:], "public_ip": i.publicIP()}}
	for _, k := range []string{tagRequest, tagProject} {
		if v := i.Labels[k]; v != "" {
			m.Tags[k] = v
		}
	}
	return m
}

// gcpDescribe answers the instance, or nil when it does not exist.
func gcpDescribe(ctx context.Context, cfg gcpConfig, name string) (*gcpInstance, error) {
	rc, out, errText, err := runGcloud(ctx, cfg.args("compute", "instances", "describe", name, "--zone", cfg.Zone), gcpShort)
	if err != nil || rc != 0 {
		if err == nil && notFoundAnswer(out, errText) {
			return nil, nil
		}
		return nil, gcpErr("gcloud compute instances describe "+name, rc, out, errText, err)
	}
	var i gcpInstance
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &i); err != nil {
		return nil, fmt.Errorf("gcloud compute instances describe %s: unreadable answer: %s", name, squeeze(strings.TrimSpace(out), 200))
	}
	return &i, nil
}

// gcpFirewallRule makes sure pulse-hands-ssh exists in the network: port 22 in, to the pulse-hands tag.
func gcpFirewallRule(ctx context.Context, cfg gcpConfig, log func(string, ...any)) error {
	rc, out, errText, err := runGcloud(ctx, cfg.args("compute", "firewall-rules", "describe", gcpFirewall), gcpShort)
	if err == nil && rc == 0 {
		return nil
	}
	if err != nil || !notFoundAnswer(out, errText) {
		return gcpErr("gcloud compute firewall-rules describe "+gcpFirewall, rc, out, errText, err)
	}
	rc, out, errText, err = runGcloud(ctx, cfg.args("compute", "firewall-rules", "create", gcpFirewall, "--network", cfg.Network,
		"--direction", "INGRESS", "--allow", "tcp:22", "--source-ranges", "0.0.0.0/0", "--target-tags", gcpNetworkTag,
		"--description", "PULSE compute machines: ssh in"), 2*time.Minute)
	if err != nil || rc != 0 {
		return gcpErr("gcloud compute firewall-rules create "+gcpFirewall, rc, out, errText, err)
	}
	log("created firewall rule %s in network %s with port 22 open to tag %s", gcpFirewall, cfg.Network, gcpNetworkTag)
	return nil
}

// Provision: login check, project, firewall rule, `gcloud compute instances create` with the cloud-init as
// user-data, the address, the host key from the serial console, then the shared finish.
func (p gcpProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error) {
	s, hubHome, log, err := provisionStart("gcp", req)
	if err != nil {
		return nil, err
	}
	cfg := gcpSettings(s, req.Size)
	rc, out, errText, err := runGcloud(ctx, []string{"auth", "list", "--filter=status:ACTIVE", "--format=value(account)"}, gcpShort)
	if err != nil || rc != 0 || strings.TrimSpace(out) == "" {
		if err == nil && rc == 0 {
			errText = "no active account"
		}
		return nil, gcpErr("not logged in to Google Cloud (run `gcloud auth login` on this machine as the daemon's user)", rc, out, errText, err)
	}
	account := strings.TrimSpace(out)
	if cfg.Project == "" {
		rc, out, _, err := runGcloud(ctx, []string{"config", "get-value", "project"}, gcpShort)
		if err == nil && rc == 0 {
			cfg.Project = strings.TrimSpace(out)
		}
	}
	var missing []string
	if cfg.Project == "" {
		missing = append(missing, "compute.gcp.project")
	}
	if cfg.Zone == "" {
		missing = append(missing, "compute.gcp.zone")
	}
	if len(missing) > 0 {
		return nil, fmt.Errorf("gcp is not configured: set %s (pulse settings set <name> <value>)", strings.Join(missing, " and "))
	}
	log("gcp project %s as %s in %s", cfg.Project, account, cfg.Zone)
	var lines []string
	_, pubkey, err := pairEnsureHubKey(hubHome, &lines)
	if err != nil {
		return nil, err
	}
	name := machineName(req.ID)
	m := &Machine{Provider: "gcp", Name: name, Executor: req.Executor, Tags: machineTags(req),
		Detail: map[string]string{"project": cfg.Project, "zone": cfg.Zone, "type": cfg.MachineType, "image": cfg.Image, "network": cfg.Network}}
	inst, err := gcpDescribe(ctx, cfg, name)
	if err != nil {
		return nil, err
	}
	if inst != nil {
		log("instance %s exists; not recreated", name)
	} else {
		if err := gcpFirewallRule(ctx, cfg, log); err != nil {
			return nil, err
		}
		rendered, err := writeCloudInit(pubkey, !req.Store.HarnessNative())
		if err != nil {
			return nil, err
		}
		defer os.Remove(rendered)
		log("gcloud compute instances create %s (%s, %s, %s GiB)", name, cfg.MachineType, cfg.Image, handsDiskGB)
		labels := fmt.Sprintf("%s=hands,%s=%s,%s=%s", tagHands, tagRequest, gcpLabel(req.ID), tagProject, gcpLabel(req.Project))
		args := append([]string{"compute", "instances", "create", name, "--zone", cfg.Zone, "--machine-type", cfg.MachineType}, cfg.imageArgs()...)
		args = append(args, "--boot-disk-size", handsDiskGB+"GB", "--boot-disk-type", "pd-balanced", "--boot-disk-auto-delete",
			"--network", cfg.Network, "--labels", labels, "--tags", gcpNetworkTag, "--metadata-from-file", "user-data="+rendered)
		var created []gcpInstance
		if err := gcpJSON(ctx, "gcloud compute instances create "+name, cfg.args(args...), gcpLong, &created); err != nil {
			return m, err
		}
		if len(created) == 0 {
			return m, fmt.Errorf("gcloud compute instances create %s made no instance", name)
		}
		inst = &created[0]
	}
	ip := inst.publicIP()
	if err := pollUntil(ctx, "the address of "+name, func() (bool, error) {
		if ip != "" && inst.Status == "RUNNING" {
			return true, nil
		}
		got, err := gcpDescribe(ctx, cfg, name)
		if err != nil {
			return false, err
		}
		if got == nil {
			return false, fmt.Errorf("instance %s vanished while booting", name)
		}
		inst, ip = got, got.publicIP()
		switch inst.Status {
		case "PROVISIONING", "STAGING", "RUNNING":
			return ip != "" && inst.Status == "RUNNING", nil
		}
		return false, fmt.Errorf("instance %s is %s", name, inst.Status)
	}); err != nil {
		return m, err
	}
	m.Detail["public_ip"] = ip
	var hostkeys []string
	if err := pollUntil(ctx, "the host key of "+name+" on its serial console", func() (bool, error) {
		rc, out, errText, err := runGcloud(ctx, cfg.scope([]string{"compute", "instances", "get-serial-port-output", name, "--zone", cfg.Zone}), gcpShort)
		if err != nil || rc != 0 {
			return false, gcpErr("gcloud compute instances get-serial-port-output "+name, rc, out, errText, err)
		}
		hostkeys = hostKeyLines(out)
		return len(hostkeys) > 0, nil
	}); err != nil {
		return m, err
	}
	return m, provisionFinish(ctx, req, m, hubHome, ip, hostkeys, log)
}

func (p gcpProvider) config(m *Machine) gcpConfig {
	return gcpConfig{Project: m.Detail["project"], Zone: m.Detail["zone"]}
}

// Release deletes the instance; its boot disk goes with it (auto-delete). The firewall rule stays: it is
// shared by every hand in the network.
func (p gcpProvider) Release(ctx context.Context, m *Machine) error {
	if m == nil || m.Name == "" {
		return nil
	}
	cfg := p.config(m)
	if cfg.Zone == "" {
		return errors.New("gcp: the machine's zone is unknown")
	}
	rc, out, errText, err := runGcloud(ctx, cfg.scope([]string{"compute", "instances", "delete", m.Name, "--zone", cfg.Zone, "--quiet"}), gcpLong)
	if err != nil || rc != 0 {
		if err == nil && notFoundAnswer(out, errText) {
			return nil
		}
		return gcpErr("gcloud compute instances delete "+m.Name, rc, out, errText, err)
	}
	return nil
}

// Probe: the instance still runs.
func (p gcpProvider) Probe(ctx context.Context, m *Machine) error {
	if m == nil || m.Name == "" {
		return errors.New("gcp: no machine")
	}
	inst, err := gcpDescribe(ctx, p.config(m), m.Name)
	if err != nil {
		return err
	}
	if inst == nil {
		return fmt.Errorf("%s is gone", m.Name)
	}
	if inst.Status != "RUNNING" {
		return fmt.Errorf("%s is %s", m.Name, pyRepr(inst.Status))
	}
	return nil
}

// Find: the instance labelled pulse=hands whose name is the executor's or whose address is host, across the
// project the compute.gcp.* settings name (or gcloud's current one).
func (p gcpProvider) Find(ctx context.Context, s *store.Store, executor, host string) (*Machine, error) {
	cfg := gcpSettings(s, "")
	var got []gcpInstance
	if err := gcpJSON(ctx, "gcloud compute instances list", cfg.args("compute", "instances", "list", "--filter=labels."+tagHands+"=hands"), gcpShort, &got); err != nil {
		return nil, fmt.Errorf("the Google Cloud CLI could not answer: %s", errMessage(err))
	}
	for _, i := range got {
		if i.Name == executor || (host != "" && i.publicIP() == host) {
			return gcpMachine(cfg, i), nil
		}
	}
	return nil, nil
}

// Stop stops the instance: compute stops costing, its disk stays.
func (p gcpProvider) Stop(ctx context.Context, m *Machine) error {
	cfg := p.config(m)
	rc, out, errText, err := runGcloud(ctx, cfg.scope([]string{"compute", "instances", "stop", m.Name, "--zone", cfg.Zone, "--quiet"}), gcpLong)
	if err != nil || rc != 0 {
		return gcpErr("gcloud compute instances stop "+m.Name, rc, out, errText, err)
	}
	return nil
}

func (p gcpProvider) Place(m *Machine) string { return m.Detail["zone"] }

func (p gcpProvider) StartHint(m *Machine) string {
	hint := "gcloud compute instances start " + m.Name + " --zone " + m.Detail["zone"]
	if m.Detail["project"] != "" {
		hint += " --project " + m.Detail["project"]
	}
	return hint
}

// Delete deletes the instance; found is false when it is already gone. The boot disk is deleted with it.
func (p gcpProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error) {
	cfg := p.config(m)
	if m.Name == "" || cfg.Zone == "" {
		return false, nil, nil
	}
	inst, err := gcpDescribe(ctx, cfg, m.Name)
	if err != nil {
		return false, nil, err
	}
	if inst == nil {
		return false, nil, nil
	}
	rc, out, errText, err := runGcloud(ctx, cfg.scope([]string{"compute", "instances", "delete", m.Name, "--zone", cfg.Zone, "--quiet"}), gcpLong)
	if err != nil || rc != 0 {
		return true, nil, gcpErr("gcloud compute instances delete "+m.Name, rc, out, errText, err)
	}
	return true, []string{"instance " + m.Name + " with its boot disk"}, nil
}
