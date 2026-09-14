package goals

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func TestGoalCanonicalRegisteredSlugs(t *testing.T) {
	s := goalStore(t)
	const canonical = "customer-support-ticket-management-syste"
	if len(canonical) != 40 {
		t.Fatal("fixture must cover Slugify's 40-character boundary")
	}
	for _, slug := range []string{canonical, canonical + "-2", strings.Repeat("a", 40)} {
		goalProject(t, s, slug, "Customer support ticket management", false)
		before := goalSnapshot(t, s)
		fallback, err := Get(s, slug)
		if err != nil {
			t.Errorf("registered identifier %q was rejected: %v", slug, err)
			continue
		}
		if fallback.Source != Generated || !strings.Contains(fallback.Text, "real product defects") ||
			goalSnapshot(t, s) != before {
			t.Fatalf("legacy goal was not a read-only default: %+v", fallback)
		}
		if err := EnsureDefault(s, slug); err != nil {
			t.Fatal(err)
		}
		if saved := mustGoal(t, s, slug); saved.Text != fallback.Text || saved.Source != Generated {
			t.Fatalf("persisted default changed: %+v", saved)
		}
		owner, err := Set(s, slug, "Reduce incorrect ticket results using the existing checks.", "human")
		if err != nil {
			t.Fatal(err)
		}
		if got := mustGoal(t, s, slug); got != owner {
			t.Fatalf("registered identifier lost the owner's goal: %+v", got)
		}
		if reset, err := Reset(s, slug, "human"); err != nil || reset.Text != fallback.Text || reset.Source != Generated {
			t.Fatalf("registered identifier could not reset: %+v, %v", reset, err)
		}
	}
	for _, slug := range []string{"", "unknown-project", strings.Repeat("b", 40), store.CommonsSlug,
		"../elsewhere", "/home/example", "file:///Users/example", "path=/home/example", "one/two", "bad\x00slug"} {
		before := goalSnapshot(t, s)
		if _, err := Get(s, slug); err == nil {
			t.Errorf("invalid or unknown identifier was accepted: %q", slug)
		}
		if _, err := Set(s, slug, "Keep product results correct.", "human"); err == nil {
			t.Errorf("invalid or unknown identifier was writable: %q", slug)
		}
		if goalSnapshot(t, s) != before {
			t.Errorf("rejected identifier changed the store: %q", slug)
		}
	}
	if _, err := Normalize("Use this opaque value " + strings.Repeat("a", 40)); err == nil {
		t.Fatal("accepting registered IDs weakened goal-text credential protection")
	}
}

func TestGoalPrivatePathReferences(t *testing.T) {
	for _, text := range []string{
		"Use /Users/example/plan.md.",
		"Use file:///Users/example/plan.md.",
		"Use FILE:///uSeRs/example/plan.md.",
		"Use file:/Users/example/plan.md.",
		"Use file://localhost/Users/example/plan.md.",
		"Use file://server/home/example/plan.md.",
		"Use file://[::1]/home/x.",
		`Use "FILE://HOST/HOME/example/plan.md".`,
		"Use [file:///Users/example/plan.md].",
		"Use <file://host/Users/example/plan.md>.",
		"Use `file://host/tmp/plan.md`.",
		"Prioritize path=/home/example/plan.md.",
		`Prioritize path="/home/example/plan.md".`,
		"Prioritize path='file://host/home/x'.",
		"Prioritize path:FILE://HOST/HOME/x.",
		"Prioritize path=~/.config/plan.md.",
		"Prioritize path=private/plan.md.",
		"Prioritize path=.ssh/id_test.",
		"Use results/private",
		`Use "results/heldout"`,
		"Use file:///data/private",
		"Use file:///C:/work/plan.md.",
		"Use file://host/C:/work/plan.md.",
		`Prioritize path=C:\work\plan.md.`,
	} {
		t.Run(text, func(t *testing.T) {
			if _, err := Normalize(text); err == nil {
				t.Errorf("shared goal accepted a private path reference: %q", text)
			}
		})
	}
	for _, text := range []string{
		"Improve /api/search error handling.",
		"Prioritize path=/api/search.",
		`Prioritize path="/api/search".`,
		"Improve [GET /api/search] responses.",
		"Improve https://example.test/home/docs.",
		"Make https://example.test/api/search reliable.",
		"Keep user data private",
		"Make project settings private",
		"Improve docs/private.md",
	} {
		if got, err := Normalize(text); err != nil || got != text {
			t.Errorf("public route was rejected or changed: %q, %v", got, err)
		}
	}
}

func TestGoalPrivatePathMutationAndStoredReadRefusals(t *testing.T) {
	for _, text := range []string{"Use file:///Users/example/plan.md.", "Prioritize path=/home/example/plan.md."} {
		t.Run(text, func(t *testing.T) {
			s := goalStore(t)
			goalProject(t, s, "one", "One", false)
			owner, err := Set(s, "one", "Improve /api/search reliability.", "human")
			if err != nil {
				t.Fatal(err)
			}
			before := goalSnapshot(t, s)
			if _, err := Set(s, "one", text, "human"); err == nil {
				t.Error("owner mutation stored a prohibited private reference")
			} else if strings.Contains(err.Error(), text) {
				t.Error("rejection echoed the private reference")
			}
			if got := mustGoal(t, s, "one"); got != owner || goalSnapshot(t, s) != before {
				t.Error("rejected private reference changed the owner's goal or audit log")
			}
			value, err := json.Marshal(storedGoal{Text: text, Source: Owner})
			if err != nil {
				t.Fatal(err)
			}
			if _, err := s.DB.Exec("UPDATE settings SET value = ? WHERE name = ?", string(value), keyPrefix+"one"); err != nil {
				t.Fatal(err)
			}
			before = goalSnapshot(t, s)
			if goal, err := Get(s, "one"); err == nil || goal.Text != "" {
				t.Error("stored private reference was released to shared contexts")
			}
			if goalSnapshot(t, s) != before {
				t.Error("read rejection silently repaired the stored goal")
			}
		})
	}
}
