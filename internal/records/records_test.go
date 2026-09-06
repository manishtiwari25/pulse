package records

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestNewNumberingTemplatesAndWIP(t *testing.T) {
	s := tempStore(t)
	addProject(t, s, "brain", t.TempDir(), true)
	addProject(t, s, "app", t.TempDir(), false)
	if _, err := s.PutRecord(store.Record{ID: "TPL-adr", Kind: "template", Project: "brain", Slug: "adr"}, nil, "# ADR template\n", "tester", ""); err != nil {
		t.Fatal(err)
	}
	first, err := New(s, "adr", "brain", "One", NewOptions{Areas: []string{"runner"}, Author: "agent"})
	if err != nil || first.ID != "ADR-001" || first.Status != "Proposed" || first.Tier != 2 || first.Delivery != "not started" {
		t.Fatalf("new ADR: %+v %v", first, err)
	}
	_, rev, err := s.GetRecord(first.ID, 0)
	if err != nil || rev.Body != "# ADR template\n" {
		t.Fatalf("template seed: %+v %v", rev, err)
	}
	if _, err := New(s, "adr", "brain", "Blocked", NewOptions{Areas: []string{"RUNNER"}}); err == nil || !strings.Contains(err.Error(), "ADR-001") {
		t.Fatalf("missing WIP conflict: %v", err)
	}
	if _, err := New(s, "adr", "brain", "All areas", NewOptions{}); err == nil {
		t.Fatal("default all area must conflict")
	}
	second, err := New(s, "adr", "brain", "Other area", NewOptions{Areas: []string{"data"}, BodySet: true})
	if err != nil || second.ID != "ADR-002" {
		t.Fatalf("separate area: %+v %v", second, err)
	}
	_, rev, err = s.GetRecord(second.ID, 0)
	if err != nil || rev.Body != "" {
		t.Fatal("explicitly empty body was replaced by a template")
	}
	app, err := New(s, "adr", "app", "App decision", NewOptions{Areas: []string{"runner"}})
	if err != nil || app.ID != "ADR-003" || app.Project != "app" {
		t.Fatalf("global ID collision handling: %+v %v", app, err)
	}
	_, rev, err = s.GetRecord(app.ID, 0)
	if err != nil || rev.Body != "# ADR template\n" {
		t.Fatal("app did not fall back to framework template")
	}
	if _, err := s.PutRecord(store.Record{ID: "TPL-adr-app", Kind: "template", Project: "app", Slug: "adr"}, nil, "# App template\n", "tester", ""); err != nil {
		t.Fatal(err)
	}
	appOther, err := New(s, "adr", "app", "Another app decision", NewOptions{Areas: []string{"other"}})
	if err != nil {
		t.Fatal(err)
	}
	_, rev, err = s.GetRecord(appOther.ID, 0)
	if err != nil || rev.Body != "# App template\n" {
		t.Fatal("app template did not override framework template")
	}
	if err := Accept(s, first.ID, "The owner agrees"); err != nil {
		t.Fatal(err)
	}
	if _, err := New(s, "adr", "brain", "Now allowed", NewOptions{Areas: []string{"runner"}}); err != nil {
		t.Fatal(err)
	}
	memory, err := New(s, "memory", "app", "Useful Pattern", NewOptions{Author: "agent"})
	if err != nil || memory.ID != "MEM-useful-pattern-app" {
		t.Fatalf("nonnumbered new: %+v %v", memory, err)
	}
	if _, err := New(s, "memory", "app", "Useful Pattern", NewOptions{}); err == nil {
		t.Fatal("new silently revised a same-title memory")
	}
	wu, err := New(s, "wu", "brain", "Small unit", NewOptions{})
	if err != nil || wu.ID != "WU-001" || wu.Tier != 1 {
		t.Fatalf("work unit defaults: %+v %v", wu, err)
	}
	for _, opts := range []NewOptions{{Status: "Accepted"}, {Status: " aCcEpTeD "}, {Tier: 3}} {
		if _, err := New(s, "adr", "brain", "Invalid", opts); err == nil {
			t.Errorf("accepted invalid creation %+v", opts)
		}
	}
}

func TestStatusAcceptanceAmendmentAndDelivery(t *testing.T) {
	s := tempStore(t)
	addProject(t, s, "brain", t.TempDir(), true)
	rec, err := New(s, "adr", "brain", "Decision", NewOptions{Author: "agent", Body: "Original body\n"})
	if err != nil {
		t.Fatal(err)
	}
	for _, actor := range []string{"agent", store.HumanAgent} {
		if err := Status(s, rec.ID, "Accepted", actor); err == nil || !strings.Contains(err.Error(), "pulse record accept") {
			t.Fatalf("status bypassed explicit acceptance: %v", err)
		}
	}
	if err := Status(s, rec.ID, "Draft", "agent"); err != nil {
		t.Fatal(err)
	}
	if err := Accept(s, rec.ID, ""); err == nil {
		t.Fatal("accept without human words")
	}
	if err := Accept(s, rec.ID, "Accept the tradeoff"); err != nil {
		t.Fatal(err)
	}
	head, rev, err := s.GetRecord(rec.ID, 0)
	if err != nil || rev.Author != store.HumanAgent || rev.Note != "Accept the tradeoff" || head.Status != "Accepted" || rev.Frontmatter["status"] != "Accepted" || rev.Rev != 3 {
		t.Fatalf("acceptance: %+v %+v %v", head, rev, err)
	}
	if err := Deliver(s, rec.ID, "delivered", "2026-09-09", "agent"); err != nil {
		t.Fatal(err)
	}
	amended, err := Amend(s, rec.ID, "Revised body\n", "agent", "Clarify rationale")
	if err != nil || amended.CurrentRev != 5 {
		t.Fatalf("amend: %+v %v", amended, err)
	}
	head, rev, err = s.GetRecord(rec.ID, 0)
	if err != nil || rev.Body != "Revised body\n" || head.Delivery != "delivered" || head.Delivered != "2026-09-09" || rev.Frontmatter["delivered"] != "2026-09-09" {
		t.Fatalf("amend/delivery: %+v %+v %v", head, rev, err)
	}
	_, old, err := s.GetRecord(rec.ID, 1)
	if err != nil || old.Body != "Original body\n" || old.Frontmatter["status"] != "Proposed" || old.Frontmatter["delivery"] != "not started" {
		t.Fatal("old revision was changed")
	}
	rendered := RenderMarkdown(head, old)
	if !strings.Contains(rendered, `"Proposed"`) || strings.Contains(rendered, `"Accepted"`) || !strings.HasSuffix(rendered, "Original body\n") {
		t.Fatal("old render mixed current head metadata into the old revision")
	}
	if eventCount(t, s, "record.created") != 1 || eventCount(t, s, "record.revised") != 4 {
		t.Fatal("each write must have exactly one record event")
	}
	for _, delivery := range []string{"unknown", "Delivered"} {
		if err := Deliver(s, rec.ID, delivery, "", "agent"); err == nil {
			t.Fatal("accepted invalid delivery", delivery)
		}
	}
	if err := Deliver(s, rec.ID, "delivered", "2026-02-30", "agent"); err == nil {
		t.Fatal("accepted invalid delivery date")
	}
	if err := Deliver(s, rec.ID, "not started", "", "agent"); err != nil {
		t.Fatal(err)
	}
	head, rev, err = s.GetRecord(rec.ID, 0)
	if err != nil || head.Delivered != "" || rev.Frontmatter["delivered"] != "" {
		t.Fatal("not started kept a stale delivery date")
	}
}

func TestConcurrentNewDoesNotReviseExistingIDs(t *testing.T) {
	s := tempStore(t)
	addProject(t, s, "brain", t.TempDir(), true)
	const n = 12
	var wg sync.WaitGroup
	errs := make(chan error, n)
	for i := 0; i < n; i++ {
		wg.Go(func() {
			rec, err := New(s, "wu", "brain", fmt.Sprintf("Unit %d", i), NewOptions{Author: "agent"})
			if err == nil && rec.CurrentRev != 1 {
				err = fmt.Errorf("%s got revision %d", rec.ID, rec.CurrentRev)
			}
			errs <- err
		})
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	rows, err := List(s, Filter{Kind: "wu"})
	if err != nil || len(rows) != n {
		t.Fatalf("concurrent creations = %d %v", len(rows), err)
	}
}

func TestConcurrentProcessesReserveNewIDs(t *testing.T) {
	s := tempStore(t)
	addProject(t, s, "brain", t.TempDir(), true)
	const n = 4
	cmds := make([]*exec.Cmd, n)
	outputs := make([]bytes.Buffer, n)
	for i := range cmds {
		cmd := exec.Command(os.Args[0], "-test.run=^TestRecordWriterProcess$")
		cmd.Env = append(os.Environ(), "PULSE_RECORD_TEST_HOME="+s.Home)
		cmd.Stdout, cmd.Stderr = &outputs[i], &outputs[i]
		if err := cmd.Start(); err != nil {
			t.Fatal(err)
		}
		cmds[i] = cmd
	}
	for i, cmd := range cmds {
		if err := cmd.Wait(); err != nil {
			t.Fatalf("writer %d: %v\n%s", i, err, &outputs[i])
		}
	}
	rows, err := List(s, Filter{Kind: "wu"})
	if err != nil || len(rows) != n {
		t.Fatalf("independent CLI writers created %d records, want %d: %v", len(rows), n, err)
	}
	for _, rec := range rows {
		if rec.CurrentRev != 1 {
			t.Fatalf("new overwrote %s as revision %d", rec.ID, rec.CurrentRev)
		}
	}
}

func TestRecordWriterProcess(t *testing.T) {
	home := os.Getenv("PULSE_RECORD_TEST_HOME")
	if home == "" {
		return
	}
	s, err := store.Open(store.Options{Home: home})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if _, err := New(s, "wu", "brain", "Concurrent unit", NewOptions{Author: "tester"}); err != nil {
		t.Fatal(err)
	}
}

func TestExportCheckoutAndPathGuards(t *testing.T) {
	s := tempStore(t)
	projectDir := t.TempDir()
	addProject(t, s, "brain", projectDir, true)
	if err := os.WriteFile(filepath.Join(projectDir, ".git"), []byte("gitdir: /elsewhere\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	rec, err := New(s, "memory", "brain", "Pattern", NewOptions{Body: "Keep me\n"})
	if err != nil {
		t.Fatal(err)
	}
	inside := filepath.Join(projectDir, "docs")
	if _, err := Export(s, inside, ExportOptions{}); err == nil {
		t.Fatal("exported into checkout without --force")
	}
	if _, err := os.Stat(inside); !os.IsNotExist(err) {
		t.Fatal("refused export created output directory")
	}
	if n, err := Export(s, inside, ExportOptions{Force: true}); err != nil || n != 1 {
		t.Fatalf("explicit force: %d %v", n, err)
	}
	link := filepath.Join(t.TempDir(), "checkout-link")
	if err := os.Symlink(projectDir, link); err != nil {
		t.Fatal(err)
	}
	if _, err := Export(s, filepath.Join(link, "export"), ExportOptions{}); err == nil {
		t.Fatal("symlink bypassed checkout guard")
	}
	output := t.TempDir()
	outside := t.TempDir()
	if err := os.Symlink(outside, filepath.Join(output, "memory")); err != nil {
		t.Fatal(err)
	}
	if _, err := Export(s, output, ExportOptions{Force: true}); err == nil {
		t.Fatal("export escaped output root via symlink")
	}
	if entries, err := os.ReadDir(outside); err != nil || len(entries) != 0 {
		t.Fatal("export wrote outside selected directory")
	}
	rec.Path = "../escape.md"
	if _, err := s.PutRecord(*rec, nil, "No\n", "tester", ""); err != nil {
		t.Fatal(err)
	}
	if _, err := Export(s, t.TempDir(), ExportOptions{Force: true}); err == nil {
		t.Fatal("export accepted parent traversal")
	}
}

func TestLeadTimeLegacyStates(t *testing.T) {
	s := tempStore(t)
	tests := []struct {
		name, status, delivery, date, delivered, state, target string
		tier                                                   int
		days                                                   *int
	}{
		{"met", "Done", "delivered", "2026-09-01", "2026-09-02", "delivered", "met", 1, intPtr(1)},
		{"missed", "Done", "delivered", "2026-09-01", "2026-09-04", "delivered", "missed", 1, intPtr(3)},
		{"overdue", "Draft", "", "2026-09-01", "", "open", "overdue", 1, intPtr(5)},
		{"retired", "Superseded (ADR-001)", "", "2026-09-01", "", "retired", "retired", 2, nil},
		{"undated", "Done", "", "2026-09-01", "", "undated", "add delivered:", 2, nil},
		{"no-date", "Draft", "", "", "", "no-date", "add date:", 2, nil},
		{"partial", "Accepted", "partially delivered", "2026-09-01", "2026-09-03", "delivered", "-", 2, intPtr(2)},
	}
	for i, tt := range tests {
		rec := store.Record{ID: fmt.Sprintf("WU-%03d", i+1), Kind: "wu", Project: "brain", Number: i + 1, Slug: tt.name, Status: tt.status, Delivery: tt.delivery, Date: tt.date, Delivered: tt.delivered, Tier: tt.tier}
		if _, err := s.PutRecord(rec, map[string]any{"tier": tt.tier}, "", "agent", ""); err != nil {
			t.Fatal(err)
		}
	}
	rows, err := LeadTime(s, "brain")
	if err != nil || len(rows) != len(tests) {
		t.Fatalf("lead-time: %+v %v", rows, err)
	}
	for i, got := range rows {
		want := tests[i]
		if got.State != want.state || got.Target != want.target || (got.Days == nil) != (want.days == nil) || got.Days != nil && *got.Days != *want.days {
			t.Errorf("%s: got %+v", want.name, got)
		}
	}
}

func intPtr(n int) *int { return &n }

func TestCLICommandsAndGuide(t *testing.T) {
	s := tempStore(t)
	addProject(t, s, "brain", t.TempDir(), true)
	var out, stderr bytes.Buffer
	c := &cli.Context{Store: s, Home: s.Home, Out: &out, Err: &stderr, Agent: "agent", Project: "brain"}
	if err := runGuide(c, nil, nil); err != nil || strings.Count(out.String(), "\n") != 1 || !strings.Contains(out.String(), "pulse migrate --records") {
		t.Fatalf("empty guide: %q %v", out.String(), err)
	}
	for _, slug := range []string{"agents", "claude", "copilot-instructions"} {
		if _, err := s.PutRecord(store.Record{ID: "INSTR-" + slug, Project: "brain", Kind: "instruction", Slug: slug}, nil, "# "+slug+"\n", "tester", ""); err != nil {
			t.Fatal(err)
		}
	}
	out.Reset()
	if err := runGuide(c, nil, []string{"--full", "--project", "brain"}); err != nil || out.String() != "# agents\n\n\n# claude\n\n\n# copilot-instructions\n" {
		t.Fatalf("full guide order: %q %v", out.String(), err)
	}
	bodyPath := filepath.Join(t.TempDir(), "body")
	if err := os.WriteFile(bodyPath, []byte("# New body\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	calls := [][]string{
		{"new", "adr", "--title", "A title with spaces", "--areas", "cli,data", "--tags", "one, two", "--body-file", bodyPath},
		{"list", "ADR", "--status", "Proposed", "--area", "cli", "--json"},
		{"show", "ADR-001", "--rev", "1", "--json"},
		{"status", "ADR-001", "Draft", "--json"},
		{"accept", "ADR-001", "--words", "The owner approves"},
		{"deliver", "ADR-001", "partially delivered", "--date", "2026-09-06"},
		{"amend", "ADR-001", "--body-file", bodyPath, "--note", "Clarified"},
		{"revisions", "ADR-001", "--json"},
		{"lead-time", "--project", "brain", "--json"},
	}
	for _, args := range calls {
		out.Reset()
		if err := runRecord(c, nil, args); err != nil {
			t.Fatalf("record %v: %v", args, err)
		}
		if args[len(args)-1] == "--json" && !json.Valid(out.Bytes()) {
			t.Fatalf("invalid JSON for %v: %s", args, out.String())
		}
	}
	if err := runRecord(c, nil, []string{"new", "adr", "--title", "Wrong", "--status", "Accepted"}); err == nil {
		t.Fatal("CLI bypassed ADR acceptance")
	}
	for _, args := range [][]string{
		{"show", "MISSING"}, {"show", "ADR-001", "--rev", "-1"},
		{"new", "wu"}, {"list", "no-kind"}, {"list", "--bogus"},
		{"amend", "ADR-001"}, {"new", "wu", "--title"}, {"status", "ADR-001"},
	} {
		if err := runRecord(c, nil, args); err == nil {
			t.Errorf("accepted invalid CLI arguments %v", args)
		}
	}
	stdin, write, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	oldStdin := os.Stdin
	os.Stdin = stdin
	t.Cleanup(func() { os.Stdin = oldStdin; stdin.Close() })
	if _, err := io.WriteString(write, "From stdin\n"); err != nil {
		t.Fatal(err)
	}
	write.Close()
	if err := runRecord(c, nil, []string{"amend", "ADR-001", "--body-file", "stdin"}); err != nil {
		t.Fatal(err)
	}
	_, rev, err := s.GetRecord("ADR-001", 0)
	if err != nil || rev.Body != "From stdin\n" {
		t.Fatal("stdin body not stored")
	}
}
