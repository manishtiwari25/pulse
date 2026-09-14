package hooks

import (
	"context"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/goals"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

func TestProjectGoalHooksAndReflectStayCurrentAndReadOnly(t *testing.T) {
	for _, key := range []string{"PULSE_JOB", "PULSE_AGENT", "PULSE_PROJECT", "PULSE_SESSION"} {
		t.Setenv(key, "")
	}
	s := newStore(t)
	if _, err := s.DB.Exec("INSERT INTO projects (slug, name, path, added_at) VALUES ('demo','Demo',?,?)",
		filepath.Join(s.Home, "missing-checkout"), s.Now()); err != nil {
		t.Fatal(err)
	}
	ro, err := store.Open(store.Options{Home: s.Home, ReadOnly: true})
	if err != nil {
		t.Fatal(err)
	}
	defer ro.Close()
	for _, text := range []string{"", "Improve useful search with measured regression outcomes."} {
		if text != "" {
			if _, err := goals.Set(s, "demo", text, "human"); err != nil {
				t.Fatal(err)
			}
		}
		goal, err := goals.Get(ro, "demo")
		if err != nil {
			t.Fatal(err)
		}
		var beforeEvents, beforeSettings int
		if err := s.DB.QueryRow("SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM settings)").
			Scan(&beforeEvents, &beforeSettings); err != nil {
			t.Fatal(err)
		}
		reflected, err := evolve.Reflect(context.Background(), ro, "demo", "", "", "")
		if err != nil || !strings.Contains(reflected.Board, goals.Context(goal)) {
			t.Fatalf("reflection dropped the project goal: %+v, %v", reflected, err)
		}
		for _, harness := range []string{"copilot", "claude"} {
			ctx := &cli.Context{Project: "demo", Agent: "reader", Session: "interactive-session"}
			result := ContextJSON(harness, func(args ...string) string {
				scoped := contextArgs(ctx, args...)
				if !slices.Equal(scoped[1:], []string{"--project", "demo", "--agent", "reader", "--session", "interactive-session"}) {
					t.Fatalf("hook dropped explicit project/identity: %v", scoped)
				}
				if args[0] == "guide" {
					return "Existing PULSE rules apply."
				}
				digest, err := tally.DigestFor(ro, ctx.Project, "", tally.Thresholds{}, "", ctx.Session)
				if err != nil {
					t.Fatal(err)
				}
				return digest
			})
			if !strings.Contains(result, goal.Text) || !strings.Contains(result, "not permission") {
				t.Fatalf("%s interactive context dropped goal: %s", harness, result)
			}
		}
		var afterEvents, afterSettings int
		if err := s.DB.QueryRow("SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM settings)").
			Scan(&afterEvents, &afterSettings); err != nil || beforeEvents != afterEvents || beforeSettings != afterSettings {
			t.Fatalf("reflect/hook read mutated store: %d/%d -> %d/%d, %v", beforeEvents, beforeSettings, afterEvents, afterSettings, err)
		}
	}
}
