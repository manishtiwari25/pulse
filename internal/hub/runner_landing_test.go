package hub

import (
	"net/http"
	"testing"
)

// GET /api/v1/runner carries the daemon's landing holds (ADR-024) so the console can say why verified
// branches are not landing: an empty list when nothing is held, one entry per held project otherwise.
func TestRunnerViewCarriesLandingHolds(t *testing.T) {
	s := additionsStore(t, true)
	h := New(s, nil)

	code, body := additionsRequest(t, h, http.MethodGet, "/api/v1/runner", "")
	if code != http.StatusOK || body["available"] != true {
		t.Fatalf("runner: %d %v", code, body)
	}
	if held, ok := body["landing"].([]any); !ok || len(held) != 0 {
		t.Fatalf("landing when nothing is held: %#v", body["landing"])
	}

	if _, err := s.DB.Exec(`INSERT INTO meta(key,value) VALUES
		('daemon','{"pid":123,"at":"2026-09-06T11:59:30Z","loops":4,"interval":30,"note":""}'),
		('landing','{"proj":{"at":"2026-09-06T11:59:31Z","note":"3 branches waiting; deferred: the checkout of /w/proj has uncommitted changes"}}')`); err != nil {
		t.Fatal(err)
	}
	code, body = additionsRequest(t, h, http.MethodGet, "/api/v1/runner", "")
	if code != http.StatusOK {
		t.Fatalf("runner: %d %v", code, body)
	}
	held, _ := body["landing"].([]any)
	if len(held) != 1 {
		t.Fatalf("landing: %#v", body["landing"])
	}
	note, _ := held[0].(map[string]any)
	if note["project"] != "proj" || note["stale"] != false || note["note"] != "3 branches waiting; deferred: the checkout of /w/proj has uncommitted changes" {
		t.Fatalf("landing hold: %#v", note)
	}
}

func TestRunnerViewCarriesCapacityHold(t *testing.T) {
	s := additionsStore(t, true)
	h := New(s, nil)
	const hold = "workers on this machine wait for a GitHub Copilot login: Sign in with GitHub (Settings -> Harness) or run pulse harness login"
	if _, err := s.DB.Exec(`INSERT INTO meta(key,value) VALUES
		('capacity','{"queued":2,"slots":0,"inflight":0,"starved_since":"","held":"` + hold + `"}')`); err != nil {
		t.Fatal(err)
	}
	code, body := additionsRequest(t, h, http.MethodGet, "/api/v1/runner", "")
	if code != http.StatusOK {
		t.Fatalf("runner: %d %v", code, body)
	}
	wantCapacity := "2 job(s) queued, 0 slot(s), 0 in flight; " + hold
	if body["capacity"] != wantCapacity || body["hold"] != hold {
		t.Fatalf("capacity hold: capacity=%#v hold=%#v", body["capacity"], body["hold"])
	}
}
