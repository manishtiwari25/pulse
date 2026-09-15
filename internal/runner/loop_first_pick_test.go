package runner

import (
	"bytes"
	"context"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

func loopFirstSuggestion(t *testing.T, s *store.Store, thread, summary string, votes int) *board.Post {
	t.Helper()
	post, err := board.CreatePost(s, board.PostInput{Kind: "suggestion", Project: "proj", Area: "evolution", Thread: thread,
		Agent: "proposer", Summary: summary, Files: "README.md",
		Body: "Class: operating-layer\nFile: README.md\nVariable: one constant\nHypothesis: README gets clearer\nTier: 1\nSignal: README.md\n"})
	if err != nil {
		t.Fatal(err)
	}
	voters := []string{"voter-one", "voter-two", "voter-three"}
	for i := 0; i < votes; i++ {
		if _, err := board.CreatePost(s, board.PostInput{Kind: "vote", Project: "proj", Area: "evolution", Thread: thread,
			Agent: voters[i], Summary: "measured", On: post.ID, Vote: "up"}); err != nil {
			t.Fatal(err)
		}
	}
	return post
}

func withClosedLoginGate(t *testing.T) {
	t.Helper()
	prev := loginGate
	loginGate = func(*store.Store) string {
		return "workers on this machine wait for a Gate Fake login: sign in with GitHub"
	}
	t.Cleanup(func() { loginGate = prev })
}

// plainSuggestion is a worker's one-line SUGGESTION: voted and pointing at evidence, but naming no File: and
// no Variable:, so the loop cannot run it as an experiment and does not claim it.
func plainSuggestion(t *testing.T, s *store.Store, thread, summary string, votes int) *board.Post {
	t.Helper()
	post, err := board.CreatePost(s, board.PostInput{Kind: "suggestion", Project: "proj", Area: "evolution", Thread: thread,
		Agent: "proposer", Summary: summary, Body: "One-variable improvement seen by the worker in run `r` of job `j` (Reflect), not made there.\n\n" + summary + "\nSignal: README.md\n"})
	if err != nil {
		t.Fatal(err)
	}
	voters := []string{"voter-one", "voter-two", "voter-three"}
	for i := 0; i < votes; i++ {
		if _, err := board.CreatePost(s, board.PostInput{Kind: "vote", Project: "proj", Area: "evolution", Thread: thread,
			Agent: voters[i], Summary: "measured", On: post.ID, Vote: "up"}); err != nil {
			t.Fatal(err)
		}
	}
	return post
}

// The loop runs the top valid card as an experiment and claims every other valid card (ADR-044 D4); the
// improve planner defers those once each and builds what the loop cannot measure - a card-less one-liner.
func TestDaemonGivesEvolutionLoopEveryValidCardAndPlannerBuildsRest(t *testing.T) {
	resetEvolvePlanReasons()
	withClosedLoginGate(t)
	s := newStore(t)
	newRepo(t, s)
	evolvePlanReady(t, s)
	for _, name := range []string{"proposer", "voter-one", "voter-two", "voter-three"} {
		join(t, s, name)
	}
	// the planner walks the tally in vote order and stops at the first job it builds: second, a valid card
	// ranked before the one-liner, must be passed over (deferred) for the one-liner to be built
	second := loopFirstSuggestion(t, s, "second", "Tune README second", 2)
	plain := plainSuggestion(t, s, "plain", "Tune the README intro", 2)
	top := loopFirstSuggestion(t, s, "top", "Tune README top", 3)

	var out bytes.Buffer
	if result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: all good'", Out: &out}); result != "done" {
		t.Fatalf("daemon = %s\n%s", result, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'experiment' AND source = ?", "suggestion:"+top.ID); n != 1 {
		t.Fatalf("top suggestion experiment count = %d\n%s", n, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'improve' AND source = ?", "suggestion:"+plain.ID); n != 1 {
		t.Fatalf("card-less suggestion improve count = %d\n%s", n, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'improve' AND source IN (?, ?)", "suggestion:"+top.ID, "suggestion:"+second.ID); n != 0 {
		t.Fatalf("improve jobs for the loop's cards = %d\n%s", n, out.String())
	}
	for _, claimed := range []*board.Post{top, second} {
		line := "plan proj: left suggestion " + claimed.ID + " to the evolution loop"
		if strings.Count(out.String(), line) != 1 {
			t.Fatalf("deferral line missing once (%q):\n%s", line, out.String())
		}
		if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'plan.deferred' AND subject = ?", claimed.ID); n != 1 {
			t.Fatalf("plan.deferred events for %s = %d", claimed.ID, n)
		}
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'plan.deferred' AND subject = ?", plain.ID); n != 0 {
		t.Fatalf("the card-less suggestion was deferred to the loop %d time(s)", n)
	}

	if result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: all good'", Out: &out}); result != "done" {
		t.Fatalf("second daemon = %s\n%s", result, out.String())
	}
	for _, claimed := range []*board.Post{top, second} {
		line := "plan proj: left suggestion " + claimed.ID + " to the evolution loop"
		if strings.Count(out.String(), line) != 1 {
			t.Fatalf("deferral line repeated (%q):\n%s", line, out.String())
		}
		if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'plan.deferred' AND subject = ?", claimed.ID); n != 1 {
			t.Fatalf("plan.deferred events for %s after second tick = %d", claimed.ID, n)
		}
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source IN (?, ?, ?)", "suggestion:"+top.ID, "suggestion:"+second.ID, "suggestion:"+plain.ID); n != 2 {
		t.Fatalf("jobs after the second tick = %d, want the experiment and the improve job only", n)
	}
}

// A voted one-liner the loop ranks first gets an agent's card job instead of an owner question (ADR-044 D1),
// and while that job is open the improve planner leaves the suggestion alone.
func TestDaemonQueuesCardJobForCardlessTopPick(t *testing.T) {
	resetEvolvePlanReasons()
	withClosedLoginGate(t)
	s := newStore(t)
	newRepo(t, s)
	evolvePlanReady(t, s)
	for _, name := range []string{"proposer", "voter-one", "voter-two", "voter-three"} {
		join(t, s, name)
	}
	plain := plainSuggestion(t, s, "plain", "Tune the README intro", 3)

	var out bytes.Buffer
	if result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: all good'", Out: &out}); result != "done" {
		t.Fatalf("daemon = %s\n%s", result, out.String())
	}
	jobs := mustRows(s.DB, "SELECT kind, title FROM jobs WHERE source = ?", "suggestion:"+plain.ID)
	if len(jobs) != 1 || jobs[0].str("kind") != "reflect" || !strings.HasPrefix(jobs[0].str("title"), evolve.CardJobTitle+plain.ID) {
		t.Fatalf("jobs for the card-less pick = %v\n%s", jobs, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question'"); n != 0 {
		t.Fatalf("the owner was asked %d question(s) for the card\n%s", n, out.String())
	}
	if !strings.Contains(out.String(), "planned card") {
		t.Fatalf("the card job was not logged:\n%s", out.String())
	}
}

func TestDaemonPlannerSuggestionBehaviorUnchangedWithLoopOff(t *testing.T) {
	resetEvolvePlanReasons()
	withClosedLoginGate(t)
	s := newStore(t)
	newRepo(t, s)
	evolvePlanReady(t, s)
	if err := s.SetSetting(humanAgent, "evolve.loop", "0"); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"proposer", "voter-one", "voter-two", "voter-three"} {
		join(t, s, name)
	}
	loopFirstSuggestion(t, s, "second", "Tune README second", 2)
	top := loopFirstSuggestion(t, s, "top", "Tune README top", 3)

	var out bytes.Buffer
	if result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: all good'", Out: &out}); result != "done" {
		t.Fatalf("daemon = %s\n%s", result, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'improve' AND source = ?", "suggestion:"+top.ID); n != 1 {
		t.Fatalf("top suggestion improve count = %d\n%s", n, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'experiment'"); n != 0 {
		t.Fatalf("experiment count with loop off = %d\n%s", n, out.String())
	}
	if strings.Contains(out.String(), "to the evolution loop") {
		t.Fatalf("loop-off daemon deferred a suggestion:\n%s", out.String())
	}
}
