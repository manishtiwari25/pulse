package runner

// Multipass compute provider (ADR-020 D-D, ADR-034): an Ubuntu VM on the hub machine itself through
// Canonical's `multipass` CLI - no account, no cost, the way to try the agents' hands on a laptop or a
// single server. The VM boots with the shared cloud-init so it has the admin user with the hub key; the host
// key is read back with `multipass exec`, the channel multipass vouches for, so the first ssh connection is
// checked instead of trusted. Multipass has no tags: a machine is PULSE's when its name starts with pulse-,
// which is what machineName gives every hand.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

type multipassProvider struct{}

func init() { RegisterProvider(multipassProvider{}) }

func (multipassProvider) Name() string { return "multipass" }
func (multipassProvider) Kind() string { return "Multipass instance" }

const (
	multipassSize  = "cpus=2,mem=4G,disk=32G"
	multipassImage = "24.04"
	multipassShort = 60 * time.Second
	multipassLong  = 20 * time.Minute
)

// runMultipass runs the multipass CLI; tests swap it for a fake `multipass`.
var runMultipass = cliCommand("multipass", "multipass is not installed on this machine (https://canonical.com/multipass)")

type multipassConfig struct {
	CPUs, Memory, Disk, Image string
}

// multipassSettings reads the size - cpus=2,mem=4G,disk=32G, any subset, keys cpus, mem/memory, disk - and
// the image.
func multipassSettings(s *store.Store, size string) (multipassConfig, error) {
	cfg := multipassConfig{Image: strings.TrimSpace(s.Setting("compute.multipass.image", multipassImage))}
	if cfg.Image == "" {
		cfg.Image = multipassImage
	}
	size = strings.TrimSpace(size)
	if size == "" {
		size = strings.TrimSpace(s.Setting("compute.multipass.size", multipassSize))
	}
	if size == "" {
		size = multipassSize
	}
	if err := cfg.parseSize(size); err != nil {
		return cfg, err
	}
	return cfg, nil
}

func (c *multipassConfig) parseSize(size string) error {
	for _, part := range strings.Split(size, ",") {
		part = strings.TrimSpace(part)
		if part == "" {
			continue
		}
		key, value, ok := strings.Cut(part, "=")
		key, value = strings.ToLower(strings.TrimSpace(key)), strings.TrimSpace(value)
		if !ok || value == "" {
			return fmt.Errorf("multipass size %s: want key=value pairs like %s", pyRepr(size), multipassSize)
		}
		switch key {
		case "cpus", "cpu":
			if n, err := strconv.Atoi(value); err != nil || n < 1 {
				return fmt.Errorf("multipass size: cpus=%s is not a count", value)
			}
			c.CPUs = value
		case "mem", "memory":
			if !multipassBytes(value) {
				return fmt.Errorf("multipass size: mem=%s is not a size like 4G", value)
			}
			c.Memory = value
		case "disk":
			if !multipassBytes(value) {
				return fmt.Errorf("multipass size: disk=%s is not a size like 32G", value)
			}
			c.Disk = value
		default:
			return fmt.Errorf("multipass size: unknown key %s (cpus, mem, disk)", pyRepr(key))
		}
	}
	return nil
}

// multipassBytes accepts what multipass does: a number with an optional K, M, G (or KiB, MiB, GiB) unit.
func multipassBytes(v string) bool {
	i := 0
	for i < len(v) && (v[i] >= '0' && v[i] <= '9' || v[i] == '.') {
		i++
	}
	if i == 0 {
		return false
	}
	if _, err := strconv.ParseFloat(v[:i], 64); err != nil {
		return false
	}
	switch strings.ToUpper(v[i:]) {
	case "", "B", "K", "M", "G", "KB", "MB", "GB", "KIB", "MIB", "GIB":
		return true
	}
	return false
}

func multipassErr(what string, rc int, stdout, stderr string, err error) error {
	return cliErr(what, rc, stdout, stderr, err)
}

type multipassInfo struct {
	State string   `json:"state"`
	IPv4  []string `json:"ipv4"`
	Image string   `json:"image_release"`
}

func (i multipassInfo) ip() string {
	if len(i.IPv4) > 0 {
		return i.IPv4[0]
	}
	return ""
}

// multipassDescribe answers `multipass info NAME`, or nil when the instance does not exist.
func multipassDescribe(ctx context.Context, name string) (*multipassInfo, error) {
	rc, out, errText, err := runMultipass(ctx, []string{"info", name, "--format", "json"}, multipassShort)
	if err != nil || rc != 0 {
		if err == nil && notFoundAnswer(out, errText) {
			return nil, nil
		}
		return nil, multipassErr("multipass info "+name, rc, out, errText, err)
	}
	var res struct {
		Info map[string]multipassInfo `json:"info"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &res); err != nil {
		return nil, fmt.Errorf("multipass info %s: unreadable answer: %s", name, squeeze(strings.TrimSpace(out), 200))
	}
	i, ok := res.Info[name]
	if !ok {
		return nil, nil
	}
	return &i, nil
}

// Provision: `multipass launch` with the cloud-init, the address from `multipass info`, the host key over
// `multipass exec`, then the shared finish.
func (p multipassProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error) {
	s, hubHome, log, err := provisionStart("multipass", req)
	if err != nil {
		return nil, err
	}
	cfg, err := multipassSettings(s, req.Size)
	if err != nil {
		return nil, err
	}
	rc, out, errText, err := runMultipass(ctx, []string{"version"}, multipassShort)
	if err != nil || rc != 0 {
		return nil, multipassErr("multipass is not answering (is multipassd running?)", rc, out, errText, err)
	}
	log("multipass: %s", squeeze(strings.TrimSpace(strings.ReplaceAll(out, "\n", "; ")), 120))
	var lines []string
	_, pubkey, err := pairEnsureHubKey(hubHome, &lines)
	if err != nil {
		return nil, err
	}
	name := machineName(req.ID)
	m := &Machine{Provider: "multipass", Name: name, Executor: req.Executor, Tags: machineTags(req),
		Detail: map[string]string{"image": cfg.Image, "cpus": cfg.CPUs, "memory": cfg.Memory, "disk": cfg.Disk}}
	info, err := multipassDescribe(ctx, name)
	if err != nil {
		return nil, err
	}
	if info != nil {
		log("instance %s exists; not recreated", name)
		if info.State != "Running" {
			if rc, out, errText, err := runMultipass(ctx, []string{"start", name}, multipassLong); err != nil || rc != 0 {
				return m, multipassErr("multipass start "+name, rc, out, errText, err)
			}
		}
	} else {
		rendered, err := writeCloudInit(pubkey)
		if err != nil {
			return nil, err
		}
		defer os.Remove(rendered)
		args := []string{"launch", cfg.Image, "--name", name, "--cloud-init", rendered, "--timeout", "1200"}
		if cfg.CPUs != "" {
			args = append(args, "--cpus", cfg.CPUs)
		}
		if cfg.Memory != "" {
			args = append(args, "--memory", cfg.Memory)
		}
		if cfg.Disk != "" {
			args = append(args, "--disk", cfg.Disk)
		}
		log("multipass launch %s (%s; cpus=%s mem=%s disk=%s)", name, cfg.Image, orDash(cfg.CPUs), orDash(cfg.Memory), orDash(cfg.Disk))
		if rc, out, errText, err := runMultipass(ctx, args, multipassLong); err != nil || rc != 0 {
			return m, multipassErr("multipass launch "+name, rc, out, errText, err)
		}
	}
	ip := ""
	if err := pollUntil(ctx, "the address of "+name, func() (bool, error) {
		got, err := multipassDescribe(ctx, name)
		if err != nil {
			return false, err
		}
		if got == nil {
			return false, fmt.Errorf("instance %s vanished while booting", name)
		}
		ip = got.ip()
		switch got.State {
		case "Starting", "Running", "Restarting", "Unknown":
			return ip != "" && got.State == "Running", nil
		}
		return false, fmt.Errorf("instance %s is %s", name, got.State)
	}); err != nil {
		return m, err
	}
	m.Detail["public_ip"] = ip
	var hostkeys []string
	if err := pollUntil(ctx, "the host key of "+name, func() (bool, error) {
		rc, out, errText, err := runMultipass(ctx, []string{"exec", name, "--", "cat", "/etc/ssh/ssh_host_ed25519_key.pub"}, multipassShort)
		if err != nil {
			return false, multipassErr("multipass exec "+name, rc, out, errText, err)
		}
		if rc != 0 {
			return false, nil
		}
		hostkeys = hostKeyLines(out)
		return len(hostkeys) > 0, nil
	}); err != nil {
		return m, err
	}
	return m, provisionFinish(ctx, req, m, hubHome, ip, hostkeys, log)
}

// Release deletes and purges the instance.
func (p multipassProvider) Release(ctx context.Context, m *Machine) error {
	if m == nil || m.Name == "" {
		return nil
	}
	rc, out, errText, err := runMultipass(ctx, []string{"delete", "--purge", m.Name}, multipassLong)
	if err != nil || rc != 0 {
		if err == nil && notFoundAnswer(out, errText) {
			return nil
		}
		return multipassErr("multipass delete --purge "+m.Name, rc, out, errText, err)
	}
	return nil
}

// Probe: the instance still runs.
func (p multipassProvider) Probe(ctx context.Context, m *Machine) error {
	if m == nil || m.Name == "" {
		return errors.New("multipass: no machine")
	}
	info, err := multipassDescribe(ctx, m.Name)
	if err != nil {
		return err
	}
	if info == nil {
		return fmt.Errorf("%s is gone", m.Name)
	}
	if info.State != "Running" {
		return fmt.Errorf("%s is %s", m.Name, pyRepr(info.State))
	}
	return nil
}

// Find: the pulse- instance named like the executor or answering at host.
func (p multipassProvider) Find(ctx context.Context, _ *store.Store, executor, host string) (*Machine, error) {
	rc, out, errText, err := runMultipass(ctx, []string{"list", "--format", "json"}, multipassShort)
	if err != nil || rc != 0 {
		return nil, fmt.Errorf("multipass could not answer: %s", errMessage(multipassErr("multipass list", rc, out, errText, err)))
	}
	var res struct {
		List []struct {
			Name  string   `json:"name"`
			State string   `json:"state"`
			IPv4  []string `json:"ipv4"`
		} `json:"list"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &res); err != nil {
		return nil, fmt.Errorf("multipass could not answer: unreadable list: %s", squeeze(strings.TrimSpace(out), 200))
	}
	for _, i := range res.List {
		if !strings.HasPrefix(i.Name, "pulse-") {
			continue
		}
		ip := ""
		if len(i.IPv4) > 0 {
			ip = i.IPv4[0]
		}
		if i.Name == executor || (host != "" && ip == host) {
			return &Machine{Provider: "multipass", Name: i.Name, Tags: map[string]string{tagHands: "hands"}, Detail: map[string]string{"public_ip": ip}}, nil
		}
	}
	return nil, nil
}

// Stop stops the instance; its disk stays.
func (p multipassProvider) Stop(ctx context.Context, m *Machine) error {
	rc, out, errText, err := runMultipass(ctx, []string{"stop", m.Name}, multipassLong)
	if err != nil || rc != 0 {
		return multipassErr("multipass stop "+m.Name, rc, out, errText, err)
	}
	return nil
}

func (p multipassProvider) Place(*Machine) string       { return "this machine" }
func (p multipassProvider) StartHint(m *Machine) string { return "multipass start " + m.Name }

// Delete deletes and purges the instance; found is false when it is already gone.
func (p multipassProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error) {
	if m.Name == "" {
		return false, nil, nil
	}
	info, err := multipassDescribe(ctx, m.Name)
	if err != nil {
		return false, nil, err
	}
	if info == nil {
		return false, nil, nil
	}
	rc, out, errText, err := runMultipass(ctx, []string{"delete", "--purge", m.Name}, multipassLong)
	if err != nil || rc != 0 {
		return true, nil, multipassErr("multipass delete --purge "+m.Name, rc, out, errText, err)
	}
	return true, []string{"instance " + m.Name + " with its disk"}, nil
}
