package evolve

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func governance(t *testing.T, s *store.Store, project string) {
	t.Helper()
	constitution := "# Constitution\n\n## Editable scope\n" +
		"| File class | Paths | Stage | Autonomy |\n| --- | --- | --- | --- |\n" +
		"| product | `value.txt`, `other.txt` | 4 | standing |\n" +
		"| operating | `README.md` | 2 | standing |\n\n## Immutable paths\n" +
		"| Path | Why |\n| --- | --- |\n| `protected.txt` | owner-owned |\n"
	for slug, body := range map[string]string{"constitution": constitution, "evaluators": "Product scope: allowed\n"} {
		if _, err := s.PutRecord(store.Record{ID: "EVO-" + slug + "-" + project, Project: project, Kind: "evolution", Slug: slug},
			nil, body, "human", "fixture"); err != nil {
			t.Fatal(err)
		}
	}
}

func baseline(t *testing.T, s *store.Store, project string) {
	t.Helper()
	if _, err := Archive(s, ArchiveRow{Project: project, Baseline: &Scores{Total: .1}, Variant: &Scores{Total: .2}, Kept: true, Tolerance: .02}); err != nil {
		t.Fatal(err)
	}
}

func TestStatusLadderBaselineCountsAndNextSteps(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	read := func(level, next string) *LoopStatus {
		t.Helper()
		got, err := Status(s, "demo")
		if err != nil || got.Level != level || !strings.Contains(got.NextHumanStep, next) {
			t.Fatalf("expected level %s and next %s, got %+v (%v)", level, next, got, err)
		}
		return got
	}
	read("B", "record new")
	governance(t, s, "demo")
	read("B", "fixture add")
	fixture(t, s, "demo", "F01", "printf 1", 1)
	read("B", "job add")
	baseline(t, s, "demo")
	read("C", "evolve setup DIR")
	_, forbidden := privateSet(t, s)
	read("C", "evolve.loop 1")
	if err := s.SetSetting("human", "evolve.loop", "1"); err != nil {
		t.Fatal(err)
	}
	status := read("D", "none")
	if status.Baseline == nil || *status.Baseline != .2 || status.Kept != 1 || !status.ProductScope.Allowed {
		t.Fatal("status did not derive the newest kept baseline or product scope")
	}
	encoded, _ := json.Marshal(status)
	assertPrivateAbsent(t, string(encoded), forbidden)
	if err := s.SetSetting("human", "stop", "1"); err != nil {
		t.Fatal(err)
	}
	read("B", "pulse start")
}

func TestLimitsUseLoopEventEpochAndDoNotResetOnRepeatedOn(t *testing.T) {
	s := testStore(t)
	t.Setenv("PULSE_NOW", "2026-09-06T10:00:00Z")
	testProject(t, s, "demo")
	governance(t, s, "demo")
	fixture(t, s, "demo", "F01", "printf 1", 1)
	privateSet(t, s)
	baseline(t, s, "demo")
	if err := s.SetSetting("human", "evolve.loop", "1"); err != nil {
		t.Fatal(err)
	}
	if err := s.SetSetting("human", "evolve.limits", "cycles=1,hours=2"); err != nil {
		t.Fatal(err)
	}
	before, err := Status(s, "demo")
	if err != nil || before.Cycles != 0 || before.Level != "D" {
		t.Fatal("an archive row before the on event in the same second consumed the new budget")
	}
	if _, err := Archive(s, ArchiveRow{Project: "demo", Baseline: &Scores{Total: .2}, Variant: &Scores{Total: .3}, Kept: true, Tolerance: .02}); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PULSE_NOW", "2026-09-06T11:00:00Z")
	if err := s.SetSetting("human", "evolve.loop", "1"); err != nil {
		t.Fatal(err)
	}
	after, err := Status(s, "demo")
	if err != nil || after.Cycles != 1 || after.Hours != 1 || !after.CapReached || after.Level != "C" {
		t.Fatal("repeated loop on reset its cap or elapsed time")
	}
	if err := s.SetSetting("human", "evolve.limits", "hours=0.5"); err != nil {
		t.Fatal(err)
	}
	after, err = Status(s, "demo")
	if err != nil || !after.CapReached {
		t.Fatal("wall-time cap did not bind")
	}
}

func TestStatusVotedVersusSignalHitRate(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	joinTestAgent(t, s, "proposer")
	joinTestAgent(t, s, "voter")
	p, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "one", Kind: "suggestion", Agent: "proposer", Summary: "one variable"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "one", Kind: "vote", Agent: "voter", Summary: "evidence", On: p.ID, Vote: "up"}); err != nil {
		t.Fatal(err)
	}
	row := ArchiveRow{Project: "demo", SuggestionPost: p.ID, Baseline: &Scores{Total: .2}, Variant: &Scores{Total: .3}, Kept: true, Tolerance: .02}
	if _, err := Archive(s, row); err != nil {
		t.Fatal(err)
	}
	row.SuggestionPost, row.Kept = "", false
	if _, err := Archive(s, row); err != nil {
		t.Fatal(err)
	}
	status, err := Status(s, "demo")
	if err != nil || status.Voted.Tried != 1 || status.Voted.Kept != 1 || status.Signal.Tried != 1 || status.Signal.Kept != 0 {
		t.Fatal("voted/signal hit rates did not reflect canonical posts and archive outcomes")
	}
}

func TestLimitAndProductScopeParsing(t *testing.T) {
	for _, value := range []string{"none", "cycles=0", "hours=0.5", "cycles=3,hours=2"} {
		if _, err := ParseLimits(value); err != nil {
			t.Errorf("valid limits refused: %s", value)
		}
	}
	for _, value := range []string{"", "budget", "cycles=-1", "hours=NaN", "tokens=5", "cycles=1,cycles=2", "hours=-1"} {
		if _, err := ParseLimits(value); err == nil {
			t.Errorf("invalid limits accepted: %s", value)
		}
	}
	if !parseProductScope("Product scope: allowed\n").Allowed || parseProductScope("## Product evaluators\n").Allowed ||
		parseProductScope("Product scope: allowed\nProduct scope: off\n").Allowed {
		t.Fatal("product scope did not require one explicit owner-owned line")
	}
}
