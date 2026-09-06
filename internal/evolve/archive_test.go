package evolve

import (
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
