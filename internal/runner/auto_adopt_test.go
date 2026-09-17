package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestAutoAdoptDraftsOnlyWhenAwayAndEnabled(t *testing.T) {
	s := autoAdoptStore(t)
	rec := seedRecord(t, s)
	if _, err := records.Draft(s, rec.ID, records.DraftInput{Body: "auto body", Author: "aurora", Note: "test", JobID: "job-1"}); err != nil {
		t.Fatal(err)
	}

	// Owner present: no adoption.
	if lines := autoAdoptDrafts(s, s.Now(), "daemon"); len(lines) != 0 {
		t.Fatalf("present owner should not auto-adopt: %v", lines)
	}

	// Owner away but auto-adopt off: no adoption.
	if err := s.SetSetting(store.HumanAgent, "owner.presence", store.OwnerAway); err != nil {
		t.Fatal(err)
	}
	if lines := autoAdoptDrafts(s, s.Now(), "daemon"); len(lines) != 0 {
		t.Fatalf("away owner with auto_adopt=0 should not adopt: %v", lines)
	}

	// Owner away and auto-adopt on: draft adopted.
	if err := s.SetSetting(store.HumanAgent, "records.auto_adopt", "1"); err != nil {
		t.Fatal(err)
	}
	lines := autoAdoptDrafts(s, s.Now(), "daemon")
	if len(lines) != 1 || !strings.Contains(lines[0], "auto-adopted") {
		t.Fatalf("expected one auto-adopted line, got %v", lines)
	}
	if d, err := s.GetDraft(rec.ID); err != nil || d != nil {
		t.Fatalf("draft should be cleared after auto-adoption: %v %v", d, err)
	}
}

func autoAdoptStore(t *testing.T) *store.Store {
	t.Helper()
	home := t.TempDir()
	// store.Open refuses to create a store while PULSE_JOB is set (WorkerGuard), and every
	// PULSE worker runs the suite with it set; clear it like newStore does.
	t.Setenv("PULSE_JOB", "")
	s, err := store.Open(store.Options{Home: home, Create: true})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func seedRecord(t *testing.T, s *store.Store) *store.Record {
	t.Helper()
	if _, err := s.DB.Exec("INSERT INTO projects (slug, name, path, added_at, added_by) VALUES (?, ?, ?, ?, ?)", "proj", "Proj", t.TempDir(), s.Now(), "human"); err != nil {
		t.Fatal(err)
	}
	rec, err := records.New(s, "memory", "proj", "A lesson", records.NewOptions{Body: "# A lesson\n", Author: "aurora"})
	if err != nil {
		t.Fatal(err)
	}
	return rec
}
