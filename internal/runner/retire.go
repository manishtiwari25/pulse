package runner

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

// Heal, retire, delete (ADR-034 rules 3-5). Each daemon tick, after reprobeDue, tendExecutors looks at every
// executor a login fault drained and the daemon has re-checked at least once since. When a login source
// exists (token.go) and an admin route is known, the login is placed once per drain episode through
// executorLogin; when that is impossible or refused and the VM is PULSE's (a compute_machines row, or an Azure
// VM tagged pulse=hands with the executor's name or address), the VM is deallocated now, the executor row is
// removed and a retired_vms row says when the VM may be deleted. The delete pass runs every tick over the
// rows past their grace: the VM, its NICs, public IPs and OS disk go; VNets and NSGs stay (they may be
// shared). Every act posts a finding; none waits on a proposal. Nothing here creates cost or mints a
// credential.

const (
	healThread = "executors"
	azShort    = 60 * time.Second
	azLong     = 15 * time.Minute
)

// sshHostname is what ssh would connect to for host (`ssh -G host` -> the hostname line); tests stub it.
var sshHostname = func(host string) string {
	out, err := exec.Command("ssh", "-G", host).Output()
	if err != nil {
		return host
	}
	for _, l := range strings.Split(string(out), "\n") {
		if f := strings.Fields(l); len(f) == 2 && f[0] == "hostname" {
			return f[1]
		}
	}
	return host
}

// retiredVM is what the retire phase knows about an executor's machine.
type retiredVM struct {
	Provider, ResourceGroup, Name, IP string
	Request                           string // the compute request that made it, when the compute worker did
}

// retireCandidates: executors drained by a login fault that the daemon re-checked at least once since the
// drain. noteExecutorRun sets backoff_minutes to exactly loginBackoffMins[0] when it drains; loginCheck
// doubles it on every refused re-check, so a value above the first backoff proves one refusal after the
// drain - a passing GitHub outage at drain time never qualifies on its own. The local executor is never one.
func retireCandidates(s *store.Store) []row {
	var out []row
	for _, r := range mustRows(s.DB, "SELECT * FROM executors WHERE status = 'drained' AND name != ? ORDER BY name", localExecutor) {
		if loginFault(r.str("probe_note")) && r.int("backoff_minutes") > loginBackoffMins[0] {
			out = append(out, r)
		}
	}
	return out
}

// identifyVM says which machine an executor runs on and whether PULSE made it: the compute_machines row,
// else the Azure VM tagged pulse=hands whose name is the executor's or whose public address is the
// executor's host. Anything else - or no usable az - is not ours, with a note saying why.
func identifyVM(s *store.Store, r row) (*retiredVM, bool, string) {
	name := r.str("name")
	if mrow := mustOne(s.DB, "SELECT request_id FROM compute_machines WHERE executor = ? AND released_at = '' ORDER BY created_at DESC LIMIT 1", name); mrow != nil {
		if m, _ := loadMachine(s, mrow.str("request_id")); m != nil {
			return &retiredVM{Provider: m.Provider, ResourceGroup: m.Detail["resource_group"], Name: m.Name, IP: m.Detail["public_ip"], Request: mrow.str("request_id")}, true, ""
		}
	}
	ip := ""
	if _, host, _, err := parseSSHAddress(r.str("address")); err == nil {
		ip = strings.ReplaceAll(sshHostname(host), "'", "")
	}
	query := fmt.Sprintf("[?tags.pulse=='hands' && (name=='%s' || publicIps=='%s')].{name:name,rg:resourceGroup,ip:publicIps}", name, ip)
	rc, out, errText, err := runAz(context.Background(), []string{"vm", "list", "-d", "--query", query, "-o", "json"}, azShort)
	if err != nil || rc != 0 {
		return nil, false, "the Azure CLI could not answer: " + errMessage(azErr("az vm list", rc, out, errText, err))
	}
	var found []struct {
		Name string `json:"name"`
		RG   string `json:"rg"`
		IP   string `json:"ip"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &found); err != nil || len(found) == 0 {
		return nil, false, fmt.Sprintf("no Azure VM tagged pulse=hands is named %s or answers at %s", name, orDash(ip))
	}
	return &retiredVM{Provider: "azure", ResourceGroup: found[0].RG, Name: found[0].Name, IP: found[0].IP}, true, ""
}

// tendExecutors is the daemon phase: heal what can be healed, retire what cannot, delete what is past its grace.
func tendExecutors(s *store.Store, now, actor string) []string {
	var lines []string
	heal := s.Setting("compute.self_heal", "1") == "1"
	retire := s.Setting("compute.self_retire", "1") == "1"
	if heal || retire {
		for _, r := range retireCandidates(s) {
			var vm *retiredVM
			ours, note, identified := false, "", false
			identify := func() {
				if !identified {
					vm, ours, note = identifyVM(s, r)
					identified = true
				}
			}
			healed := false
			if heal && r.str("healed_at") == "" {
				var why string
				healed, why = healExecutor(s, now, actor, r, func() (*retiredVM, bool) { identify(); return vm, ours })
				lines = append(lines, why)
			}
			if healed || !retire {
				continue
			}
			identify()
			lines = append(lines, retireExecutor(s, now, actor, r, vm, ours, note)...)
		}
	}
	return append(lines, deleteRetired(s, now, actor)...)
}

// healExecutor places the login source once per drain episode (healed_at) over the admin route: executors.admin,
// else the admin user at the executor's own ssh host (so the alias's key and options apply), else at the public
// address of PULSE's Azure VM. The token is read from the source and handed to executorLogin as a stream; it is
// not kept.
func healExecutor(s *store.Store, now, actor string, r row, identify func() (*retiredVM, bool)) (bool, string) {
	name := r.str("name")
	tok, origin, err := loginSource(s)
	if err != nil {
		return false, fmt.Sprintf("heal %s: login source: %s", name, errMessage(err))
	}
	if len(tok) == 0 {
		return false, fmt.Sprintf("heal %s: no login source (pulse executor token --with-token)", name)
	}
	admin := r.str("admin")
	if admin == "" {
		if vm, ours := identify(); ours && vm.Provider == "azure" {
			admin = adminRoute(r.str("address"), vm.IP)
		}
	}
	if admin == "" {
		return false, fmt.Sprintf("heal %s: no admin route (no admin address on record and the VM is not PULSE's on Azure)", name)
	}
	exec1(s.DB, "UPDATE executors SET healed_at = ? WHERE name = ?", now, name)
	res, err := placeLogin(s, now, name, admin, tokenReader(tok), false, false, actor)
	if err != nil {
		return false, fmt.Sprintf("heal %s: %s", name, redactTokens(errMessage(err)))
	}
	after := executorRow(s, name)
	if !res.OK || after == nil || after.str("status") != "active" {
		last := ""
		if n := len(res.Lines); n > 0 {
			last = res.Lines[n-1]
		}
		return false, fmt.Sprintf("heal %s refused: %s", name, redactTokens(last))
	}
	eventAt(s.DB, now, actor, "executor.healed", name, map[string]any{"source": origin, "admin": admin})
	if proj := findingProject(s); proj != "" {
		_, _ = createPost(s, now, postOpts{Kind: "finding", Project: proj, Area: computeArea, Thread: healThread,
			Summary: fmt.Sprintf("healed %s: login placed from %s", name, origin),
			Body: fmt.Sprintf("Executor `%s` was drained by a refused login (%s). The daemon placed the %s over %s and the login check "+
				"passed; the executor is active again (ADR-034). No credential was created; the one placed reaches no repository.\n",
				name, redactTokens(trunc(r.str("probe_note"), 200)), origin, admin),
			Agent: actorAgent(s, actor), Date: now})
	}
	return true, fmt.Sprintf("healed %s: login placed from %s", name, origin)
}

// adminRoute is the admin login for a VM with no admin on record: the admin user at the host part of the
// executor's address (an ssh alias keeps its IdentityFile, port and jumps; the user on the command line wins
// over the alias's User), else at the VM's public address, else nothing.
func adminRoute(address, ip string) string {
	host := strings.TrimSpace(address)
	if _, after, ok := strings.Cut(host, "@"); ok {
		host = after
	}
	if host == "" {
		host = ip
	}
	if host == "" {
		return ""
	}
	return azureAdminUser + "@" + host
}

// retireExecutor: the machine stops costing money now, the row is removed, and a finding says how to undo it.
func retireExecutor(s *store.Store, now, actor string, r row, vm *retiredVM, ours bool, note string) []string {
	name := r.str("name")
	refused := redactTokens(trunc(strings.TrimPrefix(r.str("probe_note"), "fault: "), 300))
	// A run still in flight keeps the row and the machine this tick (RemoveExecutor's own rule, previewed).
	if _, err := board.RemoveExecutor(s, name, actor, true); err != nil {
		return []string{fmt.Sprintf("retire %s postponed: %s", name, errMessage(err))}
	}
	say := func(format string, a ...any) {}
	if ours && vm.Request != "" {
		// A compute-worker machine has its own release path: deleted at once, the request marked released.
		req, err := tasks.GetCompute(s, vm.Request)
		if err != nil {
			return []string{fmt.Sprintf("retire %s postponed: %s", name, errMessage(err))}
		}
		if !releaseCompute(s, actor, *req, "its login was refused ("+refused+")", say) {
			return []string{fmt.Sprintf("retire %s postponed: releasing %s failed; the daemon retries", name, vm.Name)}
		}
		eventAt(s.DB, now, actor, "executor.retired", name, map[string]any{"provider": vm.Provider, "resource_group": vm.ResourceGroup, "name": vm.Name, "request": vm.Request, "released": true})
		return []string{fmt.Sprintf("retired %s: compute machine %s released (login refused)", name, vm.Name)}
	}
	if ours {
		rc, out, errText, err := runAz(context.Background(), []string{"vm", "deallocate", "-g", vm.ResourceGroup, "-n", vm.Name, "-o", "none"}, azLong)
		if err != nil || rc != 0 {
			return []string{fmt.Sprintf("retire %s postponed: %s", name, errMessage(azErr("az vm deallocate "+vm.Name, rc, out, errText, err)))}
		}
	}
	if _, err := board.RemoveExecutor(s, name, actor, false); err != nil {
		return []string{fmt.Sprintf("retire %s postponed: %s", name, errMessage(err))}
	}
	next := "A Copilot-only fine-grained token placed with `pulse executor token --with-token` lets the daemon heal future machines instead."
	detail := map[string]any{"ours": ours, "refused": refused}
	var summary, body, line string
	if ours {
		grace := settingInt(s, "compute.retire_grace_hours")
		if grace < 1 {
			grace = 24
		}
		deleteAfter := plusHours(now, float64(grace))
		exec1(s.DB, "INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after, deleted_at, note) VALUES (?, ?, ?, ?, ?, ?, '', '') "+
			"ON CONFLICT(executor) DO UPDATE SET provider = excluded.provider, resource_group = excluded.resource_group, name = excluded.name, "+
			"deallocated_at = excluded.deallocated_at, delete_after = excluded.delete_after, deleted_at = '', note = ''",
			name, vm.Provider, vm.ResourceGroup, vm.Name, now, deleteAfter)
		detail["provider"], detail["resource_group"], detail["name"], detail["delete_after"] = vm.Provider, vm.ResourceGroup, vm.Name, deleteAfter
		summary = fmt.Sprintf("retired %s: login refused; VM %s (%s) deallocated now, deleted at %s unless kept", name, vm.Name, vm.ResourceGroup, deleteAfter)
		body = fmt.Sprintf("The daemon retired executor `%s` (ADR-034).\n\nWhat was refused: %s\n\nWhat the daemon did: the VM `%s` in resource group `%s` (%s), "+
			"which PULSE created, is deallocated now (%s) and stops costing money; the executor row is removed. The daemon deletes the VM, its network "+
			"interfaces, public IPs and OS disk at %s (compute.retire_grace_hours=%d).\n\nUndo: `az vm start -g %s -n %s` then `pulse executor activate %s` "+
			"before %s keeps it.\n\n%s\n", name, refused, vm.Name, vm.ResourceGroup, vm.Provider, now, deleteAfter, grace, vm.ResourceGroup, vm.Name, name, deleteAfter, next)
		line = fmt.Sprintf("retired %s: deallocated %s (%s); deleted at %s unless kept", name, vm.Name, vm.ResourceGroup, deleteAfter)
	} else {
		detail["note"] = note
		summary = fmt.Sprintf("retired %s: login refused; machine left running because PULSE did not create it", name)
		body = fmt.Sprintf("The daemon retired executor `%s` (ADR-034).\n\nWhat was refused: %s\n\nWhat the daemon did: the executor row is removed; the machine at "+
			"%s was left running because PULSE did not create it (%s). Stopping it, or fixing its login and pairing it again, is yours.\n\n%s\n",
			name, refused, r.str("address"), note, next)
		line = fmt.Sprintf("retired %s: row removed; machine left running because PULSE did not create it (%s)", name, note)
	}
	eventAt(s.DB, now, actor, "executor.retired", name, detail)
	if proj := findingProject(s); proj != "" {
		_, _ = createPost(s, now, postOpts{Kind: "finding", Project: proj, Area: computeArea, Thread: computeThread, Summary: trunc(summary, 230), Body: body,
			Agent: actorAgent(s, actor), Date: now})
	}
	return []string{line}
}

// deleteRetired is the delete pass: rows past delete_after whose executor is still removed lose their VM,
// NICs, public IPs and OS disk; a row whose executor is active again is closed as kept by the owner.
func deleteRetired(s *store.Store, now, actor string) []string {
	var lines []string
	for _, r := range mustRows(s.DB, "SELECT * FROM retired_vms WHERE deleted_at = '' AND delete_after != '' AND delete_after <= ? ORDER BY executor", now) {
		name := r.str("executor")
		status := ""
		if e := mustOne(s.DB, "SELECT status FROM executors WHERE name = ?", name); e != nil {
			status = e.str("status")
		}
		if status == "active" {
			exec1(s.DB, "UPDATE retired_vms SET deleted_at = ?, note = 'kept by the owner' WHERE executor = ?", now, name)
			eventAt(s.DB, now, actor, "executor.kept", name, map[string]any{"name": r.str("name"), "resource_group": r.str("resource_group")})
			lines = append(lines, fmt.Sprintf("kept %s: the owner activated it again; %s stays", name, r.str("name")))
			continue
		}
		if r.str("provider") != "azure" {
			exec1(s.DB, "UPDATE retired_vms SET deleted_at = ?, note = ? WHERE executor = ?", now, "no delete step for provider "+r.str("provider"), name)
			lines = append(lines, fmt.Sprintf("retired %s: no delete step for provider %s", name, r.str("provider")))
			continue
		}
		found, gone, err := azureDeleteVM(r.str("resource_group"), r.str("name"))
		if err != nil {
			lines = append(lines, fmt.Sprintf("delete %s postponed: %s", r.str("name"), errMessage(err)))
			continue
		}
		detail := map[string]any{"provider": "azure", "resource_group": r.str("resource_group"), "name": r.str("name"), "deleted": gone}
		if !found {
			exec1(s.DB, "UPDATE retired_vms SET deleted_at = ?, note = 'vm not found at delete time' WHERE executor = ?", now, name)
			detail["missing"] = true
			eventAt(s.DB, now, actor, "executor.deleted", name, detail)
			lines = append(lines, fmt.Sprintf("retired %s: %s was already gone", name, r.str("name")))
			continue
		}
		exec1(s.DB, "UPDATE retired_vms SET deleted_at = ? WHERE executor = ?", now, name)
		eventAt(s.DB, now, actor, "executor.deleted", name, detail)
		if proj := findingProject(s); proj != "" {
			_, _ = createPost(s, now, postOpts{Kind: "finding", Project: proj, Area: computeArea, Thread: computeThread,
				Summary: fmt.Sprintf("deleted %s (%s)", r.str("name"), r.str("resource_group")),
				Body: fmt.Sprintf("The VM `%s` in resource group `%s`, deallocated on %s when executor `%s` was retired, is deleted with %s (ADR-034). "+
					"Its virtual network and network security group stay; they may be shared. Only this finding remains of the machine.\n",
					r.str("name"), r.str("resource_group"), r.str("deallocated_at"), name, strings.Join(gone, ", ")),
				Agent: actorAgent(s, actor), Date: now})
		}
		lines = append(lines, fmt.Sprintf("deleted %s (%s): %s", r.str("name"), r.str("resource_group"), strings.Join(gone, ", ")))
	}
	return lines
}

// azureDeleteVM: `az vm show` for the OS disk, NICs and their public IPs, `az vm delete --yes`, then the NICs,
// public IPs and disk (best effort: `az vm create --nic-delete-option Delete` may have taken them already).
// found is false when the VM is not there; gone names what was deleted.
func azureDeleteVM(rg, name string) (bool, []string, error) {
	ctx := context.Background()
	rc, out, errText, err := runAz(ctx, []string{"vm", "show", "-g", rg, "-n", name, "-o", "json"}, azShort)
	if err != nil {
		return false, nil, azErr("az vm show "+name, rc, out, errText, err)
	}
	if rc != 0 {
		if low := strings.ToLower(errText + out); strings.Contains(low, "notfound") || strings.Contains(low, "not found") || strings.Contains(low, "could not be found") {
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
