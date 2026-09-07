package records

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

const (
	templateConstitution = "# <Project> Evolution Constitution\n\nRules the loop can never change. <Project> ships the product.\n\n" +
		"## Immutable paths\n\n| Path | Why |\n| --- | --- |\n| <product hard-stop surfaces only> | Tier 2 surfaces (ADR-NNN) |\n"
	templateEvaluators = "# <Project> Evaluators\n\nWhat better means for <Project>.\n"
)

func namedProject(t *testing.T, s *store.Store, slug, name, dir string) {
	t.Helper()
	if _, err := s.DB.Exec("INSERT INTO projects (slug,name,path,brain,added_at) VALUES (?,?,?,0,?)", slug, name, dir, s.Now()); err != nil {
		t.Fatal(err)
	}
}

// seedTemplates gives the store a brain and the two template-owned records
// the way pulse migrate imports them; the constitution has two revisions so
// tests see that the current one is copied.
func seedTemplates(t *testing.T, s *store.Store) {
	t.Helper()
	addProject(t, s, "brain", t.TempDir(), true)
	templates := []struct {
		slug, body string
		front      map[string]any
	}{
		{"constitution", templateConstitution, map[string]any{"id": "CONSTITUTION", "title": "<Project> Evolution Constitution",
			"status": "Active", "date": "YYYY-MM-DD", "controlling_adr": "ADR-NNN", "loop": "off", "limits": "none"}},
		{"evaluators", templateEvaluators, map[string]any{"id": "EVALUATORS", "title": "<Project> Evaluators",
			"status": "Active", "date": "YYYY-MM-DD", "controlling_adr": "ADR-NNN", "baseline": "pending"}},
	}
	for _, tpl := range templates {
		path := "docs/evolution/_template-" + tpl.slug + ".md"
		rec := store.Record{ID: "TPL-evolution-" + tpl.slug, Project: "brain", Kind: "template", Slug: "evolution-" + tpl.slug,
			Title: textValue(tpl.front["title"]), Status: "Active", Path: path, Origin: "markdown:" + path + "@0000000"}
		if tpl.slug == "constitution" {
			if _, err := s.PutRecord(rec, tpl.front, "draft "+tpl.body, "tester", "import"); err != nil {
				t.Fatal(err)
			}
		}
		if _, err := s.PutRecord(rec, tpl.front, tpl.body, "tester", "import"); err != nil {
			t.Fatal(err)
		}
	}
}

func revisionCount(t *testing.T, s *store.Store, id string) int {
	t.Helper()
	revisions, err := s.Revisions(id)
	if err != nil {
		t.Fatal(err)
	}
	return len(revisions)
}

func TestSeedLoopRecordsCopiesTheTemplateOnce(t *testing.T) {
	s := tempStore(t)
	seedTemplates(t, s)
	namedProject(t, s, "lamplitlabs", "Lamplit Labs", t.TempDir())
	summary, err := SeedLoopRecords(s, "lamplitlabs", "alice")
	if err != nil {
		t.Fatal(err)
	}
	want := SeedSummary{Project: "lamplitlabs",
		Constitution: SeedResult{ID: "EVO-constitution-lamplitlabs", Action: "copied", Source: "TPL-evolution-constitution@r2", Rev: 1},
		Evaluators:   SeedResult{ID: "EVO-evaluators-lamplitlabs", Action: "copied", Source: "TPL-evolution-evaluators@r1", Rev: 1}}
	if summary != want {
		t.Fatalf("summary %+v, want %+v", summary, want)
	}
	rec, rev, err := s.GetRecord("EVO-constitution-lamplitlabs", 0)
	if err != nil {
		t.Fatal(err)
	}
	if rec.Kind != "evolution" || rec.Slug != "constitution" || rec.Project != "lamplitlabs" || rec.Origin != "store" ||
		rec.Title != "Lamplit Labs Evolution Constitution" || rec.Status != "Active" || rec.Date != "2026-09-06" || rec.CreatedBy != "alice" {
		t.Fatalf("copied head row: %+v", rec)
	}
	if rev.Author != "alice" || rev.Note != "seeded from TPL-evolution-constitution@r2 (ADR-030)" {
		t.Fatalf("copied revision: author %q note %q", rev.Author, rev.Note)
	}
	for key, value := range map[string]any{"id": "EVO-constitution-lamplitlabs", "title": "Lamplit Labs Evolution Constitution",
		"status": "Active", "date": "2026-09-06", "seeded_from": "brain/TPL-evolution-constitution@r2", "seeded_by": "alice",
		"loop": "off", "limits": "none", "controlling_adr": "ADR-NNN"} {
		if rev.Frontmatter[key] != value {
			t.Fatalf("frontmatter %s = %#v, want %#v (%v)", key, rev.Frontmatter[key], value, rev.Frontmatter)
		}
	}
	if len(rev.Frontmatter) != 9 {
		t.Fatalf("frontmatter keys: %v", rev.Frontmatter)
	}
	if rev.Body != strings.ReplaceAll(templateConstitution, "<Project>", "Lamplit Labs") || strings.Contains(rev.Body, "<Project>") ||
		!strings.Contains(rev.Body, "<product hard-stop surfaces only>") || !strings.Contains(rev.Body, "ADR-NNN") {
		t.Fatalf("copied body:\n%s", rev.Body)
	}
	_, evaluators, err := s.GetRecord("EVO-evaluators-lamplitlabs", 0)
	if err != nil {
		t.Fatal(err)
	}
	if evaluators.Frontmatter["baseline"] != "pending" || evaluators.Frontmatter["seeded_from"] != "brain/TPL-evolution-evaluators@r1" ||
		evaluators.Frontmatter["title"] != "Lamplit Labs Evaluators" || evaluators.Body != strings.ReplaceAll(templateEvaluators, "<Project>", "Lamplit Labs") {
		t.Fatalf("copied evaluators: %+v", evaluators)
	}
	if eventCount(t, s, "evolution.records.seeded") != 1 || eventCount(t, s, "record.created") != 4 {
		t.Fatal("seeding did not record one summary event beside the two created records")
	}
	if revisionCount(t, s, "TPL-evolution-constitution") != 2 || revisionCount(t, s, "TPL-evolution-evaluators") != 1 {
		t.Fatal("seeding wrote to the templates")
	}

	// Repeating keeps both; nothing is revised and no event is added.
	again, err := SeedLoopRecords(s, "lamplitlabs", "bob")
	if err != nil {
		t.Fatal(err)
	}
	kept := SeedSummary{Project: "lamplitlabs",
		Constitution: SeedResult{ID: "EVO-constitution-lamplitlabs", Action: "kept", Rev: 1},
		Evaluators:   SeedResult{ID: "EVO-evaluators-lamplitlabs", Action: "kept", Rev: 1}}
	if again != kept || revisionCount(t, s, "EVO-constitution-lamplitlabs") != 1 || eventCount(t, s, "evolution.records.seeded") != 1 {
		t.Fatalf("second seeding: %+v", again)
	}

	// An existing record is never touched, whatever its text; the missing one is copied.
	namedProject(t, s, "other", "", t.TempDir())
	own := store.Record{ID: "EVO-constitution-other", Project: "other", Kind: "evolution", Slug: "constitution", Title: "Own rules"}
	for _, body := range []string{"first\n", "second\n"} {
		if _, err := s.PutRecord(own, map[string]any{"title": "Own rules"}, body, "human", ""); err != nil {
			t.Fatal(err)
		}
	}
	summary, err = SeedLoopRecords(s, "other", "")
	if err != nil {
		t.Fatal(err)
	}
	if summary.Constitution != (SeedResult{ID: "EVO-constitution-other", Action: "kept", Rev: 2}) ||
		summary.Evaluators != (SeedResult{ID: "EVO-evaluators-other", Action: "copied", Source: "TPL-evolution-evaluators@r1", Rev: 1}) {
		t.Fatalf("mixed seeding: %+v", summary)
	}
	_, rev, _ = s.GetRecord("EVO-constitution-other", 0)
	if rev.Rev != 2 || rev.Body != "second\n" || rev.Frontmatter["seeded_from"] != nil {
		t.Fatalf("the owner's constitution changed: %+v", rev)
	}
	_, rev, _ = s.GetRecord("EVO-evaluators-other", 0)
	if rev.Frontmatter["seeded_by"] != store.HumanAgent || rev.Frontmatter["title"] != "other Evaluators" || rev.Author != store.HumanAgent {
		t.Fatalf("an empty actor is the human and an empty name is the slug: %+v", rev)
	}

	// The commons and unknown projects get nothing.
	var ce *cli.Error
	if _, err := SeedLoopRecords(s, store.CommonsSlug, "human"); !errors.As(err, &ce) || ce.Code != 2 {
		t.Fatalf("commons: %v", err)
	}
	if _, err := SeedLoopRecords(s, "absent", "human"); !errors.As(err, &ce) || ce.Code != 2 {
		t.Fatalf("unknown project: %v", err)
	}
	rows, err := s.ListRecords(store.RecordFilter{Kind: "evolution"})
	if err != nil || len(rows) != 4 {
		t.Fatalf("evolution records: %d (%v)", len(rows), err)
	}
}

func TestSeedLoopRecordsFallsBackToTheBrainsFirstRevision(t *testing.T) {
	s := tempStore(t)
	addProject(t, s, "brain", t.TempDir(), true)
	for _, slug := range []string{"constitution", "evaluators"} {
		rec := store.Record{ID: "EVO-" + slug, Project: "brain", Kind: "evolution", Slug: slug}
		front := map[string]any{"title": "Brain " + slug, "loop": "off"}
		if _, err := s.PutRecord(rec, front, "# Brain "+slug+"\n\nAs first written for <Project>.\n", "human", "import"); err != nil {
			t.Fatal(err)
		}
		front["loop"] = "on"
		if _, err := s.PutRecord(rec, front, "# Brain "+slug+"\n\nShaped by its owner.\n", "human", "amend"); err != nil {
			t.Fatal(err)
		}
	}
	namedProject(t, s, "app", "App", t.TempDir())
	summary, err := SeedLoopRecords(s, "app", "alice")
	if err != nil {
		t.Fatal(err)
	}
	if summary.Constitution != (SeedResult{ID: "EVO-constitution-app", Action: "copied", Source: "EVO-constitution@r1", Rev: 1}) ||
		summary.Evaluators != (SeedResult{ID: "EVO-evaluators-app", Action: "copied", Source: "EVO-evaluators@r1", Rev: 1}) {
		t.Fatalf("fallback summary: %+v", summary)
	}
	_, rev, err := s.GetRecord("EVO-constitution-app", 0)
	if err != nil {
		t.Fatal(err)
	}
	if rev.Body != "# Brain constitution\n\nAs first written for App.\n" || rev.Frontmatter["seeded_from"] != "brain/EVO-constitution@r1" ||
		rev.Frontmatter["loop"] != "off" || rev.Frontmatter["title"] != "Brain constitution" || rev.Note != "seeded from EVO-constitution@r1 (ADR-030)" {
		t.Fatalf("fallback copy: %+v", rev)
	}
	if revisionCount(t, s, "EVO-constitution") != 2 {
		t.Fatal("seeding wrote to the brain's record")
	}

	// The brain keeps its own records and is never seeded from itself.
	brain, err := SeedLoopRecords(s, "brain", "alice")
	if err != nil || brain.Constitution != (SeedResult{ID: "EVO-constitution", Action: "kept", Rev: 2}) || brain.Evaluators.Action != "kept" {
		t.Fatalf("brain: %+v %v", brain, err)
	}
	if _, err := s.DB.Exec("UPDATE records SET project = 'elsewhere' WHERE id = 'EVO-evaluators'"); err != nil {
		t.Fatal(err)
	}
	brain, err = SeedLoopRecords(s, "brain", "alice")
	if err != nil || brain.Constitution.Action != "kept" || brain.Evaluators != (SeedResult{ID: "EVO-evaluators", Action: "absent"}) {
		t.Fatalf("a brain without evaluators: %+v %v", brain, err)
	}
	if rows, _ := s.ListRecords(store.RecordFilter{Project: "brain"}); len(rows) != 1 {
		t.Fatalf("the brain received a record: %+v", rows)
	}
	if n := eventCount(t, s, "evolution.records.seeded"); n != 1 {
		t.Fatalf("seeded events after the brain's absent record: %d", n)
	}

	// Without any source text the copy is refused, and it says what to import.
	empty := tempStore(t)
	namedProject(t, empty, "lonely", "Lonely", t.TempDir())
	_, err = SeedLoopRecords(empty, "lonely", "alice")
	var ce *cli.Error
	if !errors.As(err, &ce) || ce.Code != 2 || !strings.Contains(ce.Why, "TPL-evolution-constitution") {
		t.Fatalf("no source: %v", err)
	}
	if rows, _ := empty.ListRecords(store.RecordFilter{}); len(rows) != 0 {
		t.Fatalf("a store without templates gained records: %+v", rows)
	}
	if eventCount(t, empty, "evolution.records.seeded") != 0 {
		t.Fatal("a refused seeding was recorded as done")
	}
}

func TestSeedLoopRecordsIndexesTheCheckoutsCommittedFile(t *testing.T) {
	s := tempStore(t)
	seedTemplates(t, s)
	repo := t.TempDir()
	if real, err := filepath.EvalSymlinks(repo); err == nil {
		repo = real
	}
	gitFixture(t, repo, "init", "-q", "-b", "main")
	write := func(name, text string) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(filepath.Join(repo, name)), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(repo, name), []byte(text), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("docs/evolution/constitution.md", "---\ntitle: Amistio Constitution\nstatus: Active\nloop: on\n---\n\n# Amistio Constitution\n\nThe owner's own rules.\n")
	write("docs/decisions/ADR-001-first.md", "---\nid: ADR-001\ntitle: First\nstatus: Accepted\n---\n\n# First\n")
	gitFixture(t, repo, "add", ".")
	gitFixture(t, repo, "commit", "-q", "-m", "rules")
	commit := strings.TrimSpace(gitFixture(t, repo, "rev-parse", "HEAD"))
	short := strings.TrimSpace(gitFixture(t, repo, "rev-parse", "--short", "HEAD"))
	// Only committed text counts: the working tree's evaluators.md is not HEAD's.
	write("docs/evolution/evaluators.md", "---\ntitle: Uncommitted\n---\n\n# Not yet committed\n")
	namedProject(t, s, "amistio", "Amistio", repo)
	summary, err := SeedLoopRecords(s, "amistio", "alice")
	if err != nil {
		t.Fatal(err)
	}
	if summary.Constitution != (SeedResult{ID: "EVO-constitution-amistio", Action: "imported", Source: "docs/evolution/constitution.md@" + short, Rev: 1}) ||
		summary.Evaluators != (SeedResult{ID: "EVO-evaluators-amistio", Action: "copied", Source: "TPL-evolution-evaluators@r1", Rev: 1}) {
		t.Fatalf("checkout seeding: %+v", summary)
	}
	rec, rev, err := s.GetRecord("EVO-constitution-amistio", 0)
	if err != nil {
		t.Fatal(err)
	}
	if rec.Origin != "markdown:docs/evolution/constitution.md@"+commit || rec.Path != "docs/evolution/constitution.md" ||
		rec.Title != "Amistio Constitution" || rec.Status != "Active" || rec.Date == "" {
		t.Fatalf("imported head row: %+v", rec)
	}
	if rev.Body != "\n# Amistio Constitution\n\nThe owner's own rules.\n" || rev.Frontmatter["loop"] != "on" || rev.Frontmatter["seeded_from"] != nil ||
		rev.Author != "alice" || rev.Note != "import docs/evolution/constitution.md@"+commit {
		t.Fatalf("imported revision: %+v", rev)
	}
	_, evaluators, _ := s.GetRecord("EVO-evaluators-amistio", 0)
	if !strings.Contains(evaluators.Body, "What better means for Amistio.") {
		t.Fatalf("evaluators were not copied from the template: %q", evaluators.Body)
	}
	rows, err := s.ListRecords(store.RecordFilter{Project: "amistio"})
	if err != nil || len(rows) != 2 {
		t.Fatalf("seeding indexed more than the loop's records: %+v (%v)", rows, err)
	}
	if eventCount(t, s, "records.migrated") != 1 || eventCount(t, s, "evolution.records.seeded") != 1 {
		t.Fatal("the import and the seeding were not each recorded once")
	}
	again, err := SeedLoopRecords(s, "amistio", "alice")
	if err != nil || again.Constitution.Action != "kept" || again.Evaluators.Action != "kept" || again.Constitution.Rev != 1 {
		t.Fatalf("repeat: %+v %v", again, err)
	}

	// A file HEAD has but cannot index is an error naming the reason, not a silent template copy.
	broken := t.TempDir()
	gitFixture(t, broken, "init", "-q", "-b", "main")
	if err := os.MkdirAll(filepath.Join(broken, "docs", "evolution"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(broken, "docs", "evolution", "constitution.md"), []byte("---\ntitle: broken\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitFixture(t, broken, "add", ".")
	gitFixture(t, broken, "commit", "-q", "-m", "broken")
	namedProject(t, s, "broken", "Broken", broken)
	var ce *cli.Error
	if _, err := SeedLoopRecords(s, "broken", "alice"); !errors.As(err, &ce) || !strings.Contains(ce.Why, "frontmatter") {
		t.Fatalf("unindexable file: %v", err)
	}
	if rows, _ := s.ListRecords(store.RecordFilter{Project: "broken"}); len(rows) != 0 {
		t.Fatalf("a broken file produced records: %+v", rows)
	}

	// A repository without a commit, and a plain directory, fall through to the copy.
	bare := t.TempDir()
	gitFixture(t, bare, "init", "-q", "-b", "main")
	if err := os.MkdirAll(filepath.Join(bare, "docs", "evolution"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bare, "docs", "evolution", "constitution.md"), []byte("# Unstaged\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	namedProject(t, s, "fresh", "Fresh", bare)
	namedProject(t, s, "plain", "Plain", t.TempDir())
	for _, slug := range []string{"fresh", "plain"} {
		summary, err := SeedLoopRecords(s, slug, "alice")
		if err != nil || summary.Constitution.Action != "copied" || summary.Constitution.Source != "TPL-evolution-constitution@r2" {
			t.Fatalf("%s: %+v %v", slug, summary, err)
		}
	}
}

func TestImportRepoPathsRestrictTheCommittedTree(t *testing.T) {
	repo := sourceRepo(t)
	s := tempStore(t)
	addProject(t, s, "brain", repo, true)
	summary, err := ImportRepo(s, repo, fixtureRef, "", ImportOptions{Author: "tester", Paths: []string{"docs/evolution/constitution.md"}})
	if err != nil || summary.Imported != 1 || len(summary.Skipped) != 0 {
		t.Fatalf("restricted import: %+v %v", summary, err)
	}
	rows, err := s.ListRecords(store.RecordFilter{})
	if err != nil || len(rows) != 1 || rows[0].ID != "EVO-constitution" {
		t.Fatalf("records after a restricted import: %+v (%v)", rows, err)
	}
	folder, err := ImportRepo(s, repo, fixtureRef, "", ImportOptions{Author: "tester", Paths: []string{"docs/evolution"}})
	if err != nil || folder.Imported == 0 || folder.Kinds["adr"].Imported != 0 {
		t.Fatalf("folder-restricted import: %+v %v", folder, err)
	}
}
