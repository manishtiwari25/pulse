package tally

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
)

func TestDigestShowsUnansweredMentionsForAgent(t *testing.T) {
	f := newFixture(t, false)
	if _, _, err := board.JoinAgent(f.st, board.JoinInput{Name: "alice", Runner: "copilot", Model: "m", Session: "sess-alice"}); err != nil {
		t.Fatal(err)
	}
	if _, _, err := board.JoinAgent(f.st, board.JoinInput{Name: "bob", Runner: "copilot", Model: "m", Session: "sess-bob"}); err != nil {
		t.Fatal(err)
	}
	p, err := board.CreatePost(f.st, board.PostInput{Kind: "question", Project: "fixture", Area: "api", Thread: "rate-limits", Summary: "@bob which fixture covers 429s?", Agent: "alice", Date: "2026-09-10T10:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	text, err := DigestFor(f.st, "fixture", "", Thresholds{Now: fixtureNow}, "bob", "sess-bob")
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"MENTIONS FOR YOU (1)", p.ID, "api/rate-limits", "by alice", "@bob which fixture covers 429s?"} {
		if !strings.Contains(text, want) {
			t.Fatalf("digest missing %q:\n%s", want, text)
		}
	}
	if _, err := board.CreatePost(f.st, board.PostInput{Kind: "answer", Project: "fixture", Area: p.Area, Thread: p.Thread, Summary: "answered", Agent: "bob", Date: "2026-09-10T10:30:00Z"}); err != nil {
		t.Fatal(err)
	}
	text, err = DigestFor(f.st, "fixture", "", Thresholds{Now: fixtureNow}, "bob", "sess-bob")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(text, "MENTIONS FOR YOU") {
		t.Fatalf("digest still shows answered mention:\n%s", text)
	}
}
