package runner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
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

func onboardRepo(t *testing.T, name string, files map[string]string) string {
	t.Helper()
	dir := filepath.Join(t.TempDir(), name)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	run(t, dir, "git", "init", "-q", "-b", "main")
	files["README.md"] = "# " + name + "\n"
	for rel, body := range files {
		if err := os.MkdirAll(filepath.Dir(filepath.Join(dir, rel)), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, rel), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "init")
	return dir
}

// onboardTemplates registers a brain and gives the store the template-owned
// records a registration copies (ADR-030).
func onboardTemplates(t *testing.T, s *store.Store) {
	t.Helper()
	if _, err := onboardProject(s, onboardRepo(t, "brain", map[string]string{}), testNow, onboardOpts{Actor: humanAgent, Brain: true}); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"constitution", "evaluators"} {
		title := "<Project> Evolution " + strings.ToUpper(name[:1]) + name[1:]
		rec := store.Record{ID: "TPL-evolution-" + name, Project: "brain", Kind: "template", Slug: "evolution-" + name, Title: title, Status: "Active",
			Path: "docs/evolution/_template-" + name + ".md", Origin: "markdown:docs/evolution/_template-" + name + ".md@0000000"}
		if _, err := s.PutRecord(rec, map[string]any{"id": strings.ToUpper(name), "title": title, "status": "Active", "loop": "off"},
			"# "+title+"\n\nThe <Project> loop.\n", humanAgent, "import"); err != nil {
			t.Fatal(err)
		}
	}
}

// ADR-030: onboarding reports the loop's records as registration seeded them
// and stops naming the constitution as the human's first step.
func TestOnboardReportsTheSeededRecords(t *testing.T) {
	s := newStore(t)
	dir := onboardRepo(t, "bare", map[string]string{})
	lines, err := onboardProject(s, dir, testNow, onboardOpts{Actor: humanAgent})
	if err != nil {
		t.Fatal(err)
	}
	text := strings.Join(lines, "\n")
	for _, want := range []string{
		"no docs/evolution/ scaffold written: the loop's constitution and evaluators live as records",
		"constitution: absent: pulse evolve seed --project bare",
		"evaluators: absent: pulse evolve seed --project bare",
	} {
		if !strings.Contains(text, want) {
			t.Fatalf("missing %q in:\n%s", want, text)
		}
	}
	onboardTemplates(t, s)
	copied := onboardRepo(t, "copied", map[string]string{})
	lines, err = onboardProject(s, copied, testNow, onboardOpts{Actor: humanAgent, Name: "Lamplit Labs"})
	if err != nil {
		t.Fatal(err)
	}
	text = strings.Join(lines, "\n")
	for _, want := range []string{
		"constitution: copied from TPL-evolution-constitution@r1 (EVO-constitution-lamplit-labs)",
		"evaluators: copied from TPL-evolution-evaluators@r1 (EVO-evaluators-lamplit-labs)",
		"seeded 6 starter fixtures (pulse evolve fixture list --project lamplit-labs)",
		"3. set loop: on in the constitution record",
	} {
		if !strings.Contains(text, want) {
			t.Fatalf("missing %q in:\n%s", want, text)
		}
	}
	if strings.Contains(text, "constitution: absent") || strings.Contains(text, "record new") {
		t.Fatalf("a seeded project is still told to write its records:\n%s", text)
	}
	if _, rev, err := s.GetRecord("EVO-constitution-lamplit-labs", 0); err != nil || rev.Author != humanAgent || rev.Frontmatter["seeded_by"] != humanAgent ||
		!strings.Contains(rev.Body, "The Lamplit Labs loop.") {
		t.Fatalf("seeded copy: %+v (%v)", rev, err)
	}
	// The owner's first edit makes the copy theirs: a refresh reports the
	// revision, and the untouched evaluators stay the seeded copy.
	if _, err := records.Amend(s, "EVO-constitution-lamplit-labs", "# Ours\n", humanAgent, "shaped"); err != nil {
		t.Fatal(err)
	}
	lines, err = onboardProject(s, copied, testNow, onboardOpts{Actor: humanAgent})
	if err != nil {
		t.Fatal(err)
	}
	text = strings.Join(lines, "\n")
	if !strings.Contains(text, "constitution: present (EVO-constitution-lamplit-labs r2)") ||
		!strings.Contains(text, "evaluators: copied from TPL-evolution-evaluators@r1 (EVO-evaluators-lamplit-labs)") {
		t.Fatalf("refresh after the owner's edit:\n%s", text)
	}

	// A checkout that brings its own docs/evolution/ has those files indexed
	// as its records; the folder is left alone.
	front := "---\nid: CONSTITUTION\ntitle: Amistio Constitution\nstatus: Active\ndate: 2026-01-02\nloop: off\n---\n\n"
	own := onboardRepo(t, "amistio", map[string]string{
		"docs/evolution/constitution.md": front + "# Amistio Constitution\n\nHand-written scope.\n",
		"docs/evolution/evaluators.md":   strings.Replace(front, "CONSTITUTION\ntitle: Amistio Constitution", "EVALUATORS\ntitle: Amistio Evaluators", 1) + "# Amistio Evaluators\n\nHand-written evaluators.\n",
		"docs/evolution/archive.md":      "# Archive\n",
	})
	lines, err = onboardProject(s, own, testNow, onboardOpts{Actor: humanAgent})
	if err != nil {
		t.Fatal(err)
	}
	text = strings.Join(lines, "\n")
	for _, want := range []string{
		"docs/evolution/ already present, untouched",
		"constitution: imported from docs/evolution/constitution.md (EVO-constitution-amistio)",
		"evaluators: imported from docs/evolution/evaluators.md (EVO-evaluators-amistio)",
	} {
		if !strings.Contains(text, want) {
			t.Fatalf("missing %q in:\n%s", want, text)
		}
	}
	rec, rev, err := s.GetRecord("EVO-constitution-amistio", 0)
	if err != nil || !strings.HasPrefix(rec.Origin, "markdown:docs/evolution/constitution.md@") || !strings.Contains(rev.Body, "Hand-written scope.") {
		t.Fatalf("indexed constitution: %+v %+v (%v)", rec, rev, err)
	}
	if _, err := os.Stat(filepath.Join(own, "docs", "evolution", "archive.md")); err != nil {
		t.Fatal("onboarding touched the checkout's docs/evolution/")
	}
	if rows, _ := s.ListRecords(store.RecordFilter{Project: "amistio"}); len(rows) != 2 {
		t.Fatalf("onboarding indexed more than the loop's two records: %+v", rows)
	}
}
