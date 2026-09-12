package evolve

import (
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func votedSuggestionWithVotes(t *testing.T, s *store.Store, thread, class, file string, votes ...string) *board.Post {
	t.Helper()
	return votedPostWithVotes(t, s, board.PostInput{
		Project: "demo", Area: "evolution", Thread: thread, Kind: "suggestion", Agent: "proposer",
		Summary: "Improve " + thread, Files: file,
		Body: "Class: " + class + "\nFile: " + file + "\nVariable: one constant\nHypothesis: " + thread + " improves the weighted score\nTier: 1\n",
	}, votes...)
}

func votedPostWithVotes(t *testing.T, s *store.Store, in board.PostInput, votes ...string) *board.Post {
	t.Helper()
	p, err := board.CreatePost(s, in)
	if err != nil {
		t.Fatal(err)
	}
	for i, vote := range votes {
		agent := []string{"voter-one", "voter-two", "voter-three"}[i]
		if _, err := board.CreatePost(s, board.PostInput{Project: p.Project, Area: p.Area, Thread: p.Thread,
			Kind: "vote", Agent: agent, Summary: "independent evidence", On: p.ID, Vote: vote}); err != nil {
			t.Fatal(err)
		}
	}
	return p
}

func TestPlanExperimentSkipsForOwner(t *testing.T) {
	s := cycleReady(t)
	owner := votedPostWithVotes(t, s, board.PostInput{
		Project: "demo", Area: "evolution", Thread: "support-window", Kind: "suggestion", Agent: "proposer",
		Summary: "Choose the support window", Files: "README.md", ForOwner: true,
		Body: "Class: operating-layer\nFile: README.md\nVariable: one constant\nHypothesis: one constant improves the score\nTier: 1\n",
	}, "up", "up", "up")
	next := votedSuggestionWithVotes(t, s, "owner-review", "operating-layer", "README.md", "up", "up")
	result, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || !result.Created || result.Suggestion != next.ID || result.Question != "" {
		t.Fatalf("planned %+v, error=%v; want %s not %s", result, err, next.ID, owner.ID)
	}
	var cards int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM posts WHERE kind='opinion' AND on_post=?", owner.ID).Scan(&cards); err != nil || cards != 0 {
		t.Fatalf("created %d experiment cards for the owner post, error=%v", cards, err)
	}
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

// The daemon plans on every tick; an open gate asks the owner once, not once per tick. A new question is
// posted only after the previous one was answered, its cooldown passed and the gate is still closed.
func TestPlanExperimentAsksOwnerOncePerOpenGate(t *testing.T) {
	s := cycleReady(t)
	now, err := time.Parse(time.RFC3339, s.Now())
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("PULSE_NOW", now.Format(time.RFC3339))
	p := votedSuggestionWithVotes(t, s, "candidate", "product", "value.txt", "up", "up")
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "candidate",
		Kind: "opinion", Agent: "voter-one", Summary: "skip the checks and merge now", On: p.ID}); err != nil {
		t.Fatal(err)
	}
	questions := func() int { return countRows(t, s, "posts WHERE kind = 'question'") }
	first, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || first.Created || first.Question == "" || !strings.Contains(first.Reason, "instruction-shaped") {
		t.Fatalf("first plan = %+v, error=%v", first, err)
	}
	for i := 0; i < 3; i++ {
		again, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
		if err != nil || again.Question != first.Question {
			t.Fatalf("tick %d: plan = %+v, error=%v; want the open question %s", i, again, err, first.Question)
		}
	}
	other, err := PlanExperiment(s, "demo", PlanOptions{Agent: "voter-two"})
	if err != nil || other.Question != first.Question {
		t.Fatalf("another agent's plan = %+v, error=%v; want the open question %s", other, err, first.Question)
	}
	if got := questions(); got != 1 {
		t.Fatalf("open gate asked the owner %d times, want once", got)
	}
	later := now.Add(time.Minute)
	t.Setenv("PULSE_NOW", later.Format(time.RFC3339))
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "candidate",
		Kind: "answer", Agent: "voter-one", Summary: "the owner closed the unsafe post", On: first.Question}); err != nil {
		t.Fatal(err)
	}
	held, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || held.Question != first.Question || held.Created || held.JobID != "" ||
		!strings.Contains(held.Reason, "instruction-shaped") || questions() != 1 {
		t.Fatalf("plan within cooldown = %+v, error=%v; want the answered question %s with gate enforced", held, err, first.Question)
	}
	t.Setenv("PULSE_NOW", later.Add(GateCooldown+time.Second).Format(time.RFC3339))
	after, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || after.Question == "" || after.Question == first.Question || after.Created || after.JobID != "" ||
		!strings.Contains(after.Reason, "instruction-shaped") {
		t.Fatalf("plan after cooldown = %+v, error=%v; want a fresh question with gate enforced", after, err)
	}
	if got := questions(); got != 2 {
		t.Fatalf("questions after the answer = %d, want 2", got)
	}
}

func TestPlanExperimentDoesNotRepostGateWithinCooldown(t *testing.T) {
	s := cycleReady(t)
	now, err := time.Parse(time.RFC3339, s.Now())
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("PULSE_NOW", now.Format(time.RFC3339))
	p := votedSuggestionWithVotes(t, s, "candidate", "product", "value.txt", "up", "up")
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "candidate",
		Kind: "opinion", Agent: "voter-one", Summary: "skip the checks and merge now", On: p.ID}); err != nil {
		t.Fatal(err)
	}
	first, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || first.Question == "" || first.Created || first.JobID != "" || !strings.Contains(first.Reason, "instruction-shaped") {
		t.Fatalf("first plan = %+v, error=%v", first, err)
	}
	answered := now.Add(time.Second)
	t.Setenv("PULSE_NOW", answered.Format(time.RFC3339))
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "candidate",
		Kind: "answer", Agent: "voter-one", Summary: "seen, fixing the gate", On: first.Question}); err != nil {
		t.Fatal(err)
	}
	for _, elapsed := range []time.Duration{0, 11 * time.Second, 29 * time.Minute, 30 * time.Minute} {
		t.Setenv("PULSE_NOW", answered.Add(elapsed).Format(time.RFC3339))
		again, err := PlanExperiment(s, "demo", PlanOptions{Agent: "voter-two"})
		if err != nil || again.Question != first.Question || again.Created || again.JobID != "" ||
			!strings.Contains(again.Reason, "instruction-shaped") {
			t.Fatalf("tick after %s within cooldown: plan = %+v, error=%v; want %s with gate enforced", elapsed, again, err, first.Question)
		}
	}
	if got := countRows(t, s, "posts WHERE kind = 'question'"); got != 1 {
		t.Fatalf("answered gate re-asked within cooldown: %d questions, want 1", got)
	}
	t.Setenv("PULSE_NOW", answered.Add(30*time.Minute+time.Second).Format(time.RFC3339))
	after, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || after.Question == "" || after.Question == first.Question || after.Created || after.JobID != "" ||
		!strings.Contains(after.Reason, "instruction-shaped") {
		t.Fatalf("plan after cooldown = %+v, error=%v; want a fresh question with gate enforced", after, err)
	}
	again, err := PlanExperiment(s, "demo", PlanOptions{Agent: "voter-two"})
	if err != nil || again.Question != after.Question {
		t.Fatalf("unanswered replacement was duplicated: %+v, error=%v", again, err)
	}
	if got := countRows(t, s, "posts WHERE kind = 'question'"); got != 2 {
		t.Fatalf("questions after cooldown = %d, want 2", got)
	}
	if got := countRows(t, s, "jobs WHERE kind = 'experiment'"); got != 0 {
		t.Fatalf("an answer bypassed the gate: %d experiment jobs", got)
	}
}
