package evolve

import (
	"errors"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
)

// seedTemplates registers a brain whose store holds the template-owned
// records every seeded copy starts from, the way the brain's own import
// leaves them.
func seedTemplates(t *testing.T, s *store.Store) {
	t.Helper()
	hook := board.OnProjectCreated
	board.OnProjectCreated = nil
	defer func() { board.OnProjectCreated = hook }()
	repo := t.TempDir()
	testGit(t, repo, "init", "--quiet", "--initial-branch=main")
	putFile(t, filepath.Join(repo, "README.md"), "# brain\n", 0o644)
	testGit(t, repo, "add", ".")
	testGit(t, repo, "commit", "--quiet", "-m", "start")
	if _, err := board.AddProject(s, repo, "brain", "", true, "", "human", "brain"); err != nil {
		t.Fatal(err)
	}
	for _, slug := range []string{"constitution", "evaluators"} {
		title := "<Project> Evolution " + strings.ToUpper(slug[:1]) + slug[1:]
		front := map[string]any{"id": strings.ToUpper(slug), "title": title, "status": "Active", "loop": "off", "limits": "none"}
		if slug == "evaluators" {
			front["baseline"] = "pending"
		}
		path := "docs/evolution/_template-" + slug + ".md"
		rec := store.Record{ID: "TPL-evolution-" + slug, Project: "brain", Kind: "template", Slug: "evolution-" + slug,
			Title: title, Status: "Active", Path: path, Origin: "markdown:" + path + "@0000000"}
		if _, err := s.PutRecord(rec, front, "# "+title+"\n\nWhat the <Project> loop may change.\n", "human", "import "+path); err != nil {
			t.Fatal(err)
		}
	}
}

func eventOrder(t *testing.T, s *store.Store, subject string, actions ...string) []int64 {
	t.Helper()
	seqs := make([]int64, 0, len(actions))
	for _, action := range actions {
		var seq int64
		if err := s.DB.QueryRow("SELECT COALESCE(MIN(seq), 0) FROM events WHERE action = ? AND subject = ?", action, subject).Scan(&seq); err != nil {
			t.Fatal(err)
		}
		seqs = append(seqs, seq)
	}
	return seqs
}

func TestRegisteringAProjectSeedsTheRecordsThenTheFixtures(t *testing.T) {
	s := testStore(t)
	seedTemplates(t, s)
	repo := t.TempDir()
	testGit(t, repo, "init", "--quiet", "--initial-branch=main")
	putFile(t, filepath.Join(repo, "README.md"), "# lamplit\n", 0o644)
	testGit(t, repo, "add", ".")
	testGit(t, repo, "commit", "--quiet", "-m", "start")
	for i := 0; i < 2; i++ {
		if _, err := board.AddProject(s, repo, "Lamplit Labs", "", false, "", "alice", "lamplitlabs"); err != nil {
			t.Fatal(err)
		}
	}
	for _, slug := range []string{"constitution", "evaluators"} {
		rec, rev, err := s.GetRecord("EVO-"+slug+"-lamplitlabs", 0)
		if err != nil {
			t.Fatalf("%s after registration: %v", slug, err)
		}
		want := "Lamplit Labs Evolution " + strings.ToUpper(slug[:1]) + slug[1:]
		if rec.CurrentRev != 1 || rec.Project != "lamplitlabs" || rec.Kind != "evolution" || rec.Slug != slug || rec.Title != want || rev.Author != "alice" {
			t.Fatalf("seeded %s: %+v %+v", slug, rec, rev)
		}
		if rev.Frontmatter["seeded_from"] != "brain/TPL-evolution-"+slug+"@r1" || rev.Frontmatter["seeded_by"] != "alice" ||
			rev.Frontmatter["loop"] != "off" || !strings.Contains(rev.Body, "the Lamplit Labs loop") {
			t.Fatalf("seeded %s revision: %+v", slug, rev)
		}
	}
	if fixtures, err := ListFixtures(s, "lamplitlabs", true); err != nil || len(fixtures) != 6 {
		t.Fatalf("starter fixtures after registration: %d (%v)", len(fixtures), err)
	}
	seqs := eventOrder(t, s, "lamplitlabs", "evolution.records.seeded", "evolution.fixture.seeded")
	if seqs[0] == 0 || seqs[1] == 0 || seqs[0] > seqs[1] {
		t.Fatalf("records must seed before fixtures: %v", seqs)
	}
	var failed int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action IN ('evolution.records.seed_failed', 'evolution.fixture.seed_failed', 'project.hook_failed')").Scan(&failed); err != nil || failed != 0 {
		t.Fatalf("seed failures recorded: %d (%v)", failed, err)
	}
	status, err := Status(s, "lamplitlabs")
	if err != nil || !status.Constitution || !status.Evaluators || strings.Contains(status.NextHumanStep, "evolve seed") {
		t.Fatalf("status names a seeded record as missing: %+v (%v)", status, err)
	}
	if _, _, err := s.GetRecord("EVO-constitution-brain", 0); !errors.Is(err, store.ErrNoRecord) {
		t.Fatalf("the brain received a seeded copy: %v", err)
	}
}

func TestRecordsSeedFailureIsRecordedAndFixturesStillSeed(t *testing.T) {
	s := testStore(t)
	// The binary carries the templates (ADR-038), so the copy always has a
	// source; what still fails is a committed constitution the import cannot
	// index.
	repo := testProject(t, s, "demo")
	putFile(t, filepath.Join(repo, "docs", "evolution", "constitution.md"), "---\nid: [broken\n---\n\n# Demo\n", 0o644)
	testGit(t, repo, "add", ".")
	testGit(t, repo, "commit", "--quiet", "-m", "broken constitution")
	p, err := s.Project("demo")
	if err != nil {
		t.Fatal(err)
	}
	if err := seedOnCreate(s, p, ""); err != nil {
		t.Fatalf("a records failure escaped the hook: %v", err)
	}
	var failed int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = 'evolution.records.seed_failed' AND subject = 'demo' AND actor = ?", store.HumanAgent).Scan(&failed); err != nil || failed != 1 {
		t.Fatalf("records.seed_failed events: %d (%v)", failed, err)
	}
	if fixtures, err := ListFixtures(s, "demo", true); err != nil || len(fixtures) != 6 {
		t.Fatalf("fixtures after a records failure: %d (%v)", len(fixtures), err)
	}
	for _, id := range []string{"EVO-constitution-demo", "EVO-evaluators-demo"} {
		if _, _, err := s.GetRecord(id, 0); !errors.Is(err, store.ErrNoRecord) {
			t.Fatalf("%s written after the failure: %v", id, err)
		}
	}
	bundle, err := records.FrameworkBundle()
	if err != nil {
		t.Fatal(err)
	}
	if n := countRows(t, s, "records"); n != len(bundle.Records) {
		t.Fatalf("records after the failure: %d (the framework's %d and nothing else)", n, len(bundle.Records))
	}
	seedTemplates(t, s)
	brain, err := s.Brain()
	if err != nil || brain == nil {
		t.Fatalf("brain: %+v (%v)", brain, err)
	}
	if err := seedOnCreate(s, brain, "human"); err != nil {
		t.Fatal(err)
	}
	if _, _, err := s.GetRecord("EVO-constitution", 0); !errors.Is(err, store.ErrNoRecord) {
		t.Fatalf("the hook seeded the brain's own records: %v", err)
	}
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = 'evolution.records.seed_failed'").Scan(&failed); err != nil || failed != 1 {
		t.Fatalf("the brain's skipped records were counted as a failure: %d (%v)", failed, err)
	}
}

func TestEvolveSeedCommand(t *testing.T) {
	s := testStore(t)
	seedTemplates(t, s)
	testProject(t, s, "demo")
	testProject(t, s, "other")
	governance(t, s, "other")
	if _, err := records.Amend(s, "EVO-evaluators-other", "Revised evaluators.\n", "human", "revised"); err != nil {
		t.Fatal(err)
	}
	joinTestAgent(t, s, "tester")
	out, _, err := runTestCommand(t, s, "evolve", "seed", "--project", "demo")
	want := "demo: constitution copied from TPL-evolution-constitution@r1, evaluators copied from TPL-evolution-evaluators@r1, 6 starter fixture(s) added; 6 in the set\n"
	if err != nil || out != want {
		t.Fatalf("seed demo: %q %v", out, err)
	}
	if _, rev, err := s.GetRecord("EVO-constitution-demo", 0); err != nil || rev.Author != "tester" || rev.Frontmatter["seeded_by"] != "tester" ||
		rev.Note != "seeded from TPL-evolution-constitution@r1 (ADR-030)" {
		t.Fatalf("copied revision: %+v (%v)", rev, err)
	}
	out, _, err = runTestCommand(t, s, "evolve", "seed", "--project", "")
	if err != nil || !strings.Contains(out, "demo: constitution kept (EVO-constitution-demo r1), evaluators kept (EVO-evaluators-demo r1), 0 starter fixture(s) added; 6 in the set\n") ||
		!strings.Contains(out, "other: constitution kept (EVO-constitution-other r1), evaluators kept (EVO-evaluators-other r2), 6 starter fixture(s) added; 6 in the set\n") ||
		!strings.Contains(out, "brain: constitution absent (EVO-constitution; the brain's records arrive by import), evaluators absent (EVO-evaluators; the brain's records arrive by import), 6 starter fixture(s) added; 6 in the set\n") {
		t.Fatalf("seed all: %q %v", out, err)
	}
	out, _, err = runTestCommand(t, s, "evolve", "seed", "--json", "--project", "demo")
	if err != nil || !strings.Contains(out, `"added": 0`) || !strings.Contains(out, `"action": "kept"`) || !strings.Contains(out, `"id": "EVO-evaluators-demo"`) {
		t.Fatalf("seed json: %q %v", out, err)
	}
	requireJSON(t, out)
	if n := countRows(t, s, "evolution_fixtures"); n != 18 {
		t.Fatalf("fixtures after seeding three projects: %d", n)
	}
	var seeded int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = 'evolution.records.seeded'").Scan(&seeded); err != nil || seeded != 1 {
		t.Fatalf("records.seeded events: %d (%v)", seeded, err)
	}
	var ce *cli.Error
	if _, _, err = runTestCommand(t, s, "evolve", "seed", "extra"); !errors.As(err, &ce) || ce.Code != 2 {
		t.Fatalf("seed accepted a positional argument: %v", err)
	}
	if _, _, err = runTestCommand(t, s, "evolve", "seed", "--project", "missing"); err == nil {
		t.Fatal("seed accepted an unregistered project")
	}
	if _, _, err = runTestCommand(t, s, "evolve", "seed", "--project", store.CommonsSlug); err == nil {
		t.Fatal("seed accepted the commons")
	}
}

func TestEvolveSeedIndexesTheCheckoutsRecords(t *testing.T) {
	s := testStore(t)
	seedTemplates(t, s)
	repo := testProject(t, s, "demo")
	putFile(t, filepath.Join(repo, "docs", "evolution", "constitution.md"),
		"---\nid: CONSTITUTION\ntitle: Demo Constitution\nstatus: Active\ndate: 2026-01-02\nloop: off\n---\n\n# Demo Constitution\n\nHand-written scope.\n", 0o644)
	testGit(t, repo, "add", ".")
	testGit(t, repo, "commit", "--quiet", "-m", "constitution")
	short := testGit(t, repo, "rev-parse", "--short", "HEAD")
	joinTestAgent(t, s, "tester")
	out, _, err := runTestCommand(t, s, "evolve", "seed", "--project", "demo")
	want := "demo: constitution imported from docs/evolution/constitution.md@" + short + ", evaluators copied from TPL-evolution-evaluators@r1, 6 starter fixture(s) added; 6 in the set\n"
	if err != nil || out != want {
		t.Fatalf("seed demo: %q %v", out, err)
	}
	rec, rev, err := s.GetRecord("EVO-constitution-demo", 0)
	if err != nil || !strings.HasPrefix(rec.Origin, "markdown:docs/evolution/constitution.md@") || rec.Title != "Demo Constitution" ||
		!strings.Contains(rev.Body, "Hand-written scope.") || rev.Frontmatter["seeded_from"] != nil {
		t.Fatalf("imported constitution: %+v %+v (%v)", rec, rev, err)
	}
	// The framework's records arrive with the command (ADR-038); the import
	// itself must index the one file only.
	bundle, err := records.FrameworkBundle()
	if err != nil {
		t.Fatal(err)
	}
	if n := countRows(t, s, "records"); n != len(bundle.Records)+2 {
		t.Fatalf("records after seeding: %d (the framework's %d, the constitution and the evaluators)", n, len(bundle.Records))
	}
}

func TestStatusOffersTheRecordSeedFirst(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	status, err := Status(s, "demo")
	if err != nil || status.Reason != "constitution record is missing or empty" {
		t.Fatalf("status: %+v (%v)", status, err)
	}
	want := "pulse evolve seed --project demo  (template copy), or pulse record new evolution --project demo --title Constitution --body-file FILE"
	if status.NextHumanStep != want {
		t.Fatalf("constitution step %q", status.NextHumanStep)
	}
	if _, err := s.PutRecord(store.Record{ID: "EVO-constitution-demo", Project: "demo", Kind: "evolution", Slug: "constitution"},
		nil, "# Constitution\n", "human", "fixture"); err != nil {
		t.Fatal(err)
	}
	status, err = Status(s, "demo")
	want = "pulse evolve seed --project demo  (template copy), or pulse record new evolution --project demo --title Evaluators --body-file FILE"
	if err != nil || status.Reason != "evaluators record is missing or empty" || status.NextHumanStep != want {
		t.Fatalf("evaluators step: %+v (%v)", status, err)
	}
}
