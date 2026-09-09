package runner

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

// Heal, retire, delete (ADR-034 rules 3-5). Each daemon tick, after reprobeDue, tendExecutors looks at every
// executor a login fault drained and the daemon has re-checked at least once since. When a login source
// exists (token.go) and an admin route is known, the login is placed once per drain episode through
// executorLogin; when that is impossible or refused and the machine is PULSE's (a compute_machines row, or a
// machine the configured provider finds tagged pulse=hands with the executor's name or address), the machine
// is stopped now through its Provider, the executor row is removed and a retired_vms row says when the
// machine may be deleted. The delete pass runs every tick over the rows past their grace: the provider deletes
// the machine and what was created beside it (on Azure the NICs, public IPs and OS disk; VNets and NSGs stay,
// they may be shared). Every act posts a finding; none waits on a proposal. Nothing here creates cost or mints
// a credential.

const healThread = "executors"

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

// retiredVM is what the retire phase knows about an executor's machine: the machine as its provider describes
// it and, when the compute worker made it, the request that asked for it.
type retiredVM struct {
	*Machine
	Request string
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

// retireProvider is the provider the retire phase asks about machines nobody recorded: the configured one.
func retireProvider(s *store.Store) Provider {
	return providerNamed(strings.TrimSpace(s.Setting("compute.provider", "")))
}

// identifyVM says which machine an executor runs on and whether PULSE made it: the compute_machines row,
// else the machine the configured provider finds tagged pulse=hands whose name is the executor's or whose
// public address is the executor's host. Anything else - or a provider CLI that cannot answer - is not ours,
// with a note saying why.
func identifyVM(s *store.Store, r row) (*retiredVM, bool, string) {
	name := r.str("name")
	if mrow := mustOne(s.DB, "SELECT request_id FROM compute_machines WHERE executor = ? AND released_at = '' ORDER BY created_at DESC LIMIT 1", name); mrow != nil {
		if m, _ := loadMachine(s, mrow.str("request_id")); m != nil {
			return &retiredVM{Machine: m, Request: mrow.str("request_id")}, true, ""
		}
	}
	ip := ""
	if _, host, _, err := parseSSHAddress(r.str("address")); err == nil {
		ip = strings.ReplaceAll(sshHostname(host), "'", "")
	}
	p := retireProvider(s)
	if p == nil {
		return nil, false, fmt.Sprintf("no compute provider %s is built in (have: %s)", pyRepr(s.Setting("compute.provider", "")), strings.Join(ProviderNames(), ", "))
	}
	m, err := p.Find(context.Background(), s, name, ip)
	if err != nil {
		return nil, false, errMessage(err)
	}
	if m == nil {
		return nil, false, fmt.Sprintf("no %s tagged pulse=hands is named %s or answers at %s", p.Kind(), name, orDash(ip))
	}
	if m.Provider == "" {
		m.Provider = p.Name()
	}
	return &retiredVM{Machine: m}, true, ""
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
// address of the machine PULSE made. The token is read from the source and handed to executorLogin as a
// stream; it is not kept.
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
		if vm, ours := identify(); ours {
			admin = adminRoute(r.str("address"), vm.Detail["public_ip"])
		}
	}
	if admin == "" {
		return false, fmt.Sprintf("heal %s: no admin route (no admin address on record and the machine is not one PULSE made)", name)
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

// adminRoute is the admin login for a machine with no admin on record: the admin user at the host part of the
// executor's address (an ssh alias keeps its IdentityFile, port and jumps; the user on the command line wins
// over the alias's User), else at the machine's public address, else nothing.
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
	return adminUser + "@" + host
}

// machineDetail is the event detail of a machine: its provider facts, then the standard keys on top.
func machineDetail(m *Machine, extra map[string]any) map[string]any {
	detail := map[string]any{}
	for k, v := range m.Detail {
		if v != "" {
			detail[k] = v
		}
	}
	for k, v := range extra {
		detail[k] = v
	}
	return detail
}

// saveRetired remembers a stopped machine so the delete pass finds it after the grace (ADR-034 rule 5).
func saveRetired(s *store.Store, executor string, m *Machine, now, deleteAfter string) {
	detail, _ := json.Marshal(map[string]any{"tags": m.Tags, "detail": m.Detail})
	exec1(s.DB, "INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after, deleted_at, note, detail) VALUES (?, ?, ?, ?, ?, ?, '', '', ?) "+
		"ON CONFLICT(executor) DO UPDATE SET provider = excluded.provider, resource_group = excluded.resource_group, name = excluded.name, "+
		"deallocated_at = excluded.deallocated_at, delete_after = excluded.delete_after, deleted_at = '', note = '', detail = excluded.detail",
		executor, m.Provider, m.Detail["resource_group"], m.Name, now, deleteAfter, string(detail))
}

// retiredMachine rebuilds the machine a retired_vms row describes.
func retiredMachine(r row) *Machine {
	return machineFromDetail(r.str("provider"), r.str("name"), r.str("detail"), r.str("resource_group"))
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
		eventAt(s.DB, now, actor, "executor.retired", name, machineDetail(vm.Machine, map[string]any{"provider": vm.Provider, "name": vm.Name, "request": vm.Request, "released": true}))
		return []string{fmt.Sprintf("retired %s: compute machine %s released (login refused)", name, vm.Name)}
	}
	var p Provider
	if ours {
		if p = providerNamed(vm.Provider); p == nil {
			return []string{fmt.Sprintf("retire %s postponed: no compute provider %s is built in", name, pyRepr(vm.Provider))}
		}
		if err := p.Stop(context.Background(), vm.Machine); err != nil {
			return []string{fmt.Sprintf("retire %s postponed: %s", name, redactSecrets(errMessage(err)))}
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
		saveRetired(s, name, vm.Machine, now, deleteAfter)
		place := p.Place(vm.Machine)
		detail = machineDetail(vm.Machine, detail)
		detail["provider"], detail["name"], detail["place"], detail["delete_after"] = vm.Provider, vm.Name, place, deleteAfter
		summary = fmt.Sprintf("retired %s: login refused; %s %s (%s) stopped now, deleted at %s unless kept", name, p.Kind(), vm.Name, place, deleteAfter)
		body = fmt.Sprintf("The daemon retired executor `%s` (ADR-034).\n\nWhat was refused: %s\n\nWhat the daemon did: the %s `%s` in %s (%s), "+
			"which PULSE created, is stopped now (%s) and stops costing money; the executor row is removed. The daemon deletes the machine and what "+
			"was created beside it at %s (compute.retire_grace_hours=%d).\n\nUndo: `%s` then `pulse executor activate %s` "+
			"before %s keeps it.\n\n%s\n", name, refused, p.Kind(), vm.Name, place, vm.Provider, now, deleteAfter, grace, p.StartHint(vm.Machine), name, deleteAfter, next)
		line = fmt.Sprintf("retired %s: stopped %s (%s); deleted at %s unless kept", name, vm.Name, place, deleteAfter)
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

// deleteRetired is the delete pass: rows past delete_after whose executor is still removed lose their machine
// and what was created beside it through the provider; a row whose executor is active again is closed as kept
// by the owner.
func deleteRetired(s *store.Store, now, actor string) []string {
	var lines []string
	for _, r := range mustRows(s.DB, "SELECT * FROM retired_vms WHERE deleted_at = '' AND delete_after != '' AND delete_after <= ? ORDER BY executor", now) {
		name := r.str("executor")
		m := retiredMachine(r)
		status := ""
		if e := mustOne(s.DB, "SELECT status FROM executors WHERE name = ?", name); e != nil {
			status = e.str("status")
		}
		if status == "active" {
			exec1(s.DB, "UPDATE retired_vms SET deleted_at = ?, note = 'kept by the owner' WHERE executor = ?", now, name)
			eventAt(s.DB, now, actor, "executor.kept", name, machineDetail(m, map[string]any{"provider": m.Provider, "name": m.Name}))
			lines = append(lines, fmt.Sprintf("kept %s: the owner activated it again; %s stays", name, m.Name))
			continue
		}
		p := providerNamed(m.Provider)
		if p == nil {
			exec1(s.DB, "UPDATE retired_vms SET deleted_at = ?, note = ? WHERE executor = ?", now, "no delete step for provider "+m.Provider, name)
			lines = append(lines, fmt.Sprintf("retired %s: no delete step for provider %s", name, m.Provider))
			continue
		}
		place := p.Place(m)
		found, gone, err := p.Delete(context.Background(), m)
		if err != nil {
			lines = append(lines, fmt.Sprintf("delete %s postponed: %s", m.Name, redactSecrets(errMessage(err))))
			continue
		}
		detail := machineDetail(m, map[string]any{"provider": m.Provider, "name": m.Name, "place": place, "deleted": gone})
		if !found {
			exec1(s.DB, "UPDATE retired_vms SET deleted_at = ?, note = 'vm not found at delete time' WHERE executor = ?", now, name)
			detail["missing"] = true
			eventAt(s.DB, now, actor, "executor.deleted", name, detail)
			lines = append(lines, fmt.Sprintf("retired %s: %s was already gone", name, m.Name))
			continue
		}
		exec1(s.DB, "UPDATE retired_vms SET deleted_at = ? WHERE executor = ?", now, name)
		eventAt(s.DB, now, actor, "executor.deleted", name, detail)
		if proj := findingProject(s); proj != "" {
			_, _ = createPost(s, now, postOpts{Kind: "finding", Project: proj, Area: computeArea, Thread: computeThread,
				Summary: fmt.Sprintf("deleted %s (%s)", m.Name, place),
				Body: fmt.Sprintf("The %s `%s` in %s, stopped on %s when executor `%s` was retired, is deleted with %s (ADR-034). "+
					"What may be shared with other machines (on Azure the virtual network and network security group) stays. Only this finding remains of the machine.\n",
					p.Kind(), m.Name, place, r.str("deallocated_at"), name, strings.Join(gone, ", ")),
				Agent: actorAgent(s, actor), Date: now})
		}
		lines = append(lines, fmt.Sprintf("deleted %s (%s): %s", m.Name, place, strings.Join(gone, ", ")))
	}
	return lines
}
