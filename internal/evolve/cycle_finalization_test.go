package evolve

import (
	"bytes"
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
)

func TestCycleWaitsForRunnerAndLandingFinalization(t *testing.T) {
	s := cycleReady(t)
	votedSuggestion(t, s, "candidate", "operating-layer", "README.md", 2)
	events, unsubscribe := bus.Default().Subscribe(bus.Filter{Actions: []string{"job.add"}}, 8)
	defer unsubscribe()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	finished := make(chan struct{})
	defer func() { cancel(); <-finished }()
	var out bytes.Buffer
	done := make(chan error, 1)
	go func() {
		defer close(finished)
		done <- Cycle(ctx, s, "demo", CycleOptions{Once: true, Agent: "tester", Out: &out, Interval: 10 * time.Millisecond})
	}()
	var job string
	select {
	case event := <-events:
		job = event.Subject
	case <-ctx.Done():
		t.Fatal("no fake experiment was planned")
	}
	if err := fakeArchive(s, job); err != nil {
		t.Fatal(err)
	}
	if _, err := board.FinishJob(s, job, "tester", "done", "worker finished; landing verification pending", "", nil); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-done:
		t.Fatalf("cycle reported an archive before landing/controller finalization: %v\n%s", err, out.String())
	case <-time.After(60 * time.Millisecond):
	}
	if _, err := s.DB.Exec("UPDATE runs SET status = 'done' WHERE job_id = ?", job); err != nil {
		t.Fatal(err)
	}
	archive, err := archivedForJob(s, "demo", job)
	if err != nil {
		t.Fatal(err)
	}
	if err := fakeFinalizeArchive(s, job, "land.landed", archive); err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-done:
		if err != nil || !strings.Contains(out.String(), "B-002") {
			t.Fatalf("finalized cycle: %v\n%s", err, out.String())
		}
	case <-ctx.Done():
		t.Fatal("finalized archive was not reported")
	}
}

func TestCycleFailedArchiveIsNotEmittedAsSuccess(t *testing.T) {
	s := cycleReady(t)
	votedSuggestion(t, s, "candidate", "operating-layer", "README.md", 2)
	events, unsubscribe := bus.Default().Subscribe(bus.Filter{Actions: []string{"job.add"}}, 8)
	defer unsubscribe()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	finished := make(chan struct{})
	defer func() { cancel(); <-finished }()
	var out bytes.Buffer
	done := make(chan error, 1)
	go func() {
		defer close(finished)
		done <- Cycle(ctx, s, "demo", CycleOptions{Once: true, Agent: "tester", Out: &out, JSON: true, Interval: 10 * time.Millisecond})
	}()
	select {
	case event := <-events:
		if err := fakeArchive(s, event.Subject); err != nil {
			t.Fatal(err)
		}
		if _, err := board.FinishJob(s, event.Subject, "tester", "failed", "landing failed after measurement", "", nil); err != nil {
			t.Fatal(err)
		}
	case <-ctx.Done():
		t.Fatal("no fake experiment was planned")
	}
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-ctx.Done():
		t.Fatal("failed cycle was not reported")
	}
	decoder := json.NewDecoder(&out)
	var result CycleResult
	if err := decoder.Decode(&result); err != nil {
		t.Fatal(err)
	}
	if !result.Stopped || result.Archive != nil || !strings.Contains(result.Reason, "failed") {
		t.Fatalf("failed experiment emitted a success-shaped measured result: %+v", result)
	}
}

func TestCycleFinishedWithoutCurrentArchiveStops(t *testing.T) {
	for _, priorArchive := range []bool{false, true} {
		name := "missing"
		if priorArchive {
			name = "earlier-attempt-only"
		}
		t.Run(name, func(t *testing.T) {
			s := cycleReady(t)
			votedSuggestion(t, s, "candidate", "operating-layer", "README.md", 2)
			events, unsubscribe := bus.Default().Subscribe(bus.Filter{Actions: []string{"job.add"}}, 8)
			defer unsubscribe()
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			finished := make(chan struct{})
			defer func() { cancel(); <-finished }()
			var out bytes.Buffer
			done := make(chan error, 1)
			go func() {
				defer close(finished)
				done <- Cycle(ctx, s, "demo", CycleOptions{Once: true, Agent: "tester", Out: &out, JSON: true, Interval: 10 * time.Millisecond})
			}()
			var job string
			select {
			case event := <-events:
				job = event.Subject
			case <-ctx.Done():
				t.Fatal("no fake experiment was planned")
			}
			if priorArchive {
				if err := fakeArchive(s, job); err != nil {
					t.Fatal(err)
				}
				if _, err := s.DB.Exec("UPDATE runs SET status = 'done' WHERE job_id = ?", job); err != nil {
					t.Fatal(err)
				}
			}
			now, err := time.Parse(time.RFC3339, s.Now())
			if err != nil {
				t.Fatal(err)
			}
			if _, err := s.DB.Exec(`INSERT INTO runs (id, job_id, project, agent, started_at, status)
				VALUES ('unmeasured-current-run', ?, 'demo', 'tester', ?, 'done')`,
				job, now.Add(time.Second).Format(time.RFC3339)); err != nil {
				t.Fatal(err)
			}
			if _, err := board.FinishJob(s, job, "tester", "done", "no measured result", "", nil); err != nil {
				t.Fatal(err)
			}
			if _, err := s.DB.Exec("UPDATE jobs SET branch = '' WHERE id = ?", job); err != nil {
				t.Fatal(err)
			}
			select {
			case err := <-done:
				if err != nil {
					t.Fatal(err)
				}
			case <-ctx.Done():
				t.Fatal("unmeasured completion did not stop")
			}
			var result CycleResult
			if err := json.NewDecoder(&out).Decode(&result); err != nil {
				t.Fatal(err)
			}
			if !result.Stopped || result.Archive != nil || !strings.Contains(result.Reason, "without an archive row") {
				t.Fatalf("unmeasured completion was reported as success: %+v", result)
			}
			if n := countRows(t, s, "jobs WHERE source LIKE 'evolve-feedback:%'"); n != 0 {
				t.Fatalf("unmeasured completion queued %d follow-ups", n)
			}
		})
	}
}
