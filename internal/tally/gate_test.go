package tally

import (
	"os"
	"path/filepath"
	"strings"
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

func TestRunCheckIsNoOp(t *testing.T) {
	orig := listTests
	t.Cleanup(func() { listTests = orig })
	var gotPkgs []string
	var gotRegex string
	listTests = func(dir string, pkgs []string, regex string) ([]string, error) {
		gotPkgs, gotRegex = pkgs, regex
		if strings.HasPrefix(regex, "TestMissing") {
			return nil, nil
		}
		if regex == "TestBroken" {
			return nil, os.ErrNotExist
		}
		return []string{"TestCompletionLanding"}, nil
	}
	cases := map[string]struct {
		text      string
		want      bool
		wantRegex string
	}{
		"matches":            {"check: go test ./internal/runner -run TestCompletionLanding.", false, "TestCompletionLanding."},
		"quoted":             {"check: go test ./internal/runner -run 'TestCompletion.*'", false, "TestCompletion.*"},
		"quoted alternation": {"check: go test ./internal/runner -run 'TestMissing|Test B' -count=1", true, "TestMissing|Test B"},
		"double quoted":      {"check: go test ./internal/runner -run \"TestMissing|Test's\"", true, "TestMissing|Test's"},
		"backquoted":         {"check: go test ./internal/runner -run `TestMissing|Test B`", true, "TestMissing|Test B"},
		"no-op":              {"check: go test ./internal/runner -run TestMissing", true, "TestMissing"},
		"equals":             {"check: go test ./internal/runner -run=TestMissing -count=1", true, "TestMissing"},
		"toolchain err":      {"check: go test ./internal/runner -run TestBroken", false, "TestBroken"},
		"bad regex":          {"check: go test ./internal/runner -run 'Test(' ", true, ""},
		"no -run":            {"check: go test ./internal/runner", false, ""},
		"no pkgs":            {"check: go test -run TestMissing", false, ""},
	}
	for name, tc := range cases {
		gotPkgs, gotRegex = nil, ""
		if got := RunCheckIsNoOp("/repo", tc.text, ""); got != tc.want {
			t.Fatalf("%s: RunCheckIsNoOp = %v, want %v", name, got, tc.want)
		}
		if gotRegex != tc.wantRegex {
			t.Fatalf("%s: listTests regex = %q, want %q", name, gotRegex, tc.wantRegex)
		}
		if tc.wantRegex != "" && (len(gotPkgs) != 1 || gotPkgs[0] != "./internal/runner") {
			t.Fatalf("%s: listTests pkgs = %v", name, gotPkgs)
		}
	}
	if RunCheckIsNoOp("", "check: go test ./x -run TestMissing", "") {
		t.Fatal("empty project path must not gate")
	}
}

func TestPlanGateHoldsNoOpRunCheck(t *testing.T) {
	orig := listTests
	t.Cleanup(func() { listTests = orig })
	listTests = func(string, []string, string) ([]string, error) { return nil, nil }
	f := newFixture(t, true)
	id := addEligibleSuggestion(t, f, "6", "runner-noop/model", "worker-noop", "Worker no-op check", "check: go test ./internal/runner -run TestGone. Signal: README.md", "", []string{"up", "up"})
	if _, err := f.st.DB.Exec("UPDATE agents SET kind = 'worker' WHERE name = 'runner-noop/model'"); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(f.repo, "README.md"), []byte("# r\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	res, err := Compute(f.st, "fixture", "", Thresholds{Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	for _, sg := range res.Suggestions {
		if sg.ID != id {
			continue
		}
		for i := range res.Posts {
			if res.Posts[i].ID == id {
				if got := PlanGate(f.st, res.Repo, sg, &res.Posts[i]); got != GateNoOpCheck {
					t.Fatalf("PlanGate = %q, want %q", got, GateNoOpCheck)
				}
				return
			}
		}
	}
	t.Fatal("suggestion not found")
}
