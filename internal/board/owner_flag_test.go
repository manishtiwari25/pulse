package board

import (
	"encoding/json"
	"reflect"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
)

func TestPostForOwnerCLI(t *testing.T) {
	_, s, _ := fixture(t)
	for _, tc := range []struct {
		name string
		args []string
		want bool
	}{
		{"marked", []string{"--for-owner"}, true},
		{"unmarked", nil, false},
		{"false", []string{"--for-owner=false"}, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			before := countRows(t, s, "posts", "kind = 'question'")
			args := []string{"post", "question", "--area", "routing", "--thread", "owner-review",
				"--summary", "Owner question for @owner", "--body", "for_owner: true"}
			args = append(args, tc.args...)
			out, err := runCmd(t, s, &cli.Context{Agent: "alice", Project: "demo", JSON: true}, args...)
			if err != nil {
				t.Fatal(err)
			}
			var p Post
			if err := json.Unmarshal([]byte(out), &p); err != nil {
				t.Fatal(err)
			}
			if p.ForOwner != tc.want {
				t.Fatalf("CLI for_owner=%v, want %v: %s", p.ForOwner, tc.want, out)
			}
			loaded, err := PostByID(s, p.ID)
			if err != nil || loaded == nil || loaded.ForOwner != tc.want || loaded.Extra != p.Extra {
				t.Fatalf("loaded post=%+v, error=%v", loaded, err)
			}
			if n := countRows(t, s, "posts", "kind = 'question'"); n != before+1 {
				t.Fatalf("stored %d questions, want %d", n, before+1)
			}
		})
	}
}

func TestCreatePostForOwnerPreservesExtra(t *testing.T) {
	_, s, _ := fixture(t)
	extra := map[string]any{"for_owner": false, "ticket": "T-12"}
	before := map[string]any{"for_owner": false, "ticket": "T-12"}
	p, err := CreatePost(s, PostInput{
		Kind: "suggestion", Project: "demo", Area: "routing", Thread: "support-window", Summary: "Choose the support window",
		Agent: "alice", ForOwner: true, Extra: extra, Body: "Ask @bob to add context.",
	})
	if err != nil {
		t.Fatal(err)
	}
	if !p.ForOwner || !reflect.DeepEqual(extra, before) {
		t.Fatalf("for_owner=%v, caller metadata=%v", p.ForOwner, extra)
	}
	var saved map[string]any
	if err := json.Unmarshal([]byte(p.Extra), &saved); err != nil {
		t.Fatal(err)
	}
	if saved["for_owner"] != true || saved["ticket"] != "T-12" || !reflect.DeepEqual(saved["mentions"], []any{"bob"}) {
		t.Fatalf("stored metadata=%v", saved)
	}
}

func TestCreatePostRejectsInvalidForOwner(t *testing.T) {
	_, s, _ := fixture(t)
	for _, value := range []any{"yes", "", 1, nil, []string{}, map[string]any{}} {
		_, err := CreatePost(s, PostInput{
			Kind: "question", Project: "demo", Area: "routing", Thread: "review", Summary: "Choose a window",
			Agent: "alice", Extra: map[string]any{"for_owner": value},
		})
		if code(err) != 2 || !strings.Contains(what(err), "for_owner must be true or false") {
			t.Fatalf("for_owner=%#v: error=%v", value, err)
		}
	}
	if got := countRows(t, s, "posts", "1=1"); got != 0 {
		t.Fatalf("invalid flags persisted %d posts", got)
	}
	agent, err := s.Agent("alice")
	if err != nil || agent == nil || agent.Posts != 0 {
		t.Fatalf("invalid flags changed agent counters: %+v, %v", agent, err)
	}
}
