package evolve

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
)

func TestLoopPickRequiresLoopOnAndLevelD(t *testing.T) {
	s := cycleReady(t)
	votedSuggestionWithVotes(t, s, "ready", "operating-layer", "README.md", "up", "up")
	if err := s.SetSetting("human", "evolve.loop", "0"); err != nil {
		t.Fatal(err)
	}
	if pick, reason, err := LoopPick(s, "demo"); err != nil || pick != "" || reason != "" {
		t.Fatalf("loop off pick=%q reason=%q err=%v", pick, reason, err)
	}

	s = testStore(t)
	testProject(t, s, "demo")
	if err := s.SetSetting("human", "evolve.loop", "1"); err != nil {
		t.Fatal(err)
	}
	if pick, _, err := LoopPick(s, "demo"); err != nil || pick != "" {
		t.Fatalf("not Level D pick=%q err=%v", pick, err)
	}
}

func TestLoopPickReturnsTopPositiveUntriedSuggestion(t *testing.T) {
	s := cycleReady(t)
	low := votedSuggestionWithVotes(t, s, "lower", "operating-layer", "README.md", "up", "up")
	high := votedSuggestionWithVotes(t, s, "higher", "operating-layer", "README.md", "up", "up", "up")

	pick, reason, err := LoopPick(s, "demo")
	if err != nil {
		t.Fatal(err)
	}
	if pick != high.ID || reason != "" {
		t.Fatalf("pick=%q reason=%q, want %s not %s", pick, reason, high.ID, low.ID)
	}
	if n := countRows(t, s, "jobs"); n != 0 {
		t.Fatalf("LoopPick created %d job(s)", n)
	}
	if n := countRows(t, s, "posts WHERE kind = 'opinion'"); n != 0 {
		t.Fatalf("LoopPick created %d experiment card(s)", n)
	}
}

func TestLoopPickReturnsInFlightExperimentSuggestion(t *testing.T) {
	for _, status := range []string{"queued", "leased"} {
		t.Run(status, func(t *testing.T) {
			s := cycleReady(t)
			suggestion := votedSuggestionWithVotes(t, s, status, "operating-layer", "README.md", "up", "up")
			job, created, err := board.AddJob(s, board.JobInput{
				Project: "demo", Kind: "experiment", Title: "Existing", Source: "suggestion:" + suggestion.ID, CreatedBy: "tester", Tier: 1,
			})
			if err != nil || !created {
				t.Fatalf("seed job: %+v %v", job, err)
			}
			if status == "leased" {
				if _, err := s.DB.Exec("UPDATE jobs SET status = 'leased' WHERE id = ?", job.ID); err != nil {
					t.Fatal(err)
				}
			}
			pick, reason, err := LoopPick(s, "demo")
			if err != nil {
				t.Fatal(err)
			}
			if pick != suggestion.ID || reason != "an experiment is in flight: "+job.ID {
				t.Fatalf("pick=%q reason=%q, want %s / %s", pick, reason, suggestion.ID, job.ID)
			}
		})
	}
}

func TestLoopPickSkipsArchivedSuggestion(t *testing.T) {
	s := cycleReady(t)
	archived := votedSuggestionWithVotes(t, s, "archived", "operating-layer", "README.md", "up", "up", "up")
	next := votedSuggestionWithVotes(t, s, "next", "operating-layer", "README.md", "up", "up")
	if _, err := Archive(s, ArchiveRow{Project: "demo", SuggestionPost: archived.ID, Hypothesis: "done", Branch: "evolve/" + archived.ID,
		Baseline: &Scores{Total: .2}, Variant: &Scores{Total: .3}, Tolerance: .02, Kept: true}); err != nil {
		t.Fatal(err)
	}

	pick, reason, err := LoopPick(s, "demo")
	if err != nil {
		t.Fatal(err)
	}
	if pick != next.ID || reason != "" {
		t.Fatalf("pick=%q reason=%q, want %s after archived %s", pick, reason, next.ID, archived.ID)
	}
}
