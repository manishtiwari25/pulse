package evolve

import (
	"bytes"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"testing"

	"github.com/manishtiwari25/pulse/internal/bus"
)

func TestArchiveImmutableAtomicEventAndPerProjectIDs(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	testProject(t, s, "other")
	ch, cancel := bus.Default().Subscribe(bus.Filter{Actions: []string{"evolution.archived"}}, 16)
	defer cancel()
	row := ArchiveRow{
		Project: "demo", Hypothesis: "one variable", Branch: "evolve/example",
		Baseline: &Scores{Project: "demo", Total: .2},
		Variant: &Scores{Project: "demo", Total: .4, Fixtures: []FixtureScore{
			{Name: "private-file-must-not-escape", Kind: "heldout", Score: .4, Weight: 1, Note: "private annotation"},
		}},
		Tolerance: .02, Kept: true, RunID: "run-one",
	}
	id, err := Archive(s, row)
	if err != nil || id != "B-001" {
		t.Fatalf("first archive: %s %v", id, err)
	}
	event := <-ch
	if event.Subject != id {
		t.Fatal("archive event did not use the row id")
	}
	var detail map[string]any
	if err := json.Unmarshal([]byte(event.Detail), &detail); err != nil || detail["project"] != "demo" || detail["run_id"] != "run-one" {
		t.Fatal("archive event did not carry the runner contract")
	}
	for _, query := range []string{"UPDATE evolution_archive SET note = 'changed'", "DELETE FROM evolution_archive"} {
		if _, err := s.DB.Exec(query); err == nil {
			t.Fatal("immutable archive accepted a mutation")
		}
	}
	rows, err := ListArchive(s, "demo")
	if err != nil || len(rows) != 1 || rows[0].Fixtures.Variant.Fixtures[0].Name != "H1" {
		t.Fatal("archive did not preserve both measured score sets safely")
	}
	data, _ := json.Marshal(rows)
	if strings.Contains(string(data), "private-file-must-not-escape") || strings.Contains(string(data), "private annotation") {
		t.Fatal("archive exposed private fixture metadata")
	}
	row.Project, row.Baseline.Project, row.Variant.Project = "other", "other", "other"
	if id, err := Archive(s, row); err != nil || id != "B-001" {
		t.Fatal("archive numbering was not scoped per project")
	}
	var schema string
	if err := s.DB.QueryRow("SELECT value FROM meta WHERE key = 'schema_version'").Scan(&schema); err != nil || schema != "3" {
		t.Fatal("evolution changed the core schema version")
	}
}

func TestArchiveConcurrentIDsAndShadowRefusal(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	row := ArchiveRow{Project: "demo", Baseline: &Scores{Total: .2}, Variant: &Scores{Total: .4}, Tolerance: .02, Kept: true}
	errors := make(chan error, 8)
	var workers sync.WaitGroup
	for i := 0; i < 8; i++ {
		workers.Go(func() {
			_, err := Archive(s, row)
			errors <- err
		})
	}
	workers.Wait()
	close(errors)
	for err := range errors {
		if err != nil {
			t.Fatal(err)
		}
	}
	rows, err := ListArchive(s, "demo")
	if err != nil || len(rows) != 8 {
		t.Fatal("concurrent archive rows were lost")
	}
	ids := map[string]bool{}
	for _, row := range rows {
		ids[row.ID] = true
	}
	for i := 1; i <= 8; i++ {
		if !ids[fmt.Sprintf("B-%03d", i)] {
			t.Fatal("concurrent archive numbering collided")
		}
	}
	row.Variant.Shadow = true
	if _, err := Archive(s, row); err == nil || countRows(t, s, "evolution_archive") != 8 {
		t.Fatal("shadow run reached immutable archive")
	}
}

func TestKeepContractUnchanged(t *testing.T) {
	if keep, _ := Keep(&Scores{Total: .5}, &Scores{Total: .6}, .02); !keep {
		t.Fatal("improvement was not kept")
	}
	if keep, _ := Keep(&Scores{Total: .5}, &Scores{Total: .51}, .02); keep {
		t.Fatal("sub-tolerance improvement was kept")
	}
	if keep, _ := Keep(nil, &Scores{Total: 1}, .02); keep {
		t.Fatal("missing baseline was kept")
	}
}

// ADR-031: the daemon's baseline job archives the default branch measured once
// as a kept row with baseline and variant equal. Only that exact shape passes
// the Keep rule's gate; every other kept row must still beat its baseline.
func TestArchiveAcceptsTheBaselineShapeOnly(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	ch, cancel := bus.Default().Subscribe(bus.Filter{Actions: []string{"evolution.archived"}}, 16)
	defer cancel()
	measured := func() *Scores {
		return &Scores{Project: "demo", Total: 1, Sandbox: "fleet/measure-1", Fixtures: []FixtureScore{
			{Name: "starter-one", Kind: "public", Score: 1, Weight: 1},
			{Name: "starter-two", Kind: "public", Score: 1, Weight: 2},
			{Name: "private-name", Kind: "heldout", Score: 1, Weight: 1},
		}}
	}
	row := ArchiveRow{Project: "demo", Hypothesis: BaselineHypothesis, Branch: "main", Baseline: measured(), Variant: measured(),
		Tolerance: .02, Kept: true, RunID: "run-baseline", Note: "baseline measurement"}
	id, err := Archive(s, row)
	if err != nil || id != "B-001" {
		t.Fatalf("baseline row refused: %s %v", id, err)
	}
	event := <-ch
	var detail map[string]any
	if event.Subject != id || json.Unmarshal([]byte(event.Detail), &detail) != nil || detail["kept"] != true || detail["run_id"] != "run-baseline" {
		t.Fatalf("baseline archive event: %+v", event)
	}
	rows, err := ListArchive(s, "demo")
	if err != nil || len(rows) != 1 || !rows[0].Kept || rows[0].Hypothesis != BaselineHypothesis || rows[0].Branch != "main" ||
		rows[0].BaselineScore != 1 || rows[0].VariantScore != 1 || rows[0].Note != "baseline measurement" || rows[0].Fixtures.Variant.Fixtures[2].Name != "H1" {
		t.Fatalf("baseline row as listed: %+v", rows)
	}
	if !IsBaselineRow(rows[0].Hypothesis, rows[0].SuggestionPost) || IsBaselineRow("one variable", "") || IsBaselineRow(BaselineHypothesis, "S-1") {
		t.Fatal("IsBaselineRow did not tell the baseline from an experiment")
	}
	refused := map[string]ArchiveRow{
		"another hypothesis with delta 0": {Project: "demo", Hypothesis: "one variable", Branch: "evolve/x", Baseline: measured(), Variant: measured(), Tolerance: .02, Kept: true},
		"no hypothesis with delta 0":      {Project: "demo", Branch: "evolve/x", Baseline: measured(), Variant: measured(), Tolerance: .02, Kept: true},
		"baseline shape with a suggestion": {Project: "demo", Hypothesis: BaselineHypothesis, SuggestionPost: "S-1", Branch: "main",
			Baseline: measured(), Variant: measured(), Tolerance: .02, Kept: true},
		"baseline hypothesis but a worse variant": {Project: "demo", Hypothesis: BaselineHypothesis, Branch: "main",
			Baseline: measured(), Variant: &Scores{Project: "demo", Total: .9}, Tolerance: .02, Kept: true},
	}
	unequal := measured()
	unequal.Fixtures[0].Score = .5
	refused["equal totals over unequal fixtures"] = ArchiveRow{Project: "demo", Hypothesis: BaselineHypothesis, Branch: "main",
		Baseline: measured(), Variant: unequal, Tolerance: .02, Kept: true}
	for name, row := range refused {
		if _, err := Archive(s, row); err == nil || !strings.Contains(err.Error(), "Keep rule") {
			t.Fatalf("%s: kept row accepted (%v)", name, err)
		}
	}
	if countRows(t, s, "evolution_archive") != 1 {
		t.Fatal("a refused row reached the archive")
	}
	// A discarded baseline-shaped row is still an ordinary archive row.
	row.Kept, row.Note = false, "not kept"
	if id, err := Archive(s, row); err != nil || id != "B-002" {
		t.Fatalf("discarded row: %s %v", id, err)
	}
	rows, err = ListArchive(s, "demo")
	if err != nil {
		t.Fatal(err)
	}
	var table bytes.Buffer
	printArchive(&table, false, rows)
	lines := strings.Split(strings.TrimSpace(table.String()), "\n")
	if len(lines) != 3 || !strings.Contains(lines[1], "B-002") || !strings.Contains(lines[1], "discarded") ||
		!strings.Contains(lines[2], "B-001") || !strings.Contains(lines[2], "baseline") || strings.Contains(lines[2], "kept") {
		t.Fatalf("archive table did not name the baseline row:\n%s", table.String())
	}
}
