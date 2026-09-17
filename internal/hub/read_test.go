package hub

import (
	"bytes"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func projectReaderFixture(t *testing.T) (*store.Store, *Handler) {
	t.Helper()
	s := additionsStore(t, false)
	agent, _, err := board.JoinAgent(s, board.JoinInput{Name: "reader", Runner: "test", Model: "test", Session: "reader"})
	if err != nil {
		t.Fatal(err)
	}
	for _, slug := range []string{"delta", "charlie", "bravo", "alpha"} {
		additionsProject(t, s, slug)
		if _, err := s.DB.Exec("UPDATE projects SET path=?,brain=? WHERE slug=?", lifecycleRepo(t), slug == "alpha", slug); err != nil {
			t.Fatal(err)
		}
		if _, err := board.CreatePost(s, board.PostInput{
			Kind: "finding", Project: slug, Area: "console", Thread: "readers",
			AgentRow: agent, Summary: slug + " finding",
		}); err != nil {
			t.Fatal(err)
		}
	}
	h := New(s, nil)
	h.orgCacheTTL = 0 // tests expect fresh store views on every request
	return s, h
}

func TestProjectReadersGitTimeout(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the sleeping git wrapper requires a POSIX shell")
	}
	s, h := projectReaderFixture(t)
	git, err := exec.LookPath("git")
	if err != nil {
		t.Fatal(err)
	}
	alpha, err := s.Project("alpha")
	if err != nil {
		t.Fatal(err)
	}
	bin := t.TempDir()
	// exec lets CommandContext kill the sleeper without leaving a child holding stdout.
	wrapper := "#!/bin/sh\n" +
		"if [ \"$PULSE_TEST_BLOCK_GIT\" = all ] || [ \"$2\" = \"$PULSE_TEST_BLOCK_GIT\" ]; then\n" +
		"  if [ \"$PULSE_TEST_SLEEP_GIT\" = yes ]; then exec sleep 6; fi\n" +
		"  exit 1\n" +
		"fi\n" +
		"exec \"$PULSE_TEST_REAL_GIT\" \"$@\"\n"
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(wrapper), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PULSE_TEST_REAL_GIT", git)
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	for _, tc := range []struct {
		name, blocked string
		posts         float64
	}{
		{"all_blocked", "all", 0},
		{"one_blocked", alpha.Path, 3},
	} {
		for _, route := range []string{"org", "projects", "timeline"} {
			t.Run(tc.name+"/"+route, func(t *testing.T) {
				t.Setenv("PULSE_TEST_BLOCK_GIT", tc.blocked)
				t.Setenv("PULSE_TEST_SLEEP_GIT", "")
				request := func() *httptest.ResponseRecorder {
					t.Helper()
					rec := httptest.NewRecorder()
					h.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/api/v1/"+route, nil))
					if rec.Code != http.StatusOK {
						t.Fatalf("%s: status %d: %s", route, rec.Code, rec.Body.String())
					}
					return rec
				}
				want := request().Body.Bytes()
				t.Setenv("PULSE_TEST_SLEEP_GIT", "yes")
				start := time.Now()
				got := request().Body.Bytes()
				elapsed := time.Since(start)
				t.Logf("%s with %s git: %s", route, tc.name, elapsed)
				if elapsed >= 6*time.Second {
					t.Errorf("project probes took %s, want under 6s with a 5s git timeout", elapsed)
				}
				if !bytes.Equal(got, want) {
					t.Fatalf("timed-out git changed the JSON\nwant: %s\ngot:  %s", want, got)
				}
				var data map[string]any
				if err := json.Unmarshal(got, &data); err != nil {
					t.Fatal(err)
				}
				if route == "timeline" {
					if data["total"] != tc.posts {
						t.Errorf("timeline total = %v, want %v", data["total"], tc.posts)
					}
					return
				}
				rows := data["projects"].([]any)
				if len(rows) != 4 {
					t.Fatalf("projects = %d, want 4", len(rows))
				}
				for i, slug := range []string{"alpha", "bravo", "charlie", "delta"} {
					row := rows[i].(map[string]any)
					if row["slug"] != slug || row["present"] != (tc.blocked != "all" && slug != "alpha") {
						t.Errorf("project %d lost its order or presence: %#v", i, row)
					}
				}
			})
		}
	}
}

type projectReaderSourceFunc func(project, area string) (string, error)

func (f projectReaderSourceFunc) Records(project, area string) (string, error) {
	return f(project, area)
}

func TestEachPreservesOrderAndErrors(t *testing.T) {
	s, h := projectReaderFixture(t)
	for slug, path := range map[string]string{
		"bravo":   filepath.Join(t.TempDir(), "missing"),
		"charlie": t.TempDir(),
	} {
		if _, err := s.DB.Exec("UPDATE projects SET path=? WHERE slug=?", path, slug); err != nil {
			t.Fatal(err)
		}
	}
	source := h.Source
	h.Source = projectReaderSourceFunc(func(project, area string) (string, error) {
		if project == "alpha" {
			return "", errors.New("tally unavailable for alpha")
		}
		return source.Records(project, area)
	})
	views, err := h.each()
	if err != nil {
		t.Fatal(err)
	}
	want := []struct {
		slug, err string
		present   bool
	}{
		{"alpha", "tally unavailable for alpha", true},
		{"bravo", "missing on disk", false},
		{"charlie", "missing on disk", false},
		{"delta", "", true},
		{store.CommonsSlug, "not created yet", false},
	}
	if len(views) != len(want) {
		t.Fatalf("views = %d, want %d", len(views), len(want))
	}
	for i, tc := range want {
		view := views[i]
		if view.Project == nil || view.Project.Slug != tc.slug || view.Present != tc.present || view.Error != tc.err {
			t.Errorf("view %d = %#v, want %#v", i, view, tc)
			continue
		}
		if (view.Data == nil) != (tc.err != "") {
			t.Errorf("%s data = %#v, error = %q", tc.slug, view.Data, tc.err)
		}
		summary := h.summary(view)
		if summary["present"] != (tc.err == "") || summary["missing"] != !tc.present || summary["error"] != tc.err {
			t.Errorf("%s summary lost its presence or error: %#v", tc.slug, summary)
		}
	}
}
