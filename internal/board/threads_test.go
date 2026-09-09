package board

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func makeThreadPost(t *testing.T, s *store.Store, in PostInput) *Post {
	t.Helper()
	p, err := CreatePost(s, in)
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func byThread(rows []ThreadSummary, key string) ThreadSummary {
	for _, row := range rows {
		if row.Key == key {
			return row
		}
	}
	return ThreadSummary{}
}

func TestThreadSummariesStatusParticipantsQuestionsClaimsAndHandoffs(t *testing.T) {
	_, s, _ := fixture(t)
	if err := s.SetSetting("human", "stale_days", "2"); err != nil {
		t.Fatal(err)
	}
	// Open: participant order, one unanswered question, one active claim, and
	// one handoff closed by a later claim while the later handoff remains open.
	makeThreadPost(t, s, PostInput{Kind: "question", Project: "demo", Area: "api", Thread: "rate-limits", Summary: "Why 429?", Agent: "alice", Date: "2026-09-06T08:00:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "handoff", Project: "demo", Area: "api", Thread: "rate-limits", Summary: "Please take it", Agent: "alice", Date: "2026-09-06T08:05:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "claim", Project: "demo", Area: "api", Thread: "rate-limits", Summary: "I am on it", Agent: "bob", Expires: "2026-09-06T11:00:00Z", Date: "2026-09-06T08:06:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "claim", Project: "demo", Area: "api", Thread: "rate-limits", Summary: "Old claim", Agent: "alice", Expires: "2026-09-06T09:00:00Z", Date: "2026-09-06T08:07:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "handoff", Project: "demo", Area: "api", Thread: "rate-limits", Summary: "Need deploy", Agent: "bob", Date: "2026-09-06T08:08:00Z"})

	q1 := makeThreadPost(t, s, PostInput{Kind: "question", Project: "demo", Area: "api", Thread: "answered-on", Summary: "Need answer", Agent: "alice", Date: "2026-09-06T08:10:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "answer", Project: "demo", Area: "api", Thread: "answered-on", Summary: "Named answer", Agent: "bob", On: q1.ID, Date: "2026-09-06T08:11:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "question", Project: "demo", Area: "api", Thread: "answered-later", Summary: "Need answer", Agent: "alice", Date: "2026-09-06T08:12:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "answer", Project: "demo", Area: "api", Thread: "answered-later", Summary: "Later answer", Agent: "bob", Date: "2026-09-06T08:13:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "question", Project: "demo", Area: "jobs", Thread: "ignored", Summary: "Job?", Agent: "alice", Date: "2026-09-06T08:14:00Z"})

	makeThreadPost(t, s, PostInput{Kind: "finding", Project: "demo", Area: "old", Thread: "stale", Summary: "Old news", Agent: "alice", Date: "2026-09-01T08:00:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "question", Project: "demo", Area: "old", Thread: "closed", Summary: "Closed question", Agent: "alice", Date: "2026-09-01T08:00:00Z"})
	makeThreadPost(t, s, PostInput{Kind: "answer", Project: "demo", Area: "old", Thread: "closed", Summary: "Done", Agent: "bob", Resolution: "answered", Date: "2026-09-01T08:01:00Z"})

	rows, err := ThreadSummaries(s, "demo", testNow)
	if err != nil {
		t.Fatal(err)
	}
	open := byThread(rows, "api/rate-limits")
	if open.Status != "open" || open.Posts != 5 || strings.Join(open.Participants, ",") != "alice,bob" {
		t.Fatalf("open summary = %+v", open)
	}
	if open.OpenQuestions != 1 || open.OpenHandoffs != 1 || len(open.ActiveClaims) != 1 || open.ActiveClaims[0].Agent != "bob" {
		t.Fatalf("open counts/claims = %+v", open)
	}
	if got := byThread(rows, "api/answered-on").OpenQuestions; got != 0 {
		t.Fatalf("answered-on open questions = %d", got)
	}
	if got := byThread(rows, "api/answered-later").OpenQuestions; got != 0 {
		t.Fatalf("answered-later open questions = %d", got)
	}
	if got := byThread(rows, "jobs/ignored").OpenQuestions; got != 0 {
		t.Fatalf("jobs question count = %d", got)
	}
	if stale := byThread(rows, "old/stale"); stale.Status != "stale" {
		t.Fatalf("stale = %+v", stale)
	}
	if closed := byThread(rows, "old/closed"); closed.Status != "closed" || closed.Resolution != "answered" || closed.OpenQuestions != 0 {
		t.Fatalf("closed = %+v", closed)
	}
	if rows[0].Status != "open" || rows[len(rows)-1].Status != "closed" {
		t.Fatalf("sort order = %+v", rows)
	}
}

func TestThreadDetailMissingThreadRefuses(t *testing.T) {
	_, s, _ := fixture(t)
	_, _, err := ThreadDetail(s, "demo", "api/missing", testNow)
	if code(err) != 2 || !strings.Contains(what(err), "pulse thread list --project demo") {
		t.Fatalf("missing detail error = %v", err)
	}
}

func TestResolveThreadRulesAndSuccess(t *testing.T) {
	_, s, _ := fixture(t)
	makeThreadPost(t, s, PostInput{Kind: "finding", Project: "demo", Area: "api", Thread: "rate-limits", Summary: "Bug found", Agent: "alice", Date: "2026-09-06T08:00:00Z"})
	closed := makeThreadPost(t, s, PostInput{Kind: "answer", Project: "demo", Area: "api", Thread: "closed", Summary: "Answered", Agent: "bob", Resolution: "answered", Date: "2026-09-06T08:01:00Z"})

	cases := []struct {
		name string
		in   ResolveInput
		want string
	}{
		{"bad-key", ResolveInput{Project: "demo", Key: "bad", Resolution: "answered", Summary: "done", Agent: "alice"}, "Thread must be AREA/THREAD"},
		{"unknown", ResolveInput{Project: "demo", Key: "api/missing", Resolution: "answered", Summary: "done", Agent: "alice"}, "No thread 'api/missing'; see threads with `pulse thread list --project demo`"},
		{"closed", ResolveInput{Project: "demo", Key: "api/closed", Resolution: "answered", Summary: "done", Agent: "alice"}, "Thread api/closed is already closed by " + closed.ID + " (answered)"},
		{"bad-resolution", ResolveInput{Project: "demo", Key: "api/rate-limits", Resolution: "maybe", Summary: "done", Agent: "alice"}, "Resolution must be one of: " + strings.Join(Resolutions, ", ")},
		{"missing-summary", ResolveInput{Project: "demo", Key: "api/rate-limits", Resolution: "answered", Agent: "alice"}, "A one-line --summary is required"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := ResolveThread(s, tc.in)
			if code(err) != 2 || what(err) != tc.want {
				t.Fatalf("got %q (code %d), want %q", what(err), code(err), tc.want)
			}
		})
	}
	row, err := ResolveThread(s, ResolveInput{Project: "demo", Key: "api/rate-limits", Resolution: "answered", Summary: "429s fixed; measured", Body: "details", Agent: "alice", Session: "sess-alice"})
	if err != nil {
		t.Fatal(err)
	}
	if row.Kind != "result" || row.Resolution != "answered" || row.Summary != "429s fixed; measured" || row.Body != "details" {
		t.Fatalf("result = %+v", row)
	}
	sum, _, err := ThreadDetail(s, "demo", "api/rate-limits", testNow)
	if err != nil {
		t.Fatal(err)
	}
	if sum.Status != "closed" || sum.Resolution != "answered" {
		t.Fatalf("thread after resolve = %+v", sum)
	}
}

func TestThreadCLIListJSONAndResolve(t *testing.T) {
	noTTY(t)
	_, s, _ := fixture(t)
	makeThreadPost(t, s, PostInput{Kind: "finding", Project: "demo", Area: "api", Thread: "rate-limits", Summary: "Bug found", Agent: "alice", Date: "2026-09-06T08:00:00Z"})
	out, err := runCmd(t, s, &cli.Context{JSON: true}, "thread", "list", "--project", "demo")
	if err != nil {
		t.Fatal(err)
	}
	var rows []ThreadSummary
	if err := json.Unmarshal([]byte(out), &rows); err != nil {
		t.Fatal(err)
	}
	if len(rows) != 1 || rows[0].Key != "api/rate-limits" || rows[0].LastBy != "alice" {
		t.Fatalf("json rows = %+v", rows)
	}
	out, err = runCmd(t, s, &cli.Context{}, "thread", "resolve", "api/rate-limits", "--project", "demo", "--agent", "alice", "--as", "answered", "--summary", "429s fixed; measured")
	if err != nil {
		t.Fatal(err)
	}
	id := strings.TrimSpace(out)
	var kind, resolution string
	if err := s.DB.QueryRow("SELECT kind, resolution FROM posts WHERE id = ?", id).Scan(&kind, &resolution); err != nil {
		t.Fatal(err)
	}
	if kind != "result" || resolution != "answered" {
		t.Fatalf("resolved post %s: %s/%s", id, kind, resolution)
	}
}
