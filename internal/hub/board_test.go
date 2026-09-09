package hub

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

func TestStoreSuggestionVoteReasonUsesSummary(t *testing.T) {
	for _, tc := range []struct {
		name, vote, summary string
	}{
		{"explained up", "up", "up: fewer retries"},
		{"explained down", "down", "down: no measured improvement"},
		{"bare up", "up", "up"},
		{"bare down", "down", "down"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := additionsStore(t, false)
			additionsProject(t, s, "one")
			for _, name := range []string{"proposer", "voter"} {
				if _, _, err := board.JoinAgent(s, board.JoinInput{Name: name, Runner: "test", Model: "test", Session: "s-" + name}); err != nil {
					t.Fatal(err)
				}
			}
			suggestion, err := board.CreatePost(s, board.PostInput{
				Kind: "suggestion", Project: "one", Area: "governance", Thread: "vote-reasons",
				Agent: "proposer", Session: "s-proposer", Summary: "Show why agents voted.",
			})
			if err != nil {
				t.Fatal(err)
			}
			vote, err := board.CreatePost(s, board.PostInput{
				Kind: "vote", Project: "one", Area: "governance", Thread: "vote-reasons",
				Agent: "voter", Session: "s-voter", On: suggestion.ID, Vote: tc.vote,
				Summary: tc.summary, Body: "## Vote\n\nBody detail is not the displayed reason.\n",
			})
			if err != nil {
				t.Fatal(err)
			}

			data, err := New(s, nil).snapshot("one", "")
			if err != nil {
				t.Fatal(err)
			}
			if len(data.Suggestions) != 1 || data.Suggestions[0]["id"] != suggestion.ID {
				t.Fatalf("expected the stored suggestion, got %#v", data.Suggestions)
			}
			votes, ok := data.Suggestions[0]["votes"].([]map[string]any)
			if !ok || len(votes) != 1 {
				t.Fatalf("expected one suggestion vote, got %#v", data.Suggestions[0]["votes"])
			}
			projected := votes[0]
			if projected["post_id"] != vote.ID || projected["vote"] != tc.vote || projected["classification"] != "counted" {
				t.Fatalf("expected the stored vote to be counted, got %#v", projected)
			}
			if got := projected["reason"]; got != tc.summary {
				t.Errorf("suggestion vote reason = %q, want vote post summary %q", got, tc.summary)
			}
		})
	}
}

func TestStoreAmendmentVoteReasonUsesSummary(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	additionsRecord(t, s, "EVO-constitution", "one", "evolution", "constitution", "## Workflow\n\nRun one check.\n")
	for _, name := range []string{"proposer", "voter"} {
		if _, _, err := board.JoinAgent(s, board.JoinInput{Name: name, Runner: "test", Model: "test", Session: "s-" + name}); err != nil {
			t.Fatal(err)
		}
	}
	amendment, err := board.CreatePost(s, board.PostInput{
		Kind: "amendment", Project: "one", Area: "governance", Thread: "checks", Agent: "proposer",
		Session: "s-proposer",
		Summary: "Run both checks before committing.",
		Body:    "## Current text\n\n```\nRun one check.\n```\n\n## Proposed text\n\n```\nRun both checks.\n```\n",
	})
	if err != nil {
		t.Fatal(err)
	}
	const summary = "Both checks catch more regressions."
	const restatement = "The workflow runs both checks instead of one."
	vote, err := board.CreatePost(s, board.PostInput{
		Kind: "vote", Project: "one", Area: "governance", Thread: "checks", Agent: "voter",
		Session: "s-voter",
		On:      amendment.ID, Vote: "up", TextHash: amendment.TextHash, Summary: summary,
		Body: "## Vote\n\nBody detail is not the displayed reason.\n\nRead: " + restatement + "\n",
	})
	if err != nil {
		t.Fatal(err)
	}

	data, err := New(s, nil).snapshot("one", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(data.Amendments) != 1 || data.Amendments[0]["id"] != amendment.ID {
		t.Fatalf("expected the stored amendment, got %#v", data.Amendments)
	}
	votes, ok := data.Amendments[0]["votes"].([]map[string]any)
	if !ok || len(votes) != 1 {
		t.Fatalf("expected one amendment vote, got %#v", data.Amendments[0]["votes"])
	}
	projected := votes[0]
	if projected["post_id"] != vote.ID || projected["classification"] != "counted" {
		t.Fatalf("expected the stored vote to be counted, got %#v", projected)
	}
	if got := projected["reason"]; got != summary {
		t.Errorf("amendment vote reason = %q, want vote post summary %q", got, summary)
	}
	if got := projected["read_line"]; got != restatement {
		t.Errorf("amendment vote Read restatement = %q, want %q", got, restatement)
	}
}

func TestThreadRowsCarryEnrichedStatus(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	for _, name := range []string{"alpha", "bravo"} {
		if _, _, err := board.JoinAgent(s, board.JoinInput{Name: name, Runner: "test", Model: "test", Session: "s-" + name}); err != nil {
			t.Fatal(err)
		}
	}
	question, err := board.CreatePost(s, board.PostInput{Kind: "question", Project: "one", Area: "api", Thread: "rate-limits", Agent: "alpha",
		Summary: "Why 429?", Date: "2026-09-06T08:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := board.CreatePost(s, board.PostInput{Kind: "handoff", Project: "one", Area: "api", Thread: "rate-limits", Agent: "alpha",
		Summary: "Please take it", Date: "2026-09-06T08:01:00Z"}); err != nil {
		t.Fatal(err)
	}
	if _, err := board.CreatePost(s, board.PostInput{Kind: "claim", Project: "one", Area: "api", Thread: "rate-limits", Agent: "bravo",
		Summary: "On it", Expires: "2026-09-06T13:00:00Z", Date: "2026-09-06T08:02:00Z"}); err != nil {
		t.Fatal(err)
	}
	if _, err := board.CreatePost(s, board.PostInput{Kind: "answer", Project: "one", Area: "api", Thread: "answered", Agent: "bravo",
		Summary: "Named answer", On: question.ID, Date: "2026-09-06T08:03:00Z"}); err != nil {
		t.Fatal(err)
	}
	data, err := New(s, nil).snapshot("one", "")
	if err != nil {
		t.Fatal(err)
	}
	thread := data.thread("api/rate-limits")["thread"].(map[string]any)
	if got := thread["participants"]; strings.Join(got.([]string), ",") != "alpha,bravo" {
		t.Fatalf("participants = %#v", got)
	}
	if thread["last_by"] != "bravo" || thread["last_kind"] != "claim" {
		t.Fatalf("last fields = %#v", thread)
	}
	if thread["open_questions"] != 0 || thread["open_handoffs"] != 0 {
		t.Fatalf("open counts = %#v", thread)
	}
	if got := thread["active_claims"]; len(got.([]string)) != 1 || got.([]string)[0] != "bravo" {
		t.Fatalf("active_claims = %#v", got)
	}
}

func TestSuggestionRowsCarryJobsAndPlanGates(t *testing.T) {
	s := additionsStore(t, false)
	repo := lifecycleRepo(t)
	project, err := board.AddProject(s, repo, "One", "", false, "", "human", "one")
	if err != nil {
		t.Fatal(err)
	}
	agents := map[string]*store.Agent{}
	for _, name := range []string{"alpha", "bravo", "charlie", "delta", "echo", "foxtrot"} {
		agent, _, err := board.JoinAgent(s, board.JoinInput{Name: name, Runner: "test", Model: "test", Session: "s-" + name})
		if err != nil {
			t.Fatal(err)
		}
		agents[name] = agent
	}
	suggest := func(agent, thread, summary string) *board.Post {
		t.Helper()
		post, err := board.CreatePost(s, board.PostInput{
			Kind: "suggestion", Project: project.Slug, Area: "product", Thread: thread, AgentRow: agents[agent],
			Summary: summary, Body: "Suggestion body.",
		})
		if err != nil {
			t.Fatal(err)
		}
		return post
	}
	vote := func(agent string, on *board.Post, direction string) {
		t.Helper()
		if _, err := board.CreatePost(s, board.PostInput{
			Kind: "vote", Project: project.Slug, Area: on.Area, Thread: on.Thread, AgentRow: agents[agent],
			On: on.ID, Vote: direction, Summary: "Voting " + direction, Body: "Vote body.",
		}); err != nil {
			t.Fatal(err)
		}
	}

	netZero := suggest("alpha", "net-zero", "Use offset scans for exports")
	vote("bravo", netZero, "up")
	vote("charlie", netZero, "down")
	plannable := suggest("bravo", "cache-prices", "Cache price lookup per request")
	vote("alpha", plannable, "up")
	vote("charlie", plannable, "up")
	blocked := suggest("charlie", "blocked-work", "Add retry budget to API calls")
	vote("alpha", blocked, "up")
	vote("bravo", blocked, "up")
	owner := suggest("delta", "owner-priority", "Owner should choose the support window")
	vote("alpha", owner, "up")
	vote("bravo", owner, "up")
	job, _, err := board.AddJob(s, board.JobInput{
		Project: project.Slug, Kind: "task", Title: "Carry blocked suggestion", Source: "suggestion:" + blocked.ID, CreatedBy: "human",
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("UPDATE jobs SET status='blocked' WHERE id=?", job.ID); err != nil {
		t.Fatal(err)
	}

	data, err := New(s, nil).snapshot(project.Slug, "")
	if err != nil {
		t.Fatal(err)
	}
	byID := map[string]map[string]any{}
	for _, row := range data.Suggestions {
		byID[str(row["id"])] = row
	}
	if row := byID[netZero.ID]; row["gate"] != tally.GateNetVotes || row["plannable"] != false {
		t.Fatalf("net vote gate = %#v", row)
	}
	if row := byID[plannable.ID]; row["gate"] != "" || row["plannable"] != true {
		t.Fatalf("plannable gate = %#v", row)
	}
	if row := byID[blocked.ID]; row["job"] != job.ID || row["job_status"] != "blocked" || row["gate"] != "blocked as job "+job.ID || row["plannable"] != false {
		t.Fatalf("blocked job gate = %#v", row)
	}
	if row := byID[owner.ID]; row["gate"] != tally.GateOwner || row["plannable"] != false {
		t.Fatalf("owner gate = %#v", row)
	}
	if got := data.overview()["suggestions_plannable"]; got != 1 {
		t.Fatalf("suggestions_plannable = %v, want 1", got)
	}
}

func TestOrgOwnerPresenceAndPlannableTotals(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	evolution := map[string]any{
		"loop":    map[string]any{"loop": "", "stop": "", "limits": ""},
		"archive": map[string]any{"rows": []map[string]any{}, "record": nil},
	}
	views := []projectView{{Project: &store.Project{Slug: "one"}, Data: &boardData{
		Meta:   object("repo_name branch head dirty built_at board_present", "one", "main", "abc123", "0", s.Now(), "1"),
		Params: map[string]any{}, Evolution: evolution, Posts: []map[string]any{}, Threads: []map[string]any{}, Claims: []map[string]any{},
		Proposals: []map[string]any{}, Amendments: []map[string]any{},
		Suggestions: []map[string]any{
			object("status plannable", "eligible", true),
			object("status plannable", "eligible", false),
			object("status plannable", "needs votes", false),
		},
	}}}
	org := h.org(views)
	if org["owner_presence"] != store.OwnerPresent {
		t.Fatalf("default owner_presence = %v", org["owner_presence"])
	}
	if got := org["totals"].(map[string]any)["votes_plannable"]; got != 1 {
		t.Fatalf("votes_plannable = %v, want 1", got)
	}
	if err := s.SetSetting("human", "owner.presence", store.OwnerAway); err != nil {
		t.Fatal(err)
	}
	org = h.org(views)
	if org["owner_presence"] != store.OwnerAway {
		t.Fatalf("away owner_presence = %v", org["owner_presence"])
	}
}
