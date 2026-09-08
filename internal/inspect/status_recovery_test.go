package inspect

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestRecoveryNotesMissingKeyIsEmptySlice(t *testing.T) {
	s := temporaryStore(t)

	notes, err := RecoveryNotes(s)
	if err != nil {
		t.Fatal(err)
	}
	if notes == nil || len(notes) != 0 {
		t.Fatalf("missing recovery key: %#v", notes)
	}
}

func TestRecoveryNotesSortedStaleAndMalformed(t *testing.T) {
	s := temporaryStore(t)
	execSQL(t, s, `INSERT INTO meta(key,value) VALUES
		('daemon','{"pid":123,"at":"2026-09-06T18:59:00Z","loops":4,"interval":30,"note":""}'),
		('recovery','{"zeta":{"at":"2026-09-06T18:59:31Z","note":"next retry at 2026-09-06T19:05:00Z"},"alpha":{"at":"2026-09-06T18:58:59Z","note":"held (structural: missing npm)"}}')`)

	notes, err := RecoveryNotes(s)
	if err != nil {
		t.Fatal(err)
	}
	if len(notes) != 2 {
		t.Fatalf("recovery notes: %+v", notes)
	}
	if notes[0].Project != "alpha" || notes[0].Note != "held (structural: missing npm)" || !notes[0].Stale {
		t.Fatalf("first recovery note: %+v", notes[0])
	}
	if notes[1].Project != "zeta" || notes[1].Stale {
		t.Fatalf("second recovery note: %+v", notes[1])
	}

	execSQL(t, s, `INSERT INTO meta(key,value) VALUES ('recovery','not json')
		ON CONFLICT(key) DO UPDATE SET value=excluded.value`)
	if _, err := RecoveryNotes(s); err == nil || !strings.Contains(err.Error(), "recovery notes") {
		t.Fatalf("malformed recovery JSON error: %v", err)
	}
}

func TestStatusRecoveryTextAndJSON(t *testing.T) {
	s := temporaryStore(t)
	execSQL(t, s, `INSERT INTO meta(key,value) VALUES
		('daemon','{"pid":123,"at":"2026-09-06T18:59:00Z","loops":4,"interval":30,"note":""}'),
		('planner','{"proj":{"at":"2026-09-06T18:59:31Z","note":"planner note"}}'),
		('recovery','{"proj":{"at":"2026-09-06T18:59:31Z","note":"3 failed in 7d: 1 run again, 1 re-planned (job-r), 1 held"}}')`)

	report, err := Status(s)
	if err != nil {
		t.Fatal(err)
	}
	if len(report.Recovery) != 1 || report.Recovery[0].Project != "proj" || report.Recovery[0].Note == "" {
		t.Fatalf("status recovery: %+v", report.Recovery)
	}
	text := report.Text()
	planner := "planner    proj: planner note\n"
	recovery := "recovery   proj: 3 failed in 7d: 1 run again, 1 re-planned (job-r), 1 held\n"
	plannerAt, recoveryAt := strings.Index(text, planner), strings.Index(text, recovery)
	if plannerAt < 0 || recoveryAt < 0 {
		t.Fatalf("planner/recovery lines missing:\n%s", text)
	}
	if recoveryAt < plannerAt {
		t.Fatalf("recovery line should follow planner line:\n%s", text)
	}

	empty := temporaryStore(t)
	emptyReport, err := Status(empty)
	if err != nil {
		t.Fatal(err)
	}
	if emptyReport.Recovery == nil || len(emptyReport.Recovery) != 0 {
		t.Fatalf("empty status recovery: %#v", emptyReport.Recovery)
	}
	data, err := json.Marshal(emptyReport)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), `"recovery":[]`) {
		t.Fatalf("status JSON missing empty recovery: %s", data)
	}
}
