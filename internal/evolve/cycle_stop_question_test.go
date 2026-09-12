package evolve

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestRecentlyAnsweredStopQuestionMatchesGateTarget(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	joinTestAgent(t, s, "tester")
	t.Setenv("PULSE_NOW", "2026-09-06T10:00:00Z")
	target, err := board.CreatePost(s, board.PostInput{Project: "demo", Kind: "suggestion",
		Area: "evolution", Thread: "candidate", Agent: "tester", Summary: "Reuse an existing helper"})
	if err != nil {
		t.Fatal(err)
	}
	q, err := board.CreatePost(s, board.PostInput{Project: "demo", Kind: "question",
		Area: target.Area, Thread: target.Thread, On: target.ID, Agent: "tester", Summary: StopSummary, ForOwner: true})
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("PULSE_NOW", "2026-09-06T10:00:01Z")
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Kind: "answer",
		Area: q.Area, Thread: q.Thread, On: q.ID, Agent: "tester", Summary: "Acknowledged; the gate remains closed"}); err != nil {
		t.Fatal(err)
	}
	for _, tt := range []struct {
		name, project, area, thread, on, want string
	}{
		{"same target", "demo", q.Area, q.Thread, target.ID, q.ID},
		{"other project", "other", q.Area, q.Thread, target.ID, ""},
		{"other area", "demo", "other", q.Thread, target.ID, ""},
		{"other thread", "demo", q.Area, "other", target.ID, ""},
		{"other target", "demo", q.Area, q.Thread, "", ""},
	} {
		t.Run(tt.name, func(t *testing.T) {
			id, err := recentlyAnsweredStopQuestion(s, tt.project, tt.area, tt.thread, tt.on)
			if err != nil || id != tt.want {
				t.Fatalf("recent answered stop = %q, error=%v; want %q", id, err, tt.want)
			}
		})
	}
}

func TestRecentlyAnsweredStopQuestionHoldsLoopGate(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	joinTestAgent(t, s, "tester")
	t.Setenv("PULSE_NOW", "2026-09-06T10:00:00Z")
	first, err := ownerQuestion(s, "demo", nil, "tester", nil, "", "Owner review is still required")
	if err != nil || first == "" {
		t.Fatalf("first loop question = %q, error=%v", first, err)
	}
	t.Setenv("PULSE_NOW", "2026-09-06T10:00:01Z")
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Kind: "answer",
		Area: "evolution", Thread: "loop-gate", On: first, Agent: "tester", Summary: "Acknowledged"}); err != nil {
		t.Fatal(err)
	}
	held, err := ownerQuestion(s, "demo", nil, "tester", nil, "", "Owner review is still required")
	if err != nil || held != first || countRows(t, s, "posts WHERE kind = 'question'") != 1 {
		t.Fatalf("answered loop question = %q, error=%v; want %q held", held, err, first)
	}
}

func TestStopQuestionLookupsReportErrors(t *testing.T) {
	t.Run("database", func(t *testing.T) {
		s := testStore(t)
		if err := s.DB.Close(); err != nil {
			t.Fatal(err)
		}
		for _, lookup := range []struct {
			name string
			call func(*store.Store, string, string, string, string) (string, error)
		}{
			{"open", openStopQuestion},
			{"answered", recentlyAnsweredStopQuestion},
		} {
			t.Run(lookup.name, func(t *testing.T) {
				if id, err := lookup.call(s, "demo", "evolution", "loop-gate", ""); err == nil || id != "" {
					t.Fatalf("database error hidden: id=%q error=%v", id, err)
				}
			})
		}
		if id, err := ownerQuestion(s, "demo", nil, "tester", nil, "", "Owner review is required"); err == nil || id != "" {
			t.Fatalf("owner question hid database error: id=%q error=%v", id, err)
		}
	})
	t.Run("clock", func(t *testing.T) {
		s := testStore(t)
		t.Setenv("PULSE_NOW", "not-a-timestamp")
		if id, err := recentlyAnsweredStopQuestion(s, "demo", "evolution", "loop-gate", ""); err == nil || id != "" {
			t.Fatalf("clock error hidden: id=%q error=%v", id, err)
		}
	})
}
