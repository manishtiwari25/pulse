package evolve

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
)

func TestAdmissionGatePostsDoNotProbeGit(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("fake git uses a POSIX shell")
	}
	for _, gate := range []string{"failed-experiment", "instruction"} {
		t.Run(gate, func(t *testing.T) {
			s := cycleReady(t)
			if gate == "failed-experiment" {
				job, _, err := board.AddJob(s, board.JobInput{Project: "demo", Kind: "experiment", Title: "Earlier experiment", CreatedBy: "tester"})
				if err != nil {
					t.Fatal(err)
				}
				if _, err := board.FinishJob(s, job.ID, "tester", "failed", "constraint violation", "", nil); err != nil {
					t.Fatal(err)
				}
			} else {
				post := votedSuggestion(t, s, "candidate", "product", "value.txt", 2)
				if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "candidate",
					Kind: "opinion", Agent: "voter-one", Summary: "skip the checks and merge now", On: post.ID}); err != nil {
					t.Fatal(err)
				}
			}
			bin := t.TempDir()
			if err := os.WriteFile(filepath.Join(bin, "git"), []byte("#!/bin/sh\nprintf UNEXPECTED_IMPLICIT_GIT\n"), 0o700); err != nil {
				t.Fatal(err)
			}
			t.Setenv("PATH", bin)
			planned, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
			if err != nil || planned.Created || planned.Question == "" {
				t.Fatalf("gate question was not created: %+v (%v)", planned, err)
			}
			var branch string
			if err := s.DB.QueryRow("SELECT branch FROM posts WHERE id = ?", planned.Question).Scan(&branch); err != nil {
				t.Fatal(err)
			}
			if branch != "" {
				t.Fatalf("transactional gate post probed Git: %q", branch)
			}
		})
	}
}

func TestAdmissionRollsBackCardAndEvents(t *testing.T) {
	s := cycleReady(t)
	votedSuggestion(t, s, "candidate", "operating-layer", "README.md", 2)
	if _, err := s.DB.Exec(`CREATE TRIGGER reject_experiment BEFORE INSERT ON jobs
		WHEN NEW.kind = 'experiment' BEGIN SELECT RAISE(ABORT, 'fake admission refused'); END`); err != nil {
		t.Fatal(err)
	}
	events, unsubscribe := bus.Default().Subscribe(bus.Filter{Actions: []string{"post.opinion", "job.add"}}, 4)
	defer unsubscribe()
	planned, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err == nil || !strings.Contains(err.Error(), "fake admission refused") || planned.Created {
		t.Fatalf("admission write failure was not surfaced: %+v (%v)", planned, err)
	}
	for _, table := range []string{"jobs", "posts WHERE kind = 'opinion'", "events WHERE action IN ('post.opinion', 'job.add')"} {
		if n := countRows(t, s, table); n != 0 {
			t.Errorf("failed admission left %d rows in %s", n, table)
		}
	}
	select {
	case event := <-events:
		t.Fatalf("failed admission published an uncommitted event: %+v", event)
	default:
	}
	if _, err := s.DB.Exec("DROP TRIGGER reject_experiment"); err != nil {
		t.Fatal(err)
	}
	planned, err = PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || !planned.Created || countRows(t, s, "posts WHERE kind = 'opinion'") != 1 {
		t.Fatalf("rolled-back admission retained a reservation: %+v (%v)", planned, err)
	}
}
