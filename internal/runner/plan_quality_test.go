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

func TestPlanJobsPrefersProductSuggestionOverHigherScoredTestPlumbing(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "quorum", "1")
	setSetting(t, s, "evolve.draft_hours", "0")
	for _, agent := range []string{"alice", "bob", "carol"} {
		join(t, s, agent)
	}

	plumbing, err := createPost(s, testNow, postOpts{Kind: "suggestion", Project: "proj", Area: "evolution",
		Thread: "plumbing", Agent: actorAgent(s, "alice"), Session: "s", Summary: "GIT_DIR=/tmp/x/.git go test ./internal/hub goes FAIL -> ok",
		Body: "Class: product\nFile: internal/hub/board_test.go\nVariable: test git env\nHypothesis: passes under GIT_DIR\nSignal: check:hub"})
	if err != nil {
		t.Fatal(err)
	}
	product, err := createPost(s, testNow, postOpts{Kind: "suggestion", Project: "proj", Area: "evolution",
		Thread: "product", Agent: actorAgent(s, "alice"), Session: "s", Summary: "German-locale user with a failed OAuth callback sees a localized error message",
		Body: "Class: product\nFile: src/oauth.go\nVariable: callback error copy\nHypothesis: a German-locale user sees the localized failure message\nSignal: check:product"})
	if err != nil {
		t.Fatal(err)
	}
	for _, post := range []row{plumbing, product} {
		if _, err := createPost(s, testNow, postOpts{Kind: "vote", Project: "proj", Area: "evolution",
			Thread: post.str("thread"), Agent: actorAgent(s, "bob"), Session: "v", On: post.str("id"), Vote: "up", Summary: "yes"}); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := createPost(s, testNow, postOpts{Kind: "vote", Project: "proj", Area: "evolution",
		Thread: plumbing.str("thread"), Agent: actorAgent(s, "carol"), Session: "v2", On: plumbing.str("id"), Vote: "up", Summary: "yes"}); err != nil {
		t.Fatal(err)
	}

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 1 {
		t.Fatalf("planned %d jobs, want 1", len(jobs))
	}
	if !strings.Contains(jobs[0].str("source"), product.str("id")) {
		t.Fatalf("planned higher-scored operating-layer suggestion first: %s", jobs[0].str("source"))
	}
}
