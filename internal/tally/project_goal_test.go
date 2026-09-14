package tally

import (
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/goals"
)

func TestProjectGoalCanonicalSlugBoardAndBallot(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	t.Setenv("PULSE_AGENT", "")
	t.Setenv("PULSE_SESSION", "")
	f := newFixture(t, false)
	const slug = "customer-support-ticket-management-syste"
	if _, err := f.st.DB.Exec("INSERT INTO projects (slug,name,path,added_at) VALUES (?,?,?,?)",
		slug, "Customer support tickets", filepath.Join(f.home, "long-project"), fixtureNow); err != nil {
		t.Fatal(err)
	}
	ro := openReadOnly(t, f.home)
	for _, want := range []string{"", "Improve useful ticket results using existing checks."} {
		if want != "" {
			if _, err := goals.Set(f.st, slug, want, "human"); err != nil {
				t.Fatal(err)
			}
		}
		goal, err := goals.Get(ro, slug)
		if err != nil {
			t.Fatalf("canonical ID blocked board context: %v", err)
		}
		for _, command := range []string{"board", "ballot"} {
			text, err := runCLI(t, ro, command, "--project", slug)
			if err != nil || !strings.Contains(text, goals.Context(goal)) {
				t.Fatalf("%s lost canonical goal context: %s, %v", command, text, err)
			}
			text, err = runCLI(t, ro, command, "--project", slug, "--json")
			if err != nil {
				t.Fatal(err)
			}
			var got struct {
				Project string     `json:"project"`
				Goal    goals.Goal `json:"goal"`
			}
			if err := json.Unmarshal([]byte(text), &got); err != nil || got.Project != slug || got.Goal != goal {
				t.Fatalf("%s JSON lost canonical goal: %s, %v", command, text, err)
			}
		}
	}
}

func TestProjectGoalBoardAndBallotReadSurfaces(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	t.Setenv("PULSE_AGENT", "")
	t.Setenv("PULSE_SESSION", "")
	f := newFixture(t, false)
	ro := openReadOnly(t, f.home)
	readAll := func(goal goals.Goal) {
		t.Helper()
		var before int
		if err := f.st.DB.QueryRow("SELECT count(*) FROM events").Scan(&before); err != nil {
			t.Fatal(err)
		}
		for _, command := range []string{"board", "ballot"} {
			text, err := runCLI(t, ro, command, "--project", "fixture")
			if err != nil || !strings.Contains(text, goals.Context(goal)) || !strings.Contains(text, "not permission") {
				t.Fatalf("%s read did not carry the canonical goal: %v\n%s", command, err, text)
			}
			text, err = runCLI(t, ro, command, "--project", "fixture", "--json")
			if err != nil {
				t.Fatal(err)
			}
			var result struct {
				Project string     `json:"project"`
				Goal    goals.Goal `json:"goal"`
			}
			if err := json.Unmarshal([]byte(text), &result); err != nil || result.Project != "fixture" || result.Goal != goal {
				t.Fatalf("%s JSON goal: %+v, %v", command, result, err)
			}
		}
		var after int
		if err := f.st.DB.QueryRow("SELECT count(*) FROM events").Scan(&after); err != nil || before != after {
			t.Fatalf("board/ballot reads changed events: %d -> %d, %v", before, after, err)
		}
	}
	fallback, err := goals.Get(ro, "fixture")
	if err != nil {
		t.Fatal(err)
	}
	readAll(fallback)
	var n int
	if err := f.st.DB.QueryRow("SELECT count(*) FROM settings WHERE name LIKE 'project.goal.%'").Scan(&n); err != nil || n != 0 {
		t.Fatal("read surfaces persisted the legacy fallback")
	}
	owner, err := goals.Set(f.st, "fixture", "Make product error handling reliable and useful with existing checks.", "human")
	if err != nil {
		t.Fatal(err)
	}
	readAll(owner)
	for _, command := range []string{"board", "ballot"} {
		text, err := runCLI(t, ro, command, "--project", "commons")
		if err != nil || strings.Contains(text, owner.Text) || strings.Contains(text, "## Project goal") {
			t.Fatalf("commons inherited another project's goal: %s, %v", text, err)
		}
	}
}

func TestProjectGoalBoardReadPropagatesStorageFailure(t *testing.T) {
	f := newFixture(t, false)
	if _, err := f.st.DB.Exec("DROP TABLE settings"); err != nil {
		t.Fatal(err)
	}
	if text, err := DigestFor(f.st, "fixture", "", Thresholds{}, "", ""); err == nil || text != "" {
		t.Fatal("board silently replaced unreadable goal storage with a default")
	}
}
