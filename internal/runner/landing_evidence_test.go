package runner

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestLandingEvidenceTransactionFencesCurrentCarrier(t *testing.T) {
	for _, fault := range []string{"receipt-write", "newer-run", "newer-generation"} {
		t.Run(fault, func(t *testing.T) {
			s, project, job, root, _ := measuredKeepFixture(t)
			deferMeasuredKeep(t, s, project, job, root)
			runID := lastRunID(s, job.str("id"))
			switch fault {
			case "receipt-write":
				exec1(s.DB, `CREATE TRIGGER reject_receipt BEFORE INSERT ON events
					WHEN NEW.action = 'land.landed' BEGIN SELECT RAISE(ABORT, 'fake receipt refused'); END`)
			case "newer-run":
				now, err := time.Parse(time.RFC3339, testNow)
				if err != nil {
					t.Fatal(err)
				}
				exec1(s.DB, `INSERT INTO runs (id, job_id, project, agent, started_at, status)
					VALUES ('newer-run', ?, 'proj', 'proposer', ?, 'done')`, job.str("id"), now.Add(time.Second).Format(time.RFC3339))
			case "newer-generation":
				exec1(s.DB, "UPDATE jobs SET generation = generation + 1 WHERE id = ?", job.str("id"))
			}
			err := recordLanding(s, project, job.str("branch"), "daemon", "land.landed", " | fake finalization",
				landOpts{Job: job, ArchiveID: "B-002", RunID: runID}, map[string]any{"fake": true})
			if err == nil {
				t.Fatal("a failed or stale finalization was accepted")
			}
			current := jobRow(s.DB, job.str("id"))
			if current.str("branch") != job.str("branch") || current.str("result") != job.str("result") {
				t.Fatalf("failed receipt cleared or changed the current carrier: %v", current)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed'"); n != 0 {
				t.Fatalf("failed or stale finalization left %d success receipts", n)
			}
		})
	}
}

func TestLandingReceiptFailureRetriesFinalization(t *testing.T) {
	for _, path := range []string{"landed", "merged", "discarded"} {
		t.Run(path, func(t *testing.T) {
			s, project, job, root, calls := measuredKeepFixture(t)
			if path == "merged" {
				deferMeasuredKeep(t, s, project, job, root)
				if err := os.WriteFile(filepath.Join(root, "README.md"), []byte("# proj\n"), 0o644); err != nil {
					t.Fatal(err)
				}
				run(t, root, "git", "merge", "--ff-only", job.str("branch"))
			}
			if path == "discarded" {
				evolve.Measure = func(_ context.Context, _ *store.Store, slug, _, ref string, _ evolve.MeasureOptions) (*evolve.Scores, error) {
					*calls++
					score := .7
					if ref == "main" {
						score = .8
					}
					return &evolve.Scores{Project: slug, Total: score}, nil
				}
			}
			exec1(s.DB, `CREATE TRIGGER reject_finalization BEFORE INSERT ON events
				WHEN NEW.action IN ('land.landed', 'land.merged', 'land.dropped')
				BEGIN SELECT RAISE(ABORT, 'fake finalization refused'); END`)
			state, note := landExperiment(s, project, job, "daemon", testNow, true)
			if state != "error" || !strings.Contains(note, "fake finalization refused") {
				t.Fatalf("receipt failure was hidden: %s: %s", state, note)
			}
			if current := jobRow(s.DB, job.str("id")); current.str("status") != "done" || current.str("branch") != job.str("branch") {
				t.Fatalf("failed receipt lost its pending carrier: %v", current)
			}
			if path != "discarded" {
				if exists, why := branchExists(root, job.str("branch")); !exists || why != "" {
					t.Fatalf("failed kept receipt lost its retry artifact: %v, %s", exists, why)
				}
			}
			if feedback, err := evolve.PlanFeedback(s, "proj", "daemon"); err != nil || feedback != nil {
				t.Fatalf("failed receipt produced finalized feedback: %+v (%v)", feedback, err)
			}
			exec1(s.DB, "DROP TRIGGER reject_finalization")
			state, note = landExperiment(s, project, jobRow(s.DB, job.str("id")), "daemon", testNow, true)
			want := "merged"
			if path == "discarded" {
				want = "dropped"
			}
			if state != want {
				t.Fatalf("receipt retry did not finalize %s: %s: %s", path, state, note)
			}
			if *calls != 2 || count(t, s, "SELECT COUNT(*) AS n FROM evolution_archive") != 2 {
				t.Fatal("receipt retry remeasured or duplicated the archive")
			}
			if feedback, err := evolve.PlanFeedback(s, "proj", "daemon"); err != nil || feedback == nil {
				t.Fatalf("successful receipt retry produced no feedback: %+v (%v)", feedback, err)
			}
		})
	}
}

func TestDiscardFinalizationRetriesWithoutAnotherMeasurement(t *testing.T) {
	s, project, job, root, calls := measuredKeepFixture(t)
	before := headOf(t, root)
	evolve.Measure = func(_ context.Context, _ *store.Store, slug, _, ref string, _ evolve.MeasureOptions) (*evolve.Scores, error) {
		*calls++
		score := .7
		if ref == "main" {
			score = .8
		}
		return &evolve.Scores{Project: slug, Total: score}, nil
	}
	lock := filepath.Join(root, ".git", "refs", "heads", job.str("branch")+".lock")
	if err := os.WriteFile(lock, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	state, note := landExperiment(s, project, job, "daemon", testNow, true)
	if state != "deferred" || !strings.Contains(note, "archive B-002") {
		t.Fatalf("failed discard cleanup did not stay pending: %s: %s", state, note)
	}
	if jobRow(s.DB, job.str("id")).str("branch") != job.str("branch") {
		t.Fatal("failed discard cleanup cleared the carrier")
	}
	if feedback, err := evolve.PlanFeedback(s, "proj", "daemon"); err != nil || feedback != nil {
		t.Fatalf("pending discard produced finalized feedback: %+v (%v)", feedback, err)
	}
	if err := os.Remove(lock); err != nil {
		t.Fatal(err)
	}
	state, note = landExperiment(s, project, job, "daemon", testNow, true)
	if state != "dropped" {
		t.Fatalf("discard did not finish after its cleanup gate opened: %s: %s", state, note)
	}
	if *calls != 2 || count(t, s, "SELECT COUNT(*) AS n FROM evolution_archive") != 2 || headOf(t, root) != before {
		t.Fatal("retry remeasured, rearchived or landed a discarded change")
	}
	if exists, why := branchExists(root, job.str("branch")); exists || why != "" {
		t.Fatalf("finalized discard retained the branch: %v, %s", exists, why)
	}
	for i := range 2 {
		feedback, err := evolve.PlanFeedback(s, "proj", "daemon")
		if err != nil || (feedback != nil) != (i == 0) {
			t.Fatalf("discard feedback %d: %+v (%v)", i, feedback, err)
		}
		if feedback != nil && !strings.Contains(feedback.Brief, "Outcome: discarded") {
			t.Fatal("discarded measurement became kept feedback")
		}
	}
}
