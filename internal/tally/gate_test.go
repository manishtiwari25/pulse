package tally

import (
	"os"
	"path/filepath"
	"testing"
)

func addEligibleSuggestion(t *testing.T, f *fixture, key, agent, thread, summary, body, extra string, votes []string) string {
	t.Helper()
	id := f.post(t, fpost{kind: "suggestion", area: "evolution", thread: thread, agent: agent, date: "2026-09-01T0" + key + ":00:00Z", summary: summary, body: body, session: "s-" + key, extra: extra})
	for i, vote := range votes {
		voter := "runner-v" + key + string(rune('a'+i)) + "/model"
		f.post(t, fpost{kind: "vote", area: "evolution", thread: thread, agent: voter, date: "2026-09-02T0" + key + ":0" + string(rune('1'+i)) + ":00Z", summary: vote, session: "s-" + voter, on: id, vote: vote})
	}
	return id
}

func TestPlanGateOutcomes(t *testing.T) {
	f := newFixture(t, true)
	if _, err := f.st.DB.Exec("INSERT INTO jobs (id, project, kind, title, brief, source, status, created_at) VALUES ('J-blocked', 'fixture', 'improve', 'blocked', '', ?, 'blocked', ?)",
		"suggestion:"+f.ids["eligible"], fixtureNow); err != nil {
		t.Fatal(err)
	}
	net := addEligibleSuggestion(t, f, "1", "runner-net/model", "net", "Break even", "No signal needed.", "", []string{"up", "down"})
	owner := addEligibleSuggestion(t, f, "2", "runner-owner/model", "support-window", "Choose the support window", "No signal needed.", `{"for_owner":true}`, []string{"up", "up"})
	noEvidence := addEligibleSuggestion(t, f, "3", "runner-worker/model", "worker-no-signal", "Worker card", "No signal line.", "", []string{"up", "up"})
	ownerPath := addEligibleSuggestion(t, f, "4", "runner-worker-path/model", "worker-owner-path", "Worker owner path", "Signal: docs/decisions/adr.md", "", []string{"up", "up"})
	plannable := addEligibleSuggestion(t, f, "5", "runner-plain/model", "owner-review", "Release candidate wording fix", "No signal needed for non-workers.", "", []string{"up", "up"})
	if _, err := f.st.DB.Exec("UPDATE agents SET kind = 'worker' WHERE name IN ('runner-worker/model', 'runner-worker-path/model')"); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(f.repo, "docs", "decisions"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(f.repo, "docs", "decisions", "adr.md"), []byte("# ADR\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	res, err := Compute(f.st, "fixture", "", Thresholds{Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	suggestions := map[string]Suggestion{}
	posts := map[string]*Post{}
	for _, sg := range res.Suggestions {
		suggestions[sg.ID] = sg
	}
	for i := range res.Posts {
		posts[res.Posts[i].ID] = &res.Posts[i]
	}
	tests := map[string]string{
		f.ids["eligible"]: "blocked as job J-blocked",
		net:               GateNetVotes,
		owner:             GateOwner,
		noEvidence:        GateNoEvidence,
		ownerPath:         GateOwnerPaths,
		plannable:         "",
	}
	for id, want := range tests {
		if got := PlanGate(f.st, res.Repo, suggestions[id], posts[id]); got != want {
			t.Fatalf("PlanGate(%s) = %q, want %q (suggestion %+v)", id, got, want, suggestions[id])
		}
	}
}
