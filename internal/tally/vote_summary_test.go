package tally

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestBoardVoteSummaryRendering(t *testing.T) {
	for _, tc := range []struct {
		name, kind, vote, summary string
		json, allAreas            bool
		warnings                  int
	}{
		{name: "explained up", kind: "vote", vote: "up", summary: "up: fewer retries"},
		{name: "explained down", kind: "vote", vote: "down", summary: "down: no measured improvement"},
		{name: "bare up", kind: "vote", vote: "up", summary: "up", warnings: 1},
		{name: "bare down", kind: "vote", vote: "down", summary: "down", warnings: 1},
		{name: "padded mixed case", kind: "vote", vote: "down", summary: " \tDoWn \t", warnings: 1},
		{name: "all areas", kind: "vote", vote: "up", summary: "up", allAreas: true, warnings: 2},
		{name: "one word reason", kind: "vote", vote: "up", summary: "Agreed"},
		{name: "non-vote up", kind: "finding", summary: "up"},
		{name: "non-vote down", kind: "finding", summary: "down"},
		{name: "JSON bare vote", kind: "vote", vote: "up", summary: "up", json: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			home := t.TempDir()
			if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
				t.Fatal(err)
			}
			s, err := store.Open(store.Options{Home: home, Now: fixtureNow})
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { s.Close() })
			for _, project := range []string{"fixture", "other"} {
				if _, err := s.DB.Exec("INSERT INTO projects (slug, name, path, added_at) VALUES (?, ?, ?, ?)",
					project, project, filepath.Join(s.Home, project), fixtureNow); err != nil {
					t.Fatal(err)
				}
			}
			f := &fixture{st: s}
			suggestion := f.post(t, fpost{
				kind: "suggestion", area: "governance", thread: "vote-reasons", agent: "test/proposer",
				session: "s-proposer", date: "2026-09-09T12:00:00Z", summary: "Show why agents voted.",
			})
			const body = "Body detail is not the displayed reason."
			id := f.post(t, fpost{
				kind: tc.kind, area: "governance", thread: "vote-reasons", agent: "test/voter",
				session: "s-voter", date: fixtureNow, summary: tc.summary, body: body, on: suggestion, vote: tc.vote,
			})
			for _, scope := range []struct{ project, area string }{{"fixture", "outside"}, {"other", "governance"}} {
				target := f.post(t, fpost{
					project: scope.project, kind: "suggestion", area: scope.area, thread: "outside",
					agent: scope.project + "/proposer", date: "2026-09-09T12:00:00Z", summary: "Another suggestion.",
				})
				f.post(t, fpost{
					project: scope.project, kind: "vote", area: scope.area, thread: "outside",
					agent: scope.project + "/voter", date: fixtureNow, summary: "up", on: target, vote: "up",
				})
			}

			var out, diagnostics bytes.Buffer
			c := &cli.Context{Store: s, Project: "fixture", Now: fixtureNow, JSON: tc.json, Out: &out, Err: &diagnostics}
			fs := flag.NewFlagSet("board", flag.ContinueOnError)
			area := "governance"
			if tc.allAreas {
				area = ""
			}
			fs.String("area", area, "")
			if err := runBoard(c, fs, nil); err != nil {
				t.Fatal(err)
			}
			wantWarning := ""
			if tc.warnings > 0 {
				wantWarning = fmt.Sprintf("warning: %d vote post(s) have a bare up/down summary; include an explanatory phrase in the vote summary\n", tc.warnings)
			}
			if got := diagnostics.String(); got != wantWarning {
				t.Errorf("board diagnostic = %q, want %q", got, wantWarning)
			}
			if tc.json {
				var result Result
				if err := json.Unmarshal(out.Bytes(), &result); err != nil {
					t.Fatalf("board JSON: %v\n%s", err, out.String())
				}
				for _, post := range result.Posts {
					if post.ID == id {
						if post.Summary != tc.summary {
							t.Errorf("JSON summary = %q, want %q", post.Summary, tc.summary)
						}
						return
					}
				}
				t.Fatalf("board JSON is missing post %s", id)
			}
			wantText, err := DigestFor(s, "fixture", area, Thresholds{Now: fixtureNow}, "", "")
			if err != nil {
				t.Fatal(err)
			}
			if out.String() != wantText {
				t.Errorf("diagnostic changed board stdout:\n%s\nwant:\n%s", out.String(), wantText)
			}
			if strings.Contains(out.String(), body) {
				t.Error("board displayed body detail instead of the vote summary")
			}
			for _, line := range strings.Split(out.String(), "\n") {
				if strings.Contains(line, "governance/vote-reasons") && strings.Contains(line, "2 posts") {
					if !strings.HasSuffix(line, tc.summary) {
						t.Errorf("thread line = %q, want vote summary suffix %q", line, tc.summary)
					}
					return
				}
			}
			t.Fatal("board text is missing the vote-reasons thread")
		})
	}
}
