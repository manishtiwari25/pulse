package tasks

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestMain(m *testing.M) {
	os.Setenv("PULSE_HOME", filepath.Join(os.TempDir(), "pulse-tasks-test-never-used"))
	stdin = strings.NewReader("")
	os.Exit(m.Run())
}

func newStore(t *testing.T) *store.Store {
	t.Helper()
	home := t.TempDir()
	if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(store.Options{Home: home, Now: "2026-09-06T21:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	if _, err := s.DB.Exec("INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)", filepath.Join(home, "demo"), s.Now()); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("INSERT INTO agents (name, kind, created_at) VALUES ('demo-lead', 'lead', ?)", s.Now()); err != nil {
		t.Fatal(err)
	}
	return s
}

func events(t *testing.T, s *store.Store, action string) []bus.Event {
	t.Helper()
	evs, err := bus.Since(s, 0, bus.Filter{Actions: []string{action}}, 0)
	if err != nil {
		t.Fatal(err)
	}
	return evs
}

func TestNewGetListAndIDs(t *testing.T) {
	s := newStore(t)
	if _, err := New(s, "nope", "x", "", 0, "human"); err == nil {
		t.Fatal("unknown project accepted")
	}
	if _, err := New(s, "demo", "   ", "", 0, "human"); err == nil {
		t.Fatal("empty title accepted")
	}
	a, err := New(s, "demo", "  Ship   the thing ", "do it well\n", 3, "human")
	if err != nil {
		t.Fatal(err)
	}
	b, err := New(s, store.CommonsSlug, "Second", "", 0, "demo-lead")
	if err != nil {
		t.Fatal(err)
	}
	if a.ID != "TASK-001" || b.ID != "TASK-002" {
		t.Fatalf("ids %s %s", a.ID, b.ID)
	}
	if a.Title != "Ship the thing" || a.Brief != "do it well\n" || a.Status != "open" || a.Priority != 3 || a.CurrentRev != 1 || a.CreatedBy != "human" {
		t.Fatalf("task %+v", a)
	}
	got, revs, err := Get(s, "TASK-001")
	if err != nil || got.ID != a.ID || len(revs) != 1 || revs[0].Brief != "do it well\n" {
		t.Fatalf("Get %+v %+v %v", got, revs, err)
	}
	all, _ := List(s, "", "")
	demo, _ := List(s, "demo", "open")
	none, _ := List(s, "demo", "done")
	if len(all) != 2 || len(demo) != 1 || len(none) != 0 || all[0].ID != "TASK-002" {
		t.Fatalf("list %d %d %d", len(all), len(demo), len(none))
	}
	if evs := events(t, s, "task.created"); len(evs) != 2 || evs[0].Subject != "TASK-001" || !strings.Contains(evs[0].Detail, `"project": "demo"`) {
		t.Fatalf("events %+v", evs)
	}
	if _, _, err := Get(s, "TASK-999"); err == nil {
		t.Fatal("unknown task found")
	}
}

func TestAmendCreatesRevisionsAndRefusesFinished(t *testing.T) {
	s := newStore(t)
	a, _ := New(s, "demo", "Title", "v1", 0, "human")
	if _, err := s.DB.Exec("INSERT INTO jobs (id, project, kind, title, brief, source, created_at) VALUES ('J-1', 'demo', 'task', 'Title', 'old brief', 'task:TASK-001', ?)", s.Now()); err != nil {
		t.Fatal(err)
	}
	if _, err := Amend(s, a.ID, "", "", "", "human"); err == nil {
		t.Fatal("no-op amend accepted")
	}
	b, err := Amend(s, a.ID, "", "v2", "more detail", "human")
	if err != nil {
		t.Fatal(err)
	}
	if b.CurrentRev != 2 || b.Brief != "v2" || b.Title != "Title" {
		t.Fatalf("amended %+v", b)
	}
	c, err := Amend(s, a.ID, "New title", "", "", "demo-lead")
	if err != nil {
		t.Fatal(err)
	}
	if c.CurrentRev != 3 || c.Title != "New title" || c.Brief != "v2" {
		t.Fatalf("amended %+v", c)
	}
	_, revs, _ := Get(s, a.ID)
	if len(revs) != 3 || revs[1].Note != "more detail" || revs[2].Author != "demo-lead" || revs[0].Brief != "v1" {
		t.Fatalf("revisions %+v", revs)
	}
	if evs := events(t, s, "task.amended"); len(evs) != 2 || !strings.Contains(evs[0].Detail, `"rev": 2`) {
		t.Fatalf("events %+v", evs)
	}
	var brief string
	s.DB.QueryRow("SELECT brief FROM jobs WHERE id = 'J-1'").Scan(&brief)
	if !strings.Contains(brief, "task amended: rev 2") || !strings.Contains(brief, "rev 3") {
		t.Fatalf("queued job brief not marked: %q", brief)
	}
	// immutability
	if _, err := s.DB.Exec("UPDATE task_revisions SET brief = 'hacked' WHERE task_id = ? AND rev = 1", a.ID); err == nil {
		t.Fatal("revision update allowed")
	}
	if _, err := s.DB.Exec("DELETE FROM task_revisions WHERE task_id = ?", a.ID); err == nil {
		t.Fatal("revision delete allowed")
	}
	if _, err := s.DB.Exec("DELETE FROM tasks WHERE id = ?", a.ID); err == nil {
		t.Fatal("task delete allowed")
	}
	// finished tasks refuse
	for _, st := range []string{"planned", "in_progress", "review", "done"} {
		if _, err := SetStatus(s, a.ID, st, "demo-lead"); err != nil {
			t.Fatal(st, err)
		}
	}
	if _, err := Amend(s, a.ID, "", "v3", "", "human"); err == nil {
		t.Fatal("amend of a done task accepted")
	}
	if _, err := Cancel(s, a.ID, "x", "human"); err == nil {
		t.Fatal("cancel of a done task accepted")
	}
}

func TestStateMachine(t *testing.T) {
	s := newStore(t)
	a, _ := New(s, "demo", "T", "", 0, "human")
	if _, err := SetStatus(s, a.ID, "bogus", "x"); err == nil {
		t.Fatal("bogus status accepted")
	}
	if _, err := SetStatus(s, a.ID, "open", "x"); err == nil {
		t.Fatal("same status accepted")
	}
	// skipping forward is fine, backwards is not
	b, err := SetStatus(s, a.ID, "in_progress", "demo-lead")
	if err != nil || b.Status != "in_progress" {
		t.Fatal(err)
	}
	if _, err := SetStatus(s, a.ID, "planned", "demo-lead"); err == nil {
		t.Fatal("backward move accepted")
	}
	if evs := events(t, s, "task.status"); len(evs) != 1 || !strings.Contains(evs[0].Detail, `"from": "open"`) || !strings.Contains(evs[0].Detail, `"to": "in_progress"`) {
		t.Fatalf("events %+v", evs)
	}
	// any -> cancelled, via SetStatus too
	c, err := SetStatus(s, a.ID, "cancelled", "human")
	if err != nil || c.Status != "cancelled" {
		t.Fatal(err)
	}
	if _, err := SetStatus(s, a.ID, "done", "human"); err == nil {
		t.Fatal("cancelled -> done accepted")
	}
}

func TestCancelCancelsQueuedJobsOnly(t *testing.T) {
	s := newStore(t)
	a, _ := New(s, "demo", "T", "", 0, "human")
	for _, row := range [][2]string{{"J-q", "queued"}, {"J-l", "leased"}, {"J-d", "done"}} {
		if _, err := s.DB.Exec("INSERT INTO jobs (id, project, kind, title, source, status, created_at) VALUES (?, 'demo', 'task', 'T', 'task:TASK-001', ?, ?)", row[0], row[1], s.Now()); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := SetLead(s, a.ID, "demo-lead", "J-q", "runner"); err != nil {
		t.Fatal(err)
	}
	c, err := Cancel(s, a.ID, "changed my mind", "human")
	if err != nil || c.Status != "cancelled" || c.Lead != "demo-lead" || c.JobID != "J-q" {
		t.Fatalf("%+v %v", c, err)
	}
	var q, l, d string
	s.DB.QueryRow("SELECT status FROM jobs WHERE id = 'J-q'").Scan(&q)
	s.DB.QueryRow("SELECT status FROM jobs WHERE id = 'J-l'").Scan(&l)
	s.DB.QueryRow("SELECT status FROM jobs WHERE id = 'J-d'").Scan(&d)
	if q != "cancelled" || l != "leased" || d != "done" {
		t.Fatalf("jobs %s %s %s", q, l, d)
	}
	evs := events(t, s, "task.cancelled")
	if len(evs) != 1 || !strings.Contains(evs[0].Detail, `"cancelled_jobs": 1`) || !strings.Contains(evs[0].Detail, `"leased_jobs": 1`) || !strings.Contains(evs[0].Detail, `"lead": "demo-lead"`) {
		t.Fatalf("events %+v", evs)
	}
	jobs, _ := Jobs(s, a.ID)
	if len(jobs) != 3 {
		t.Fatalf("jobs %d", len(jobs))
	}
}

func TestWorkForSummarizesTaskJobs(t *testing.T) {
	s := newStore(t)
	task, err := New(s, "demo", "Owner task", "brief", 0, "human")
	if err != nil {
		t.Fatal(err)
	}
	empty, err := New(s, "demo", "Empty task", "", 0, "human")
	if err != nil {
		t.Fatal(err)
	}
	jobs := []struct {
		id, project, kind, title, source, status, agent, leaseUntil, createdAt string
	}{
		{"JOB-plan", "demo", "plan", "Plan task", "lead:demo:20260906T210000Z", "leased", "demo-lead", "", "2026-09-06T21:01:00Z"},
		{"JOB-queued", "demo", "improve", "Queued work", "task:TASK-001", "queued", "", "", "2026-09-06T21:02:00Z"},
		{"JOB-blocked", "demo", "improve", "Blocked work", "task:TASK-001", "blocked", "", "", "2026-09-06T21:03:00Z"},
		{"JOB-run", "demo", "improve", "Running work", "task:TASK-001", "leased", "cinder", "2026-09-06T22:00:00Z", "2026-09-06T21:04:00Z"},
		{"JOB-expired", "demo", "improve", "Expired lease", "task:TASK-001", "leased", "ember", "2026-09-06T20:00:00Z", "2026-09-06T21:05:00Z"},
		{"JOB-done", "demo", "improve", "Done work", "task:TASK-001", "done", "", "", "2026-09-06T21:06:00Z"},
		{"JOB-failed", "demo", "improve", "Failed work", "task:TASK-001", "failed", "", "", "2026-09-06T21:07:00Z"},
		{"JOB-cancelled", "demo", "improve", "Cancelled work", "task:TASK-001", "cancelled", "", "", "2026-09-06T21:08:00Z"},
		{"JOB-other", "other", "improve", "Wrong project", "task:TASK-001", "failed", "", "", "2026-09-06T21:09:00Z"},
	}
	for _, job := range jobs {
		if _, err := s.DB.Exec(`INSERT INTO jobs (id, project, kind, title, source, status, lease_agent, lease_until, created_at)
			VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`, job.id, job.project, job.kind, job.title, job.source, job.status, job.agent, job.leaseUntil, job.createdAt); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := s.DB.Exec(`INSERT INTO events(at, actor, action, subject, detail) VALUES
		('2026-09-06T21:00:30Z', 'demo-lead', 'job.lease', 'JOB-plan', '{}'),
		('2026-09-06T21:10:00Z', 'cinder', 'job.lease', 'JOB-run', '{}'),
		('2026-09-06T21:11:00Z', 'ember', 'job.lease', 'JOB-expired', '{}')`); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec(`INSERT INTO runs (id, job_id, agent, project, started_at, status, mode, executor)
		VALUES ('RUN-1', 'JOB-run', 'cinder', 'demo', '2026-09-06T21:12:00Z', 'running', 'real', 'remote')`); err != nil {
		t.Fatal(err)
	}
	task, err = SetLead(s, task.ID, "demo-lead", "JOB-plan", "runner")
	if err != nil {
		t.Fatal(err)
	}

	work, err := WorkFor(s, []Task{*task, *empty})
	if err != nil {
		t.Fatal(err)
	}
	got := work[task.ID]
	if got.Total != 8 || got.Queued != 1 || got.Blocked != 1 || got.Running != 3 || got.Done != 1 || got.Failed != 1 {
		t.Fatalf("summary = %+v", got)
	}
	if len(got.Working) != 2 {
		t.Fatalf("working = %+v", got.Working)
	}
	if got.Working[0] != (Worker{Job: "JOB-plan", Kind: "plan", Title: "Plan task", Agent: "demo-lead", Since: "2026-09-06T21:00:30Z"}) {
		t.Fatalf("planning worker = %+v", got.Working[0])
	}
	wantRun := Worker{Job: "JOB-run", Kind: "improve", Title: "Running work", Agent: "cinder", Since: "2026-09-06T21:12:00Z", Executor: "remote"}
	if got.Working[1] != wantRun {
		t.Fatalf("running worker = %+v", got.Working[1])
	}
	emptyWork := work[empty.ID]
	if emptyWork.Total != 0 || len(emptyWork.Working) != 0 {
		t.Fatalf("empty work = %+v", emptyWork)
	}
	encoded, err := json.Marshal(emptyWork)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(encoded), `"working":[]`) {
		t.Fatalf("empty working must marshal as [], got %s", encoded)
	}
}

func TestMessagesSinceUnreadAndImmutable(t *testing.T) {
	s := newStore(t)
	a, _ := New(s, "demo", "T", "", 0, "human")
	if _, err := SendMessage(s, "demo", "", "human", "   "); err == nil {
		t.Fatal("empty body accepted")
	}
	m1, err := SendMessage(s, "demo", "", "human", "hello lead")
	if err != nil {
		t.Fatal(err)
	}
	m2, err := Reply(s, a.ID, "demo-lead", "on it")
	if err != nil {
		t.Fatal(err)
	}
	m3, err := SendMessage(s, "demo", a.ID, "human", "thanks")
	if err != nil {
		t.Fatal(err)
	}
	if m1.ID != "MSG-20260906T210000Z-1" || m2.ID != "MSG-20260906T210000Z-2" || m3.ID != "MSG-20260906T210000Z-3" {
		t.Fatalf("ids %s %s %s", m1.ID, m2.ID, m3.ID)
	}
	if m1.TaskID != "" || m2.TaskID != a.ID || m2.Project != "demo" {
		t.Fatalf("%+v %+v", m1, m2)
	}
	if _, err := SendMessage(s, store.CommonsSlug, a.ID, "human", "x"); err == nil {
		t.Fatal("task/project mismatch accepted")
	}
	all, _ := Messages(s, "demo", "", "", "")
	onTask, _ := Messages(s, "", a.ID, "", "")
	since, _ := Messages(s, "demo", "", m1.ID, "")
	unread, _ := Messages(s, "demo", "", "", "demo-lead")
	if len(all) != 3 || len(onTask) != 2 || len(since) != 2 || since[0].ID != m2.ID || len(unread) != 2 {
		t.Fatalf("lists %d %d %d %d", len(all), len(onTask), len(since), len(unread))
	}
	n, err := MarkRead(s, []string{m1.ID, m3.ID, "MSG-nope"}, "demo-lead")
	if err != nil || n != 2 {
		t.Fatalf("marked %d %v", n, err)
	}
	n, _ = MarkRead(s, []string{m1.ID}, "demo-lead")
	if n != 0 {
		t.Fatalf("re-mark counted %d", n)
	}
	unread, _ = Messages(s, "demo", "", "", "demo-lead")
	if len(unread) != 0 {
		t.Fatalf("unread after mark: %+v", unread)
	}
	got, _ := GetMessage(s, m1.ID)
	if strings.Join(got.ReadBy, ",") != "demo-lead,human" {
		t.Fatalf("read_by %v", got.ReadBy)
	}
	evs := events(t, s, "message.sent")
	if len(evs) != 3 || evs[1].Detail != `{"author": "demo-lead", "project": "demo", "task_id": "TASK-001"}` {
		t.Fatalf("events %+v", evs)
	}
	if _, err := s.DB.Exec("UPDATE messages SET body = 'x' WHERE id = ?", m1.ID); err == nil {
		t.Fatal("message body update allowed")
	}
	if _, err := s.DB.Exec("DELETE FROM messages WHERE id = ?", m1.ID); err == nil {
		t.Fatal("message delete allowed")
	}
}

func TestComputeAutoApproveUnderCapsElseProposal(t *testing.T) {
	s := newStore(t)
	if _, err := RequestCompute(s, "demo", "demo-lead", "", 0, "x"); err == nil {
		t.Fatal("zero hours accepted")
	}
	// caps closed: proposal for the owner
	r1, err := RequestCompute(s, "demo", "demo-lead", "Standard_B2ms", 4, "run the  parity suite")
	if err != nil {
		t.Fatal(err)
	}
	if r1.ID != "CR-001" || r1.Status != "pending" || r1.ProposalPost == "" {
		t.Fatalf("%+v", r1)
	}
	var kind, area, thread, summary, project string
	if err := s.DB.QueryRow("SELECT kind, area, thread, summary, project FROM posts WHERE id = ?", r1.ProposalPost).Scan(&kind, &area, &thread, &summary, &project); err != nil {
		t.Fatal(err)
	}
	if kind != "proposal" || area != "runner" || thread != "compute" || project != "demo" || summary != "compute request CR-001: Standard_B2ms for 4h - run the parity suite" {
		t.Fatalf("post %s %s %s %q", kind, area, thread, summary)
	}
	if !strings.HasPrefix(r1.ProposalPost, "20260906T210000Z-demo-lead") {
		t.Fatalf("post id %s", r1.ProposalPost)
	}
	var posts int
	s.DB.QueryRow("SELECT posts FROM agents WHERE name = 'demo-lead'").Scan(&posts)
	if posts != 1 {
		t.Fatalf("agent posts %d", posts)
	}
	// second same-second post gets -2
	r2, _ := RequestCompute(s, "demo", "demo-lead", "Standard_B2ms", 1, "again")
	if r2.ProposalPost != r1.ProposalPost+"-2" {
		t.Fatalf("collision id %s vs %s", r2.ProposalPost, r1.ProposalPost)
	}
	// open the caps
	for k, v := range map[string]string{"compute.auto_approve": "1", "compute.max_vms": "1", "compute.max_hours_per_day": "8"} {
		if err := s.SetSetting("human", k, v); err != nil {
			t.Fatal(k, err)
		}
	}
	r3, err := RequestCompute(s, "demo", "demo-lead", "", 2, "under caps")
	if err != nil {
		t.Fatal(err)
	}
	if r3.Status != "approved" || r3.DecidedBy != "pulse" || r3.Size != "Standard_B2ms" || r3.ProposalPost != "" {
		t.Fatalf("%+v", r3)
	}
	caps, _ := GetCaps(s)
	if caps.VMsNow != 1 || caps.HoursToday != 7 || !caps.AutoApprove || caps.MaxVMs != 1 {
		t.Fatalf("caps %+v", caps)
	}
	// vm cap reached -> pending again
	r4, _ := RequestCompute(s, "demo", "demo-lead", "", 1, "over the vm cap")
	if r4.Status != "pending" {
		t.Fatalf("%+v", r4)
	}
	// hours cap
	if err := s.SetSetting("human", "compute.max_vms", "5"); err != nil {
		t.Fatal(err)
	}
	r5, _ := RequestCompute(s, "demo", "demo-lead", "", 3, "over the hours cap")
	if r5.Status != "pending" {
		t.Fatalf("hours cap: %+v (caps %+v)", r5, caps)
	}
	// approve / deny
	if _, err := Approve(s, r3.ID, "", "human"); err == nil {
		t.Fatal("approve of an approved request accepted")
	}
	ok, err := Approve(s, r1.ID, "go ahead", "")
	if err != nil || ok.Status != "approved" || ok.DecidedBy != "human" || ok.Words != "go ahead" {
		t.Fatalf("%+v %v", ok, err)
	}
	no, err := Deny(s, r2.ID, "not now", "human")
	if err != nil || no.Status != "denied" {
		t.Fatalf("%+v %v", no, err)
	}
	var answer, result string
	if err := s.DB.QueryRow("SELECT answer, result_post FROM decisions WHERE post_id = ?", r1.ProposalPost).Scan(&answer, &result); err != nil || answer != "yes" || result == "" {
		t.Fatalf("decision %s %s %v", answer, result, err)
	}
	var resolution, rsum string
	s.DB.QueryRow("SELECT resolution, summary FROM posts WHERE id = ?", result).Scan(&resolution, &rsum)
	if resolution != "accepted" || !strings.HasPrefix(rsum, "accepted: compute request CR-001") {
		t.Fatalf("result post %s %q", resolution, rsum)
	}
	if evs := events(t, s, "compute.approved"); len(evs) != 2 {
		t.Fatalf("approved events %+v", evs)
	}
	if evs := events(t, s, "compute.denied"); len(evs) != 1 {
		t.Fatalf("denied events %+v", evs)
	}
	if evs := events(t, s, "decide"); len(evs) != 2 {
		t.Fatalf("decide events %+v", evs)
	}
	// runner hooks
	rd, err := SetComputeStatus(s, r1.ID, "ready", "hand-1", "runner")
	if err != nil || rd.Status != "ready" || rd.Executor != "hand-1" {
		t.Fatalf("%+v %v", rd, err)
	}
	if _, err := s.DB.Exec("DELETE FROM compute_requests WHERE id = ?", r1.ID); err == nil {
		t.Fatal("compute delete allowed")
	}
	list, _ := ListCompute(s, "demo", "pending")
	if len(list) != 2 {
		t.Fatalf("pending %d", len(list))
	}
}

// lockedBuffer is a bytes.Buffer safe to read while Watch writes to it.
type lockedBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (l *lockedBuffer) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.Write(p)
}

func (l *lockedBuffer) String() string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.b.String()
}

func (l *lockedBuffer) Bytes() []byte { return []byte(l.String()) }

func ctxFor(s *store.Store, agent, project string, jsonOut bool) (*cli.Context, *lockedBuffer) {
	out := &lockedBuffer{}
	return &cli.Context{Store: s, Home: s.Home, Out: out, Err: out, Agent: agent, Project: project, JSON: jsonOut}, out
}

func runCmd(t *testing.T, c *cli.Context, name string, args ...string) error {
	t.Helper()
	cmd, ok := cli.Lookup(name)
	if !ok {
		t.Fatalf("no command %s", name)
	}
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	cmd.Flags(fs)
	var positional []string
	for len(args) > 0 {
		if !strings.HasPrefix(args[0], "-") {
			positional = append(positional, args[0])
			args = args[1:]
			continue
		}
		if err := fs.Parse(args); err != nil {
			t.Fatal(err)
		}
		args = fs.Args()
	}
	return cmd.Run(c, fs, positional)
}

func TestCommandsEndToEnd(t *testing.T) {
	s := newStore(t)
	briefFile := filepath.Join(t.TempDir(), "brief.md")
	os.WriteFile(briefFile, []byte("# Brief\n\nDo the thing.\n"), 0o644)

	c, out := ctxFor(s, "", "demo", false)
	if err := runCmd(t, c, "task", "new", "--title", "Ship it", "--brief-file", briefFile, "--priority", "2"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "TASK-001 created in demo: Ship it") {
		t.Fatalf("out %q", out.String())
	}
	c, out = ctxFor(s, "demo-lead", "", true)
	if err := runCmd(t, c, "task", "show", "TASK-001"); err != nil {
		t.Fatal(err)
	}
	var show struct {
		Task      Task       `json:"task"`
		Revisions []Revision `json:"revisions"`
		Messages  []Message  `json:"messages"`
		Jobs      []Job      `json:"jobs"`
	}
	if err := json.Unmarshal(out.Bytes(), &show); err != nil {
		t.Fatal(err, out.String())
	}
	if show.Task.CreatedBy != "human" || show.Task.Brief != "# Brief\n\nDo the thing.\n" || len(show.Revisions) != 1 || show.Messages == nil || show.Jobs == nil {
		t.Fatalf("show %s", out.String())
	}
	c, out = ctxFor(s, "demo-lead", "", false)
	if err := runCmd(t, c, "task", "reply", "TASK-001", "starting", "now"); err != nil {
		t.Fatal(err)
	}
	if err := runCmd(t, c, "task", "status", "TASK-001", "in_progress"); err != nil {
		t.Fatal(err)
	}
	if err := runCmd(t, c, "task", "amend", "TASK-001", "--note", "clarified", "--brief", "Do it faster."); err != nil {
		t.Fatal(err)
	}
	if err := runCmd(t, c, "task", "list"); err != nil {
		t.Fatal(err)
	}
	text := out.String()
	if !strings.Contains(text, "MSG-") || !strings.Contains(text, "TASK-001 is in_progress") || !strings.Contains(text, "amended: rev 2") || !strings.Contains(text, "in_progress") {
		t.Fatalf("out %q", text)
	}
	c, out = ctxFor(s, "", "demo", false)
	if err := runCmd(t, c, "msg", "send", "--task", "TASK-001", "how", "is", "it", "going?"); err != nil {
		t.Fatal(err)
	}
	c, out = ctxFor(s, "demo-lead", "demo", true)
	if err := runCmd(t, c, "msg", "list", "--unread"); err != nil {
		t.Fatal(err)
	}
	var list struct{ Messages []Message }
	json.Unmarshal(out.Bytes(), &list)
	if len(list.Messages) != 1 || list.Messages[0].Author != "human" || list.Messages[0].Body != "how is it going?" {
		t.Fatalf("unread %s", out.String())
	}
	c, out = ctxFor(s, "demo-lead", "", false)
	if err := runCmd(t, c, "msg", "read", list.Messages[0].ID); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "1 marked read for demo-lead") {
		t.Fatalf("out %q", out.String())
	}
	c, out = ctxFor(s, "demo-lead", "demo", false)
	if err := runCmd(t, c, "compute", "request", "--size", "Standard_D4s", "--hours", "3", "--reason", "parity runs"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "CR-001 pending") {
		t.Fatalf("out %q", out.String())
	}
	c, out = ctxFor(s, "", "", false)
	if err := runCmd(t, c, "compute", "approve", "CR-001", "--words", "yes please"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "CR-001 approved by human") {
		t.Fatalf("out %q", out.String())
	}
	c, out = ctxFor(s, "", "", true)
	if err := runCmd(t, c, "compute", "list"); err != nil {
		t.Fatal(err)
	}
	var cl struct {
		Requests []ComputeRequest
		Caps     Caps
	}
	json.Unmarshal(out.Bytes(), &cl)
	if len(cl.Requests) != 1 || cl.Caps.VMsNow != 1 {
		t.Fatalf("compute list %s", out.String())
	}
	c, out = ctxFor(s, "", "", false)
	if err := runCmd(t, c, "task", "cancel", "TASK-001", "--reason", "done differently"); err != nil {
		t.Fatal(err)
	}
	if err := runCmd(t, c, "task", "bogus"); err == nil {
		t.Fatal("bogus subcommand accepted")
	}
	if err := runCmd(t, c, "task", "show", "TASK-404"); err == nil {
		t.Fatal("unknown task shown")
	}
	// text show renders sections
	c, out = ctxFor(s, "", "", false)
	if err := runCmd(t, c, "task", "show", "TASK-001"); err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"TASK-001  cancelled  Ship it", "REVISIONS", "MESSAGES", "starting now", "JOBS", "Do it faster."} {
		if !strings.Contains(out.String(), want) {
			t.Fatalf("show text missing %q:\n%s", want, out.String())
		}
	}
}

func TestWatchDeliversEventsFromAnotherConnection(t *testing.T) {
	s := newStore(t)
	other, err := store.Open(store.Options{Home: s.Home, Now: "2026-09-06T21:05:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	defer other.Close()

	c, out := ctxFor(s, "", "demo", false)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- Watch(ctx, c, -1, bus.Filter{Project: "demo"}, 10*time.Millisecond, false) }()
	time.Sleep(60 * time.Millisecond)

	if _, err := New(other, "demo", "From elsewhere", "", 0, "human"); err != nil {
		t.Fatal(err)
	}
	if _, err := New(other, store.CommonsSlug, "Not demo", "", 0, "human"); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) && !strings.Contains(out.String(), "TASK-001") {
		time.Sleep(20 * time.Millisecond)
	}
	cancel()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	text := out.String()
	if !strings.Contains(text, "2026-09-06T21:05:00Z human task.created TASK-001") || strings.Contains(text, "TASK-002") {
		t.Fatalf("watch output %q", text)
	}

	c, out = ctxFor(s, "", "", true)
	ctx, cancel = context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	go func() {
		done <- Watch(ctx, c, 0, bus.Filter{Actions: []string{"task.created"}}, 10*time.Millisecond, false)
	}()
	for time.Now().Before(deadline) && strings.Count(out.String(), "\n") < 2 {
		time.Sleep(20 * time.Millisecond)
	}
	cancel()
	<-done
	lines := strings.Split(strings.TrimSpace(out.String()), "\n")
	if len(lines) != 2 {
		t.Fatalf("json lines %q", out.String())
	}
	var e bus.Event
	if err := json.Unmarshal([]byte(lines[1]), &e); err != nil || e.Subject != "TASK-002" || e.Action != "task.created" {
		t.Fatalf("json line %q %v", lines[1], err)
	}
}

func TestComputeAutoApproveHeldWithoutLoginSource(t *testing.T) {
	s := newStore(t)
	for k, v := range map[string]string{"compute.auto_approve": "1", "compute.max_vms": "2", "compute.max_hours_per_day": "8"} {
		if err := s.SetSetting("human", k, v); err != nil {
			t.Fatal(k, err)
		}
	}
	old := LoginSourceCheck
	t.Cleanup(func() { LoginSourceCheck = old })
	LoginSourceCheck = func(*store.Store) bool { return false }
	r, err := RequestCompute(s, "demo", "demo-lead", "", 1, "no login anywhere")
	if err != nil {
		t.Fatal(err)
	}
	if r.Status != "pending" || r.ProposalPost == "" {
		t.Fatalf("held request: %+v", r)
	}
	var body string
	s.DB.QueryRow("SELECT body FROM posts WHERE id = ?", r.ProposalPost).Scan(&body)
	if !strings.Contains(body, NoLoginSource) || !strings.Contains(body, "pulse executor token --with-token") {
		t.Fatalf("proposal body:\n%s", body)
	}
	caps, _ := GetCaps(s)
	if caps.LoginSource {
		t.Fatalf("caps: %+v", caps)
	}
	LoginSourceCheck = func(*store.Store) bool { return true }
	r2, _ := RequestCompute(s, "demo", "demo-lead", "", 1, "with a source")
	if r2.Status != "approved" {
		t.Fatalf("with a source: %+v", r2)
	}
}
