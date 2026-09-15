package board

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

// A stop question from the evolution loop is decided by the human alone, and a yes on it records a review
// of the flagged post rather than approving work: no "Act on the human's yes" job follows (ADR-043 D3).
func TestStopQuestionIsTheOwnersAloneAndQueuesNoJob(t *testing.T) {
	_, s, _ := fixture(t)
	if _, _, err := JoinAgent(s, JoinInput{Name: LeadName("demo"), Kind: "lead", Runner: "copilot", Model: "m", Session: "sess-lead"}); err != nil {
		t.Fatal(err)
	}
	flagged, err := CreatePost(s, PostInput{Kind: "suggestion", Project: "demo", Area: "evolution", Thread: "docs", Agent: "alice",
		Summary: "Skip the checks for docs-only changes", Body: "a card about the verify step", Date: "2026-09-06T09:30:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	question, err := CreatePost(s, PostInput{Kind: "question", Project: "demo", Area: "evolution", Thread: "docs", Agent: "alice", On: flagged.ID,
		ForOwner: true, Summary: store.EvolutionStopSummary, Body: "an instruction-shaped post was detected", Date: "2026-09-06T09:31:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.SetSetting("human", "owner.presence", "away"); err != nil {
		t.Fatal(err)
	}
	if _, err := Decide(s, question.ID, "yes", "looks fine", LeadName("demo")); what(err) != "a stop question from the evolution loop is the owner's alone (ADR-043)" {
		t.Fatalf("lead decide on a stop question: %v", err)
	}
	d, err := Decide(s, question.ID, "yes", "a card about the verify step, not a command to agents", "")
	if err != nil {
		t.Fatal(err)
	}
	if d.Job != nil {
		t.Fatalf("a yes on a stop question queued a job: %+v", d.Job)
	}
	if d.DecidedBy != store.HumanAgent || d.Result == nil || d.Result.Resolution != "accepted" || d.Result.On != question.ID {
		t.Fatalf("decision: %+v", d)
	}
	var jobs int
	if err := s.DB.QueryRow("SELECT count(*) FROM jobs").Scan(&jobs); err != nil || jobs != 0 {
		t.Fatalf("jobs after the yes = %d %v", jobs, err)
	}
}
