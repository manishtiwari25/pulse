package runner

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
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
// made, ADR-034) and pulse-request=<request id>. Release is `az vm delete --yes` (disk and nic go with it
// through the delete options) and the leftovers the CLI creates beside a VM (public ip, vnet, nsg), best
// effort. The retire phase (ADR-034) finds a tagged VM with `az vm list`, stops it with `az vm deallocate` and
// deletes it with the VM's NICs, public IPs and OS disk; VNets and NSGs stay, they may be shared.

const (
	azureAdminUser = adminUser
	azureWorkdir   = handsWorkdir
	azureDiskGB    = handsDiskGB
	azureImage     = "Ubuntu2404"
	azureVMSize    = "Standard_B2ms"
	azShort        = 60 * time.Second
	azLong         = 15 * time.Minute
)

type azureProvider struct{}

func init() { RegisterProvider(azureProvider{}) }

func (azureProvider) Name() string { return "azure" }
func (azureProvider) Kind() string { return "Azure VM" }

// runAz runs the Azure CLI; tests swap it for a fake `az`.
var runAz = cliCommand("az", "the Azure CLI (az) is not installed on this machine", "AZURE_CORE_NO_COLOR=true", "AZURE_CORE_ONLY_SHOW_ERRORS=true")

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

func azureVMName(requestID string) string { return machineName(requestID) }

// azErr turns a failed az call into one redacted line.
func azErr(what string, rc int, stdout, stderr string, err error) error {
	return cliErr(what, rc, stdout, stderr, err)
}

// Provision is azure-provision.sh: login check, resource group, `az vm create` with cloud-init and the hub
// key as the admin key, the host key through run-command into known_hosts, wait for cloud-init, then pair.
func (p azureProvider) Provision(ctx context.Context, req ProvisionRequest) (*Machine, error) {
	s, hubHome, log, err := provisionStart("azure", req)
	if err != nil {
		return nil, err
	}
	cfg, err := azureSettings(s, req.Size)
	if err != nil {
		return nil, err
	}
	rc, out, errText, err := runAz(ctx, cfg.args("account", "show", "--query", "name", "-o", "tsv"), azShort)
	if err != nil || rc != 0 {
		return nil, azErr("not logged in to Azure (run `az login` on this machine as the daemon's user)", rc, out, errText, err)
	}
	log("azure subscription: %s", strings.TrimSpace(out))
	var lines []string
	if _, _, err = pairEnsureHubKey(hubHome, &lines); err != nil {
		return nil, err
	}
	pubPath := filepath.Join(hubHome, ".ssh", "pulse_hands_ed25519.pub")
	vm := azureVMName(req.ID)
	m := &Machine{Provider: "azure", Name: vm, Executor: req.Executor, Tags: machineTags(req),
		Detail: map[string]string{"resource_group": cfg.ResourceGroup, "location": cfg.Location, "size": cfg.VMSize, "image": cfg.Image, "subscription": cfg.Subscription}}
	if rc, _, _, err := runAz(ctx, cfg.args("group", "show", "--name", cfg.ResourceGroup, "-o", "none"), azShort); err != nil || rc != 0 {
		rc, out, errText, err := runAz(ctx, cfg.args("group", "create", "--name", cfg.ResourceGroup, "--location", cfg.Location, "--tags", tagHands+"=hands", "-o", "none"), 120*time.Second)
		if err != nil || rc != 0 {
			return nil, azErr("az group create "+cfg.ResourceGroup, rc, out, errText, err)
		}
		log("created resource group %s in %s", cfg.ResourceGroup, cfg.Location)
	}
	rendered, err := writeCloudInit("")
	if err != nil {
		return nil, err
	}
	defer os.Remove(rendered)
	if rc, _, _, err := runAz(ctx, cfg.args("vm", "show", "--resource-group", cfg.ResourceGroup, "--name", vm, "-o", "none"), azShort); err != nil || rc != 0 {
		log("az vm create %s (%s, %s, %s GiB)", vm, cfg.VMSize, cfg.Image, azureDiskGB)
		rc, out, errText, err := runAz(ctx, cfg.args("vm", "create", "--resource-group", cfg.ResourceGroup, "--name", vm, "--location", cfg.Location,
			"--image", cfg.Image, "--size", cfg.VMSize, "--admin-username", azureAdminUser, "--ssh-key-values", pubPath,
			"--custom-data", rendered, "--public-ip-sku", "Standard", "--os-disk-size-gb", azureDiskGB, "--storage-sku", "StandardSSD_LRS",
			"--os-disk-delete-option", "Delete", "--nic-delete-option", "Delete",
			"--tags", tagHands+"=hands", tagRequest+"="+req.ID, tagProject+"="+req.Project, "-o", "json"), 20*time.Minute)
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
			if !sleepCtx(ctx, provisionWait) {
				return m, ctx.Err()
			}
		}
		rc, out, errText, err := runAz(ctx, cfg.args("vm", "show", "--resource-group", cfg.ResourceGroup, "--name", vm, "-d", "--query", "publicIps", "-o", "tsv"), azShort)
		if err != nil || rc != 0 {
			return m, azErr("az vm show "+vm, rc, out, errText, err)
		}
		ip = strings.TrimSpace(strings.Split(out, ",")[0])
	}
	if ip == "" {
		return m, fmt.Errorf("vm %s has no public address", vm)
	}
	rc, out, errText, err = runAz(ctx, cfg.args("vm", "run-command", "invoke", "--resource-group", cfg.ResourceGroup, "--name", vm, "--command-id", "RunShellScript",
		"--scripts", "cat /etc/ssh/ssh_host_ed25519_key.pub", "--query", "value[0].message", "-o", "tsv"), 10*time.Minute)
	if err != nil || rc != 0 {
		return m, azErr("az vm run-command invoke (host key) "+vm, rc, out, errText, err)
	}
	hostkeys := hostKeyLines(out)
	if len(hostkeys) == 0 {
		return m, fmt.Errorf("vm %s gave no ssh host key", vm)
	}
	return m, provisionFinish(ctx, req, m, hubHome, ip, hostkeys, log)
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
	if rc, _, _, err := runAz(ctx, cfg.args("vm", "show", "--resource-group", cfg.ResourceGroup, "--name", m.Name, "-o", "none"), azShort); err == nil && rc == 0 {
		rc, out, errText, err := runAz(ctx, cfg.args("vm", "delete", "--resource-group", cfg.ResourceGroup, "--name", m.Name, "--yes", "-o", "none"), azLong)
		if err != nil || rc != 0 {
			return azErr("az vm delete "+m.Name, rc, out, errText, err)
		}
	} else if err != nil {
		return azErr("az vm show "+m.Name, rc, "", "", err)
	}
	for _, res := range [][2]string{{"public-ip", m.Name + "PublicIP"}, {"vnet", m.Name + "VNET"}, {"nsg", m.Name + "NSG"}} {
		if rc, _, _, err := runAz(ctx, cfg.args("network", res[0], "show", "--resource-group", cfg.ResourceGroup, "--name", res[1], "-o", "none"), azShort); err == nil && rc == 0 {
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
	rc, out, errText, err := runAz(ctx, cfg.args("vm", "show", "--resource-group", cfg.ResourceGroup, "--name", m.Name, "-d", "--query", "powerState", "-o", "tsv"), azShort)
	if err != nil || rc != 0 {
		return azErr("az vm show "+m.Name, rc, out, errText, err)
	}
	if state := strings.TrimSpace(out); state != "VM running" {
		return fmt.Errorf("%s is %s", m.Name, pyRepr(state))
	}
	return nil
}

// Find: the VM tagged pulse=hands whose name is the executor's or whose public address is host, across the
// subscription `az login` left here.
func (p azureProvider) Find(ctx context.Context, _ *store.Store, executor, host string) (*Machine, error) {
	query := fmt.Sprintf("[?tags.pulse=='hands' && (name=='%s' || publicIps=='%s')].{name:name,rg:resourceGroup,ip:publicIps}", executor, host)
	rc, out, errText, err := runAz(ctx, []string{"vm", "list", "-d", "--query", query, "-o", "json"}, azShort)
	if err != nil || rc != 0 {
		return nil, fmt.Errorf("the Azure CLI could not answer: %s", errMessage(azErr("az vm list", rc, out, errText, err)))
	}
	var found []struct {
		Name string `json:"name"`
		RG   string `json:"rg"`
		IP   string `json:"ip"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &found); err != nil || len(found) == 0 {
		return nil, nil
	}
	return &Machine{Provider: "azure", Name: found[0].Name, Tags: map[string]string{tagHands: "hands"},
		Detail: map[string]string{"resource_group": found[0].RG, "public_ip": found[0].IP}}, nil
}

// Stop deallocates the VM: it stops costing money, its disk stays.
func (p azureProvider) Stop(ctx context.Context, m *Machine) error {
	rc, out, errText, err := runAz(ctx, []string{"vm", "deallocate", "-g", m.Detail["resource_group"], "-n", m.Name, "-o", "none"}, azLong)
	if err != nil || rc != 0 {
		return azErr("az vm deallocate "+m.Name, rc, out, errText, err)
	}
	return nil
}

func (p azureProvider) Place(m *Machine) string { return m.Detail["resource_group"] }
func (p azureProvider) StartHint(m *Machine) string {
	return "az vm start -g " + m.Detail["resource_group"] + " -n " + m.Name
}

// Delete: `az vm show` for the OS disk, NICs and their public IPs, `az vm delete --yes`, then the NICs, public
// IPs and disk (best effort: `az vm create --nic-delete-option Delete` may have taken them already). found is
// false when the VM is not there; gone names what was deleted.
func (p azureProvider) Delete(ctx context.Context, m *Machine) (bool, []string, error) {
	rg, name := m.Detail["resource_group"], m.Name
	rc, out, errText, err := runAz(ctx, []string{"vm", "show", "-g", rg, "-n", name, "-o", "json"}, azShort)
	if err != nil {
		return false, nil, azErr("az vm show "+name, rc, out, errText, err)
	}
	if rc != 0 {
		if notFoundAnswer(out, errText) {
			return false, nil, nil
		}
		return false, nil, azErr("az vm show "+name, rc, out, errText, nil)
	}
	var vm struct {
		StorageProfile struct {
			OSDisk struct {
				ManagedDisk struct {
					ID string `json:"id"`
				} `json:"managedDisk"`
			} `json:"osDisk"`
		} `json:"storageProfile"`
		NetworkProfile struct {
			NetworkInterfaces []struct {
				ID string `json:"id"`
			} `json:"networkInterfaces"`
		} `json:"networkProfile"`
	}
	if err := json.Unmarshal([]byte(out), &vm); err != nil {
		return false, nil, fmt.Errorf("az vm show %s: unreadable answer", name)
	}
	var nics, ips []string
	for _, n := range vm.NetworkProfile.NetworkInterfaces {
		if n.ID == "" {
			continue
		}
		nics = append(nics, n.ID)
		rc, out, _, err := runAz(ctx, []string{"network", "nic", "show", "--ids", n.ID, "-o", "json"}, azShort)
		if err != nil || rc != 0 {
			continue
		}
		var nic struct {
			IPConfigurations []struct {
				PublicIPAddress *struct {
					ID string `json:"id"`
				} `json:"publicIPAddress"`
			} `json:"ipConfigurations"`
		}
		if json.Unmarshal([]byte(out), &nic) == nil {
			for _, c := range nic.IPConfigurations {
				if c.PublicIPAddress != nil && c.PublicIPAddress.ID != "" {
					ips = append(ips, c.PublicIPAddress.ID)
				}
			}
		}
	}
	rc, out, errText, err = runAz(ctx, []string{"vm", "delete", "-g", rg, "-n", name, "--yes", "-o", "none"}, azLong)
	if err != nil || rc != 0 {
		return true, nil, azErr("az vm delete "+name, rc, out, errText, err)
	}
	gone := []string{"vm " + name}
	if len(nics) > 0 {
		_, _, _, _ = runAz(ctx, append([]string{"network", "nic", "delete", "--ids"}, append(nics, "-o", "none")...), azLong)
		gone = append(gone, fmt.Sprintf("%d nic(s)", len(nics)))
	}
	if len(ips) > 0 {
		_, _, _, _ = runAz(ctx, append([]string{"network", "public-ip", "delete", "--ids"}, append(ips, "-o", "none")...), azLong)
		gone = append(gone, fmt.Sprintf("%d public ip(s)", len(ips)))
	}
	if disk := vm.StorageProfile.OSDisk.ManagedDisk.ID; disk != "" {
		_, _, _, _ = runAz(ctx, []string{"disk", "delete", "--ids", disk, "--yes", "-o", "none"}, azLong)
		gone = append(gone, "os disk")
	}
	return true, gone, nil
}
