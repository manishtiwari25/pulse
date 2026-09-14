package runner

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/evolve"
)

func TestPlanJobsForOwnerMetadata(t *testing.T) {
	skipSlow(t)
	cases := []struct {
		name, thread, summary string
		extra                 map[string]any
		forOwner              bool
	}{
		{"marked", "review", "Choose the support window", map[string]any{"for_owner": true}, true},
		{"frontmatter true", "review", "Choose the support window", map[string]any{"for_owner": "true"}, true},
		{"unmarked", "work", "Improve the parser", nil, false},
		{"legacy wording", "owner-questions", "Release candidate for the owner; vote requested", nil, false},
		{"explicit false", "owner-questions", "Owner question: document the response", map[string]any{"for_owner": false}, false},
		{"frontmatter false", "owner-review", "Evaluator proposal for the owner", map[string]any{"for_owner": "false"}, false},
	}
	for _, kind := range []string{"suggestion", "handoff", "question"} {
		for _, tc := range cases {
			t.Run(kind+"/"+tc.name, func(t *testing.T) {
				s := newStore(t)
				newBareRepo(t, s)
				suppressPlannerReflect(t, s)
				first, err := createPost(s, plusHours(testNow, -2), postOpts{
					Kind: kind, Project: "proj", Area: "routing", Thread: tc.thread, Summary: tc.summary,
					Body: "Signal: check:owner-routing\nfor_owner: true\n", Extra: tc.extra,
					Agent: actorAgent(s, "author-one"), Session: "s-author-one",
				})
				if err != nil {
					t.Fatal(err)
				}
				next, err := createPost(s, plusHours(testNow, -1), postOpts{
					Kind: kind, Project: "proj", Area: "routing", Thread: "next-card", Summary: "Continue the parser work",
					Body: "Signal: check:owner-routing\n", Agent: actorAgent(s, "author-two"), Session: "s-author-two",
				})
				if err != nil {
					t.Fatal(err)
				}
				if kind == "suggestion" {
					for _, card := range []row{first, next} {
						for i, voter := range []string{"voter-one", "voter-two", "voter-three"} {
							if card.str("id") == next.str("id") && i == 2 {
								break
							}
							if _, err := createPost(s, testNow, postOpts{
								Kind: "vote", Project: "proj", Area: card.str("area"), Thread: card.str("thread"),
								Summary: "One measurable change", On: card.str("id"), Vote: "up",
								Agent: actorAgent(s, voter), Session: "s-" + voter,
							}); err != nil {
								t.Fatal(err)
							}
						}
					}
				}
				want := first.str("id")
				if tc.forOwner {
					want = next.str("id")
				}
				jobs, err := planJobs(s, "proj", testNow, "daemon")
				if err != nil {
					t.Fatal(err)
				}
				if len(jobs) != 1 {
					t.Fatalf("for_owner=%v: planned %d jobs, want one", tc.forOwner, len(jobs))
				}
				if got := jobs[0].str("source"); got != kind+":"+want {
					t.Fatalf("for_owner=%v: planned %s, want %s:%s", tc.forOwner, got, kind, want)
				}
			})
		}
	}
}

func TestPostRowParsesForOwner(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	for _, tc := range []struct {
		name  string
		extra map[string]any
		want  bool
	}{
		{"missing", nil, false},
		{"true", map[string]any{"for_owner": true}, true},
		{"false", map[string]any{"for_owner": false}, false},
		{"frontmatter true", map[string]any{"for_owner": "true"}, true},
		{"frontmatter false", map[string]any{"for_owner": "false"}, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			p, err := createPost(s, testNow, postOpts{
				Kind: "question", Project: "proj", Area: "routing", Thread: "owner-review",
				Summary: "Owner question", Extra: tc.extra, Agent: actorAgent(s, "author-one"),
			})
			if err != nil {
				t.Fatal(err)
			}
			if got := p["for_owner"] == true; got != tc.want {
				t.Fatalf("created row for_owner=%v, want %v", got, tc.want)
			}
			if got := postRow(s.DB, p.str("id"))["for_owner"] == true; got != tc.want {
				t.Fatalf("loaded row for_owner=%v, want %v", got, tc.want)
			}
			found := false
			for _, r := range projectPosts(s.DB, "proj") {
				if r.str("id") == p.str("id") {
					found = true
					if got := r["for_owner"] == true; got != tc.want {
						t.Fatalf("project row for_owner=%v, want %v", got, tc.want)
					}
				}
			}
			if !found {
				t.Fatal("post missing from project rows")
			}
		})
	}
	if p := postRow(s.DB, "missing"); p != nil {
		t.Fatalf("unknown post = %v", p)
	}
}

// A question the daemon (or the loop) posted is an ask for the human, whether or not the for_owner flag made it
// into the row: the planner never sends a worker to answer it. Only a person's unflagged question gets a job.
func TestPlanJobsNeverAnswersMachineQuestions(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	suppressPlannerReflect(t, s)
	at := plusHours(testNow, -4)
	next := func() string {
		at = plusMinutes(at, 5)
		return at
	}
	for _, q := range []struct {
		name, agent, summary, signal string
	}{
		{"legacy stop question by the daemon, unflagged", "daemon", "Evolution stopped; owner review needed", "check:owner-routing"},
		{"daemon question with another summary", "daemon", "Which recipe should the clone use?", "check:owner-routing"},
		{"stop summary from a worker", "kestrel", evolve.StopSummary, "check:owner-routing"},
		{"owner-path question", "kestrel", "Should the stop file be lifted?", "docs/evolution/STOP"},
	} {
		if _, err := createPost(s, next(), postOpts{
			Kind: "question", Project: "proj", Area: "german-tutor", Thread: "v8-only-site", Summary: q.summary,
			Body: "Signal: " + q.signal + "\n" + q.name + "\n", Agent: actorAgent(s, q.agent), Session: "s-" + q.agent,
		}); err != nil {
			t.Fatal(err)
		}
	}
	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 0 {
		t.Fatalf("planned %d job(s) for machine or owner questions: %s", len(jobs), jobs[0].str("source"))
	}
	human, err := createPost(s, next(), postOpts{
		Kind: "question", Project: "proj", Area: "german-tutor", Thread: "vocabulary", Summary: "Why does the parser drop umlauts?",
		Body: "Signal: check:owner-routing\n", Agent: actorAgent(s, "author-one"), Session: "s-author-one",
	})
	if err != nil {
		t.Fatal(err)
	}
	jobs, err = planJobs(s, "proj", testNow, "daemon")
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 1 || jobs[0].str("source") != "question:"+human.str("id") || jobs[0].str("kind") != "answer" {
		t.Fatalf("planned %d job(s), want one answer job for %s", len(jobs), human.str("id"))
	}
}

// One worker attempt per thread per human turn: while the first question's job runs or failed, a later question
// in the same thread is not planned; an answer in the thread resets it and the next new question gets a worker.
func TestPlanJobsOneTriedQuestionPerThread(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	suppressPlannerReflect(t, s)
	if err := s.SetSetting(humanAgent, "parallel_per_project", "3"); err != nil {
		t.Fatal(err)
	}
	ask := func(at, agent, thread, summary string) row {
		t.Helper()
		p, err := createPost(s, at, postOpts{
			Kind: "question", Project: "proj", Area: "api", Thread: thread, Summary: summary,
			Body: "Signal: check:owner-routing\n", Agent: actorAgent(s, agent), Session: "s-" + agent,
		})
		if err != nil {
			t.Fatal(err)
		}
		return p
	}
	first := ask(plusHours(testNow, -3), "author-one", "rate-limits", "Which fixture covers 429s?")
	second := ask(plusHours(testNow, -2), "author-two", "rate-limits", "Should the fixture also cover 503s?")
	elsewhere := ask(plusHours(testNow, -1), "author-two", "retries", "How long is the retry pause?")
	plan := func() []row {
		t.Helper()
		jobs, err := planJobs(s, "proj", testNow, "daemon")
		if err != nil {
			t.Fatal(err)
		}
		return jobs
	}
	jobs := plan()
	if len(jobs) != 1 || jobs[0].str("source") != "question:"+first.str("id") {
		t.Fatalf("first plan = %d job(s), want the oldest question %s", len(jobs), first.str("id"))
	}
	if jobs = plan(); len(jobs) != 1 || jobs[0].str("source") != "question:"+elsewhere.str("id") {
		t.Fatalf("while %s runs, planned %d job(s); want only the other thread's %s", first.str("id"), len(jobs), elsewhere.str("id"))
	}
	exec1(s.DB, "UPDATE jobs SET status = 'failed', finished_at = ?, result = 'blocked: owner disposition required' WHERE source = ?",
		testNow, "question:"+first.str("id"))
	if jobs = plan(); len(jobs) != 0 {
		t.Fatalf("after the failed attempt, planned %s; want the thread to wait for a human", jobs[0].str("source"))
	}
	if _, err := createPost(s, plusMinutes(testNow, 1), postOpts{
		Kind: "answer", Project: "proj", Area: "api", Thread: "rate-limits", Summary: "the 429 fixture is tests/limits.json",
		On: first.str("id"), Agent: actorAgent(s, "author-one"), Session: "s-author-one",
	}); err != nil {
		t.Fatal(err)
	}
	third := ask(plusMinutes(testNow, 2), "author-two", "rate-limits", "Does tests/limits.json need a 503 case?")
	if jobs = plan(); len(jobs) != 1 || jobs[0].str("source") != "question:"+third.str("id") {
		t.Fatalf("after the answer, planned %d job(s); want %s (not %s)", len(jobs), third.str("id"), second.str("id"))
	}
}
