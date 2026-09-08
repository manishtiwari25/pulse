package hub

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
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
