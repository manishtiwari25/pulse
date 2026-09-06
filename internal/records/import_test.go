package records

import (
	"bytes"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

const fixtureRef = "02691f8"

func tempStore(t *testing.T) *store.Store {
	t.Helper()
	home := t.TempDir()
	t.Setenv("PULSE_HOME", home)
	t.Setenv("PULSE_JOB", "")
	if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(store.Options{Home: home, Now: "2026-09-06T21:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func addProject(t *testing.T, s *store.Store, slug, dir string, brain bool) {
	t.Helper()
	_, err := s.DB.Exec("INSERT INTO projects (slug,name,path,brain,added_at) VALUES (?,?,?,?,?)", slug, slug, dir, brain, s.Now())
	if err != nil {
		t.Fatal(err)
	}
}

func sourceRepo(t *testing.T) string {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git is unavailable")
	}
	_, file, _, _ := runtime.Caller(0)
	repo := filepath.Clean(filepath.Join(filepath.Dir(file), "..", ".."))
	if err := exec.Command("git", "-C", repo, "cat-file", "-e", fixtureRef+"^{commit}").Run(); err != nil {
		t.Skip("committed source fixtures at " + fixtureRef + " are unavailable")
	}
	return repo
}

func gitFixture(t *testing.T, repo string, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", repo}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull,
		"GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@x", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@x")
	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, output)
	}
	return string(output)
}

func eventCount(t *testing.T, s *store.Store, action string) int {
	t.Helper()
	var n int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = ?", action).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

func TestImportRealRepoAndExportRoundTrip(t *testing.T) {
	repo := sourceRepo(t)
	s := tempStore(t)
	addProject(t, s, "brain", repo, true)
	dry, err := ImportRepo(s, repo, fixtureRef, "", ImportOptions{DryRun: true, Author: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if rows, err := List(s, Filter{}); err != nil || len(rows) != 0 {
		t.Fatalf("dry run wrote records: %d %v", len(rows), err)
	}
	if eventCount(t, s, "records.migrated") != 0 || eventCount(t, s, "record.created") != 0 {
		t.Fatal("dry run wrote events")
	}
	first, err := ImportRepo(s, repo, fixtureRef, "", ImportOptions{Author: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	if first.Imported != dry.Imported || first.Revised != 0 {
		t.Fatalf("dry-run mismatch: dry=%+v actual=%+v", dry, first)
	}
	if first.Imported != 159 || len(first.Skipped) != 15 {
		t.Fatalf("imported %d, skipped %d; want 159 records and 15 non-document/index files", first.Imported, len(first.Skipped))
	}
	counts := map[string]int{}
	for _, kind := range store.RecordKinds {
		counts[kind] = first.Kinds[kind].Imported
	}
	t.Logf("Import counts at %s: %v; total %d; skipped %d", fixtureRef, counts, first.Imported, len(first.Skipped))
	for _, skip := range first.Skipped {
		t.Logf("Skipped %s: %s", skip.Path, skip.Reason)
	}
	adrTree := gitFixture(t, repo, "ls-tree", "-r", "--name-only", fixtureRef, "--", "docs/decisions")
	adrCount := 0
	for _, name := range strings.Split(adrTree, "\n") {
		if strings.HasPrefix(filepath.Base(name), "ADR-") && strings.HasSuffix(name, ".md") {
			adrCount++
		}
	}
	if counts["adr"] != adrCount || adrCount != 26 {
		t.Fatalf("ADRs imported %d, git lists %d (02691f8 has 26, not 27)", counts["adr"], adrCount)
	}
	wantCounts := map[string]int{
		"adr": 26, "plan": 20, "wu": 21, "feature": 16, "memory": 3,
		"workflow": 23, "skill": 13, "guide": 5, "context": 4, "architecture": 1,
		"evolution": 3, "instruction": 3, "template": 9, "usage": 2, "prompt": 10,
	}
	if !reflect.DeepEqual(counts, wantCounts) {
		t.Errorf("counts = %#v, want %#v", counts, wantCounts)
	}
	for _, id := range []string{"ADR-019", "PLAN-026", "WU-022", "FEAT-016", "EVO-constitution", "EVO-archive", "CTX-environment", "GUIDE-docs", "USAGE-versions", "PROMPT-eli5", "TPL-adr", "TPL-wu"} {
		if _, _, err := s.GetRecord(id, 0); err != nil {
			t.Errorf("missing %s: %v", id, err)
		}
	}
	rec, rev, err := s.GetRecord("INSTR-agents", 0)
	if err != nil || rec.Date == "" || len(rev.Frontmatter) != 0 || rec.Title != "PULSE - Agent Operating Guide" {
		t.Fatalf("instruction metadata: %+v %+v %v", rec, rev, err)
	}
	archive, _, err := s.GetRecord("EVO-archive", 0)
	if err != nil || archive.Kind != "evolution" || archive.Slug != "archive" || archive.Path != "docs/evolution/archive.md" {
		t.Fatalf("historical archive metadata: %+v %v", archive, err)
	}
	if eventCount(t, s, "record.created") != first.Imported || eventCount(t, s, "records.migrated") != 1 || eventCount(t, s, "record.imported") != 0 {
		t.Fatal("import did not use one created event per record and one migration summary")
	}
	var preview bytes.Buffer
	migrateContext := &cli.Context{Home: s.Home, Out: &preview, Err: &preview, Agent: "tester"}
	if err := runMigrate(migrateContext, nil, []string{"--records", "--repo", repo, "--ref", fixtureRef, "--dry-run", "--json"}); err != nil {
		t.Fatalf("migration command: %v", err)
	}
	if !strings.Contains(preview.String(), `"dry_run": true`) || eventCount(t, s, "records.migrated") != 1 {
		t.Fatalf("CLI dry run wrote events or did not return preview: %s", preview.String())
	}
	second, err := ImportRepo(s, repo, fixtureRef, "", ImportOptions{Author: "tester"})
	if err != nil || second.Imported != 0 || second.Revised != 0 {
		t.Fatalf("idempotency: %+v %v", second, err)
	}
	third, err := ImportRepo(s, repo, fixtureRef, "", ImportOptions{ForceRev: true, Author: "tester"})
	if err != nil || third.Imported != 0 || third.Revised != 0 {
		t.Fatalf("unchanged force-rev: %+v %v", third, err)
	}
	output := filepath.Join(t.TempDir(), "export")
	n, err := Export(s, output, ExportOptions{})
	if err != nil || n != first.Imported {
		t.Fatalf("export %d: %v", n, err)
	}
	rows, err := List(s, Filter{})
	if err != nil {
		t.Fatal(err)
	}
	for _, rec := range rows {
		original := gitFixture(t, repo, "show", fixtureRef+":"+rec.Path)
		exported, err := os.ReadFile(filepath.Join(output, filepath.FromSlash(rec.Path)))
		if err != nil {
			t.Fatal(err)
		}
		f1, b1, err := ParseFrontmatter(original)
		if err != nil {
			t.Fatal(err)
		}
		f2, b2, err := ParseFrontmatter(string(exported))
		if err != nil || !reflect.DeepEqual(f1, f2) || b1 != b2 {
			t.Errorf("%s: export round-trip differs (frontmatter equal %v, body equal %v): %v", rec.Path, reflect.DeepEqual(f1, f2), b1 == b2, err)
		}
	}
	if _, err := os.Stat(filepath.Join(output, "README.md")); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("root README was exported")
	}
	if _, err := Export(s, output, ExportOptions{}); err != nil {
		t.Fatalf("re-export existing output: %v", err)
	}
	var guide bytes.Buffer
	c := &cli.Context{Store: s, Out: &guide, Err: &guide, Project: "brain"}
	if err := runGuide(c, nil, nil); err != nil || guide.String() != gitFixture(t, repo, "show", fixtureRef+":AGENTS.md") {
		t.Fatalf("guide did not reproduce AGENTS: %v", err)
	}
}

func TestImportForceRevAndProjectIsolation(t *testing.T) {
	repo := sourceRepo(t)
	s := tempStore(t)
	addProject(t, s, "brain", repo, true)
	addProject(t, s, "app", t.TempDir(), false)
	// Seed a prior imported path through the public API instead of changing git.
	file := "docs/context/environment.md"
	_, err := s.PutRecord(store.Record{ID: "CTX-environment", Project: "brain", Kind: "context", Slug: "environment", Path: file, Origin: "markdown:" + file + "@old"}, map[string]any{}, "Old recipe\n", "tester", "")
	if err != nil {
		t.Fatal(err)
	}
	summary, err := ImportRepo(s, repo, fixtureRef, "brain", ImportOptions{ForceRev: true, Author: "tester"})
	if err != nil || summary.Revised != 1 {
		t.Fatalf("force-rev: %+v %v", summary, err)
	}
	head, rev, err := s.GetRecord("CTX-environment", 0)
	if err != nil || rev.Rev != 2 || !strings.Contains(rev.Body, "# Environment") || head.Origin != "markdown:"+file+"@old" {
		t.Fatalf("revised recipe: %+v %+v %v", head, rev, err)
	}
	_, old, err := s.GetRecord("CTX-environment", 1)
	if err != nil || old.Body != "Old recipe\n" {
		t.Fatal("old revision changed")
	}
	other, err := ImportRepo(s, repo, fixtureRef, "app", ImportOptions{Author: "tester"})
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"EVO-constitution-app", "EVO-archive-app", "CTX-environment-app", "INSTR-agents-app", "PROMPT-eli5-app", "TPL-adr-app"} {
		rec, _, err := s.GetRecord(id, 0)
		if err != nil || rec.Project != "app" {
			t.Errorf("project-scoped id %s: %+v %v", id, rec, err)
		}
	}
	if other.Kinds["adr"].Imported != 0 {
		t.Fatal("numbered IDs must not overwrite another project's ADRs")
	}
	if _, err := ImportRepo(s, repo, fixtureRef, "missing", ImportOptions{}); err == nil {
		t.Fatal("unknown project accepted")
	}
}
