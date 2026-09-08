package evolve

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func votedSuggestionWithVotes(t *testing.T, s *store.Store, thread, class, file string, votes ...string) *board.Post {
	t.Helper()
	p, err := board.CreatePost(s, board.PostInput{
		Project: "demo", Area: "evolution", Thread: thread, Kind: "suggestion", Agent: "proposer",
		Summary: "Improve " + thread, Files: file,
		Body: "Class: " + class + "\nFile: " + file + "\nVariable: one constant\nHypothesis: " + thread + " improves the weighted score\nTier: 1\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	for i, vote := range votes {
		agent := []string{"voter-one", "voter-two", "voter-three"}[i]
		if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: thread,
			Kind: "vote", Agent: agent, Summary: "independent evidence", On: p.ID, Vote: vote}); err != nil {
			t.Fatal(err)
		}
	}
	return p
}

func TestPlanExperimentLevelGateHasNoSideEffects(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	joinTestAgent(t, s, "tester")

	result, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(result.Reason, "constitution record is missing") || !strings.Contains(result.Reason, "owner:") ||
		result.Created || result.JobID != "" || result.Question != "" {
		t.Fatalf("gate result: %+v", result)
	}
	if countRows(t, s, "jobs") != 0 || countRows(t, s, "posts") != 0 {
		t.Fatal("level gate planned a job or posted to the board")
	}
}

func TestPlanExperimentQueuesTopEligibleSuggestion(t *testing.T) {
	s := cycleReady(t)
	low := votedSuggestionWithVotes(t, s, "lower", "operating-layer", "README.md", "up", "up")
	high := votedSuggestionWithVotes(t, s, "higher", "operating-layer", "README.md", "up", "up", "up")

	result, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !result.Created || result.JobID == "" || result.Suggestion != high.ID {
		t.Fatalf("planned %+v, want %s not %s", result, high.ID, low.ID)
	}
	job, err := board.JobByID(s, result.JobID)
	if err != nil || job == nil {
		t.Fatalf("job: %+v %v", job, err)
	}
	if job.Kind != "experiment" || job.Source != "suggestion:"+high.ID || job.Branch != "evolve/"+high.ID ||
		job.Priority != 30 || job.Tier != 1 || job.CreatedBy != "tester" || !strings.Contains(job.Brief, "Experiment card") {
		t.Fatalf("job contract: %+v", job)
	}
	var card string
	if err := s.DB.QueryRow("SELECT body FROM posts WHERE kind = 'opinion' AND on_post = ? AND summary = ?", high.ID, "Experiment card: "+high.ID).Scan(&card); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(card, "Suggestion: "+high.ID) || !strings.Contains(card, "Branch: evolve/"+high.ID) {
		t.Fatalf("card post:\n%s", card)
	}
}

func TestPlanExperimentAgentRowDoesNotNeedStoredAgent(t *testing.T) {
	s := cycleReady(t)
	high := votedSuggestionWithVotes(t, s, "higher", "operating-layer", "README.md", "up", "up", "up")
	agentRow := &store.Agent{Name: "daemon", Display: "daemon", Sessions: []string{}}

	result, err := PlanExperiment(s, "demo", PlanOptions{Agent: "ignored", AgentRow: agentRow})
	if err != nil {
		t.Fatal(err)
	}
	if !result.Created || result.Suggestion != high.ID {
		t.Fatalf("planned with synthetic agent: %+v", result)
	}
	if a, err := s.Agent("daemon"); err != nil || a != nil {
		t.Fatalf("synthetic plan created an agent row: %+v %v", a, err)
	}
	job, err := board.JobByID(s, result.JobID)
	if err != nil || job == nil || job.CreatedBy != "daemon" {
		t.Fatalf("job actor: %+v %v", job, err)
	}
}

func TestPlanExperimentPositiveScoreOption(t *testing.T) {
	s := cycleReady(t)
	zero := votedSuggestionWithVotes(t, s, "zero", "operating-layer", "README.md", "up", "down")
	result, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester", RequirePositiveScore: true})
	if err != nil {
		t.Fatal(err)
	}
	if result.JobID != "" || result.Reason != "no eligible, untried, in-scope suggestion with net votes > 0; vote with pulse ballot" {
		t.Fatalf("positive-only result: %+v", result)
	}
	result, err = PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !result.Created || result.Suggestion != zero.ID {
		t.Fatalf("non-positive score should be allowed without the daemon option: %+v", result)
	}
}

func TestPlanExperimentInFlightExperiment(t *testing.T) {
	s := cycleReady(t)
	job, created, err := board.AddJob(s, board.JobInput{Project: "demo", Kind: "experiment", Title: "Existing", Source: "manual", CreatedBy: "tester", Tier: 1})
	if err != nil || !created {
		t.Fatalf("seed job: %+v %v", job, err)
	}
	result, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if result.JobID != job.ID || result.Reason != "an experiment is in flight: "+job.ID || result.Created {
		t.Fatalf("in-flight result: %+v", result)
	}

	s = cycleReady(t)
	job, created, err = board.AddJob(s, board.JobInput{Project: "demo", Kind: "experiment", Title: "Landing", Source: "manual", CreatedBy: "tester", Tier: 1})
	if err != nil || !created {
		t.Fatalf("seed landing job: %+v %v", job, err)
	}
	if _, err := board.FinishJob(s, job.ID, "tester", "done", "waiting to land", "evolve/waiting", nil); err != nil {
		t.Fatal(err)
	}
	result, err = PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if result.JobID != job.ID || result.Reason != "an experiment is in flight: "+job.ID || result.Created {
		t.Fatalf("landing result: %+v", result)
	}
}

func TestPlanExperimentSkipsArchivedSuggestion(t *testing.T) {
	s := cycleReady(t)
	archived := votedSuggestionWithVotes(t, s, "archived", "operating-layer", "README.md", "up", "up", "up")
	next := votedSuggestionWithVotes(t, s, "next", "operating-layer", "README.md", "up", "up")
	if _, err := Archive(s, ArchiveRow{Project: "demo", SuggestionPost: archived.ID, Hypothesis: "done", Branch: "evolve/" + archived.ID,
		Baseline: &Scores{Total: .2}, Variant: &Scores{Total: .3}, Tolerance: .02, Kept: true}); err != nil {
		t.Fatal(err)
	}
	result, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !result.Created || result.Suggestion != next.ID {
		t.Fatalf("archived suggestion was not skipped: %+v", result)
	}
}

func TestPlanExperimentRespectsProductScope(t *testing.T) {
	s := cycleReady(t)
	if _, err := s.PutRecord(store.Record{ID: "EVO-evaluators-demo", Project: "demo", Kind: "evolution", Slug: "evaluators"},
		nil, "Product scope: off\n", "human", "fixture"); err != nil {
		t.Fatal(err)
	}
	product := votedSuggestionWithVotes(t, s, "product", "product", "value.txt", "up", "up", "up")
	operating := votedSuggestionWithVotes(t, s, "operating", "operating-layer", "README.md", "up", "up")
	result, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !result.Created || result.Suggestion != operating.ID || result.Suggestion == product.ID {
		t.Fatalf("product-scope result: %+v", result)
	}
}
