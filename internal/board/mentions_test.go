package board

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func TestParseMentions(t *testing.T) {
	cases := []struct {
		name string
		text string
		want []string
	}{
		{"simple", "please ask @alice and @bob", []string{"alice", "bob"}},
		{"case punctuation brackets", `("@Alice," and [@BOB])`, []string{"alice", "bob"}},
		{"owner", "@owner and @human", []string{"human"}},
		{"dedupe", "@alice @ALICE @alice!", []string{"alice"}},
		{"emails", "not x@y.test or x@alice.test but yes @bob", []string{"bob"}},
		{"invalid", "@a @1abc @bad_name @alice@example.test", nil},
		{"boundary", "mid@alice ok (@bob) and '{@cinder}'", []string{"bob", "cinder"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := ParseMentions(tc.text); strings.Join(got, ",") != strings.Join(tc.want, ",") {
				t.Fatalf("ParseMentions(%q) = %v, want %v", tc.text, got, tc.want)
			}
		})
	}
}

func TestMentionsFor(t *testing.T) {
	_, s, _ := fixture(t)
	old, err := CreatePost(s, PostInput{Kind: "question", Project: "demo", Area: "api", Thread: "same-date", Summary: "@bob first", Agent: "alice", Date: "2026-09-06T09:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := CreatePost(s, PostInput{Kind: "answer", Project: "demo", Area: old.Area, Thread: old.Thread, Summary: "answered", Agent: "bob", Date: old.Date}); err != nil {
		t.Fatal(err)
	}
	p1, err := CreatePost(s, PostInput{Kind: "question", Project: "demo", Area: "api", Thread: "rate-limits", Summary: "@bob which fixture covers 429s?", Agent: "alice", Date: "2026-09-06T10:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := CreatePost(s, PostInput{Kind: "answer", Project: "demo", Area: p1.Area, Thread: p1.Thread, Summary: "the retry fixture", Agent: "bob", Date: "2026-09-06T10:30:00Z"}); err != nil {
		t.Fatal(err)
	}
	p2, err := CreatePost(s, PostInput{Kind: "handoff", Project: "demo", Area: "docs", Thread: "copy", Summary: "needs you", Body: "@bob please finish", Agent: "alice", Date: "2026-09-06T11:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	p3, err := CreatePost(s, PostInput{Kind: "question", Project: store.CommonsSlug, Area: "questions", Thread: "shared", Summary: "@bob commons ping", Agent: "alice", Date: "2026-09-06T12:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("INSERT INTO posts (id, project, area, thread, kind, agent, date, summary, extra, created_at) VALUES ('legacy-invalid-extra', 'demo', 'api', 'legacy', 'question', 'alice', '2026-09-06T13:00:00Z', '@bob legacy', 'not json', ?)", testNow); err != nil {
		t.Fatal(err)
	}
	got, err := MentionsFor(s, "demo", "bob", 0)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 1 || got[0].ID != p2.ID {
		t.Fatalf("demo mentions = %v, want only %s", ids(got), p2.ID)
	}
	got, err = MentionsFor(s, "", "bob", 1)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 1 || got[0].ID != p3.ID {
		t.Fatalf("limited mentions = %v, want %s", ids(got), p3.ID)
	}
	n, err := MentionCount(s, "", "bob")
	if err != nil {
		t.Fatal(err)
	}
	if n != 2 {
		t.Fatalf("MentionCount = %d, want 2", n)
	}
}

func ids(posts []Post) []string {
	out := make([]string, len(posts))
	for i, p := range posts {
		out[i] = p.ID
	}
	return out
}
