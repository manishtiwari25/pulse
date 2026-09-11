package records

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// The embedded bundle is the framework a fresh copy of PULSE carries (ADR-038):
// every framework kind, the records the loop and the guide need, nothing
// personal, in a stable order.
func TestFrameworkBundleCarriesTheFramework(t *testing.T) {
	b, err := FrameworkBundle()
	if err != nil {
		t.Fatal(err)
	}
	if b.Version != BundleVersion || len(b.Records) < 60 {
		t.Fatalf("bundle version %d with %d records", b.Version, len(b.Records))
	}
	kinds := map[string]int{}
	ids := map[string]BundleRecord{}
	for i, r := range b.Records {
		kinds[r.Kind]++
		if _, dup := ids[r.ID]; dup {
			t.Fatalf("%s occurs twice", r.ID)
		}
		ids[r.ID] = r
		if !frameworkKind(r.Kind) || r.Slug == "" || strings.TrimSpace(r.Body) == "" {
			t.Fatalf("record %d is not a framework record: %+v", i, r.ID)
		}
		for _, key := range []string{"seeded_from", "seeded_by"} {
			if _, ok := r.Frontmatter[key]; ok {
				t.Fatalf("%s carries %s; the bundle is the source, not a copy", r.ID, key)
			}
		}
		if shape := secretShaped(r.Body); shape != "" {
			t.Fatalf("%s carries a %s", r.ID, shape)
		}
		if retiredStatus(textValue(r.Frontmatter["status"])) {
			t.Fatalf("%s is %s and still bundled", r.ID, r.Frontmatter["status"])
		}
		if strings.Contains(r.Body, "/Users/") || strings.Contains(r.Body, "/home/") {
			t.Fatalf("%s names a home directory", r.ID)
		}
	}
	for _, kind := range FrameworkKinds {
		if kinds[kind] == 0 {
			t.Errorf("no %s record in the bundle", kind)
		}
	}
	for _, id := range []string{"INSTR-agents", "GUIDE-new-repo", "TPL-evolution-constitution", "TPL-evolution-evaluators", "TPL-adr", "TPL-plan", "WF-pulse", "ARCH-overview"} {
		if _, ok := ids[id]; !ok {
			t.Errorf("bundle lacks %s", id)
		}
	}
	sorted := *b
	sorted.Records = append([]BundleRecord(nil), b.Records...)
	sortBundle(&sorted)
	for i := range b.Records {
		if b.Records[i].ID != sorted.Records[i].ID {
			t.Fatalf("bundle is not in export order at %d: %s vs %s", i, b.Records[i].ID, sorted.Records[i].ID)
		}
	}
	// The file on disk is the bundle re-encoded: an export that changes nothing rewrites nothing.
	encoded, err := encodeBundle(b)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(encoded, bundleJSON) {
		t.Fatal("seed/bundle.json is not in encodeBundle's form; run pulse seed export")
	}
}

func TestEnsureFrameworkSeedsAFreshStoreOnceAndKeepsWhatIsThere(t *testing.T) {
	s := tempStore(t)
	b, err := FrameworkBundle()
	if err != nil {
		t.Fatal(err)
	}
	// A record present by id, whatever its text, is the owner's; seeding leaves it alone.
	mine := store.Record{ID: "TPL-adr", Project: "app", Kind: "template", Slug: "adr", Title: "My ADR template", Origin: "store"}
	if _, err := s.PutRecord(mine, map[string]any{"id": "TPL-adr", "title": "My ADR template"}, "# Mine\n", "alice", ""); err != nil {
		t.Fatal(err)
	}
	result, err := EnsureFramework(s, "alice")
	if err != nil {
		t.Fatal(err)
	}
	if len(result.Written) != len(b.Records)-1 || result.Kept != 1 || !strings.HasPrefix(result.From, "pulse@") {
		t.Fatalf("first seeding: written %d kept %d from %q", len(result.Written), result.Kept, result.From)
	}
	rec, rev, err := s.GetRecord("INSTR-agents", 0)
	if err != nil {
		t.Fatal(err)
	}
	if rec.Project != store.CommonsSlug || rec.Kind != "instruction" || rec.Slug != "agents" || rec.Origin != "seed:"+result.From ||
		rec.Title == "" || rec.CreatedBy != "alice" || rev.Rev != 1 {
		t.Fatalf("seeded head row: %+v", rec)
	}
	// AGENTS.md has no frontmatter, and pulse guide prints the body bare: the seeding adds none.
	if len(rev.Frontmatter) != 0 || rev.Note != "seeded from the framework bundle (ADR-038)" || !strings.Contains(rev.Body, "PULSE") {
		t.Fatalf("seeded revision: %+v", rev)
	}
	if _, kept, err := s.GetRecord("TPL-adr", 0); err != nil || kept.Body != "# Mine\n" || kept.Rev != 1 {
		t.Fatalf("the owner's TPL-adr was touched: %+v %v", kept, err)
	}
	if eventCount(t, s, "records.framework.seeded") != 1 {
		t.Fatal("seeding did not record one summary event")
	}
	// Everything present: nothing written, no lock taken, no event.
	again, err := EnsureFramework(s, "")
	if err != nil {
		t.Fatal(err)
	}
	if len(again.Written) != 0 || again.Kept != len(b.Records) {
		t.Fatalf("second seeding: %+v", again)
	}
	if eventCount(t, s, "records.framework.seeded") != 1 || revisionCount(t, s, "INSTR-agents") != 1 {
		t.Fatal("a store that has everything was written to")
	}
	status, err := FrameworkStatus(s)
	if err != nil {
		t.Fatal(err)
	}
	if status.Missing == nil || len(status.Missing) != 0 || status.Present != len(b.Records) || status.Records != len(b.Records) {
		t.Fatalf("status after seeding: %+v", status)
	}
}

// The loop's seeding (ADR-030) and the guide find the framework's records
// wherever the store came from: a project that is not the brain, in a store
// that has never seen a PULSE checkout, seeds its constitution from the
// bundled template.
func TestFrameworkRecordsServeTheLoopAndTheGuideWithoutABrain(t *testing.T) {
	s := tempStore(t)
	namedProject(t, s, "lamplitlabs", "Lamplit Labs", t.TempDir())
	if _, err := SeedLoopRecords(s, "lamplitlabs", "alice"); err == nil {
		t.Fatal("seeding the loop before the framework records exist should fail")
	}
	if _, err := EnsureFramework(s, "alice"); err != nil {
		t.Fatal(err)
	}
	summary, err := SeedLoopRecords(s, "lamplitlabs", "alice")
	if err != nil {
		t.Fatal(err)
	}
	if summary.Constitution.Action != "copied" || summary.Constitution.Source != "TPL-evolution-constitution@r1" ||
		summary.Evaluators.Action != "copied" || summary.Evaluators.Source != "TPL-evolution-evaluators@r1" {
		t.Fatalf("loop seeding: %+v", summary)
	}
	_, rev, err := s.GetRecord("EVO-constitution-lamplitlabs", 0)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(rev.Body, "<Project>") || !strings.Contains(rev.Body, "Lamplit Labs") || rev.Frontmatter["seeded_from"] != store.CommonsSlug+"/TPL-evolution-constitution@r1" {
		t.Fatalf("constitution copied from the bundled template: %+v", rev)
	}
	var guide bytes.Buffer
	c := &cli.Context{Store: s, Out: &guide, Err: &guide, Project: "lamplitlabs"}
	if err := runGuide(c, nil, nil); err != nil {
		t.Fatalf("guide without a brain: %v\n%s", err, guide.String())
	}
	if !strings.HasPrefix(guide.String(), "# PULSE") || strings.Contains(guide.String(), "No instruction records") {
		t.Fatalf("guide did not print the framework's instruction:\n%s", guide.String())
	}
	var status bytes.Buffer
	c = &cli.Context{Store: s, Out: &status, Err: &status, Project: "lamplitlabs", JSON: true}
	if err := runSeed(c, nil, []string{"status", "--json"}); err != nil {
		t.Fatalf("seed status: %v\n%s", err, status.String())
	}
	var decoded map[string]any
	if err := json.Unmarshal(status.Bytes(), &decoded); err != nil {
		t.Fatalf("seed status --json: %v\n%s", err, status.String())
	}
	if decoded["missing"] == nil || len(decoded["missing"].([]any)) != 0 || decoded["brain"] != nil {
		t.Fatalf("seed status: %v", decoded)
	}
}

func TestExportBundleWritesTheFrameworkRecordsAndRefusesSecrets(t *testing.T) {
	s := tempStore(t)
	brainDir := t.TempDir()
	addProject(t, s, "brain", brainDir, true)
	addProject(t, s, "app", t.TempDir(), false)
	put := func(id, project, kind, slug, status, body string) {
		t.Helper()
		front := map[string]any{"id": id, "title": strings.ToUpper(slug), "status": status}
		rec := store.Record{ID: id, Project: project, Kind: kind, Slug: slug, Title: strings.ToUpper(slug), Status: status, Origin: "store"}
		if _, err := s.PutRecord(rec, front, body, "tester", ""); err != nil {
			t.Fatal(err)
		}
	}
	put("INSTR-agents", "brain", "instruction", "agents", "", "# Agents\n")
	put("TPL-adr", "brain", "template", "adr", "Proposed", "# ADR\n")
	put("PROMPT-old", "brain", "prompt", "old", "Deprecated", "# Old\n")
	put("ADR-001", "brain", "adr", "001-x", "Accepted", "# Not framework\n")
	put("WF-app", "app", "workflow", "app", "", "# Another project's\n")
	// Seeded into the commons before this brain registered: still the framework's.
	put("WF-shared", store.CommonsSlug, "workflow", "shared", "", "# Seeded first\n")
	b, skipped, err := BuildBundle(s)
	if err != nil {
		t.Fatal(err)
	}
	if len(b.Records) != 3 || b.Records[0].ID != "INSTR-agents" || b.Records[1].ID != "WF-shared" || b.Records[2].ID != "TPL-adr" || len(skipped) != 1 || !strings.HasPrefix(skipped[0], "PROMPT-old") {
		t.Fatalf("bundle %+v skipped %v", b.Records, skipped)
	}
	path, err := BundlePath(s, "")
	if err != nil || path != filepath.Join(brainDir, bundleFile) {
		t.Fatalf("BundlePath: %s %v", path, err)
	}
	summary, err := ExportBundle(s, path)
	if err != nil {
		t.Fatal(err)
	}
	if summary.Records != 3 || len(summary.Added) != 3 || summary.Same() {
		t.Fatalf("first export: %+v", summary)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := decodeBundle(data)
	if err != nil || len(decoded.Records) != 3 || decoded.Records[2].Frontmatter["status"] != "Proposed" {
		t.Fatalf("written bundle: %+v %v", decoded, err)
	}
	again, err := ExportBundle(s, path)
	if err != nil || !again.Same() || again.Unchanged != 3 {
		t.Fatalf("second export: %+v %v", again, err)
	}
	put("TPL-adr", "brain", "template", "adr", "Proposed", "# ADR\n\nToken: ghp_abcdefghijklmnopqrstuvwxyz0123456789\n")
	if _, _, err := BuildBundle(s); err == nil || !strings.Contains(err.Error(), "secret-shaped") {
		t.Fatalf("a secret-shaped body was bundled: %v", err)
	}
	if shape := secretShaped("AKIAABCDEFGHIJKLMNOP"); shape == "" {
		t.Fatal("an AWS key id is not secret-shaped")
	}
	if secretShaped("classic `ghp_` is refused") != "" || secretShaped("sk-s in prose") != "" {
		t.Fatal("prose that names a token prefix is not a secret")
	}
	for status, want := range map[string]bool{"Deprecated": true, "retired": true, "Superseded": true, "Proposed": false, "Accepted": false, "": false} {
		if retiredStatus(status) != want {
			t.Errorf("retiredStatus(%q) = %v", status, !want)
		}
	}
}
