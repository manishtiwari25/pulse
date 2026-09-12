package runner

import (
	"fmt"
	"os"
	"strconv"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func newCompletionJob(t *testing.T, kind string) (*store.Store, row, row, string) {
	t.Helper()
	s := newStore(t)
	project, _ := newRepo(t, s)
	agent := "aurora"
	if kind == leadKind {
		agent = leadName("proj")
		if _, _, err := joinAgent(s, agent, "", "", "", "", leadKind, ""); err != nil {
			t.Fatal(err)
		}
	} else {
		join(t, s, agent)
	}
	job, _, err := addJob(s, "proj", kind, "Completion regression", "Report the outcome.", 0, 0, "", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	return s, job, project, agent
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
	for name, output := range map[string]string{
		"empty":        "",
		"unstructured": "Starting work; everything looks fine.\n",
		"empty label":  "SUMMARY:   \n",
	} {
		t.Run(name, func(t *testing.T) {
			s, job, _, agent := newCompletionJob(t, "task")
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
	for _, kind := range []string{"answer", "ballot", leadKind, "task"} {
		t.Run(kind, func(t *testing.T) {
			s, job, _, agent := newCompletionJob(t, kind)
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
			s, job, _, agent := newCompletionJob(t, tc.kind)
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
