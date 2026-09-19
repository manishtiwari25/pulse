package runner

import (
	"encoding/json"
	"reflect"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func TestIdleNoteEmptyProject(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)

	got := idleNote(s, "proj", testNow)
	if !strings.Contains(got, "no suggestions") || !strings.Contains(got, "reflect due") {
		t.Fatalf("empty idle note: %q", got)
	}
}

func TestIdleNoteRejectedSuggestion(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	author := join(t, s, "author")
	voter1 := join(t, s, "voter-one")
	voter2 := join(t, s, "voter-two")
	card, err := createPost(s, testNow, postOpts{Kind: "suggestion", Project: "proj", Area: "code", Thread: "plan",
		Summary: "try a measured change", Body: "one variable\n", Agent: rowAgent(author)})
	if err != nil {
		t.Fatal(err)
	}
	for _, voter := range []row{voter1, voter2} {
		if _, err := createPost(s, testNow, postOpts{Kind: "vote", Project: "proj", Area: "code", Thread: "plan",
			Summary: "down", Body: "not ready\n", Agent: rowAgent(voter), On: card.str("id"), Vote: "down"}); err != nil {
			t.Fatal(err)
		}
	}
	got := idleNote(s, "proj", testNow)
	if !strings.Contains(got, "no eligible suggestion (1 rejected)") {
		t.Fatalf("downvoted idle note: %q", got)
	}
}

func TestBallotDueSkipsWhenOnlyAuthorCouldVote(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	author := join(t, s, "author")
	card, err := createPost(s, "2026-09-06T08:00:00Z", postOpts{Kind: "suggestion", Project: "proj", Area: "code", Thread: "plan",
		Summary: "try a measured change", Body: "one variable\n", Agent: rowAgent(author)})
	if err != nil {
		t.Fatal(err)
	}
	suggestions := suggestionRecords(s, "proj", testNow)
	if cards := ballotDue(s, "proj", testNow, suggestions); len(cards) != 0 {
		t.Fatalf("ballot due with only author voter: %+v", cards)
	}
	join(t, s, "voter")
	suggestions = suggestionRecords(s, "proj", testNow)
	cards := ballotDue(s, "proj", testNow, suggestions)
	if len(cards) != 1 || cards[0].ID != card.str("id") {
		t.Fatalf("ballot due with available voter: %+v", cards)
	}
}

func TestIdleNoteHandoffsAndTried(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	worker := join(t, s, "worker-one")
	h, err := createPost(s, testNow, postOpts{Kind: "handoff", Project: "proj", Area: "code", Thread: "handoff",
		Summary: "continue this", Body: "leftover work\n", Agent: rowAgent(worker)})
	if err != nil {
		t.Fatal(err)
	}

	if got := idleNote(s, "proj", testNow); !strings.Contains(got, "1 handoffs") || strings.Contains(got, "1 handoffs (") {
		t.Fatalf("handoff idle note: %q", got)
	}
	if _, _, err := addJob(s, "proj", "task", "try handoff", "brief", 0, 1, "handoff:"+h.str("id"), humanAgent, false); err != nil {
		t.Fatal(err)
	}
	if got := idleNote(s, "proj", testNow); !strings.Contains(got, "1 handoffs (1 tried)") {
		t.Fatalf("tried handoff idle note: %q", got)
	}
}

func TestIdleNoteQuestions(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	worker := join(t, s, "worker-one")
	if _, err := createPost(s, testNow, postOpts{Kind: "question", Project: "proj", Area: "code", Thread: "q",
		Summary: "what next?", Body: "what next?\n", Agent: rowAgent(worker)}); err != nil {
		t.Fatal(err)
	}

	if got := idleNote(s, "proj", testNow); !strings.Contains(got, "1 questions") {
		t.Fatalf("question idle note: %q", got)
	}
}

func TestRecordIdleNotesReadableByInspect(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	recordIdleNotes(s, map[string]string{"proj": "no suggestions; reflect due"}, testNow)

	raw, exists, err := s.Meta("planner")
	if err != nil || !exists {
		t.Fatalf("planner meta missing: %v", err)
	}
	var decoded map[string]struct {
		At   string `json:"at"`
		Note string `json:"note"`
	}
	if err := json.Unmarshal([]byte(raw), &decoded); err != nil || decoded["proj"].At != testNow || decoded["proj"].Note != "no suggestions; reflect due" {
		t.Fatalf("planner meta json: %s %v", raw, err)
	}
}

func TestIdleNotesChanged(t *testing.T) {
	got := idleNotesChanged(map[string]string{"a": "same", "b": "old", "gone": "old"}, map[string]string{"b": "new", "a": "same", "c": "newer"})
	want := []string{"idle b: new", "idle c: newer"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("changed notes = %#v, want %#v", got, want)
	}
}

func rowAgent(r row) *store.Agent {
	return &store.Agent{Name: r.str("name"), Display: r.str("display"), Sessions: jsonList(r.str("sessions")), Kind: r.str("kind")}
}
