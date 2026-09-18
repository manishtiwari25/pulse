package runner

import (
	"strings"
	"testing"
)

func TestSuggestionQualityScorePrefersConcreteSignals(t *testing.T) {
	cases := []struct {
		body string
		want int
	}{
		{"One variable: README.md opening paragraph. Signal: visitors bounce. Hypothesis: a shorter lead raises retention.", 85},
		{"Add `ParseConfig()` error handling in cmd/start.go.", 90},
		{"gofmt everything and fix trailing whitespace", 0},
		{"Increase test coverage", 15},
		{"Make it better", 50},
	}
	for _, tc := range cases {
		got := suggestionQualityScore(tc.body)
		if got != tc.want {
			t.Errorf("qualityScore(%q) = %d, want %d", tc.body, got, tc.want)
		}
	}
}

func TestPlanJobsPrefersHighQualitySuggestion(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "quorum", "1")
	setSetting(t, s, "evolve.draft_hours", "0")
	for _, agent := range []string{"alice", "bob"} {
		join(t, s, agent)
	}

	low, err := createPost(s, testNow, postOpts{Kind: "suggestion", Project: "proj", Area: "evolution",
		Thread: "low", Agent: actorAgent(s, "alice"), Session: "s", Summary: "Format code",
		Body: "gofmt all files and remove trailing whitespace"})
	if err != nil {
		t.Fatal(err)
	}
	high, err := createPost(s, testNow, postOpts{Kind: "suggestion", Project: "proj", Area: "evolution",
		Thread: "high", Agent: actorAgent(s, "alice"), Session: "s", Summary: "Fix README",
		Body: "Signal: README.md\nOne variable: opening paragraph.\nHypothesis: a shorter lead raises retention."})
	if err != nil {
		t.Fatal(err)
	}
	for _, post := range []row{low, high} {
		if _, err := createPost(s, testNow, postOpts{Kind: "vote", Project: "proj", Area: "evolution",
			Thread: post.str("thread"), Agent: actorAgent(s, "bob"), Session: "v", On: post.str("id"), Vote: "up", Summary: "yes"}); err != nil {
			t.Fatal(err)
		}
	}

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 1 {
		t.Fatalf("planned %d jobs, want 1", len(jobs))
	}
	if !strings.Contains(jobs[0].str("source"), high.str("id")) {
		t.Fatalf("planned low-quality suggestion first: %s", jobs[0].str("source"))
	}
}
