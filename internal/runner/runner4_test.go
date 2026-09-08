package runner

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/hooks"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

// Phase 3 of the runner: leads (D-C), compute (D-D), the daemon's goroutine set (D-E), constitution
// auto-apply (D-H), planning for every project (D-J) and experiment landing (D-M).

func setSetting(t *testing.T, s *store.Store, key, value string) {
	t.Helper()
	if err := s.SetSetting(humanAgent, key, value); err != nil {
		t.Fatalf("setting %s: %v", key, err)
	}
}

func onceDaemon(t *testing.T, s *store.Store, o daemonOpts) string {
	t.Helper()
	var out bytes.Buffer
	o.Out = &out
	if o.Interval == 0 {
		o.Interval = 1
	}
	o.MaxLoops = 1
	if result := daemon(context.Background(), s, o); result != "done" {
		t.Fatalf("daemon = %s\n%s", result, out.String())
	}
	return out.String()
}

// -- leads (D-C) --------------------------------------------------------------------------------------------

func TestDaemonOnceLeadJobFromOwnerMessage(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "minutes_per_job", "1")
	task, err := tasks.New(s, "proj", "Add a badge", "Put a build badge in the README", 3, humanAgent)
	if err != nil {
		t.Fatal(err)
	}
	msg, err := tasks.SendMessage(s, "proj", task.ID, humanAgent, "Please make it the blue one")
	if err != nil {
		t.Fatal(err)
	}
	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: answered the owner'"})

	lead, err := s.Agent("proj-lead")
	if err != nil || lead == nil || lead.Kind != "lead" {
		t.Fatalf("lead agent: %v %v\n%s", lead, err, log)
	}
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = 'lead'")
	if len(jobs) != 1 {
		t.Fatalf("lead jobs: %v\n%s", jobs, log)
	}
	job := jobs[0]
	if job.str("status") != "done" || job.str("lease_agent") != "proj-lead" || job.int("priority") != leadJobPriority {
		t.Fatalf("lead job: %v\n%s", job, log)
	}
	for _, want := range []string{task.ID, "Please make it the blue one", "pulse task show <id>", "pulse job add --project proj --kind task --source task:<id>", "pulse task status <id> planned", "pulse msg send --project proj"} {
		if !strings.Contains(job.str("brief"), want) {
			t.Fatalf("lead brief lacks %q:\n%s", want, job.str("brief"))
		}
	}
	runs := mustRows(s.DB, "SELECT * FROM runs WHERE job_id = ?", job.str("id"))
	if len(runs) != 1 || runs[0].str("agent") != "proj-lead" || runs[0].str("status") != "done" {
		t.Fatalf("runs: %v\n%s", runs, log)
	}
	got, err := tasks.GetMessage(s, msg.ID)
	if err != nil || !contains(got.ReadBy, "proj-lead") {
		t.Fatalf("message not read by the lead: %+v %v", got, err)
	}
	tk, _, err := tasks.Get(s, task.ID)
	if err != nil || tk.Lead != "proj-lead" || tk.JobID != job.str("id") {
		t.Fatalf("task lead: %+v %v", tk, err)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'agent.join' AND subject = 'proj-lead'"); n != 1 {
		t.Fatalf("agent.join for the lead logged %d times", n)
	}
	// A second tick queues nothing: the messages are read and the task has its lead job.
	log2 := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'"})
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'lead'"); n != 1 {
		t.Fatalf("lead jobs after the second tick: %d\n%s", n, log2)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'agent.resume' AND subject = 'proj-lead'"); n > 1 {
		t.Fatalf("the daemon re-joined the lead %d times", n)
	}
}

func TestLeadJobsAreTheLeadsAlone(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	if _, _, err := ensureLead(s, "proj"); err != nil {
		t.Fatal(err)
	}
	leadJob, _, err := addJob(s, "proj", "lead", "Lead: plan", "plan it", 0, leadJobPriority, "lead:proj:x", "daemon", true)
	if err != nil {
		t.Fatal(err)
	}
	taskJob, _, err := addJob(s, "proj", "task", "a task", "do it", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	got, err := leaseJob(s, "aurora", testNow, leaseOpts{})
	if err != nil || got == nil || got.str("id") != taskJob.str("id") {
		t.Fatalf("worker leased %v (%v); wanted the task job, never the lead's", got, err)
	}
	if got, err := leaseJob(s, "aurora", testNow, leaseOpts{}); err != nil || got != nil {
		t.Fatalf("worker leased the lead job: %v %v", got, err)
	}
	setSetting(t, s, "parallel_per_project", "2")
	got, err = leaseJob(s, "proj-lead", testNow, leaseOpts{})
	if err != nil || got == nil || got.str("id") != leadJob.str("id") {
		t.Fatalf("lead leased %v (%v); wanted its lead job", got, err)
	}
	if got, err := leaseJob(s, "proj-lead", testNow, leaseOpts{}); err != nil || got != nil {
		t.Fatalf("lead leased a worker job: %v %v", got, err)
	}
}

func TestTendTasksFollowsTheLeadsJobs(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	if _, _, err := ensureLead(s, "proj"); err != nil {
		t.Fatal(err)
	}
	task, err := tasks.New(s, "proj", "Two changes", "change a and b", 0, humanAgent)
	if err != nil {
		t.Fatal(err)
	}
	a, _, err := addJob(s, "proj", "task", "change a", "do a", 1, 0, "task:"+task.ID, "proj-lead", false)
	if err != nil {
		t.Fatal(err)
	}
	b, _, err := addJob(s, "proj", "task", "change b", "do b", 1, 0, "task:"+task.ID, "proj-lead", false)
	if err != nil {
		t.Fatal(err)
	}
	// The lead itself may not have set the status; the planner does from the jobs.
	tendTasks(s, "proj", testNow, "daemon")
	tk, _, _ := tasks.Get(s, task.ID)
	if tk.Status != "planned" || tk.Lead != "proj-lead" || tk.JobID != a.str("id") {
		t.Fatalf("after planning: %+v", tk)
	}
	if _, err := leaseJob(s, "w1", testNow, leaseOpts{JobID: a.str("id")}); err != nil {
		t.Fatal(err)
	}
	tendTasks(s, "proj", testNow, "daemon")
	if tk, _, _ = tasks.Get(s, task.ID); tk.Status != "in_progress" {
		t.Fatalf("after a lease: %+v", tk)
	}
	if _, err := finishJob(s, a.str("id"), "w1", "done", "a done", "", -1); err != nil {
		t.Fatal(err)
	}
	tendTasks(s, "proj", testNow, "daemon")
	if tk, _, _ = tasks.Get(s, task.ID); tk.Status != "in_progress" {
		t.Fatalf("one job left: %+v", tk)
	}
	if _, err := leaseJob(s, "w1", testNow, leaseOpts{JobID: b.str("id")}); err != nil {
		t.Fatal(err)
	}
	if _, err := finishJob(s, b.str("id"), "w1", "failed", "b failed", "", -1); err != nil {
		t.Fatal(err)
	}
	lines := tendTasks(s, "proj", testNow, "daemon")
	if tk, _, _ = tasks.Get(s, task.ID); tk.Status != "review" {
		t.Fatalf("all jobs finished: %+v\n%v", tk, lines)
	}
	posts := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'result' AND area = 'tasks' AND thread = ?", strings.ToLower(task.ID))
	if len(posts) != 1 || posts[0].str("agent") != "proj-lead" || !strings.Contains(posts[0].str("summary"), "1 job(s) done, 1 failed") {
		t.Fatalf("review post: %v", posts)
	}
	// A second pass changes nothing.
	if lines := tendTasks(s, "proj", testNow, "daemon"); len(lines) != 0 {
		t.Fatalf("second pass: %v", lines)
	}
}

// -- constitution auto-apply (D-H) -------------------------------------------------------------------------

const testConstitution = `# Constitution

## Hard stops

Never cross a hard stop.

## Vote rules

| Rule | Value |
| --- | --- |
| Votes per identity per suggestion | 1 (the latest wins) |
| Amendment quorum | 1 distinct voter |
| Amendment supermajority | 2/3 of counted votes |
| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |

## Amendments

Per docs/workflows/amend.md.
`

const testAmendment = "## Current text\n\n```\n| Votes per identity per suggestion | 1 (the latest wins) |\n```\n\n" +
	"## Proposed text\n\n```\n| Votes per identity per suggestion | 1 (the latest wins; a fleet is one voter) |\n```\n\n## Why\n\nFleets voted twice.\n"

// passedAmendment builds a constitution record and one amendment with a counted up-vote in project proj.
func passedAmendment(t *testing.T, s *store.Store) string {
	t.Helper()
	rec := store.Record{ID: "EVO-constitution", Project: "proj", Kind: "evolution", Slug: "constitution", Title: "Constitution",
		Status: "active", Date: "2026-09-01", Path: "docs/evolution/constitution.md"}
	if _, err := s.PutRecord(rec, map[string]any{"loop": "off", "limits": "none"}, testConstitution, humanAgent, "proj"); err != nil {
		t.Fatal(err)
	}
	for _, a := range []string{"prop", "voter"} {
		if _, _, err := joinAgent(s, a, "r", "m", "s-"+a, "", "worker", ""); err != nil {
			t.Fatal(err)
		}
	}
	am, err := board.CreatePost(s, board.PostInput{Kind: "amendment", Project: "proj", Area: "governance", Thread: "fleet-votes", Agent: "prop", Session: "s-prop",
		Summary: "Count a fleet as one voter", Body: testAmendment, Date: "2026-09-02T08:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := board.CreatePost(s, board.PostInput{Kind: "vote", Project: "proj", Area: "governance", Thread: "fleet-votes", Agent: "voter", Session: "s-voter",
		Summary: "up", Body: "up\n\nRead: a fleet counts once; quorum untouched.", On: am.ID, Vote: "up", TextHash: tally.TextHash(testAmendment), Date: "2026-09-03T08:00:00Z"}); err != nil {
		t.Fatal(err)
	}
	return am.ID
}

func TestDaemonOnceAppliesPassedAmendment(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	amID := passedAmendment(t, s)
	th := tally.LoadThresholds(s, "proj", tally.Thresholds{Now: testNow})
	if passed, err := tally.PassedAmendments(s, "proj", th); err != nil || len(passed) != 1 || passed[0].ID != amID {
		t.Fatalf("fixture not passed: %v %v", passed, err)
	}
	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'"})
	if !strings.Contains(log, "amendment "+amID+" of proj applied") {
		t.Fatalf("daemon log:\n%s", log)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'constitution.amended'"); n != 1 {
		t.Fatalf("constitution.amended logged %d times\n%s", n, log)
	}
	ratified := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'result' AND resolution = 'ratified' AND thread = 'fleet-votes'")
	if len(ratified) != 1 || ratified[0].str("agent") != tally.SystemAgent {
		t.Fatalf("ratification: %v\n%s", ratified, log)
	}
	rec, rev, err := s.GetRecord("EVO-constitution", 0)
	if err != nil || rec == nil || rev == nil || rev.Rev != 2 || !strings.Contains(rev.Body, "a fleet is one voter") {
		t.Fatalf("constitution not rewritten: %v %+v %v", rec, rev, err)
	}
	if passed, _ := tally.PassedAmendments(s, "proj", th); len(passed) != 0 {
		t.Fatalf("still passed after ratification: %v", passed)
	}
	// The next tick has nothing to do and applies nothing twice.
	log2 := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'"})
	if strings.Contains(log2, "amendment ") || count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'constitution.amended'") != 1 {
		t.Fatalf("second tick:\n%s", log2)
	}
}

func TestAutoApplyRefusalIsToldOnce(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	amID := passedAmendment(t, s)
	setSetting(t, s, "stop", "1") // ratification paused: the tally still lists it as passed, ApplyAmendment refuses
	refused := amendRefusals{}
	lines := autoApplyAmendments(s, "proj", testNow, refused)
	if len(lines) != 1 || !strings.Contains(lines[0], amID) || !strings.Contains(lines[0], "left to the human") {
		t.Fatalf("first pass: %v", lines)
	}
	if _, ok := refused[amID]; !ok {
		t.Fatalf("refusal not remembered: %v", refused)
	}
	if lines := autoApplyAmendments(s, "proj", testNow, refused); len(lines) != 0 {
		t.Fatalf("second pass repeats the refusal: %v", lines)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'constitution.amended'"); n != 0 {
		t.Fatalf("amended under STOP: %d", n)
	}
}

// -- compute (D-D) with a fake az -------------------------------------------------------------------------

type fakeAz struct {
	mu      sync.Mutex
	calls   [][]string
	created map[string]bool
	deleted []string
}

func (f *fakeAz) run(_ context.Context, args []string, _ time.Duration) (int, string, string, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, args)
	for _, a := range args {
		if store.SecretShaped(a) {
			return 1, "", "", errors.New("secret-shaped argument reached az: " + a)
		}
	}
	cmd := strings.Join(args, " ")
	switch {
	case strings.HasPrefix(cmd, "account show"):
		return 0, "Pulse Sub\n", "", nil
	case strings.HasPrefix(cmd, "group show"):
		return 3, "", "ResourceGroupNotFound", nil
	case strings.HasPrefix(cmd, "group create"):
		return 0, "", "", nil
	case strings.HasPrefix(cmd, "vm show"):
		name := argAfter(args, "--name")
		if !f.created[name] {
			return 3, "", "ResourceNotFound", nil
		}
		if contains(args, "powerState") {
			return 0, "VM running\n", "", nil
		}
		if contains(args, "publicIps") {
			return 0, "203.0.113.5\n", "", nil
		}
		return 0, "", "", nil
	case strings.HasPrefix(cmd, "vm create"):
		f.created[argAfter(args, "--name")] = true
		return 0, `{"publicIpAddress": "203.0.113.5", "powerState": "VM running"}`, "", nil
	case strings.HasPrefix(cmd, "vm run-command invoke"):
		return 0, "Enable succeeded: \n[stdout]\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE root@vm\n[stderr]\n", "", nil
	case strings.HasPrefix(cmd, "vm delete"):
		name := argAfter(args, "--name")
		delete(f.created, name)
		f.deleted = append(f.deleted, name)
		return 0, "", "", nil
	case strings.HasPrefix(cmd, "network"):
		return 3, "", "ResourceNotFound", nil
	}
	return 1, "", "unexpected az " + cmd, nil
}

func argAfter(args []string, flag string) string {
	for i, a := range args {
		if a == flag && i+1 < len(args) {
			return args[i+1]
		}
	}
	return ""
}

func (f *fakeAz) has(prefix string) bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, c := range f.calls {
		if strings.HasPrefix(strings.Join(c, " "), prefix) {
			return true
		}
	}
	return false
}

func TestDaemonOnceComputeApproveToReadyToReleased(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	hubHome := t.TempDir()
	if err := os.MkdirAll(filepath.Join(hubHome, ".ssh"), 0o700); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(filepath.Join(hubHome, ".ssh", "pulse_hands_ed25519"), []byte("fake private key\n"), 0o600)
	os.WriteFile(filepath.Join(hubHome, ".ssh", "pulse_hands_ed25519.pub"), []byte("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUBHUB pulse-hub\n"), 0o644)
	for k, v := range map[string]string{"compute.provider": "azure", "compute.max_vms": "2", "compute.max_hours_per_day": "24",
		"compute.azure.resource_group": "pulse-hands", "compute.azure.location": "westeurope", "compute.azure.vm_size": "Standard_B2ms", "minutes_per_job": "1"} {
		setSetting(t, s, k, v)
	}
	az := &fakeAz{created: map[string]bool{}}
	oldAz, oldSSH, oldPair := runAz, runSSH, pairHands
	t.Cleanup(func() { runAz, runSSH, pairHands = oldAz, oldSSH, oldPair })
	runAz = az.run
	// ADR-034: with an executor token on the hub the machine is born logged in.
	writeExecutorToken(t, s)
	placed := stubPlaceLogin(t, true)
	var sshCalls []string
	var sshMu sync.Mutex
	runSSH = func(_ context.Context, argv []string, stdin []byte, _ time.Duration) (int, string, string, error) {
		sshMu.Lock()
		sshCalls = append(sshCalls, strings.Join(argv, " "))
		sshMu.Unlock()
		return 0, "pulse-ready\n", "", nil
	}
	var paired struct {
		sync.Mutex
		admin, name string
		opts        pairOpts
	}
	pairHands = func(st *store.Store, admin, name, now string, o pairOpts) (*pairReport, error) {
		paired.Lock()
		defer paired.Unlock()
		paired.admin, paired.name, paired.opts = admin, name, o
		if _, err := addExecutor(st, now, name, admin, o.Workdir, o.Workers, o.Projects, humanAgent); err != nil {
			return nil, err
		}
		exec1(st.DB, "UPDATE executors SET status = 'active' WHERE name = ?", name) // the real pairing probes it active
		return &pairReport{OK: true, Lines: []string{"paired " + name}}, nil
	}

	req, err := tasks.RequestCompute(s, "proj", "aurora", "", 2, "the fixtures take an hour here")
	if err != nil {
		t.Fatal(err)
	}
	if req.Status != "pending" || req.ProposalPost == "" {
		t.Fatalf("request: %+v", req)
	}
	if _, err := tasks.Approve(s, req.ID, "go ahead", humanAgent); err != nil {
		t.Fatal(err)
	}
	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'", HubHome: hubHome})

	got, err := tasks.GetCompute(s, req.ID)
	if err != nil || got.Status != "ready" || got.Executor != "hands-cr-001" {
		t.Fatalf("after provisioning: %+v %v\n%s", got, err, log)
	}
	for _, action := range []string{"compute.provisioning", "compute.ready"} {
		if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = ?", action, req.ID) != 1 {
			t.Fatalf("%s not logged once\n%s", action, log)
		}
	}
	if !az.has("vm create --resource-group pulse-hands --name pulse-cr-001 --location westeurope --image Ubuntu2404 --size Standard_B2ms --admin-username pulseadmin") {
		t.Fatalf("az calls: %v", az.calls)
	}
	if !az.has("group create") || !az.has("vm run-command invoke") {
		t.Fatalf("az calls: %v", az.calls)
	}
	create := ""
	for _, c := range az.calls {
		if strings.HasPrefix(strings.Join(c, " "), "vm create") {
			create = strings.Join(c, " ")
		}
	}
	if !strings.Contains(create, "--tags pulse=hands pulse-request=CR-001 pulse-project=proj") || !strings.Contains(create, "--custom-data ") {
		t.Fatalf("vm create: %s", create)
	}
	paired.Lock()
	if paired.name != "hands-cr-001" || paired.admin != "pulseadmin@pulse-cr-001-admin" || paired.opts.Projects != "proj" || !paired.opts.InstallCopilot || paired.opts.Workdir != azureWorkdir {
		t.Fatalf("pairing: %s %s %+v", paired.admin, paired.name, paired.opts)
	}
	paired.Unlock()
	ex := executorRow(s, "hands-cr-001")
	if ex == nil || ex.str("status") != "active" || ex.str("admin") != "pulseadmin@203.0.113.5" {
		t.Fatalf("executor: %v", ex)
	}
	if calls := placed.calls(); len(calls) != 1 || calls[0].name != "hands-cr-001" || calls[0].admin != "pulseadmin@203.0.113.5" || calls[0].token != testExecutorToken {
		t.Fatalf("login placement: %+v", calls)
	}
	known, _ := os.ReadFile(filepath.Join(hubHome, ".ssh", "known_hosts"))
	if !strings.Contains(string(known), "203.0.113.5") {
		t.Fatalf("known_hosts: %q", known)
	}
	sshCfg, _ := os.ReadFile(filepath.Join(hubHome, ".ssh", "config"))
	if !strings.Contains(string(sshCfg), "Host pulse-cr-001-admin") || !strings.Contains(string(sshCfg), "HostName 203.0.113.5") {
		t.Fatalf("ssh config: %q", sshCfg)
	}
	m, mrow := loadMachine(s, req.ID)
	if m == nil || m.Name != "pulse-cr-001" || m.Tags["pulse"] != "hands" || m.Tags["pulse-request"] != req.ID || mrow.str("released_at") != "" {
		t.Fatalf("machine memory: %+v %v", m, mrow)
	}
	findings := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'compute'")
	if len(findings) != 1 || !strings.Contains(findings[0].str("summary"), "ready: executor hands-cr-001") || findings[0].str("on_post") != req.ProposalPost {
		t.Fatalf("findings: %v", findings)
	}
	if len(sshCalls) == 0 || !strings.Contains(sshCalls[0], "cloud-init status --wait") {
		t.Fatalf("ssh: %v", sshCalls)
	}

	// Nothing is due while the machine is fresh.
	log2 := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'", HubHome: hubHome})
	if got, _ := tasks.GetCompute(s, req.ID); got.Status != "ready" {
		t.Fatalf("released too early: %+v\n%s", got, log2)
	}
	// Three hours later the two hours are over: release, executor removed, request released.
	later, err := store.Open(store.Options{Home: s.Home, Now: "2026-09-06T13:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	defer later.Close()
	log3 := onceDaemon(t, later, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'", HubHome: hubHome})
	got, _ = tasks.GetCompute(later, req.ID)
	if got.Status != "released" {
		t.Fatalf("after the hours: %+v\n%s", got, log3)
	}
	if len(az.deleted) != 1 || az.deleted[0] != "pulse-cr-001" {
		t.Fatalf("deleted: %v", az.deleted)
	}
	if executorRow(later, "hands-cr-001") != nil {
		t.Fatal("executor still registered")
	}
	if _, mrow := loadMachine(later, req.ID); mrow.str("released_at") == "" {
		t.Fatalf("machine not marked released: %v", mrow)
	}
	if n := count(t, later, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = 'compute' AND summary LIKE '%released%'"); n != 1 {
		t.Fatalf("release finding: %d\n%s", n, log3)
	}
	if !strings.Contains(log3, "hour(s) are up") {
		t.Fatalf("log:\n%s", log3)
	}
	if exists := invokeOK(t, s, "compute", "", "providers"); !strings.Contains(exists, "azure") {
		t.Fatalf("pulse compute providers: %q", exists)
	}
}

func invokeOK(t *testing.T, s *store.Store, name, project string, args ...string) string {
	t.Helper()
	out, errText, code := invoke(t, s, name, project, args...)
	if code != 0 {
		t.Fatalf("%s %v: %d %s", name, args, code, errText)
	}
	return out
}

func TestComputeFailsOverCapsAndRedacts(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	for k, v := range map[string]string{"compute.provider": "azure", "compute.max_vms": "1", "compute.max_hours_per_day": "1",
		"compute.azure.resource_group": "rg", "compute.azure.location": "westeurope", "compute.azure.vm_size": "Standard_B2ms"} {
		setSetting(t, s, k, v)
	}
	req, err := tasks.RequestCompute(s, "proj", "aurora", "", 4, "long run")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := tasks.Approve(s, req.ID, "fine", humanAgent); err != nil {
		t.Fatal(err)
	}
	old := runAz
	t.Cleanup(func() { runAz = old })
	runAz = func(context.Context, []string, time.Duration) (int, string, string, error) {
		t.Fatal("az was called although the caps refuse")
		return 1, "", "", nil
	}
	w := newComputeWorker(context.Background(), s, computeOpts{})
	w.tick(context.Background(), testNow)
	w.wait(0)
	got, _ := tasks.GetCompute(s, req.ID)
	if got.Status != "failed" {
		t.Fatalf("over the caps: %+v", got)
	}
	f := mustOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'")
	if f == nil || !strings.Contains(f.str("summary"), "cap:") {
		t.Fatalf("finding: %v", f)
	}
	if got := redactSecrets("token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 leaked"); strings.Contains(got, "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789") {
		t.Fatalf("redact: %q", got)
	}
	if names := ProviderNames(); !contains(names, "azure") {
		t.Fatalf("providers: %v", names)
	}
}

// -- experiment landing (D-M) ------------------------------------------------------------------------------

type fakeEvolve struct {
	scores   map[string]float64
	measured []string
	archived []evolve.ArchiveRow
	err      error
}

func (f *fakeEvolve) install(t *testing.T) {
	t.Helper()
	oldM, oldA := evolve.Measure, evolve.Archive
	t.Cleanup(func() { evolve.Measure, evolve.Archive = oldM, oldA })
	evolve.Measure = func(_ context.Context, _ *store.Store, project, repo, ref string, _ evolve.MeasureOptions) (*evolve.Scores, error) {
		f.measured = append(f.measured, ref)
		if f.err != nil {
			return nil, f.err
		}
		total, ok := f.scores[ref]
		if !ok {
			return nil, errors.New("no score for " + ref)
		}
		return &evolve.Scores{Project: project, Repo: repo, Ref: ref, Total: total, Sandbox: "fake"}, nil
	}
	evolve.Archive = func(s *store.Store, row evolve.ArchiveRow) (string, error) {
		f.archived = append(f.archived, row)
		id := "B-00" + string(rune('1'+len(f.archived)-1))
		kept := 0
		if row.Kept {
			kept = 1
		}
		if _, err := s.DB.Exec("INSERT INTO evolution_archive (id, project, suggestion_post, hypothesis, branch, baseline_score, variant_score, tolerance, kept, sandbox, run_id, at, note) "+
			"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)", id, row.Project, row.SuggestionPost, row.Hypothesis, row.Branch, row.Baseline.Total, row.Variant.Total,
			row.Tolerance, kept, row.Sandbox, row.RunID, s.Now(), row.Note); err != nil {
			return "", err
		}
		return id, nil
	}
}

// experimentJob queues an experiment for a suggestion and runs it through `pulse work` with a commit.
func experimentJob(t *testing.T, s *store.Store, dir, file string) (job row, suggestion string) {
	t.Helper()
	sug, err := createPost(s, testNow, postOpts{Kind: "suggestion", Project: "proj", Area: "evolution", Thread: "speedup-" + file,
		Summary: "Try " + file, Body: "A hypothesis about " + file, Agent: actorAgent(s, "aurora")})
	if err != nil {
		t.Fatal(err)
	}
	brief := "Experiment card\nSuggestion: " + sug.str("id") + "\nHypothesis: " + file + " makes it faster\nBranch: evolve/" + sug.str("id") + "\n"
	j, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "experiment", Title: "Experiment: " + file, Brief: brief,
		Source: "suggestion:" + sug.str("id"), Tier: 1, Priority: 30, CreatedBy: "aurora", Branch: "evolve/" + sug.str("id")})
	if err != nil {
		t.Fatal(err)
	}
	out, errText, code := invoke(t, s, "work", "proj", "--job", j.ID, "--worker-cmd",
		"sh -c 'echo "+file+" > "+file+" && git add "+file+" && git commit -q -m \"try "+file+"\" && echo SUMMARY: tried'")
	if code != 0 || !strings.HasPrefix(out, "done ") || !strings.Contains(out, "branch evolve/"+sug.str("id")) {
		t.Fatalf("work: %d %q %q", code, out, errText)
	}
	if !strings.Contains(run(t, dir, "git", "branch", "--list", "evolve/*"), "evolve/"+sug.str("id")) {
		t.Fatalf("no evolve branch: %s", run(t, dir, "git", "branch", "-a"))
	}
	return jobRow(s.DB, j.ID), sug.str("id")
}

func TestExperimentLandingKeptDroppedUnavailable(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	proj, dir := newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")
	setSetting(t, s, "evolve.tolerance", "0.05")
	setSetting(t, s, "jobs_per_day", "20")
	fake := &fakeEvolve{scores: map[string]float64{"main": 0.50}}
	fake.install(t)

	// Unavailable: nothing lands, the job keeps its branch, one post says so - and only once.
	kept, keptSug := experimentJob(t, s, dir, "fast.txt")
	fake.err = evolve.ErrNotImplemented
	res := landPending(s, proj, "daemon", testNow, true, 0, false)
	if len(res) != 1 || res[0].State != "unmeasured" {
		t.Fatalf("unavailable: %+v", res)
	}
	if j := jobRow(s.DB, kept.str("id")); j.str("branch") != "evolve/"+keptSug || j.str("status") != "done" {
		t.Fatalf("job after unavailable: %v", j)
	}
	landPending(s, proj, "daemon", testNow, true, 0, false)
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result' AND area = 'jobs' AND thread = ? AND summary LIKE '%measurement unavailable%'", slugify(kept.str("id"), 40)); n != 1 {
		t.Fatalf("unavailable posts: %d", n)
	}
	if !strings.Contains(run(t, dir, "git", "branch", "--list", "evolve/*"), keptSug) {
		t.Fatal("branch deleted while unmeasured")
	}

	// Kept: the variant beats main by more than the tolerance -> normal landing + tested.
	fake.err = nil
	fake.scores["evolve/"+keptSug] = 0.80
	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: nothing'"})
	if !strings.Contains(log, "landing landed: evolve/"+keptSug+" kept") {
		t.Fatalf("daemon log:\n%s", log)
	}
	if !fileExists(filepath.Join(dir, "fast.txt")) {
		t.Fatal("kept experiment not on main")
	}
	if j := jobRow(s.DB, kept.str("id")); j.str("branch") != "" || !strings.Contains(j.str("result"), "landed") {
		t.Fatalf("kept job: %v", j)
	}
	tested := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'result' AND resolution = 'tested' AND thread = ?", "speedup-fast-txt")
	if len(tested) != 1 || tested[0].str("on_post") != keptSug {
		t.Fatalf("tested post: %v", tested)
	}
	if len(fake.archived) != 1 || !fake.archived[0].Kept || fake.archived[0].SuggestionPost != keptSug || fake.archived[0].Tolerance != 0.05 ||
		fake.archived[0].Hypothesis != "fast.txt makes it faster" || fake.archived[0].RunID == "" {
		t.Fatalf("archive: %+v", fake.archived)
	}
	if fake.archived[0].RunID != mustOne(s.DB, "SELECT id FROM runs WHERE job_id = ?", kept.str("id")).str("id") {
		t.Fatalf("archive run id %s", fake.archived[0].RunID)
	}

	// Dropped: within the tolerance -> branch deleted, dropped post, nothing on main.
	dropped, dropSug := experimentJob(t, s, dir, "slow.txt")
	fake.scores["evolve/"+dropSug] = 0.52
	res = landPending(s, proj, "daemon", testNow, true, 0, false)
	if len(res) != 1 || res[0].State != "dropped" {
		t.Fatalf("dropped: %+v", res)
	}
	if fileExists(filepath.Join(dir, "slow.txt")) || strings.Contains(run(t, dir, "git", "branch", "--list", "evolve/*"), dropSug) {
		t.Fatal("dropped experiment landed or kept its branch")
	}
	if j := jobRow(s.DB, dropped.str("id")); j.str("branch") != "" || !strings.Contains(j.str("result"), "dropped") {
		t.Fatalf("dropped job: %v", j)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result' AND resolution = 'dropped' AND thread = 'speedup-slow-txt' AND on_post = ?", dropSug); n != 1 {
		t.Fatalf("dropped post: %d", n)
	}
	if len(fake.archived) != 2 || fake.archived[1].Kept {
		t.Fatalf("archive: %+v", fake.archived)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.dropped'"); n != 1 {
		t.Fatalf("land.dropped events: %d", n)
	}
	for _, action := range []string{"land.landed", "land.dropped", "land.unmeasured"} {
		if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", action) == 0 {
			t.Fatalf("no %s event", action)
		}
	}
}

func TestBriefFieldsAndTolerance(t *testing.T) {
	f := briefFields("Experiment card\nSuggestion: `S-1`\n- Hypothesis: it helps\nBranch: evolve/S-1\nBranch: twice\n")
	if f["suggestion"] != "S-1" || f["hypothesis"] != "it helps" || f["branch"] != "" {
		t.Fatalf("fields: %v", f)
	}
	s := newStore(t)
	if got := evolveTolerance(s); got != defaultEvolveTolerance {
		t.Fatalf("default tolerance %v", got)
	}
	setSetting(t, s, "evolve.tolerance", "0.1")
	if got := evolveTolerance(s); got != 0.1 {
		t.Fatalf("tolerance %v", got)
	}
}

// -- the daemon's goroutine set (D-E) ----------------------------------------------------------------------

func TestDaemonStopSwitchCancelsAndBridgesEvents(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "minutes_per_job", "1")
	// A hook fires on job.add through the daemon's hooks runner: proof that the bridge feeds in-process subscribers.
	scratch := t.TempDir()
	marker := filepath.Join(scratch, "fired")
	script := filepath.Join(scratch, "hook.sh")
	if err := os.WriteFile(script, []byte("#!/bin/sh\ncat > "+marker+"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := hooks.Add(s, "job.add", "script", script, "proj", humanAgent); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var out bytes.Buffer
	done := make(chan string, 1)
	go func() {
		done <- daemon(ctx, s, daemonOpts{Interval: 30, Grace: 5 * time.Second, WorkerCmd: "sh -c 'echo SUMMARY: ok'", Out: &out})
	}()
	// Another "process" queues a job: the daemon wakes before its 30s interval and runs it.
	time.Sleep(300 * time.Millisecond)
	other, err := store.Open(store.Options{Home: s.Home, Now: testNow})
	if err != nil {
		t.Fatal(err)
	}
	defer other.Close()
	if _, _, err := addJob(other, "proj", "task", "wake up", "echo", 1, 0, "test", humanAgent, false); err != nil {
		t.Fatal(err)
	}
	woke := func() bool {
		return count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'done' AND title = 'wake up'") > 0
	}
	deadline := time.Now().Add(20 * time.Second)
	for time.Now().Before(deadline) && !woke() {
		time.Sleep(50 * time.Millisecond)
	}
	if !woke() {
		cancel()
		t.Fatalf("the daemon did not wake on job.add:\n%s", out.String())
	}
	// The stop switch flips from another process: the loop ends as "stopped" without a signal.
	if err := other.SetSetting(humanAgent, "stop", "1"); err != nil {
		t.Fatal(err)
	}
	select {
	case result := <-done:
		if result != "stopped" {
			t.Fatalf("daemon = %s\n%s", result, out.String())
		}
	case <-time.After(20 * time.Second):
		cancel()
		t.Fatalf("the daemon did not stop on the stop switch:\n%s", out.String())
	}
	deadline = time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) && !fileExists(marker) {
		time.Sleep(50 * time.Millisecond)
	}
	if !fileExists(marker) {
		t.Fatalf("the job.add hook never fired:\n%s", out.String())
	}
	var beat map[string]any
	if r := mustOne(s.DB, "SELECT value FROM meta WHERE key = 'daemon'"); r == nil || json.Unmarshal([]byte(r.str("value")), &beat) != nil || beat["note"] != "exited" {
		t.Fatalf("heartbeat: %v", r)
	}
}

// D-J: one tick plans every active project before it leases anything.
func TestDaemonOncePlansEveryActiveProject(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	other := filepath.Join(t.TempDir(), "other")
	if err := os.MkdirAll(other, 0o755); err != nil {
		t.Fatal(err)
	}
	run(t, other, "git", "init", "-q", "-b", "main")
	run(t, other, "git", "config", "commit.gpgsign", "false")
	os.WriteFile(filepath.Join(other, "README.md"), []byte("# other\n"), 0o644)
	run(t, other, "git", "add", "-A")
	run(t, other, "git", "commit", "-q", "-m", "init")
	if _, _, err := addProject(s, other, "Other", "", false, "", humanAgent, "other"); err != nil {
		t.Fatal(err)
	}
	join(t, s, "aurora")
	setSetting(t, s, "jobs_per_day", "1") // one lease per tick: planning must still reach both projects
	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: ok'"})
	for _, slug := range []string{"proj", "other"} {
		if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND kind = 'reflect' AND created_by = 'daemon'", slug); n != 1 {
			t.Fatalf("%s planned %d reflect job(s)\n%s", slug, n, log)
		}
		if !strings.Contains(log, "planned reflect job-") {
			t.Fatalf("planner silent:\n%s", log)
		}
	}
	// Both were planned before the single lease of the tick.
	planned := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = 'reflect' ORDER BY created_at, id")
	leased := mustRows(s.DB, "SELECT * FROM runs")
	if len(planned) != 2 || len(leased) != 1 {
		t.Fatalf("planned %d leased %d\n%s", len(planned), len(leased), log)
	}
	for _, j := range planned {
		if j.str("created_at") > leased[0].str("started_at") {
			t.Fatalf("job %s planned after the lease at %s\n%s", j.str("id"), leased[0].str("started_at"), log)
		}
	}
}
