package tally

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestPostForOwnerRoundTrip(t *testing.T) {
	for _, tc := range []struct {
		name, extra string
		want        bool
	}{
		{"missing", `{}`, false},
		{"true", `{"for_owner":true,"ticket":"T-12"}`, true},
		{"false", `{"for_owner":false}`, false},
		{"frontmatter true", `{"for_owner":"true"}`, true},
		{"frontmatter false", `{"for_owner":"false"}`, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			f := newFixture(t, true)
			id := f.post(t, fpost{kind: "question", area: "routing", thread: "owner-review", summary: "Owner question",
				agent: "runner-routing/model", date: fixtureNow, extra: tc.extra})
			p, err := LoadPost(f.st, id)
			if err != nil || p == nil || p.ForOwner != tc.want || p.Extra != tc.extra {
				t.Fatalf("loaded post=%+v, error=%v", p, err)
			}
			posts, err := PostsFor(f.st, "fixture", "routing")
			if err != nil || len(posts) != 1 || posts[0].ForOwner != tc.want {
				t.Fatalf("posts=%+v, error=%v", posts, err)
			}
			shown, err := Show(f.st, id)
			if err != nil || strings.Contains(shown, "\nfor_owner: true\n") != tc.want {
				t.Fatalf("frontmatter=%s, error=%v", shown, err)
			}
			if got := PostJSON(p)["for_owner"] == "true"; got != tc.want {
				t.Fatalf("frontmatter JSON for_owner=%v, want %v", got, tc.want)
			}
			raw, err := json.Marshal(p)
			if err != nil || strings.Contains(string(raw), `"for_owner":true`) != tc.want {
				t.Fatalf("post JSON=%s, error=%v", raw, err)
			}
		})
	}
}

func TestPostForOwnerInvalidMetadataSurfacesError(t *testing.T) {
	f := newFixture(t, true)
	id := f.post(t, fpost{kind: "question", area: "routing", thread: "review", summary: "Choose the window",
		agent: "runner-routing/model", date: fixtureNow, extra: `{"for_owner":"maybe"}`})
	if _, err := LoadPost(f.st, id); err == nil || !strings.Contains(err.Error(), id) {
		t.Fatalf("invalid flag load error=%v", err)
	}
	if _, err := PostsFor(f.st, "fixture", "routing"); err == nil || !strings.Contains(err.Error(), id) {
		t.Fatalf("invalid flag list error=%v", err)
	}
}

func TestQueueForOwnerMetadata(t *testing.T) {
	f := newFixture(t, true)
	before, err := WorkWanted(f.st, nil, "", Thresholds{Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	ids := map[string]bool{}
	for _, kind := range []string{"handoff", "question"} {
		id := f.post(t, fpost{kind: kind, area: "routing", thread: kind, summary: "Choose the window",
			agent: "runner-" + kind + "/model", date: fixtureNow, extra: `{"for_owner":true}`})
		ids[id] = true
	}
	id := addEligibleSuggestion(t, f, "2", "runner-routing/model", "support-window", "Choose the support window",
		"No signal needed.", `{"for_owner":true}`, []string{"up", "up"})
	ids[id] = true
	after, err := WorkWanted(f.st, nil, "", Thresholds{Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	found := 0
	for _, it := range after.Work {
		if ids[it.ID] {
			found++
			if it.Blocked != GateOwner {
				t.Errorf("%s gate=%q, want %q", it.Kind, it.Blocked, GateOwner)
			}
		}
	}
	if found != 3 || plannableWork(after.Work) != plannableWork(before.Work) {
		t.Fatalf("owner cards=%d, plannable before=%d after=%d", found, plannableWork(before.Work), plannableWork(after.Work))
	}
}
