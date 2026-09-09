package board

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
)

func TestVoteSummaryWarnings(t *testing.T) {
	noTTY(t)
	for _, command := range []string{"vote", "post"} {
		for _, tc := range []struct {
			name, summary, vote      string
			amendment, json, warning bool
		}{
			{name: "bare up", summary: "up", vote: "up", warning: true},
			{name: "bare down", summary: "down", vote: "down", warning: true},
			{name: "padded mixed case", summary: " \tDoWn\n", vote: "down", warning: true},
			{name: "opposite direction", summary: "up", vote: "down", warning: true},
			{name: "explained up", summary: "up: fewer retries", vote: "up"},
			{name: "explained down", summary: "down: no measured improvement", vote: "down"},
			{name: "one word reason", summary: "Agreed", vote: "up"},
			{name: "word containing up", summary: "backup", vote: "up"},
			{name: "normalized explanation", summary: " \tup\nbecause fewer retries ", vote: "up"},
			{name: "amendment bare", summary: "up", vote: "up", amendment: true, warning: true},
			{name: "amendment explained", summary: "clarifies the rule", vote: "up", amendment: true},
			{name: "JSON bare up", summary: "up", vote: "up", json: true},
			{name: "JSON bare down", summary: "down", vote: "down", json: true},
			{name: "JSON amendment bare", summary: "down", vote: "down", amendment: true, json: true},
		} {
			t.Run(command+"/"+tc.name, func(t *testing.T) {
				_, s, _ := fixture(t)
				suggestion, amendment, _ := seedPosts(t, s)
				target := suggestion
				body := "The explanation in the body is not the displayed reason."
				if tc.amendment {
					target = amendment
					body = "Read: use the new line.\n" + body
				}
				args := []string{"vote", target.ID, tc.vote}
				if command == "post" {
					args = []string{"post", "vote", "--area", target.Area, "--thread", target.Thread, "--on", target.ID, "--vote", tc.vote}
				}
				args = append(args, "--summary", tc.summary, "--body", body, "--text-hash", target.TextHash)
				var diagnostics bytes.Buffer
				out, err := runCmd(t, s, &cli.Context{Agent: "bob", Project: "demo", JSON: tc.json, Err: &diagnostics}, args...)
				if err != nil {
					t.Fatal(err)
				}
				id := strings.TrimSpace(out)
				if tc.json {
					var result Post
					if err := json.Unmarshal([]byte(out), &result); err != nil {
						t.Fatalf("vote JSON: %v\n%s", err, out)
					}
					id = result.ID
				}
				row, err := PostByID(s, id)
				if err != nil || row == nil {
					t.Fatalf("stored vote %q: %+v, %v", id, row, err)
				}
				if row.Kind != "vote" || row.Vote != tc.vote || row.On != target.ID ||
					row.Summary != strings.Join(strings.Fields(tc.summary), " ") || row.Body != body || row.TextHash != target.TextHash {
					t.Fatalf("warning changed stored vote: %+v", row)
				}
				if n := countRows(t, s, "posts", "kind = 'vote'"); n != 1 {
					t.Fatalf("stored %d votes, want 1", n)
				}
				wantOut := row.ID + "\n"
				if tc.json {
					var expected bytes.Buffer
					if err := cli.WriteJSON(&expected, row); err != nil {
						t.Fatal(err)
					}
					wantOut = expected.String()
				}
				if out != wantOut {
					t.Errorf("stdout = %q, want %q", out, wantOut)
				}
				wantWarning := ""
				if tc.warning {
					wantWarning = fmt.Sprintf("warning: vote %s has a bare up/down summary; include an explanatory phrase in --summary\n", row.ID)
				}
				if got := diagnostics.String(); got != wantWarning {
					t.Errorf("diagnostic = %q, want %q", got, wantWarning)
				}
			})
		}
	}
}

func TestNonVoteSummaryWarnings(t *testing.T) {
	noTTY(t)
	for _, summary := range []string{"up", "down"} {
		t.Run(summary, func(t *testing.T) {
			_, s, _ := fixture(t)
			var diagnostics bytes.Buffer
			out, err := runCmd(t, s, &cli.Context{Agent: "bob", Project: "demo", Err: &diagnostics},
				"post", "finding", "--area", "runner", "--thread", "status", "--summary", summary)
			if err != nil {
				t.Fatal(err)
			}
			if out == "" || diagnostics.Len() != 0 {
				t.Fatalf("non-vote output = %q, diagnostic = %q", out, diagnostics.String())
			}
		})
	}
}

func TestRejectedVoteSummaryDoesNotWarn(t *testing.T) {
	noTTY(t)
	for _, command := range []string{"vote", "post"} {
		t.Run(command, func(t *testing.T) {
			_, s, _ := fixture(t)
			target, _, _ := seedPosts(t, s)
			args := []string{"vote", target.ID, "up"}
			if command == "post" {
				args = []string{"post", "vote", "--area", target.Area, "--thread", target.Thread, "--on", target.ID, "--vote", "up"}
			}
			args = append(args, "--summary", "up")
			var diagnostics bytes.Buffer
			out, err := runCmd(t, s, &cli.Context{Agent: "alice", Project: "demo", Err: &diagnostics}, args...)
			if code(err) != 2 || what(err) != "No self-votes: alice wrote "+target.ID {
				t.Fatalf("self-vote refusal = %v", err)
			}
			if out != "" || diagnostics.Len() != 0 {
				t.Fatalf("rejected vote output = %q, diagnostic = %q", out, diagnostics.String())
			}
			if n := countRows(t, s, "posts", "kind = 'vote'"); n != 0 {
				t.Fatalf("stored %d rejected votes", n)
			}
		})
	}
}

type voteSummaryErrorWriter struct{ err error }

func (w voteSummaryErrorWriter) Write([]byte) (int, error) { return 0, w.err }

func TestPrintPostOutputErrors(t *testing.T) {
	writeErr := errors.New("write failed")
	for _, stream := range []string{"stdout", "stderr"} {
		t.Run(stream, func(t *testing.T) {
			c := &cli.Context{Out: io.Discard, Err: io.Discard}
			if stream == "stdout" {
				c.Out = voteSummaryErrorWriter{writeErr}
			} else {
				c.Err = voteSummaryErrorWriter{writeErr}
			}
			if err := printPost(c, &Post{ID: "vote-id", Kind: "vote", Summary: "up"}); !errors.Is(err, writeErr) {
				t.Fatalf("%s write error = %v, want %v", stream, err, writeErr)
			}
		})
	}
}
