package runner

import (
	"bytes"
	"context"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
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

func TestDaemonGivesEvolutionLoopFirstPickAndPlannerBuildsRest(t *testing.T) {
	resetEvolvePlanReasons()
	withClosedLoginGate(t)
	s := newStore(t)
	newRepo(t, s)
	evolvePlanReady(t, s)
	for _, name := range []string{"proposer", "voter-one", "voter-two", "voter-three"} {
		join(t, s, name)
	}
	second := loopFirstSuggestion(t, s, "second", "Tune README second", 2)
	top := loopFirstSuggestion(t, s, "top", "Tune README top", 3)

	var out bytes.Buffer
	if result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: all good'", Out: &out}); result != "done" {
		t.Fatalf("daemon = %s\n%s", result, out.String())
	}
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE source IN (?, ?) ORDER BY kind", "suggestion:"+top.ID, "suggestion:"+second.ID)
	if len(jobs) != 2 {
		t.Fatalf("jobs = %v\n%s", jobs, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'experiment' AND source = ?", "suggestion:"+top.ID); n != 1 {
		t.Fatalf("top suggestion experiment count = %d\n%s", n, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'improve' AND source = ?", "suggestion:"+second.ID); n != 1 {
		t.Fatalf("second suggestion improve count = %d\n%s", n, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'improve' AND source = ?", "suggestion:"+top.ID); n != 0 {
		t.Fatalf("top suggestion improve count = %d\n%s", n, out.String())
	}
	line := "plan proj: left suggestion " + top.ID + " to the evolution loop"
	if strings.Count(out.String(), line) != 1 {
		t.Fatalf("deferral line missing once (%q):\n%s", line, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'plan.deferred' AND subject = ?", top.ID); n != 1 {
		t.Fatalf("plan.deferred events = %d", n)
	}

	if result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: all good'", Out: &out}); result != "done" {
		t.Fatalf("second daemon = %s\n%s", result, out.String())
	}
	if strings.Count(out.String(), line) != 1 {
		t.Fatalf("deferral line repeated (%q):\n%s", line, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'plan.deferred' AND subject = ?", top.ID); n != 1 {
		t.Fatalf("plan.deferred events after second tick = %d", n)
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
