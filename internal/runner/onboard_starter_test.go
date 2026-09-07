package runner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/evolve"
)

// ADR-029: onboarding seeds the starter fixtures and its human steps read
// like pulse evolve status.
func TestOnboardSeedsStartersAndNamesTheHumanSteps(t *testing.T) {
	s := newStore(t)
	dir := filepath.Join(t.TempDir(), "seeded")
	os.MkdirAll(dir, 0o755)
	run(t, dir, "git", "init", "-q", "-b", "main")
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# seeded\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "init")
	lines, err := onboardProject(s, dir, testNow, onboardOpts{Actor: humanAgent})
	if err != nil {
		t.Fatal(err)
	}
	text := strings.Join(lines, "\n")
	for _, want := range []string{
		"seeded 6 starter fixtures (pulse evolve fixture list --project seeded)",
		"What stays human:",
		"1. record the baseline: pulse job add --project seeded --kind experiment --title 'Establish baseline' --brief 'Hypothesis: ONE_VARIABLE' (the runner measures the starter fixtures; costs real agent runs)",
		"2. keep at least two private fixtures in evolve.heldout_dir: pulse evolve setup DIR",
		"3. set loop: on in the constitution record",
		"4. keep the daemon alive with `pulse service install`",
	} {
		if !strings.Contains(text, want) {
			t.Fatalf("missing %q in:\n%s", want, text)
		}
	}
	if strings.Contains(text, "B- row") || strings.Contains(text, "starter fixtures absent") {
		t.Fatalf("stale wording:\n%s", text)
	}
	fixtures, err := evolve.ListFixtures(s, "seeded", true)
	if err != nil || len(fixtures) != 6 {
		t.Fatalf("starter fixtures after onboarding: %d (%v)", len(fixtures), err)
	}
	status, err := evolve.Status(s, "seeded")
	if err != nil || status.PublicFixtures != 6 {
		t.Fatalf("status after onboarding: %+v (%v)", status, err)
	}

	// Onboarding again refreshes and says nothing about seeding; a project
	// whose starters are gone is pointed at the seed command.
	lines, err = onboardProject(s, dir, testNow, onboardOpts{Actor: humanAgent})
	if err != nil {
		t.Fatal(err)
	}
	if text = strings.Join(lines, "\n"); strings.Contains(text, "seeded 6") || strings.Contains(text, "starter fixtures absent") {
		t.Fatalf("refresh wording:\n%s", text)
	}
	if _, err := s.DB.Exec("UPDATE evolution_fixtures SET name = 'renamed-' || name WHERE project = 'seeded'"); err != nil {
		t.Fatal(err)
	}
	lines, err = onboardProject(s, dir, testNow, onboardOpts{Actor: humanAgent})
	if err != nil {
		t.Fatal(err)
	}
	if text = strings.Join(lines, "\n"); !strings.Contains(text, "starter fixtures absent: pulse evolve fixture seed --project seeded") {
		t.Fatalf("absent-starters wording:\n%s", text)
	}
}
