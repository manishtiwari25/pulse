package evolve

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

// cardlessSuggestion is the shape the runner posts from a worker's one-line SUGGESTION: no File:, no Variable:.
func cardlessSuggestion(t *testing.T, s *store.Store, thread, summary string, votes ...string) *board.Post {
	t.Helper()
	return votedPostWithVotes(t, s, board.PostInput{
		Project: "demo", Area: "evolution", Thread: thread, Kind: "suggestion", Agent: "proposer",
		Summary: summary, Body: "One-variable improvement seen by the worker in run `run-1` of job `job-1` (Daily reflect), not made there.\n\n" + summary + "\n",
	}, votes...)
}

func suggestionStatus(t *testing.T, s *store.Store, id string) tally.Suggestion {
	t.Helper()
	result, err := tally.Compute(s, "demo", "", tally.Thresholds{})
	if err != nil {
		t.Fatal(err)
	}
	for _, sg := range result.Suggestions {
		if sg.ID == id {
			return sg
		}
	}
	t.Fatalf("suggestion %s not in the tally", id)
	return tally.Suggestion{}
}

func finishCardJob(t *testing.T, s *store.Store, id, status string) {
	t.Helper()
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'leased', lease_agent = 'card-writer' WHERE id = ?", id); err != nil {
		t.Fatal(err)
	}
	if _, err := board.FinishJob(s, id, "card-writer", status, "SUMMARY: card posted", "", nil); err != nil {
		t.Fatal(err)
	}
}

// ADR-044 D1-D3: a voted pick without a card gets one reflect job with the suggestion's source, asked of an
// agent and never of the owner; tally holds the suggestion while the job runs and marks it carded after;
// the card the agent posts earns its own votes and is then the experiment.
func TestPlanExperimentQueuesACardJobForACardlessPick(t *testing.T) {
	s := cycleReady(t)
	joinTestAgent(t, s, "card-writer")
	idea := cardlessSuggestion(t, s, "idea", "Shorten the README intro", "up", "up")

	res, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !res.CardCreated || res.CardJob == "" || res.Created || res.JobID != "" || res.Question != "" || res.Suggestion != idea.ID {
		t.Fatalf("plan = %+v; want a new card job for %s and no experiment", res, idea.ID)
	}
	if !strings.Contains(res.Reason, "card job "+res.CardJob) {
		t.Fatalf("reason %q does not name the card job", res.Reason)
	}
	if n := countRows(t, s, "posts WHERE kind = 'question'"); n != 0 {
		t.Fatalf("the owner was asked for the card: %d question(s)", n)
	}
	job, err := board.JobByID(s, res.CardJob)
	if err != nil || job == nil {
		t.Fatalf("card job: %+v %v", job, err)
	}
	if job.Kind != "reflect" || job.Tier != 0 || job.Source != "suggestion:"+idea.ID || !strings.HasPrefix(job.Title, CardJobTitle+idea.ID) || job.Priority != cardJobPriority {
		t.Fatalf("card job = %+v", job)
	}
	for _, want := range []string{
		"pulse post suggestion --project demo --area evolution --thread idea --on " + idea.ID,
		"File: <the one repository-relative file", "Variable: <the one thing that changes",
		"Hypothesis:", "Tier: 0 | 1", "Refines: " + idea.ID,
		"change no files and make no commits", "Do not vote on your own card", "SUMMARY: no card:",
		"product scope on this project - ", "Shorten the README intro",
	} {
		if !strings.Contains(job.Brief, want) {
			t.Fatalf("card job brief lacks %q:\n%s", want, job.Brief)
		}
	}

	// The second tick reuses the open job: one card job per pick, and the pick is held meanwhile.
	again, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if again.CardCreated || again.CardJob != res.CardJob || countRows(t, s, "jobs") != 1 {
		t.Fatalf("second tick = %+v with %d job(s)", again, countRows(t, s, "jobs"))
	}
	if sg := suggestionStatus(t, s, idea.ID); sg.Status != "eligible" || sg.Job != res.CardJob || tally.InFlightGate(sg) == "" {
		t.Fatalf("tally while the card is written = %+v; want eligible, held by the card job", sg)
	}
	claims, reason, err := LoopClaims(s, "demo", "daemon")
	if err != nil {
		t.Fatal(err)
	}
	if claims[idea.ID] || !strings.Contains(reason, "valid experiment card") {
		t.Fatalf("claims = %v reason %q; a card-less pick is held by its card job, not claimed", claims, reason)
	}

	// The job ends: the pick is carded, terminal for the loop and the ballot, and no second card job follows.
	finishCardJob(t, s, res.CardJob, "done")
	if sg := suggestionStatus(t, s, idea.ID); sg.Status != "carded" {
		t.Fatalf("tally after the card job = %+v; want carded", sg)
	}
	after, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if after.CardJob != "" || after.Created || countRows(t, s, "jobs") != 1 || !strings.Contains(after.Reason, "no eligible") {
		t.Fatalf("plan after carding = %+v with %d job(s)", after, countRows(t, s, "jobs"))
	}

	// The card the agent posted on the thread earns its own votes and becomes the experiment.
	card := votedPostWithVotes(t, s, board.PostInput{
		Project: "demo", Area: "evolution", Thread: "idea", Kind: "suggestion", Agent: "card-writer", On: idea.ID,
		Summary: "Cut the README intro to one paragraph",
		Body:    "Class: operating-layer\nFile: README.md\nVariable: intro length\nHypothesis: the README check scores higher\nTier: 1\nSignal: README.md\nRefines: " + idea.ID + "\n",
	})
	if res, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"}); err != nil || res.Created {
		t.Fatalf("a card without quorum was planned: %+v %v", res, err)
	}
	for _, voter := range []string{"voter-one", "voter-two"} {
		if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "idea",
			Kind: "vote", Agent: voter, Summary: "one file, one variable", On: card.ID, Vote: "up"}); err != nil {
			t.Fatal(err)
		}
	}
	claims, _, err = LoopClaims(s, "demo", "daemon")
	if err != nil || !claims[card.ID] || claims[idea.ID] {
		t.Fatalf("claims = %v (%v); want the card and not the carded pick", claims, err)
	}
	res, err = PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !res.Created || res.Suggestion != card.ID {
		t.Fatalf("plan with the voted card = %+v; want an experiment for %s", res, card.ID)
	}
	if job, _ := board.JobByID(s, res.JobID); job == nil || job.Kind != "experiment" || job.Source != "suggestion:"+card.ID {
		t.Fatalf("experiment job = %+v", job)
	}
}

func TestPlanExperimentWritesOneCardAtATime(t *testing.T) {
	s := cycleReady(t)
	joinTestAgent(t, s, "card-writer")
	first := cardlessSuggestion(t, s, "first", "Rename the flag", "up", "up", "up")
	cardlessSuggestion(t, s, "second", "Trim the log line", "up", "up")

	res, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !res.CardCreated || res.Suggestion != first.ID {
		t.Fatalf("plan = %+v; want one card job, for the top pick %s", res, first.ID)
	}
	if _, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"}); err != nil {
		t.Fatal(err)
	}
	if n := countRows(t, s, "jobs WHERE kind = 'reflect'"); n != 1 {
		t.Fatalf("card jobs = %d; the loop writes one card at a time", n)
	}
	// A failed card job leaves the pick tried, and the next card-less pick gets its turn.
	finishCardJob(t, s, res.CardJob, "failed")
	if sg := suggestionStatus(t, s, first.ID); sg.Status != "tried" {
		t.Fatalf("tally after a failed card job = %+v; want tried", sg)
	}
	next, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !next.CardCreated || next.Suggestion == first.ID {
		t.Fatalf("plan after the failure = %+v; want a card job for the second pick", next)
	}
}

// The loop plans the first valid card even when a card-less suggestion outranks it; the card-less
// pick gets its card job in the same tick.
func TestPlanExperimentRunsTheFirstValidCardPastACardlessPick(t *testing.T) {
	s := cycleReady(t)
	idea := cardlessSuggestion(t, s, "idea", "Make the daemon faster", "up", "up", "up")
	card := votedSuggestionWithVotes(t, s, "card", "operating-layer", "README.md", "up", "up")

	res, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !res.Created || res.Suggestion != card.ID {
		t.Fatalf("plan = %+v; want the experiment for %s", res, card.ID)
	}
	if n := countRows(t, s, "jobs WHERE kind = 'reflect' AND source = 'suggestion:"+idea.ID+"'"); n != 1 {
		t.Fatalf("card jobs for the card-less pick = %d, want 1", n)
	}
	pick, reason, err := LoopPick(s, "demo")
	if err != nil || pick != card.ID || !strings.Contains(reason, "in flight") {
		t.Fatalf("pick=%q reason=%q err=%v; want the in-flight card %s", pick, reason, err, card.ID)
	}
}

// ADR-044 D4: the loop claims every eligible suggestion whose card is valid, not only the first; a
// card-less suggestion and one held by an open improve job are not claimed.
func TestLoopClaimsEveryValidCard(t *testing.T) {
	s := cycleReady(t)
	idea := cardlessSuggestion(t, s, "idea", "Make the daemon faster", "up", "up", "up")
	first := votedSuggestionWithVotes(t, s, "first", "operating-layer", "README.md", "up", "up", "up")
	second := votedSuggestionWithVotes(t, s, "second", "operating-layer", "README.md", "up", "up")
	building := votedSuggestionWithVotes(t, s, "building", "operating-layer", "README.md", "up", "up")
	if _, _, err := board.AddJob(s, board.JobInput{Project: "demo", Kind: "improve", Title: "Implement", Tier: 1,
		Source: "suggestion:" + building.ID, CreatedBy: "tester"}); err != nil {
		t.Fatal(err)
	}

	claims, reason, err := LoopClaims(s, "demo", "daemon")
	if err != nil {
		t.Fatal(err)
	}
	if !claims[first.ID] || !claims[second.ID] || claims[idea.ID] || claims[building.ID] || reason != "" {
		t.Fatalf("claims = %v reason %q; want %s and %s only", claims, reason, first.ID, second.ID)
	}
	if pick, _, err := LoopPick(s, "demo"); err != nil || pick != first.ID {
		t.Fatalf("pick = %q (%v), want the top valid card %s", pick, err, first.ID)
	}
	res, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if !res.Created || res.Suggestion != first.ID {
		t.Fatalf("plan = %+v; want the experiment for %s", res, first.ID)
	}
	if n := countRows(t, s, "jobs WHERE source = 'suggestion:"+building.ID+"'"); n != 1 {
		t.Fatalf("jobs for the suggestion an improve job holds = %d, want the improve job alone", n)
	}
}

// A card question the planner asked the owner before ADR-044 is moot: it neither blocks the readout nor
// outranks the carded state of the pick it was about.
func TestReadinessIgnoresPreADR044CardQuestions(t *testing.T) {
	s := cycleReady(t)
	joinTestAgent(t, s, "card-writer")
	idea := cardlessSuggestion(t, s, "idea", "Shorten the README intro", "up", "up")
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: idea.Area, Thread: idea.Thread, Kind: "question",
		Agent: "tester", ForOwner: true, On: idea.ID, Summary: StopSummary,
		Body: cardNeeded + "; name exactly one File: and one Variable: in a new suggestion; the original post is immutable; owner: pulse post suggestion ... The workflow was not bypassed."}); err != nil {
		t.Fatal(err)
	}
	res, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || !res.CardCreated {
		t.Fatalf("plan = %+v (%v); want a card job despite the old question", res, err)
	}
	finishCardJob(t, s, res.CardJob, "done")
	status, err := Status(s, "demo")
	if err != nil {
		t.Fatal(err)
	}
	if status.Planning.State == "blocked" || strings.Contains(status.Planning.Reason, cardNeeded) {
		t.Fatalf("planning = %+v; an old card question is not a gate", status.Planning)
	}
}
