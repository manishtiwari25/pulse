package board

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/parity"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestDecideYesQueuesJob(t *testing.T) {
	noTTY(t)
	home, s, _ := fixture(t)
	sug, _, _ := seedPosts(t, s)
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	pyHome := cloneHome(t, home)
	s = openHome(t, home)

	out, err := runCmd(t, s, &cli.Context{}, "decide", sug.ID, "yes", "--words", "do it")
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(out), "\n")
	if !strings.HasPrefix(lines[0], "recorded yes as "+stampOf(testNow)+"-human") {
		t.Fatalf("out %q", out)
	}
	pyOut, pyErr, pyCode := oracle(t, pyHome, nil, "decide", sug.ID, "yes", "--words", "do it")
	if pyCode != 0 || !strings.HasPrefix(pyOut, "recorded yes as ") {
		t.Fatalf("oracle: %s %s", pyOut, pyErr)
	}
	pyStore, err := store.Open(store.Options{Home: pyHome, SkipRecords: true})
	if err != nil {
		t.Fatal(err)
	}
	defer pyStore.Close()
	for _, q := range []string{
		"SELECT kind, project, area, thread, agent, summary, body, on_post, resolution, session, branch, name FROM posts WHERE kind = 'result'",
		"SELECT post_id, project, answer, words, decided_by FROM decisions",
		"SELECT project, kind, tier, title, source, priority, status, created_by FROM jobs WHERE source LIKE 'decide:%'",
		"SELECT action, actor, substr(subject, 1, 4) FROM events WHERE action IN ('decide', 'decide.job', 'job.add') ORDER BY seq",
	} {
		if g, p := dumpQuery(t, s, q), dumpQuery(t, pyStore, q); g != p {
			t.Errorf("rows differ for %s\n go: %s\n py: %s", q, g, p)
		}
	}
	// the decision, the result post and the queued job
	d := countRows(t, s, "decisions", "post_id = ?", sug.ID)
	if d != 1 {
		t.Fatalf("%d decisions", d)
	}
	var brief string
	if err := s.DB.QueryRow("SELECT brief FROM jobs WHERE source = ?", "decide:"+sug.ID).Scan(&brief); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(brief, "with the words 'do it'") || !strings.Contains(brief, "PROPOSAL: Cache the build") {
		t.Fatalf("brief %q", brief)
	}
	// twice is refused, in both tools with the same words
	_, err = runCmd(t, s, &cli.Context{}, "decide", sug.ID, "no")
	want := "Post " + sug.ID + " was already decided; a new proposal needs a new post"
	if what(err) != want || code(err) != 2 {
		t.Fatalf("again: %v", err)
	}
	_, pyErr, pyCode = oracle(t, pyHome, nil, "decide", sug.ID, "no")
	if oracleMessage(pyErr) != want || pyCode != 2 {
		t.Fatalf("oracle again: %q %d", pyErr, pyCode)
	}
	// an agent may not decide
	_, err = runCmd(t, s, &cli.Context{Agent: "alice"}, "decide", sug.ID, "no")
	if code(err) != 2 || !strings.Contains(what(err), "human's call") {
		t.Fatalf("agent decide: %v", err)
	}
	// the decision job is reused, not duplicated
	job, err := DecisionJob(s, sug, &Post{ID: "x"}, "", "human")
	if err != nil || job == nil {
		t.Fatalf("decision job: %v %v", job, err)
	}
	if n := countRows(t, s, "jobs", "source = ?", "decide:"+sug.ID); n != 1 {
		t.Fatalf("%d decision jobs", n)
	}
}

func TestDecideNoAndMarkers(t *testing.T) {
	_, s, _ := fixture(t)
	sug, _, _ := seedPosts(t, s)
	d, err := Decide(s, sug.ID, "no", "", "")
	if err != nil {
		t.Fatal(err)
	}
	if d.Result.Resolution != "declined" || d.Result.Agent != "human" || d.Job != nil || d.DecidedBy != "human" {
		t.Fatalf("no: %+v", d)
	}
	if !strings.HasPrefix(d.Result.Summary, "declined: Cache the build") || !strings.Contains(d.Result.Body, "(no words recorded)") {
		t.Fatalf("result %+v", d.Result)
	}
	if _, err := Decide(s, sug.ID, "maybe", "", ""); what(err) != "The answer is yes or no" {
		t.Fatalf("answer: %v", err)
	}
	if _, err := Decide(s, "nope", "yes", "", ""); what(err) != "No post 'nope'" {
		t.Fatalf("missing: %v", err)
	}
	// a proposal that authorizes a command queues no job
	prop, err := CreatePost(s, PostInput{Kind: "suggestion", Project: "demo", Area: "hands", Thread: "vm", Summary: "pair the vm", Body: "pair: hands-2\n", Agent: "alice", Date: "2026-09-06T09:10:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	d, err = Decide(s, prop.ID, "yes", "go", "")
	if err != nil || d.Job != nil {
		t.Fatalf("marker yes: %+v %v", d, err)
	}
	auth, err := AuthorizingDecision(s, "pair: hands-2", "", 0)
	if err != nil || auth == nil || !auth.Valid || auth.PostID != prop.ID {
		t.Fatalf("authorizing: %+v %v", auth, err)
	}
	if id, err := AgentGate(s, "pair: hands-2", "alice", ""); err != nil || id != prop.ID {
		t.Fatalf("gate: %s %v", id, err)
	}
	if _, err := AgentGate(s, "pair: other", "alice", ""); code(err) != 2 {
		t.Fatalf("gate other: %v", err)
	}
	if _, err := AgentGate(s, "pair: hands-2", "alice", "2026-09-20T10:00:00Z"); code(err) != 2 || !strings.Contains(what(err), "does not authorize") {
		t.Fatalf("gate old: %v", err)
	}
	if id, err := AgentGate(s, "pair: other", "", ""); err != nil || id != "" {
		t.Fatalf("human gate: %v", err)
	}
	// a commons proposal queues no job
	cp, err := CreatePost(s, PostInput{Kind: "suggestion", Project: store.CommonsSlug, Area: "a", Thread: "b", Summary: "shared idea", Agent: "alice", Date: "2026-09-06T09:20:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if d, err := Decide(s, cp.ID, "yes", "", ""); err != nil || d.Job != nil {
		t.Fatalf("commons yes: %+v %v", d, err)
	}
}

func TestOwnerAwayLeadDecideGuards(t *testing.T) {
	_, s, _ := fixture(t)
	if _, _, err := JoinAgent(s, JoinInput{Name: LeadName("demo"), Kind: "lead", Runner: "copilot", Model: "m", Session: "sess-lead"}); err != nil {
		t.Fatal(err)
	}
	if _, _, err := JoinAgent(s, JoinInput{Name: LeadName("other"), Kind: "lead", Runner: "copilot", Model: "m", Session: "sess-other-lead"}); err != nil {
		t.Fatal(err)
	}
	sug, _, _ := seedPosts(t, s)
	if _, err := Decide(s, sug.ID, "yes", "", LeadName("demo")); what(err) != "pulse decide is the human's call; a lead decides only while owner.presence is away" {
		t.Fatalf("present lead decide: %v", err)
	}
	if err := s.SetSetting("human", "owner.presence", "away"); err != nil {
		t.Fatal(err)
	}
	if _, err := Decide(s, sug.ID, "yes", "", "alice"); what(err) != "only a project's lead decides for the owner" {
		t.Fatalf("non-lead decide: %v", err)
	}
	if _, err := Decide(s, sug.ID, "yes", "", LeadName("other")); what(err) != "only the proposal project's lead decides for the owner" {
		t.Fatalf("wrong lead decide: %v", err)
	}

	d, err := Decide(s, sug.ID, "yes", "ship it", LeadName("demo"))
	if err != nil {
		t.Fatal(err)
	}
	if d.Result.Agent != LeadName("demo") || d.DecidedBy != LeadName("demo") {
		t.Fatalf("lead result/decision: %+v", d)
	}
	wantBody := "The owner is away (owner.presence = away); **demo-lead** answered **yes** on the owner's behalf.\n\nship it\n\nProposal: " + sug.ID + "\n"
	if d.Result.Body != wantBody {
		t.Fatalf("result body:\n%s\nwant:\n%s", d.Result.Body, wantBody)
	}
	var decidedBy string
	if err := s.DB.QueryRow("SELECT decided_by FROM decisions WHERE post_id = ?", sug.ID).Scan(&decidedBy); err != nil || decidedBy != LeadName("demo") {
		t.Fatalf("decided_by = %q %v", decidedBy, err)
	}
	if d.Job == nil || !strings.HasPrefix(d.Job.Title, "Act on the lead's yes (owner away): ") ||
		!strings.Contains(d.Job.Brief, "demo-lead, deciding for the owner who is away (owner.presence), answered YES") ||
		!strings.Contains(d.Job.Brief, "the recorded decision (result "+d.Result.ID+") is the approval for what it proposes, nothing more") {
		t.Fatalf("lead job: %+v", d.Job)
	}
	var detail string
	if err := s.DB.QueryRow("SELECT detail FROM events WHERE action = 'decide' AND subject = ?", sug.ID).Scan(&detail); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(detail, `"on_behalf": "owner-away"`) {
		t.Fatalf("event detail %q", detail)
	}
}

func TestOwnerAwayLeadDecideStillWaitsForOwner(t *testing.T) {
	_, s, _ := fixture(t)
	if _, _, err := JoinAgent(s, JoinInput{Name: LeadName("demo"), Kind: "lead", Runner: "copilot", Model: "m", Session: "sess-lead"}); err != nil {
		t.Fatal(err)
	}
	if err := s.SetSetting("human", "owner.presence", "away"); err != nil {
		t.Fatal(err)
	}
	compute, err := CreatePost(s, PostInput{Kind: "suggestion", Project: "demo", Area: "compute", Thread: "vm", Summary: "Rent a vm", Body: "need capacity", Agent: "alice", Date: "2026-09-06T09:30:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("INSERT INTO compute_requests (id, project, requested_by, size, hours, reason, proposal_post, at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
		"CR-001", "demo", "alice", "Standard_B2ms", 1, "need capacity", compute.ID, s.Now(), s.Now()); err != nil {
		t.Fatal(err)
	}
	if _, err := Decide(s, compute.ID, "yes", "", LeadName("demo")); what(err) != "compute is money: it waits for the owner; compute.auto_approve and the caps pre-authorize it" {
		t.Fatalf("compute decide: %v", err)
	}

	marker, err := CreatePost(s, PostInput{Kind: "suggestion", Project: "demo", Area: "hands", Thread: "vm", Summary: "pair the vm", Body: "pair: x\n", Agent: "alice", Date: "2026-09-06T09:31:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Decide(s, marker.ID, "yes", "", LeadName("demo")); what(err) != "a proposal that authorizes a command waits for the human (ADR-021)" {
		t.Fatalf("marker decide: %v", err)
	}
	if auth, err := AuthorizingDecision(s, "pair: x", "", 0); err != nil || auth != nil {
		t.Fatalf("lead marker should not authorize: %+v %v", auth, err)
	}

	// Tier 2 is the owner's: a proposal whose summary names an ADR, or that points at an owner-only path.
	adr, err := CreatePost(s, PostInput{Kind: "suggestion", Project: "demo", Area: "evolution", Thread: "adr", Summary: "Accept ADR-023 and start its plan", Body: "the record is ready", Agent: "alice", Date: "2026-09-06T09:32:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Decide(s, adr.ID, "no", "not now", LeadName("demo")); what(err) != "a proposal that names ADR-023 is a Tier 2 decision: it waits for the owner" {
		t.Fatalf("adr decide: %v", err)
	}
	paths, err := CreatePost(s, PostInput{Kind: "suggestion", Project: "demo", Area: "evolution", Thread: "ci", Summary: "Speed up CI", Body: "Edit .github/workflows/ci.yml to cache modules", Agent: "alice", Date: "2026-09-06T09:33:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Decide(s, paths.ID, "yes", "cheap", LeadName("demo")); what(err) != "a proposal that points at .github/ is a Tier 2 decision: it waits for the owner" {
		t.Fatalf("owner path decide: %v", err)
	}
	if n := countRows(t, s, "decisions", ""); n != 0 {
		t.Fatalf("decisions recorded = %d", n)
	}
	// The human decides all of them.
	if _, err := Decide(s, adr.ID, "yes", "go", ""); err != nil {
		t.Fatalf("human adr decide: %v", err)
	}

	for _, tt := range []struct{ summary, body, want string }{
		{"Retry per ADR-036", "cites the decision", "names ADR-036"},
		{"Retry with backoff", "as ADR-036 decided; edit internal/runner", ""},
		{"Tidy docs", "touch docs/decisions/ADR-001.md", "points at docs/decisions/"},
		{"Kill switch", "create docs/evolution/STOP", "points at docs/evolution/STOP"},
		{"Small fix", "internal/tally/gate.go", ""},
	} {
		if got := Tier2Proposal(tt.summary, tt.body); got != tt.want {
			t.Fatalf("Tier2Proposal(%q, %q) = %q, want %q", tt.summary, tt.body, got, tt.want)
		}
	}
}

func TestLeadYesJobBriefNeverAcceptsAnADR(t *testing.T) {
	_, s, _ := fixture(t)
	if _, _, err := JoinAgent(s, JoinInput{Name: LeadName("demo"), Kind: "lead", Runner: "copilot", Model: "m", Session: "sess-lead"}); err != nil {
		t.Fatal(err)
	}
	if err := s.SetSetting("human:dashboard", "owner.presence", "away"); err != nil {
		t.Fatal(err)
	}
	sug, _, _ := seedPosts(t, s)
	d, err := Decide(s, sug.ID, "yes", "small and reversible", LeadName("demo"))
	if err != nil {
		t.Fatal(err)
	}
	if d.Job == nil || strings.Contains(d.Job.Brief, "If it accepts an ADR") || !strings.Contains(d.Job.Brief, "never accepts, changes or supersedes an ADR") ||
		!strings.Contains(d.Job.Brief, "Tier 2 waits for the owner") {
		t.Fatalf("lead yes brief: %+v", d.Job)
	}
	// An agent-written `away` grants nothing, even to the right lead.
	if err := s.SetSetting(LeadName("demo"), "owner.presence", "away"); err != nil {
		t.Fatal(err)
	}
	other, err := CreatePost(s, PostInput{Kind: "suggestion", Project: "demo", Area: "evolution", Thread: "b", Summary: "Another small thing", Body: "Signal: README.md", Agent: "alice", Date: "2026-09-06T09:40:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Decide(s, other.ID, "yes", "", LeadName("demo")); what(err) != "pulse decide is the human's call; a lead decides only while owner.presence is away" {
		t.Fatalf("agent-written away: %v", err)
	}
}

func TestJobsAddFinish(t *testing.T) {
	noTTY(t)
	_, s, _ := fixture(t)
	out, err := runCmd(t, s, &cli.Context{Agent: "alice", Project: "demo"}, "job", "add", "--title", "Do a thing", "--source", "task:1", "--priority", "5")
	if err != nil {
		t.Fatal(err)
	}
	id := strings.TrimSpace(out)
	if !strings.HasPrefix(id, "job-"+stampOf(testNow)+"-") || len(id) != len("job-20260906T100000Z-abcdef") {
		t.Fatalf("id %q", id)
	}
	out, err = runCmd(t, s, &cli.Context{Agent: "alice", Project: "demo"}, "job", "add", "--title", "Do a thing", "--source", "task:1")
	if err != nil || strings.TrimSpace(out) != "already queued: "+id {
		t.Fatalf("dedupe: %q %v", out, err)
	}
	if _, err := runCmd(t, s, &cli.Context{Project: "demo"}, "job", "add", "--title", "t", "--kind", "party"); what(err) != "Job kind must be one of: "+strings.Join(JobKinds, ", ") {
		t.Fatalf("kind: %v", err)
	}
	if _, err := runCmd(t, s, &cli.Context{Project: "demo"}, "job", "add", "--title", "t", "--tier", "2"); what(err) != "The runner takes Tier 0 and Tier 1 jobs only; Tier 2 needs an ADR and the human" {
		t.Fatalf("tier: %v", err)
	}
	if _, _, err := AddJob(s, JobInput{Project: "zzz", Kind: "task", Title: "t"}); what(err) != "No project 'zzz'" {
		t.Fatalf("project: %v", err)
	}
	// an experiment names its branch up front (evolve cycles)
	exp, created, err := AddJob(s, JobInput{Project: "demo", Kind: "experiment", Title: "try it", Source: "suggestion:x", Branch: "evolve/x", CreatedBy: "evolve"})
	if err != nil || !created || exp.Branch != "evolve/x" || exp.Kind != "experiment" {
		t.Fatalf("branch: %+v %v", exp, err)
	}
	// lease it to bob by hand, then alice may not finish it
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'leased', lease_agent = 'bob', lease_until = '2026-09-06T11:00:00Z' WHERE id = ?", id); err != nil {
		t.Fatal(err)
	}
	_, err = runCmd(t, s, &cli.Context{Agent: "alice"}, "job", "finish", id, "done")
	if what(err) != "Job "+id+" is leased by bob, not alice" {
		t.Fatalf("leased: %v", err)
	}
	gen := 3
	if _, err := FinishJob(s, id, "bob", "done", "", "", &gen); !strings.HasPrefix(what(err), "Stale lease:") {
		t.Fatalf("generation: %v", err)
	}
	if _, err := FinishJob(s, id, "bob", "paused", "", "", nil); what(err) != "A job ends done, failed, cancelled, blocked, or back to queued" {
		t.Fatalf("status: %v", err)
	}
	if _, err := FinishJob(s, "nope", "bob", "done", "", "", nil); what(err) != "No job 'nope'" {
		t.Fatalf("missing: %v", err)
	}
	out, err = runCmd(t, s, &cli.Context{Agent: "bob"}, "job", "finish", id, "done", "--result", "SUMMARY: ok", "--branch", "pulse/bob/x")
	if err != nil || strings.TrimSpace(out) != id+" done" {
		t.Fatalf("finish: %q %v", out, err)
	}
	j, _ := JobByID(s, id)
	if j.Status != "done" || j.Result != "SUMMARY: ok" || j.Branch != "pulse/bob/x" || j.FinishedAt != testNow || j.LeaseUntil != "" || j.LeaseAgent != "bob" {
		t.Fatalf("job %+v", j)
	}
	// the human may finish anything; back to queued clears the lease agent
	j, err = FinishJob(s, id, "human", "queued", "", "", nil)
	if err != nil || j.LeaseAgent != "" || j.FinishedAt != "" {
		t.Fatalf("requeue %+v %v", j, err)
	}
	if n := countRows(t, s, "events", "action IN ('job.add', 'job.done', 'job.queued')"); n != 4 {
		t.Fatalf("%d job events", n)
	}
}

func TestAgentsJoinSpawnPrune(t *testing.T) {
	noTTY(t)
	home, s := newHome(t)
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	pyHome := cloneHome(t, home)
	s = openHome(t, home)

	out, err := runCmd(t, s, &cli.Context{Session: "s1"}, "agent", "join", "--name", "Alice", "--runner", "copilot", "--model", "m1")
	if err != nil || strings.TrimSpace(out) != "joined alice (copilot/m1); export PULSE_AGENT=alice" {
		t.Fatalf("join: %q %v", out, err)
	}
	pyOut, _, _ := oracle(t, pyHome, nil, "agent", "join", "--name", "Alice", "--runner", "copilot", "--model", "m1", "--session", "s1")
	if strings.TrimSpace(pyOut) != strings.TrimSpace(out) {
		t.Fatalf("oracle join %q", pyOut)
	}
	out, err = runCmd(t, s, &cli.Context{Session: "s2"}, "agent", "join", "--name", "alice", "--runner", "copilot")
	if err != nil || strings.TrimSpace(out) != "welcome back alice (copilot/m1); export PULSE_AGENT=alice" {
		t.Fatalf("resume: %q %v", out, err)
	}
	a, _ := s.Agent("alice")
	if strings.Join(a.Sessions, ",") != "s1,s2" {
		t.Fatalf("sessions %v", a.Sessions)
	}
	refusals := [][]string{
		{"agent", "join", "--name", "alice", "--runner", "claude"},
		{"agent", "join", "--name", "human"},
		{"agent", "join", "--name", "A"},
		{"agent", "join", "--name", "9lives"},
	}
	for _, args := range refusals {
		_, err := runCmd(t, s, &cli.Context{}, args...)
		_, pyErr, pyCode := oracle(t, pyHome, nil, args...)
		if code(err) != 2 || pyCode != 2 || what(err) != oracleMessage(pyErr) {
			t.Fatalf("%v: go %q / py %q", args, what(err), oracleMessage(pyErr))
		}
	}
	for _, want := range []string{"aurora", "borealis"} {
		out, err = runCmd(t, s, &cli.Context{}, "agent", "spawn", "--runner", "copilot", "--model", "m")
		if err != nil || strings.TrimSpace(out) != want {
			t.Fatalf("spawn: %q %v", out, err)
		}
	}
	if _, err := s.DB.Exec("UPDATE agents SET name = name WHERE 0"); err != nil {
		t.Fatal(err)
	}
	a, _ = s.Agent("aurora")
	if a.Kind != "worker" {
		t.Fatalf("spawned kind %q", a.Kind)
	}
	// list matches the oracle's format on identical rows
	out, err = runCmd(t, s, &cli.Context{}, "agent", "list")
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"aurora", "borealis"} {
		if _, _, c := oracle(t, pyHome, nil, "agent", "spawn", "--runner", "copilot", "--model", "m"); c != 0 {
			t.Fatalf("oracle spawn %s", name)
		}
	}
	pyOut, _, _ = oracle(t, pyHome, nil, "agent", "list")
	strip := func(s string) string {
		var keep []string
		for _, l := range strings.Split(s, "\n") {
			if strings.HasPrefix(l, "human") || l == "" { // the human row's timestamps differ
				continue
			}
			keep = append(keep, strings.TrimRight(l, " "))
		}
		sort.Strings(keep) // the fixed Go clock ties last_active_at; Python's order is time-driven
		return strings.Join(keep, "\n")
	}
	if strip(parity.LegacyAgentListing(out)) != strip(pyOut) {
		t.Fatalf("list differs\n go: %s\n py: %s", out, pyOut)
	}
	// prune: alice has been quiet for 20 days, borealis holds a lease
	if _, err := s.DB.Exec("UPDATE agents SET last_active_at = '2026-08-01T00:00:00Z', last_useful_at = '2026-08-01T00:00:00Z' WHERE name IN ('alice', 'borealis')"); err != nil {
		t.Fatal(err)
	}
	repo := gitRepo(t)
	if _, _, err := RegisterProject(s, repo, "", "", false, "", "human", ""); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("INSERT INTO jobs (id, project, kind, title, status, created_at, lease_agent) VALUES ('job-1', ?, 'task', 't', 'leased', ?, 'borealis')", filepath.Base(repo), testNow); err != nil {
		t.Fatal(err)
	}
	out, err = runCmd(t, s, &cli.Context{Now: testNow}, "agent", "prune", "--dry-run")
	if err != nil || strings.TrimSpace(out) != "would prune alice: no useful output since 2026-08-01T00:00:00Z (limit 14 days)" {
		t.Fatalf("dry prune: %q %v", out, err)
	}
	if a, _ := s.Agent("alice"); a.Status != "active" {
		t.Fatal("dry-run pruned")
	}
	out, err = runCmd(t, s, &cli.Context{Now: testNow}, "agent", "prune", "--days", "30")
	if err != nil || strings.TrimSpace(out) != "pruned alice: no useful output since 2026-08-01T00:00:00Z (limit 30 days)" {
		t.Fatalf("prune: %q %v", out, err)
	}
	if a, _ := s.Agent("alice"); a.Status != "pruned" || a.PrunedAt != testNow {
		t.Fatalf("pruned %+v", a)
	}
	if _, err := ResolveAgent(s, "alice", true); !strings.HasPrefix(what(err), "Agent 'alice' was pruned on "+testNow) {
		t.Fatalf("pruned resolve: %v", err)
	}
	if _, _, err := JoinAgent(s, JoinInput{Name: "alice"}); what(err) != "The name 'alice' belongs to an agent pruned on "+testNow+"; names are never reused" {
		t.Fatalf("rejoin: %v", err)
	}
	// legacy ids resolve to their agent
	if _, err := s.DB.Exec("UPDATE agents SET legacy_id = 'copilot/m' WHERE name = 'aurora'"); err != nil {
		t.Fatal(err)
	}
	if a, err := ResolveAgent(s, "copilot/m", true); err != nil || a.Name != "aurora" {
		t.Fatalf("legacy id: %v %v", a, err)
	}
	if a, err := ResolveAgent(s, "", false); err != nil || a != nil {
		t.Fatalf("optional: %v %v", a, err)
	}
}

func TestInitRegistersProject(t *testing.T) {
	noTTY(t)
	home, s := newHome(t)
	repo := gitRepo(t)
	if err := os.MkdirAll(filepath.Join(repo, "docs", "context"), 0o755); err != nil {
		t.Fatal(err)
	}
	recipe := "---\ntitle: Environment\nareas: [runner, tooling]\n---\n\n# Environment\n\n| tool | pin |\n|---|---|\n| go | go@1.26 |\n"
	if err := os.WriteFile(filepath.Join(repo, "docs", "context", "environment.md"), []byte(recipe), 0o644); err != nil {
		t.Fatal(err)
	}
	out, err := runCmd(t, s, &cli.Context{}, "init", repo, "--name", "My App", "--verify", "go test ./...")
	if err != nil {
		t.Fatal(err)
	}
	repo, _ = filepath.EvalSymlinks(repo)
	lines := strings.Split(strings.TrimSpace(out), "\n")
	home, _ = filepath.EvalSymlinks(home)
	if len(lines) != 4 || lines[0] != "registered project my-app at "+repo || !strings.HasPrefix(lines[1], "queued onboarding job job-") ||
		lines[2] != "imported docs/context/environment.md as CTX-environment-my-app" || lines[3] != "store: "+filepath.Join(home, store.StoreName) {
		t.Fatalf("out:\n%s\nrepo %s home %s", out, repo, home)
	}
	p, _ := s.Project("my-app")
	if p == nil || p.Path != repo || p.VerifyCmd != "go test ./..." || p.DefaultBranch != "main" || p.AddedBy != "human" || p.Status != "active" {
		t.Fatalf("project %+v", p)
	}
	if n := countRows(t, s, "jobs", "project = 'my-app' AND kind = 'onboard' AND status = 'queued' AND tier = 0 AND priority = 20 AND source = 'onboard:my-app'"); n != 1 {
		t.Fatalf("%d onboarding jobs", n)
	}
	rec, rev, err := s.GetRecord("CTX-environment-my-app", 0)
	if err != nil {
		t.Fatal(err)
	}
	if rec.Kind != "context" || rec.Slug != "environment" || rec.Project != "my-app" || rec.Title != "Environment" ||
		strings.Join(rec.Areas, ",") != "runner,tooling" || !strings.HasPrefix(rec.Origin, "markdown:docs/context/environment.md@") || rec.Path != "docs/context/environment.md" {
		t.Fatalf("record %+v", rec)
	}
	if !strings.Contains(rev.Body, "go@1.26") || rev.Frontmatter["title"] != "Environment" {
		t.Fatalf("revision %+v", rev)
	}
	var brief string
	if err := s.DB.QueryRow("SELECT brief FROM jobs WHERE source = 'onboard:my-app'").Scan(&brief); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(brief, "already in the store") {
		t.Fatalf("brief should mention the imported recipe: %s", brief)
	}
	// a second init refreshes, keeps the job, imports nothing twice
	out, err = runCmd(t, s, &cli.Context{}, "init", "--path", repo)
	if err != nil {
		t.Fatal(err)
	}
	lines = strings.Split(strings.TrimSpace(out), "\n")
	if lines[0] != "refreshed project my-app at "+repo || !strings.HasPrefix(lines[1], "already queued onboarding job job-") || len(lines) != 3 {
		t.Fatalf("second init:\n%s", out)
	}
	if n := countRows(t, s, "record_revisions", "record_id = 'CTX-environment-my-app'"); n != 1 {
		t.Fatalf("%d revisions", n)
	}
	if n := countRows(t, s, "events", "action IN ('project.add', 'project.refresh', 'job.add', 'record.imported')"); n != 4 {
		t.Fatalf("%d init events", n)
	}
	// a brain without a recipe: CTX-environment id, brief asks for the recipe
	brain := gitRepo(t)
	res, err := Init(s, brain, "", "", true, "", "human")
	if err != nil {
		t.Fatal(err)
	}
	if !res.Project.Brain || res.Environment != "" || EnvironmentRecordID(res.Project) != "CTX-environment" {
		t.Fatalf("brain %+v", res)
	}
	if !strings.Contains(res.Job.Brief, "pulse env set --project "+res.Project.Slug) {
		t.Fatalf("brain brief %s", res.Job.Brief)
	}
	if b, _ := s.Brain(); b == nil || b.Slug != res.Project.Slug {
		t.Fatal("brain flag")
	}
	// not a git checkout, not a directory
	plain := t.TempDir()
	if _, err := Init(s, plain, "", "", false, "", "human"); code(err) != 2 || !strings.HasPrefix(what(err), "Not a git checkout: ") {
		t.Fatalf("plain dir: %v", err)
	}
	if _, err := Init(s, filepath.Join(plain, "missing"), "", "", false, "", "human"); !strings.HasPrefix(what(err), "Not a directory: ") {
		t.Fatalf("missing: %v", err)
	}
	// the slug "commons" is reserved; duplicates get -2
	c1 := gitRepo(t)
	p1, _, err := RegisterProject(s, c1, "commons", "", false, "", "human", "")
	if err != nil || p1.Slug != "commons-project" {
		t.Fatalf("commons slug: %v %v", p1, err)
	}
	c2 := gitRepo(t)
	p2, _, err := RegisterProject(s, c2, "My App", "", false, "", "human", "")
	if err != nil || p2.Slug != "my-app-2" {
		t.Fatalf("dup slug: %v %v", p2, err)
	}
}

func TestImportRegistry(t *testing.T) {
	_, s := newHome(t)
	repo := gitRepo(t)
	reg := filepath.Join(t.TempDir(), "projects.json")
	if err := os.WriteFile(reg, []byte(`{"projects": [{"slug": "one", "path": "`+repo+`", "brain": true}, {"path": "/nope/missing"}], "commons": "/tmp/commons"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := ImportRegistry(s, reg, "migrate")
	if err != nil || len(got) != 1 || got[0].Slug != "one" || !got[0].Created {
		t.Fatalf("import %+v %v", got, err)
	}
	if v, ok, _ := s.Meta("commons_path"); !ok || v != "/tmp/commons" {
		t.Fatalf("commons_path %q", v)
	}
	if b, _ := s.Brain(); b == nil || b.Slug != "one" {
		t.Fatal("brain")
	}
	if got, err := ImportRegistry(s, filepath.Join(t.TempDir(), "none.json"), "migrate"); err != nil || len(got) != 0 {
		t.Fatalf("unreadable registry: %v %v", got, err)
	}
}

func TestExportStore(t *testing.T) {
	_, s, p := fixture(t)
	if _, err := ExportStore(s, "", "", false); what(err) != "export --sql PATH or --db PATH" {
		t.Fatalf("no target: %v", err)
	}
	inside := filepath.Join(p.Path, "dump.sql")
	if _, err := ExportStore(s, inside, "", false); !strings.HasPrefix(what(err), "Refusing to export into the registered project demo") {
		t.Fatalf("inside: %v", err)
	}
	dir := t.TempDir()
	db := filepath.Join(dir, "copy.sqlite")
	got, err := ExportStore(s, "", db, false)
	if err != nil || got != db {
		t.Fatalf("db: %s %v", got, err)
	}
	if _, err := ExportStore(s, "", db, false); what(err) != db+" exists; pass --force to overwrite" {
		t.Fatalf("exists: %v", err)
	}
	if _, err := ExportStore(s, "", db, true); err != nil {
		t.Fatalf("force: %v", err)
	}
	copyDir := filepath.Join(dir, "home")
	if err := os.MkdirAll(copyDir, 0o755); err != nil {
		t.Fatal(err)
	}
	copyFile(t, db, filepath.Join(copyDir, store.StoreName))
	c, err := store.Open(store.Options{Home: copyDir, SkipRecords: true})
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	if n := countRows(t, c, "agents", ""); n != 3 {
		t.Fatalf("%d agents in the copy", n)
	}
	sqlPath := filepath.Join(dir, "sub", "dump.sql")
	if _, err := ExportStore(s, sqlPath, "", false); err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(sqlPath)
	text := string(data)
	if !strings.HasPrefix(text, "BEGIN TRANSACTION;\n") || !strings.HasSuffix(text, "COMMIT;\n") ||
		!strings.Contains(text, "CREATE TABLE agents") || !strings.Contains(text, `INSERT INTO "agents" VALUES('alice'`) ||
		!strings.Contains(text, "CREATE TRIGGER posts_no_update") {
		t.Fatalf("dump:\n%s", text[:min(len(text), 600)])
	}
}

func TestBoardWritesReachBusSubscribers(t *testing.T) {
	_, s, _ := fixture(t)
	ch, cancel := bus.Default().Subscribe(bus.Filter{}, 16)
	defer cancel()
	p, err := CreatePost(s, PostInput{Kind: "finding", Project: "demo", Area: "a", Thread: "b", Summary: "seen on the bus", Agent: "alice"})
	if err != nil {
		t.Fatal(err)
	}
	select {
	case e := <-ch:
		if e.Action != "post.finding" || e.Subject != p.ID || e.Actor != "alice" || e.ID == 0 {
			t.Fatalf("event %+v", e)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("no event published in-process")
	}
	if n := countRows(t, s, "events", "action = 'post.finding' AND subject = ?", p.ID); n != 1 {
		t.Fatalf("%d rows for the same event", n)
	}
}

func TestDecideIsAtomicUnderRace(t *testing.T) {
	for _, tc := range []struct {
		name    string
		answers []string
	}{
		{"yes", []string{"yes"}},
		{"no", []string{"no"}},
		{"mixed", []string{"yes", "no"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			testDecideIsAtomicUnderRace(t, tc.answers)
		})
	}
}

func testDecideIsAtomicUnderRace(t *testing.T, answers []string) {
	t.Helper()
	_, s, _ := fixture(t)
	sug, _, _ := seedPosts(t, s)
	// The action family also includes decide.job, whose subject is the queued job.
	ch, cancel := bus.Default().Subscribe(bus.Filter{Actions: []string{"decide"}, Subject: sug.ID}, 16)
	defer cancel()
	const racers = 6
	errs := make(chan error, racers)
	for i := 0; i < racers; i++ {
		go func(i int) {
			answer := answers[i%len(answers)]
			_, err := Decide(s, sug.ID, answer, "", "")
			errs <- err
		}(i)
	}
	wins, refused := 0, 0
	for i := 0; i < racers; i++ {
		switch err := <-errs; {
		case err == nil:
			wins++
		case strings.HasSuffix(what(err), "was already decided; a new proposal needs a new post"):
			refused++
		default:
			t.Fatalf("unexpected: %v", err)
		}
	}
	if wins != 1 || refused != racers-1 {
		t.Fatalf("wins %d refused %d", wins, refused)
	}
	if n := countRows(t, s, "decisions", "post_id = ?", sug.ID); n != 1 {
		t.Fatalf("%d decisions", n)
	}
	if n := countRows(t, s, "posts", "kind = 'result' AND on_post = ?", sug.ID); n != 1 {
		t.Fatalf("%d result posts", n)
	}
	if n := countRows(t, s, "events", "action = 'decide' AND subject = ?", sug.ID); n != 1 {
		t.Fatalf("%d decide events", n)
	}
	select {
	case e := <-ch:
		if e.Action != "decide" || e.Subject != sug.ID || e.ID == 0 {
			t.Fatalf("event %+v", e)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("decide event never published after commit")
	}
	select {
	case e := <-ch:
		t.Fatalf("second decide event published: %+v", e)
	case <-time.After(100 * time.Millisecond):
	}
}
