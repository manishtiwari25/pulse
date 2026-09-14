package evolve

import (
	"sync"
	"testing"
	"time"
)

func TestFeedbackQueueIsAtomicAndDurable(t *testing.T) {
	s := cycleReady(t)
	votedSuggestion(t, s, "candidate", "operating-layer", "README.md", 2)
	planned, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if err := fakeLanding(s, planned.JobID, 0); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	errs := make(chan error, 4)
	for range 4 {
		wg.Go(func() {
			_, err := PlanFeedback(s, "demo", "tester")
			errs <- err
		})
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	if n := countRows(t, s, "jobs WHERE source = 'evolve-feedback:demo:B-002'"); n != 1 {
		t.Fatalf("concurrent reconciliation queued %d follow-ups", n)
	}
	if n := countRows(t, s, "events WHERE action = 'evolution.feedback'"); n != 1 {
		t.Fatalf("concurrent reconciliation emitted %d feedback events", n)
	}
	for _, state := range []string{"done", "failed", "cancelled"} {
		if _, err := s.DB.Exec("UPDATE jobs SET status = ? WHERE source = 'evolve-feedback:demo:B-002'", state); err != nil {
			t.Fatal(err)
		}
		if job, err := PlanFeedback(s, "demo", "tester"); err != nil || job != nil {
			t.Fatalf("reconciliation repeated a %s follow-up: %+v (%v)", state, job, err)
		}
	}
}

func TestCurrentRunCannotBorrowAnEarlierArchive(t *testing.T) {
	s := cycleReady(t)
	votedSuggestion(t, s, "candidate", "operating-layer", "README.md", 2)
	planned, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if err := fakeLanding(s, planned.JobID, 0); err != nil {
		t.Fatal(err)
	}
	now, err := time.Parse(time.RFC3339, s.Now())
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec(`INSERT INTO runs (id, job_id, agent, project, started_at, status)
		VALUES ('newer-unmeasured-run', ?, 'tester', 'demo', ?, 'done')`,
		planned.JobID, now.Add(time.Second).Format(time.RFC3339)); err != nil {
		t.Fatal(err)
	}
	if archive, err := archivedForJob(s, "demo", planned.JobID); err != nil || archive != nil {
		t.Fatalf("a new run reused an earlier attempt's archive: %+v (%v)", archive, err)
	}
	if job, err := PlanFeedback(s, "demo", "tester"); err != nil || job != nil {
		t.Fatalf("a new unmeasured run queued feedback: %+v (%v)", job, err)
	}
	if n := countRows(t, s, "evolution_archive"); n != 2 {
		t.Fatalf("archive reconciliation invented a row: %d", n)
	}
}
