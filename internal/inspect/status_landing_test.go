package inspect

import (
	"encoding/json"
	"strings"
	"testing"
)

// A landing hold the daemon wrote (ADR-024: the checkout is dirty, off its branch, locked or unreadable) is
// a status line after recovery, stale when the daemon stopped refreshing it, and an empty list - never
// null - in the JSON when nothing is held.
func TestStatusLandingHoldTextAndJSON(t *testing.T) {
	s := temporaryStore(t)
	execSQL(t, s, `INSERT INTO meta(key,value) VALUES
		('daemon','{"pid":123,"at":"2026-09-06T18:59:00Z","loops":4,"interval":30,"note":""}'),
		('recovery','{"proj":{"at":"2026-09-06T18:59:31Z","note":"1 held"}}'),
		('landing','{"zeta":{"at":"2026-09-06T18:59:31Z","note":"2 branches waiting; deferred: the checkout of /w/zeta has uncommitted changes"},"alpha":{"at":"2026-09-06T18:58:59Z","note":"1 branch waiting; locked: another landing holds /w/alpha/.git/pulse-land.lock"}}')`)

	report, err := Status(s)
	if err != nil {
		t.Fatal(err)
	}
	if len(report.Landing) != 2 || report.Landing[0].Project != "alpha" || !report.Landing[0].Stale || report.Landing[1].Project != "zeta" || report.Landing[1].Stale {
		t.Fatalf("status landing: %+v", report.Landing)
	}
	text := report.Text()
	recovery := "recovery   proj: 1 held\n"
	landing := "landing    zeta: 2 branches waiting; deferred: the checkout of /w/zeta has uncommitted changes\n"
	recoveryAt, landingAt := strings.Index(text, recovery), strings.Index(text, landing)
	if recoveryAt < 0 || landingAt < 0 || landingAt < recoveryAt {
		t.Fatalf("recovery then landing lines expected:\n%s", text)
	}
	if !strings.Contains(text, "landing    alpha: 1 branch waiting; locked: another landing holds /w/alpha/.git/pulse-land.lock (stale)\n") {
		t.Fatalf("stale landing line:\n%s", text)
	}

	execSQL(t, s, `INSERT INTO meta(key,value) VALUES ('landing','not json') ON CONFLICT(key) DO UPDATE SET value=excluded.value`)
	if _, err := LandingNotes(s); err == nil || !strings.Contains(err.Error(), "landing notes") {
		t.Fatalf("malformed landing JSON error: %v", err)
	}

	empty := temporaryStore(t)
	emptyReport, err := Status(empty)
	if err != nil {
		t.Fatal(err)
	}
	if emptyReport.Landing == nil || len(emptyReport.Landing) != 0 || strings.Contains(emptyReport.Text(), "landing    ") {
		t.Fatalf("empty status landing: %#v\n%s", emptyReport.Landing, emptyReport.Text())
	}
	data, err := json.Marshal(emptyReport)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), `"landing":[]`) {
		t.Fatalf("status JSON missing empty landing: %s", data)
	}
}
