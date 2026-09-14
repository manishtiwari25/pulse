package runner

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func newCompletionJob(t *testing.T, kind string) (*store.Store, row, row, string) {
	t.Helper()
	s := newStore(t)
	project, agents := completionFixture(t, s)
	return s, completionJob(t, s, kind), project, agents[kind]
}

// completionAgents maps a job kind to the agent that works it in the shared fixture: lead
// jobs run under the project lead, everything else under the worker "aurora".
type completionAgents map[string]string

// completionFixture registers the project checkout, the worker agent and the lead agent on s
// once; every job kind (including leadKind) can then be queued on the same store, so one
// snapshot serves a whole table instead of one fixture builder per agent kind.
func completionFixture(t *testing.T, s *store.Store) (row, completionAgents) {
	t.Helper()
	project, _ := newBareRepo(t, s)
	worker, lead := "aurora", leadName("proj")
	join(t, s, worker)
	if _, _, err := joinAgent(s, lead, "", "", "", "", leadKind, ""); err != nil {
		t.Fatal(err)
	}
	agents := completionAgents{}
	for _, kind := range []string{"task", "answer", "ballot", "experiment", leadKind} {
		agents[kind] = worker
	}
	agents[leadKind] = lead
	return project, agents
}

// completionSuite is the shared fixture plus its snapshot: table-driven tests build one in the
// parent and call job per case to get a fresh store with exactly one queued job of that kind.
type completionSuite struct {
	project row
	agents  completionAgents
	snap    *completionSnapshot
}

func newCompletionSuite(t *testing.T) *completionSuite {
	t.Helper()
	s := newStore(t)
	project, agents := completionFixture(t, s)
	snap := snapshotCompletionStore(t, s)
	t.Cleanup(func() { snap.s.Close() })
	return &completionSuite{project: project, agents: agents, snap: snap}
}

// job resets the store to the fixture snapshot and queues one job of kind on it.
func (c *completionSuite) job(t *testing.T, kind string) (*store.Store, row, row, string) {
	t.Helper()
	s := c.snap.reset(t)
	return s, completionJob(t, s, kind), c.project, c.agents[kind]
}

// completionJob queues one fresh job of kind on the shared fixture's project.
func completionJob(t *testing.T, s *store.Store, kind string) row {
	t.Helper()
	job, _, err := addJob(s, "proj", kind, "Completion regression", "Report the outcome.", 0, 0, "", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	return job
}

// completionSnapshot captures the shared fixture store so a table-driven test can reset to it
// between cases: posts are immutable (no DELETE), so the reset reopens a copy of the file
// instead of clearing rows. VACUUM INTO writes a consistent copy from WAL mode.
type completionSnapshot struct {
	home, file string
	s          *store.Store
}

func snapshotCompletionStore(t *testing.T, s *store.Store) *completionSnapshot {
	t.Helper()
	home := os.Getenv("PULSE_HOME")
	file := filepath.Join(t.TempDir(), "snapshot.sqlite")
	if _, err := s.DB.Exec("VACUUM INTO ?", file); err != nil {
		t.Fatal(err)
	}
	return &completionSnapshot{home: home, file: file, s: s}
}

// reset closes the current store, restores the snapshot file and reopens it; the returned
// store is the fixture as it was right after snapshotCompletionStore.
func (c *completionSnapshot) reset(t *testing.T) *store.Store {
	t.Helper()
	if err := c.s.Close(); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(c.home, store.StoreName)
	for _, suffix := range []string{"-wal", "-shm"} {
		os.Remove(path + suffix)
	}
	data, err := os.ReadFile(c.file)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(store.Options{Home: c.home, Now: testNow})
	if err != nil {
		t.Fatal(err)
	}
	c.s = s
	return s
}

func runCompletion(t *testing.T, s *store.Store, job row, agent, command string) row {
	t.Helper()
	r, reason, err := workOnce(s, agent, job.str("project"), job.str("id"), testNow, runOpts{WorkerCmd: command})
	if err != nil || r == nil {
		t.Fatalf("work: reason=%q err=%v", reason, err)
	}
	return r
}

func completionWorkerCommand(t *testing.T, status, result, output string, exit int, directAnswer string) string {
	t.Helper()
	for key, val := range map[string]string{
		"PULSE_COMPLETION_HELPER": "1",
		"PULSE_COMPLETION_STATUS": status,
		"PULSE_COMPLETION_RESULT": result,
		"PULSE_COMPLETION_OUTPUT": output,
		"PULSE_COMPLETION_EXIT":   strconv.Itoa(exit),
		"PULSE_COMPLETION_ANSWER": directAnswer,
	} {
		t.Setenv(key, val)
	}
	return shellQuote(buildPulse(t)) + " -test.run=^TestCompletionWorkerProcess$"
}

// The existing test binary stands in for a worker; it only opens the fixture store.
func TestCompletionWorkerProcess(t *testing.T) {
	if os.Getenv("PULSE_COMPLETION_HELPER") != "1" {
		return
	}
	s, err := store.Open(store.Options{Home: os.Getenv("PULSE_HOME"), Now: testNow})
	if err != nil {
		t.Fatal(err)
	}
	job := jobRow(s.DB, os.Getenv("PULSE_JOB"))
	agent := os.Getenv("PULSE_AGENT")
	if mode := os.Getenv("PULSE_COMPLETION_ANSWER"); mode != "" {
		id, ok := strings.CutPrefix(job.str("source"), "question:")
		if !ok {
			t.Fatal("fixture answer has no question source")
		}
		q := postRow(s.DB, id)
		on := id
		if mode == "thread" {
			on = ""
		}
		if _, err := createPost(s, testNow, postOpts{Kind: "answer", Project: q.str("project"), Area: q.str("area"),
			Thread: q.str("thread"), Summary: "The worker already answered.", Body: "Direct worker answer.\n",
			On: on, Agent: actorAgent(s, agent), Session: os.Getenv("PULSE_SESSION")}); err != nil {
			t.Fatal(err)
		}
	}
	if status := os.Getenv("PULSE_COMPLETION_STATUS"); status != "" {
		gen := job.int("generation")
		if _, err := board.FinishJob(s, job.str("id"), agent, status, os.Getenv("PULSE_COMPLETION_RESULT"),
			"pulse/"+agent+"/"+job.str("id"), &gen); err != nil {
			t.Fatal(err)
		}
	}
	fmt.Print(os.Getenv("PULSE_COMPLETION_OUTPUT"))
	exit, err := strconv.Atoi(os.Getenv("PULSE_COMPLETION_EXIT"))
	if err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	os.Exit(exit)
}

func TestCompletionRejectsUnreportedSuccess(t *testing.T) {
	skipSlow(t)
	suite := newCompletionSuite(t)
	for name, output := range map[string]string{
		"empty":        "",
		"unstructured": "Starting work; everything looks fine.\n",
		"empty label":  "SUMMARY:   \n",
	} {
		t.Run(name, func(t *testing.T) {
			s, job, _, agent := suite.job(t, "task")
			command := completionWorkerCommand(t, "", "", output, 0, "")
			r := runCompletion(t, s, job, agent, command)
			if r.str("status") != "failed" || !strings.Contains(r.str("summary"), "worker wrote no report") {
				t.Errorf("exit-zero output %q: status=%s summary=%q", output, r.str("status"), r.str("summary"))
			}
			if j := jobRow(s.DB, job.str("id")); j.str("status") != "failed" || j.str("branch") != "" {
				t.Errorf("unreported job: status=%s branch=%q", j.str("status"), j.str("branch"))
			}
		})
	}
}

func TestCompletionSummaryOnlyKinds(t *testing.T) {
	skipSlow(t)
	suite := newCompletionSuite(t)
	for _, kind := range []string{"answer", "ballot", leadKind, "task"} {
		t.Run(kind, func(t *testing.T) {
			s, job, _, agent := suite.job(t, kind)
			command := completionWorkerCommand(t, "", "", "SUMMARY: investigated without changing code\n", 0, "")
			r := runCompletion(t, s, job, agent, command)
			if r.str("status") != "done" || r.int("commits") != 0 || r.str("branch") != "" {
				t.Fatalf("summary-only %s: status=%s commits=%d branch=%q", kind, r.str("status"), r.int("commits"), r.str("branch"))
			}
		})
	}
}

func TestCompletionWorkerFinishAuthoritative(t *testing.T) {
	skipSlow(t)
	suite := newCompletionSuite(t)
	for _, tc := range []struct {
		name, kind, status, result, output, want string
		exit                                     int
	}{
		{"lead without artifacts", leadKind, "done", "SUMMARY: reviewed the queue", "", "done", 0},
		{"explicit finish without text", "task", "done", "", "", "done", 0},
		{"raw finish result", "task", "done", "reviewed the queue", "Worker exited normally.\n", "done", 0},
		{"worker failure", "task", "failed", "SUMMARY: cannot finish", "", "failed", 0},
		{"empty worker failure", "task", "failed", "", "", "failed", 1},
		{"nonzero exit overrides done", "task", "done", "SUMMARY: claimed success", "", "failed", 1},
		{"cancel overrides done", "task", "done", "SUMMARY: claimed success", "", "failed", 5},
		{"cancelled finish", "task", "cancelled", "SUMMARY: stopped", "", "failed", 0},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, job, _, agent := suite.job(t, tc.kind)
			command := completionWorkerCommand(t, tc.status, tc.result, tc.output, tc.exit, "")
			r := runCompletion(t, s, job, agent, command)
			j := jobRow(s.DB, job.str("id"))
			if r.str("status") != tc.want || j.str("status") != tc.want {
				t.Errorf("run=%s job=%s, want %s; summary=%q", r.str("status"), j.str("status"), tc.want, r.str("summary"))
			}
			if r.str("branch") != "" || j.str("branch") != "" || len(pendingLandings(s.DB, "proj")) != 0 {
				t.Errorf("zero-artifact completion retained branch: run=%q job=%q", r.str("branch"), j.str("branch"))
			}
		})
	}
}
