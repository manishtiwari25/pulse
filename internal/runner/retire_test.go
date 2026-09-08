package runner

import (
	"context"
	"errors"
	"io"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

// -- ADR-034: heal, retire, delete ---------------------------------------------------------------------------

const testDaemon = "daemon"

type placedLogin struct{ name, admin, token, actor string }

// placeLoginStub records every executorLogin the daemon would run and, when ok, activates the executor as a
// passing login check would.
type placeLoginStub struct {
	mu   sync.Mutex
	ok   bool
	seen []placedLogin
}

func (p *placeLoginStub) calls() []placedLogin {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]placedLogin(nil), p.seen...)
}

func stubPlaceLogin(t *testing.T, ok bool) *placeLoginStub {
	t.Helper()
	p := &placeLoginStub{ok: ok}
	old := placeLogin
	t.Cleanup(func() { placeLogin = old })
	placeLogin = func(s *store.Store, now, name, admin string, token io.Reader, isTTY, remove bool, actor string) (loginResult, error) {
		raw, _ := io.ReadAll(token)
		p.mu.Lock()
		p.seen = append(p.seen, placedLogin{name, admin, strings.TrimSpace(string(raw)), actor})
		ok := p.ok
		p.mu.Unlock()
		setExecutorAdmin(s, name, admin)
		if !ok {
			return loginResult{OK: false, Kind: "refused", RC: 1, Lines: []string{"copilot refuses the login: Copilot Requests permission missing"}}, nil
		}
		note := "ok: login placed"
		setExecutorStatus(s, now, name, "active", actor, &note)
		return loginResult{OK: true, Kind: "pat", Lines: []string{"login placed"}}, nil
	}
	return p
}

// retireAz is a fake az for the retire and delete passes: every argv is recorded; vm list answers with the
// tagged machines it knows; vm show answers the shape the delete pass reads.
type retireAz struct {
	mu      sync.Mutex
	calls   [][]string
	listOut string // what `az vm list` prints
	missing bool   // `az vm show` says not found
	broken  bool   // az itself is missing or not logged in
}

func (f *retireAz) run(_ context.Context, args []string, _ time.Duration) (int, string, string, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, args)
	for _, a := range args {
		if store.SecretShaped(a) {
			return 1, "", "", errors.New("secret-shaped argument reached az: " + a)
		}
	}
	if f.broken {
		return 1, "", "", errors.New(`exec: "az": executable file not found in $PATH`)
	}
	cmd := strings.Join(args, " ")
	switch {
	case strings.HasPrefix(cmd, "vm list -d --query"):
		if f.listOut == "" {
			return 0, "[]\n", "", nil
		}
		return 0, f.listOut, "", nil
	case strings.HasPrefix(cmd, "vm deallocate"):
		return 0, "", "", nil
	case strings.HasPrefix(cmd, "vm show"):
		if f.missing {
			return 3, "", "(ResourceNotFound) The Resource 'Microsoft.Compute/virtualMachines/x' under resource group 'pulse-hands' was not found.", nil
		}
		return 0, `{"name":"pulse-cr-007","storageProfile":{"osDisk":{"managedDisk":{"id":"/subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Compute/disks/pulse-cr-007_OsDisk"}}},` +
			`"networkProfile":{"networkInterfaces":[{"id":"/subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Network/networkInterfaces/pulse-cr-007VMNic"}]}}`, "", nil
	case strings.HasPrefix(cmd, "network nic show"):
		return 0, `{"ipConfigurations":[{"publicIPAddress":{"id":"/subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Network/publicIPAddresses/pulse-cr-007PublicIP"}}],` +
			`"networkSecurityGroup":{"id":"/subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Network/networkSecurityGroups/pulse-cr-007NSG"}}`, "", nil
	case strings.HasPrefix(cmd, "vm delete"), strings.HasPrefix(cmd, "network nic delete"), strings.HasPrefix(cmd, "network public-ip delete"), strings.HasPrefix(cmd, "disk delete"):
		return 0, "", "", nil
	}
	return 1, "", "unexpected az " + cmd, nil
}

func (f *retireAz) joined() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	var out []string
	for _, c := range f.calls {
		out = append(out, strings.Join(c, " "))
	}
	return out
}

func (f *retireAz) has(prefix string) bool {
	for _, c := range f.joined() {
		if strings.HasPrefix(c, prefix) {
			return true
		}
	}
	return false
}

func installRetireAz(t *testing.T, f *retireAz) {
	t.Helper()
	oldAz, oldHost := runAz, sshHostname
	t.Cleanup(func() { runAz, sshHostname = oldAz, oldHost })
	runAz = f.run
	sshHostname = func(host string) string {
		if host == "vm-seven" {
			return "203.0.113.7"
		}
		return host
	}
}

// drainedByLogin registers an executor the way a refused login leaves it after one refused re-check:
// status drained, a login-fault probe note and a backoff above the first step.
func drainedByLogin(t *testing.T, s *store.Store, name, address string) {
	t.Helper()
	if _, err := addExecutor(s, testNow, name, address, "/srv/pulse", 1, "*", humanAgent); err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE executors SET status = 'drained', probe_note = ?, backoff_minutes = 30, next_probe_at = ? WHERE name = ?",
		"fault: copilot refuses the login: Error: Authentication token found but could not be validated", plusHours(testNow, 1), name)
}

func TestRetireCandidatesRule(t *testing.T) {
	s := newStore(t)
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	// Drained by a login fault but never re-checked since: not yet.
	addExecutor(s, testNow, "vm-fresh", "pulse@fresh", "/srv/pulse", 1, "*", humanAgent)
	exec1(s.DB, "UPDATE executors SET status = 'drained', probe_note = 'fault: not logged in', backoff_minutes = ? WHERE name = 'vm-fresh'", loginBackoffMins[0])
	// Drained for another reason with a big backoff: not a login fault.
	addExecutor(s, testNow, "vm-disk", "pulse@disk", "/srv/pulse", 1, "*", humanAgent)
	exec1(s.DB, "UPDATE executors SET status = 'drained', probe_note = 'fault: disk full', backoff_minutes = 360 WHERE name = 'vm-disk'")
	// Active with an old login note: not drained.
	addExecutor(s, testNow, "vm-ok", "pulse@ok", "/srv/pulse", 1, "*", humanAgent)
	exec1(s.DB, "UPDATE executors SET status = 'active', probe_note = 'not logged in', backoff_minutes = 30 WHERE name = 'vm-ok'")
	got := retireCandidates(s)
	if len(got) != 1 || got[0].str("name") != "vm-seven" {
		t.Fatalf("candidates: %v", got)
	}
}

func TestTendRetiresOurAzureVM(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	az := &retireAz{listOut: `[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]`}
	installRetireAz(t, az)
	stubGh(t, "")
	placed := stubPlaceLogin(t, true)

	lines := tendExecutors(s, testNow, testDaemon)
	if placed.calls() != nil {
		t.Fatalf("login placed without a source: %+v", placed.calls())
	}
	calls := az.joined()
	if len(calls) != 2 || !strings.HasPrefix(calls[0], "vm list -d --query [?tags.pulse=='hands' && (name=='vm-seven' || publicIps=='203.0.113.7')].{name:name,rg:resourceGroup,ip:publicIps} -o json") ||
		calls[1] != "vm deallocate -g pulse-hands -n pulse-cr-007 -o none" {
		t.Fatalf("az calls: %q", calls)
	}
	if ex := mustOne(s.DB, "SELECT status FROM executors WHERE name = 'vm-seven'"); ex == nil || ex.str("status") != "removed" {
		t.Fatalf("executor row: %v", ex)
	}
	r := mustOne(s.DB, "SELECT * FROM retired_vms WHERE executor = 'vm-seven'")
	if r == nil || r.str("provider") != "azure" || r.str("resource_group") != "pulse-hands" || r.str("name") != "pulse-cr-007" ||
		r.str("deallocated_at") != testNow || r.str("delete_after") != plusHours(testNow, 24) || r.str("deleted_at") != "" {
		t.Fatalf("retired_vms: %v", r)
	}
	ev := mustOne(s.DB, "SELECT * FROM events WHERE action = 'executor.retired' AND subject = 'vm-seven'")
	if d := jsonMap(ev.str("detail")); ev == nil || d["delete_after"] != plusHours(testNow, 24) || d["name"] != "pulse-cr-007" || d["resource_group"] != "pulse-hands" || d["provider"] != "azure" {
		t.Fatalf("event: %v", ev)
	}
	posts := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'compute'")
	if len(posts) != 1 {
		t.Fatalf("findings: %v", posts)
	}
	body := posts[0].str("body")
	for _, want := range []string{
		"retired vm-seven: login refused; VM pulse-cr-007 (pulse-hands) deallocated now, deleted at " + plusHours(testNow, 24) + " unless kept",
	} {
		if posts[0].str("summary") != want {
			t.Fatalf("summary: %q", posts[0].str("summary"))
		}
	}
	for _, want := range []string{
		"What was refused: copilot refuses the login",
		"deallocated now (" + testNow + ")",
		"deletes the VM, its network interfaces, public IPs and OS disk at " + plusHours(testNow, 24) + " (compute.retire_grace_hours=24)",
		"Undo: `az vm start -g pulse-hands -n pulse-cr-007` then `pulse executor activate vm-seven` before " + plusHours(testNow, 24) + " keeps it.",
		"A Copilot-only fine-grained token placed with `pulse executor token --with-token` lets the daemon heal future machines instead.",
	} {
		if !strings.Contains(body, want) {
			t.Fatalf("body lacks %q:\n%s", want, body)
		}
	}
	if len(lines) < 2 || !strings.Contains(lines[0], "no login source") || !strings.Contains(lines[1], "retired vm-seven: deallocated pulse-cr-007") {
		t.Fatalf("lines: %q", lines)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question'") != 0 {
		t.Fatal("the daemon asked instead of telling")
	}

	// A second tick: the row is removed, nothing is a candidate, nothing is due; az is not called again.
	before := len(az.joined())
	tendExecutors(s, testNow, testDaemon)
	if len(az.joined()) != before {
		t.Fatalf("az called again: %q", az.joined()[before:])
	}
}

func TestTendLeavesForeignMachineRunning(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	drainedByLogin(t, s, "laptop", "pulse@laptop.local")
	az := &retireAz{}
	installRetireAz(t, az)
	stubGh(t, "")
	tendExecutors(s, testNow, testDaemon)
	if az.has("vm deallocate") || !az.has("vm list") {
		t.Fatalf("az calls: %q", az.joined())
	}
	if ex := mustOne(s.DB, "SELECT status FROM executors WHERE name = 'laptop'"); ex.str("status") != "removed" {
		t.Fatalf("executor row: %v", ex)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM retired_vms") != 0 {
		t.Fatal("a foreign machine got a retired_vms row")
	}
	p := mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'")
	if p == nil || p.str("summary") != "retired laptop: login refused; machine left running because PULSE did not create it" ||
		!strings.Contains(p.str("body"), "left running because PULSE did not create it (no Azure VM tagged pulse=hands is named laptop or answers at laptop.local)") {
		t.Fatalf("finding: %v", p)
	}
}

func TestTendWithoutAzIsNotOurs(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	az := &retireAz{broken: true}
	installRetireAz(t, az)
	stubGh(t, "")
	lines := tendExecutors(s, testNow, testDaemon)
	if az.has("vm deallocate") {
		t.Fatalf("deallocated without az: %q", az.joined())
	}
	if ex := mustOne(s.DB, "SELECT status FROM executors WHERE name = 'vm-seven'"); ex.str("status") != "removed" {
		t.Fatalf("executor row: %v", ex)
	}
	p := mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'")
	if p == nil || !strings.Contains(p.str("body"), "left running because PULSE did not create it (the Azure CLI could not answer:") {
		t.Fatalf("finding: %v", p)
	}
	if !strings.Contains(strings.Join(lines, "\n"), "the Azure CLI could not answer") {
		t.Fatalf("lines: %q", lines)
	}
}

func TestTendHealsFromExecutorToken(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	setExecutorAdmin(s, "vm-seven", "pulseadmin@vm-seven-admin")
	az := &retireAz{listOut: `[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]`}
	installRetireAz(t, az)
	writeExecutorToken(t, s)
	placed := stubPlaceLogin(t, true)

	lines := tendExecutors(s, testNow, testDaemon)
	calls := placed.calls()
	if len(calls) != 1 || calls[0].name != "vm-seven" || calls[0].admin != "pulseadmin@vm-seven-admin" || calls[0].token != testExecutorToken || calls[0].actor != testDaemon {
		t.Fatalf("placement: %+v", calls)
	}
	if len(az.joined()) != 0 {
		t.Fatalf("az touched while healing: %q", az.joined())
	}
	ex := executorRow(s, "vm-seven")
	if ex == nil || ex.str("status") != "active" || ex.str("healed_at") != "" {
		// activation clears healed_at: the episode is over
		t.Fatalf("executor after heal: %v", ex)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.healed' AND subject = 'vm-seven'") != 1 {
		t.Fatal("no executor.healed event")
	}
	p := mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'executors'")
	if p == nil || p.str("summary") != "healed vm-seven: login placed from executor token" || strings.Contains(p.str("body"), testExecutorToken) {
		t.Fatalf("finding: %v", p)
	}
	if len(lines) != 1 || lines[0] != "healed vm-seven: login placed from executor token" {
		t.Fatalf("lines: %q", lines)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM retired_vms") != 0 {
		t.Fatal("healed and retired")
	}
}

func TestTendHealRefusedRetiresAndDoesNotHealTwice(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	az := &retireAz{listOut: `[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]`}
	installRetireAz(t, az)
	writeExecutorToken(t, s)
	placed := stubPlaceLogin(t, false)
	setSetting(t, s, "compute.self_retire", "0")

	// No admin on record: the route is the admin user at the executor's own ssh host, so the alias's key applies
	// (pulseadmin@<public ip> would miss it). The refusal marks the episode.
	lines := tendExecutors(s, testNow, testDaemon)
	calls := placed.calls()
	if len(calls) != 1 || calls[0].admin != "pulseadmin@vm-seven" {
		t.Fatalf("placement: %+v", calls)
	}
	if ex := executorRow(s, "vm-seven"); ex.str("status") != "drained" || ex.str("healed_at") != testNow {
		t.Fatalf("after refused heal: %v", ex)
	}
	if !strings.Contains(strings.Join(lines, "\n"), "heal vm-seven refused") || az.has("vm deallocate") {
		t.Fatalf("self_retire=0 yet: %q %q", lines, az.joined())
	}
	// Second tick: healed_at is set, so no second placement; still nothing retired while self_retire=0.
	tendExecutors(s, testNow, testDaemon)
	if len(placed.calls()) != 1 {
		t.Fatalf("healed twice: %+v", placed.calls())
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.healed'") != 0 {
		t.Fatal("a refused heal was recorded as healed")
	}
	// With retirement on, the same tick falls through to deallocation.
	setSetting(t, s, "compute.self_retire", "1")
	tendExecutors(s, testNow, testDaemon)
	if len(placed.calls()) != 1 || !az.has("vm deallocate -g pulse-hands -n pulse-cr-007 -o none") {
		t.Fatalf("retire after refusal: %+v %q", placed.calls(), az.joined())
	}
	if ex := mustOne(s.DB, "SELECT status FROM executors WHERE name = 'vm-seven'"); ex.str("status") != "removed" {
		t.Fatalf("executor row: %v", ex)
	}
	// Switching self_heal off keeps the daemon's hands off a new candidate entirely.
	setSetting(t, s, "compute.self_heal", "0")
	setSetting(t, s, "compute.self_retire", "0")
	drainedByLogin(t, s, "vm-eight", "pulse@vm-eight")
	before := len(az.joined())
	tendExecutors(s, testNow, testDaemon)
	if len(placed.calls()) != 1 || len(az.joined()) != before || executorRow(s, "vm-eight").str("status") != "drained" {
		t.Fatalf("switched off but acted: %+v %q", placed.calls(), az.joined()[before:])
	}
}

func TestTendPostponesWhileARunIsInFlight(t *testing.T) {
	s := newStore(t)
	_, slug := newRepo(t, s)
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	az := &retireAz{listOut: `[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]`}
	installRetireAz(t, az)
	stubGh(t, "")
	exec1(s.DB, "INSERT INTO runs (id, job_id, project, agent, status, started_at, executor) VALUES ('run-1', 'job-1', ?, 'aurora', 'running', ?, 'vm-seven')", slug, testNow)
	lines := tendExecutors(s, testNow, testDaemon)
	if az.has("vm deallocate") || executorRow(s, "vm-seven").str("status") != "drained" {
		t.Fatalf("retired with a run in flight: %q %q", lines, az.joined())
	}
	if !strings.Contains(strings.Join(lines, "\n"), "retire vm-seven postponed") {
		t.Fatalf("lines: %q", lines)
	}
}

func TestTendDeletesAfterGraceAndKeepsWhatTheOwnerRevived(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	drainedByLogin(t, s, "vm-nine", "pulse@vm-nine")
	az := &retireAz{listOut: `[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]`}
	installRetireAz(t, az)
	stubGh(t, "")
	tendExecutors(s, testNow, testDaemon)
	if count(t, s, "SELECT COUNT(*) AS n FROM retired_vms WHERE deleted_at = ''") != 2 {
		t.Fatalf("retired rows: %v", mustRows(s.DB, "SELECT * FROM retired_vms"))
	}
	// The owner starts vm-nine's machine again and activates it before the grace ends: the row revives.
	oldProbe := runSSH
	t.Cleanup(func() { runSSH = oldProbe })
	runSSH = func(argv []string, stdin []byte, _ time.Duration) (int, string, string, error) {
		return 0, "hands=ok git=ok copilot=ok keys=- keys_ok=yes remotes=0 workdir=/srv/pulse logged_in=yes disk_free_mb=20000\n", "", nil
	}
	if out := invokeOK(t, s, "executor", "", "activate", "vm-nine"); !strings.Contains(out, "vm-nine") {
		t.Fatalf("activate: %q", out)
	}
	if ex := executorRow(s, "vm-nine"); ex == nil || ex.str("status") != "active" {
		t.Fatalf("revived executor: %v", ex)
	}

	// Before the grace ends nothing is deleted.
	az.calls = nil
	tendExecutors(s, plusHours(testNow, 23), testDaemon)
	if len(az.joined()) != 0 {
		t.Fatalf("deleted early: %q", az.joined())
	}

	// At the grace: vm-seven's machine goes (VM, NIC, public IP, OS disk; never the VNet or NSG); vm-nine is kept.
	later := plusHours(testNow, 24)
	lines := tendExecutors(s, later, testDaemon)
	want := []string{
		"vm show -g pulse-hands -n pulse-cr-007 -o json",
		"network nic show --ids /subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Network/networkInterfaces/pulse-cr-007VMNic -o json",
		"vm delete -g pulse-hands -n pulse-cr-007 --yes -o none",
		"network nic delete --ids /subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Network/networkInterfaces/pulse-cr-007VMNic -o none",
		"network public-ip delete --ids /subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Network/publicIPAddresses/pulse-cr-007PublicIP -o none",
		"disk delete --ids /subscriptions/s/resourceGroups/pulse-hands/providers/Microsoft.Compute/disks/pulse-cr-007_OsDisk --yes -o none",
	}
	got := az.joined()
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("delete sequence:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	for _, c := range got {
		if strings.Contains(c, "vnet") || strings.Contains(c, "nsg") || strings.Contains(c, "NSG") {
			t.Fatalf("shared network deleted: %s", c)
		}
	}
	seven := mustOne(s.DB, "SELECT * FROM retired_vms WHERE executor = 'vm-seven'")
	if seven.str("deleted_at") != later || seven.str("note") != "" {
		t.Fatalf("vm-seven row: %v", seven)
	}
	nine := mustOne(s.DB, "SELECT * FROM retired_vms WHERE executor = 'vm-nine'")
	if nine.str("deleted_at") != later || nine.str("note") != "kept by the owner" {
		t.Fatalf("vm-nine row: %v", nine)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.deleted' AND subject = 'vm-seven'") != 1 ||
		count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.kept' AND subject = 'vm-nine'") != 1 {
		t.Fatal("events missing")
	}
	p := mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute' AND summary = 'deleted pulse-cr-007 (pulse-hands)'")
	if p == nil {
		t.Fatalf("delete finding missing: %v", mustRows(s.DB, "SELECT summary FROM posts"))
	}
	if !strings.Contains(strings.Join(lines, "\n"), "deleted pulse-cr-007 (pulse-hands): vm pulse-cr-007, 1 nic(s), 1 public ip(s), os disk") ||
		!strings.Contains(strings.Join(lines, "\n"), "kept vm-nine") {
		t.Fatalf("lines: %q", lines)
	}
	// Nothing is due twice.
	az.calls = nil
	tendExecutors(s, plusHours(later, 1), testDaemon)
	if len(az.joined()) != 0 {
		t.Fatalf("deleted twice: %q", az.joined())
	}
}

func TestTendDeleteClosesAMissingVM(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	az := &retireAz{missing: true}
	installRetireAz(t, az)
	exec1(s.DB, "INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after) VALUES ('vm-gone', 'azure', 'pulse-hands', 'pulse-cr-gone', ?, ?)", testNow, testNow)
	lines := tendExecutors(s, testNow, testDaemon)
	got := az.joined()
	if len(got) != 1 || got[0] != "vm show -g pulse-hands -n pulse-cr-gone -o json" {
		t.Fatalf("az calls: %q", got)
	}
	r := mustOne(s.DB, "SELECT * FROM retired_vms WHERE executor = 'vm-gone'")
	if r.str("deleted_at") != testNow || r.str("note") != "vm not found at delete time" {
		t.Fatalf("row: %v", r)
	}
	if !strings.Contains(strings.Join(lines, "\n"), "was already gone") {
		t.Fatalf("lines: %q", lines)
	}
}

func TestDaemonTickTendsExecutors(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	drainedByLogin(t, s, "vm-seven", "pulse@vm-seven")
	az := &retireAz{listOut: `[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]`}
	installRetireAz(t, az)
	stubGh(t, "")
	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'"})
	if !strings.Contains(log, "retired vm-seven: deallocated pulse-cr-007 (pulse-hands)") {
		t.Fatalf("log:\n%s", log)
	}
	if !az.has("vm deallocate -g pulse-hands -n pulse-cr-007 -o none") {
		t.Fatalf("az calls: %q", az.joined())
	}
}

// -- ADR-034 rule 6: no login source, no machine -------------------------------------------------------------

func TestCapsAllowHoldsWithoutLoginSource(t *testing.T) {
	caps := tasks.Caps{MaxVMs: 2, MaxHoursPerDay: 24, LoginSource: false}
	req := tasks.ComputeRequest{Hours: 1, At: testNow}
	if why := capsAllow(caps, req, testNow); why != tasks.NoLoginSource {
		t.Fatalf("why: %q", why)
	}
	caps.LoginSource = true
	if why := capsAllow(caps, req, testNow); why != "" {
		t.Fatalf("with a source: %q", why)
	}
	// Caps come first: a cap failure is a failure even without a source.
	caps.LoginSource, caps.MaxVMs = false, 0
	if why := capsAllow(caps, req, testNow); !strings.HasPrefix(why, "cap:") {
		t.Fatalf("cap before source: %q", why)
	}
}

func TestComputeHeldWithoutLoginSourceAndExplainedOncePerDay(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	for k, v := range map[string]string{"compute.provider": "azure", "compute.max_vms": "2", "compute.max_hours_per_day": "24",
		"compute.azure.resource_group": "rg", "compute.azure.location": "westeurope", "compute.azure.vm_size": "Standard_B2ms", "compute.auto_approve": "1"} {
		setSetting(t, s, k, v)
	}
	stubGh(t, "")
	old := runAz
	t.Cleanup(func() { runAz = old })
	runAz = func(context.Context, []string, time.Duration) (int, string, string, error) {
		t.Fatal("az was called without a login source")
		return 1, "", "", nil
	}
	// Auto-approval holds: the request stays pending and its proposal says why.
	req, err := tasks.RequestCompute(s, "proj", "aurora", "", 2, "long run")
	if err != nil {
		t.Fatal(err)
	}
	if req.Status != "pending" || req.ProposalPost == "" {
		t.Fatalf("auto-approved without a source: %+v", req)
	}
	if p := mustOne(s.DB, "SELECT body FROM posts WHERE id = ?", req.ProposalPost); p == nil || !strings.Contains(p.str("body"), tasks.NoLoginSource) {
		t.Fatalf("proposal: %v", p)
	}
	caps, _ := tasks.GetCaps(s)
	if caps.LoginSource {
		t.Fatalf("caps: %+v", caps)
	}
	// The worker explains the hold once a day, and never buys.
	w := newComputeWorker(context.Background(), s, computeOpts{})
	w.tick(context.Background(), testNow)
	w.wait(0)
	w.tick(context.Background(), plusHours(testNow, 1))
	w.wait(0)
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = 'compute'"); n != 1 {
		t.Fatalf("hold findings on one day: %d", n)
	}
	f := mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'")
	if !strings.HasPrefix(f.str("summary"), tasks.NoLoginSource+": compute "+req.ID+" waits; place a Copilot-only token with `pulse executor token --with-token`") {
		t.Fatalf("summary: %q", f.str("summary"))
	}
	// An approved request (the owner said yes) is held the same way, not failed.
	if _, err := tasks.Approve(s, req.ID, "fine", humanAgent); err != nil {
		t.Fatal(err)
	}
	w.tick(context.Background(), plusHours(testNow, 2))
	w.wait(0)
	if got, _ := tasks.GetCompute(s, req.ID); got.Status != "approved" {
		t.Fatalf("held request: %+v", got)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = 'compute'"); n != 1 {
		t.Fatalf("hold findings on one day: %d", n)
	}
	// A new day, a fresh worker (as after a daemon restart): one more note, still no machine.
	nextDay := plusHours(testNow, 24)
	later, err := store.Open(store.Options{Home: s.Home, Now: nextDay})
	if err != nil {
		t.Fatal(err)
	}
	defer later.Close()
	w2 := newComputeWorker(context.Background(), later, computeOpts{})
	w2.tick(context.Background(), nextDay)
	w2.wait(0)
	w2.tick(context.Background(), plusHours(nextDay, 1))
	w2.wait(0)
	if n := count(t, later, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = 'compute'"); n != 2 {
		t.Fatalf("hold findings over two days: %d", n)
	}
	if got, _ := tasks.GetCompute(later, req.ID); got.Status != "approved" {
		t.Fatalf("held request: %+v", got)
	}
	// With a source the same request goes on to provisioning (az answers, so it fails there - not a hold).
	writeExecutorToken(t, later)
	runAz = func(context.Context, []string, time.Duration) (int, string, string, error) {
		return 1, "", "no subscription", nil
	}
	w2.tick(context.Background(), plusHours(nextDay, 2))
	w2.wait(0)
	if got, _ := tasks.GetCompute(later, req.ID); got.Status == "approved" {
		t.Fatalf("still held with a source: %+v", got)
	}
}

func TestExecutorLoginRecordsAdminRoute(t *testing.T) {
	s := newStore(t)
	if _, err := addExecutor(s, testNow, "vm-seven", "pulse@vm-seven", "/srv/pulse", 1, "*", humanAgent); err != nil {
		t.Fatal(err)
	}
	old := runSSH
	t.Cleanup(func() { runSSH = old })
	runSSH = func(argv []string, stdin []byte, _ time.Duration) (int, string, string, error) { return 0, "", "", nil }
	if _, err := executorLogin(s, testNow, "vm-seven", "pulseadmin@vm-seven-admin", nil, false, true, humanAgent); err != nil {
		t.Fatal(err)
	}
	if ex := executorRow(s, "vm-seven"); ex.str("admin") != "pulseadmin@vm-seven-admin" {
		t.Fatalf("admin not recorded: %v", ex)
	}
}

func TestAdminRoutePrefersTheExecutorHost(t *testing.T) {
	for _, c := range []struct{ address, ip, want string }{
		{"pulse@pulse-vm1", "20.0.0.1", "pulseadmin@pulse-vm1"},
		{"pulse-vm1", "20.0.0.1", "pulseadmin@pulse-vm1"},
		{"pulse@", "20.0.0.1", "pulseadmin@20.0.0.1"},
		{"", "20.0.0.1", "pulseadmin@20.0.0.1"},
		{"", "", ""},
	} {
		if got := adminRoute(c.address, c.ip); got != c.want {
			t.Errorf("adminRoute(%q, %q) = %q, want %q", c.address, c.ip, got, c.want)
		}
	}
}
