package evolve

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
)

func fakeFinalizeArchive(s *store.Store, jobID, action string, proof *Archived) error {
	current, err := archivedForJob(s, "demo", jobID)
	if err != nil {
		return err
	}
	if current == nil {
		return errors.New("fake finalization needs a measured current run")
	}
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("UPDATE runs SET status = 'done' WHERE id = ?", current.RunID); err != nil {
		return err
	}
	if _, err := tx.Exec("UPDATE jobs SET status = 'done', branch = '', result = 'fake controller finished', finished_at = ? WHERE id = ?", s.Now(), jobID); err != nil {
		return err
	}
	if proof != nil {
		if err := tx.Event("tester", action, proof.Branch, map[string]any{
			"project": proof.Project, "job": jobID, "archive": proof.ID, "run_id": proof.RunID,
		}); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func TestCycleRequiresCurrentFinalizationEvidence(t *testing.T) {
	for _, evidence := range []string{"missing", "legacy", "prior-run", "wrong-archive", "wrong-project", "wrong-branch", "discarded-keep", "landed", "merged"} {
		t.Run(evidence, func(t *testing.T) {
			s := cycleReady(t)
			votedSuggestion(t, s, "candidate", "operating-layer", "README.md", 2)
			events, unsubscribe := bus.Default().Subscribe(bus.Filter{Actions: []string{"job.add"}}, 8)
			defer unsubscribe()
			ctx, cancel := context.WithTimeout(context.Background(), 4*time.Second)
			var out bytes.Buffer
			done := make(chan error, 1)
			finished := make(chan struct{})
			go func() {
				defer close(finished)
				done <- Cycle(ctx, s, "demo", CycleOptions{Once: true, Agent: "tester", JSON: true, Out: &out, Interval: 10 * time.Millisecond})
			}()
			defer func() { cancel(); <-finished }()
			var jobID string
			select {
			case event := <-events:
				jobID = event.Subject
			case <-ctx.Done():
				t.Fatal("no experiment reached the fake runner")
			}
			if err := fakeArchive(s, jobID); err != nil {
				t.Fatal(err)
			}
			current, err := archivedForJob(s, "demo", jobID)
			if err != nil || current == nil {
				t.Fatalf("current archive: %+v (%v)", current, err)
			}
			proof := *current
			action := "land.landed"
			switch evidence {
			case "legacy":
				proof.ID, proof.RunID = "", ""
			case "wrong-archive":
				proof.ID = "B-001"
			case "wrong-project":
				proof.Project = "another-project"
			case "wrong-branch":
				proof.Branch = "evolve/another-branch"
			case "discarded-keep":
				action = "land.dropped"
			case "merged":
				action = "land.merged"
			case "prior-run":
				if _, err := s.DB.Exec("UPDATE runs SET status = 'done' WHERE job_id = ?", jobID); err != nil {
					t.Fatal(err)
				}
				now, err := time.Parse(time.RFC3339, s.Now())
				if err != nil {
					t.Fatal(err)
				}
				if _, err := s.DB.Exec(`INSERT INTO runs (id, job_id, agent, project, started_at, status)
					VALUES ('current-retry', ?, 'tester', 'demo', ?, 'done')`, jobID, now.Add(time.Second).Format(time.RFC3339)); err != nil {
					t.Fatal(err)
				}
				if _, err := Archive(s, ArchiveRow{Project: "demo", SuggestionPost: current.SuggestionPost,
					Hypothesis: current.Hypothesis, Branch: current.Branch, Baseline: current.Fixtures.Baseline,
					Variant: current.Fixtures.Variant, Kept: true, Tolerance: current.Tolerance, RunID: "current-retry"}); err != nil {
					t.Fatal(err)
				}
			}
			var receipt *Archived
			if evidence != "missing" {
				receipt = &proof
			}
			if err := fakeFinalizeArchive(s, jobID, action, receipt); err != nil {
				t.Fatal(err)
			}
			select {
			case err := <-done:
				if err != nil {
					t.Fatal(err)
				}
			case <-ctx.Done():
				t.Fatal("missing finalization evidence left the observer waiting indefinitely")
			}
			var result CycleResult
			if err := json.NewDecoder(&out).Decode(&result); err != nil {
				t.Fatal(err)
			}
			delivered := evidence == "landed" || evidence == "merged"
			if delivered {
				if result.Stopped || result.Archive == nil || !result.Archive.Kept {
					t.Fatalf("current landing proof did not finalize the kept measurement: %+v", result)
				}
			} else if !result.Stopped || result.Archive != nil || !strings.Contains(result.Reason, "finalization evidence") {
				t.Errorf("%s proof produced successful cycle feedback: %+v", evidence, result)
			}
			if feedback, err := PlanFeedback(s, "demo", "tester"); err != nil || feedback != nil {
				t.Errorf("reconciliation queued unproven or duplicate feedback: %+v (%v)", feedback, err)
			}
			wantFeedback := 0
			if delivered {
				wantFeedback = 1
			}
			if n := countRows(t, s, "jobs WHERE source LIKE 'evolve-feedback:%'"); n != wantFeedback {
				t.Errorf("%s evidence queued %d feedback jobs; want %d", evidence, n, wantFeedback)
			}
			archived, err := archivedForJob(s, "demo", jobID)
			if err != nil || archived == nil || !archived.Kept || archived.VariantScore != .4 {
				t.Fatalf("delivery handling changed the measured archive: %+v (%v)", archived, err)
			}
		})
	}
}
