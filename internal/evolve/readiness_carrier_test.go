package evolve

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
)

func TestReadinessMatchesBlockedExperimentCarrier(t *testing.T) {
	s := cycleReady(t)
	suggestion := votedSuggestion(t, s, "candidate", "operating-layer", "README.md", 2)
	job, _, err := board.AddJob(s, board.JobInput{
		Project: "demo", Kind: "experiment", Title: "Blocked experiment",
		Source: "suggestion:" + suggestion.ID, CreatedBy: "tester", Tier: 1,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := board.FinishJob(s, job.ID, "tester", "blocked", "needs an owner-provided tool", "", nil); err != nil {
		t.Fatal(err)
	}
	planned, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || planned.Created || planned.JobID != job.ID {
		t.Fatalf("existing carrier fixture: %+v (%v)", planned, err)
	}
	status, err := Status(s, "demo")
	if err != nil {
		t.Fatal(err)
	}
	if status.Planning.State != "blocked" || status.Planning.Job != job.ID ||
		status.Planning.Suggestion != suggestion.ID || !strings.Contains(status.Planning.Reason, "blocked") ||
		status.Planning.Reason != planned.Reason {
		t.Errorf("readiness disagrees with the existing blocked carrier: %+v; plan=%+v", status.Planning, planned)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	reflected, err := Reflect(ctx, s, "demo", "", "tester", "")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(strings.Join(reflected.NextCommands, "\n"), "pulse evolve cycle") {
		t.Error("reflect recommended a cycle whose existing carrier is blocked")
	}
}
