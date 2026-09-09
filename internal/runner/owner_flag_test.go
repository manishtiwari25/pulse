package runner

import "testing"

func TestPlanJobsForOwnerMetadata(t *testing.T) {
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
				newRepo(t, s)
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
	newRepo(t, s)
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
