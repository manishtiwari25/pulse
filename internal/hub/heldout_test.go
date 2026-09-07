package hub

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// heldoutRequest is foldersRequest with a JSON body and, when confirmed, the
// intent header a held-out write needs; host is the Host a browser would send.
func heldoutRequest(t *testing.T, h http.Handler, method, path, body, host string, confirmed bool) (int, map[string]any, string) {
	t.Helper()
	req := httptest.NewRequest(method, path, strings.NewReader(body))
	req.Host = host
	if method == http.MethodPost {
		req.Header.Set("Content-Type", "application/json")
	}
	if confirmed {
		req.Header.Set(heldoutHeader, "yes")
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	var result map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &result); err != nil {
		t.Fatalf("%s %s: status %d, non-JSON response %q: %v", method, path, rec.Code, rec.Body, err)
	}
	return rec.Code, result, rec.Body.String()
}

func TestHeldoutRoutes(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	repo := lifecycleRepo(t)
	if status, result := additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(repo)+`,"name":"My App"}`); status != 200 {
		t.Fatalf("register: %d %#v", status, result)
	}
	loopback := "127.0.0.1:8765"
	dir := filepath.Join(t.TempDir(), "owner")
	if real, err := filepath.EvalSymlinks(filepath.Dir(dir)); err == nil {
		dir = filepath.Join(real, "owner")
	}
	requests := []string{"Rename the greeting helper without touching the tests.", "Add a decision record for the cache.", "Replaced from the console."}
	private := append([]string{dir, "H01", "H02"}, requests...)
	assertPrivate := func(raw string) {
		t.Helper()
		for _, text := range private {
			if strings.Contains(raw, text) {
				t.Fatalf("a hub response carried owner-private data: %s", raw)
			}
		}
	}
	counts := func(status int, result map[string]any, raw string, count, pending, created float64) {
		t.Helper()
		assertPrivate(raw)
		if status != 200 || result["configured"] != true || result["present"] != true || result["count"] != count ||
			result["pending"] != pending || result["created"] != created || result["problem"] != "" {
			t.Fatalf("write response: %d %#v", status, result)
		}
	}
	post := func(path, body string) (int, map[string]any, string) {
		t.Helper()
		return heldoutRequest(t, h, "POST", "/api/v1/"+path, body, loopback, true)
	}

	status, result, raw := heldoutRequest(t, h, "GET", "/api/v1/heldout", "", "example.com", false)
	if status != 200 || result["configured"] != false || result["present"] != false || result["count"] != 0.0 || result["pending"] != 0.0 {
		t.Fatalf("status before setup: %d %#v", status, result)
	}
	if problem, ok := result["problem"].(string); !ok || problem != "" {
		t.Fatalf("problem is always a string: %#v", result)
	}
	judges := result["judges"].([]any)
	if len(judges) != 6 {
		t.Fatalf("judges: %#v", judges)
	}
	for _, j := range judges {
		judge := j.(map[string]any)
		name, _ := judge["name"].(string)
		label, _ := judge["label"].(string)
		if !strings.HasPrefix(name, "starter-") || label == "" {
			t.Fatalf("judge: %#v", judge)
		}
	}
	assertPrivate(raw)

	setup := `{"dir":` + jsonString(dir) + `}`
	status, result, raw = heldoutRequest(t, h, "POST", "/api/v1/heldout", setup, loopback, false)
	if status != 403 || result["error"] != "missing X-Pulse-Heldout: yes" {
		t.Fatalf("unconfirmed setup: %d %#v", status, result)
	}
	assertPrivate(raw)
	status, result, raw = heldoutRequest(t, h, "POST", "/api/v1/heldout", setup, "example.com", true)
	if status != 403 || result["error"] != "Held-out fixtures are written only on loopback" {
		t.Fatalf("setup on a foreign host: %d %#v", status, result)
	}
	assertPrivate(raw)
	if _, err := os.Stat(dir); !os.IsNotExist(err) {
		t.Fatal("a refused setup created the directory")
	}
	status, result, raw = post("heldout", `{"dir":`+jsonString(filepath.Join(repo, "private"))+`}`)
	if status != 400 {
		t.Fatalf("setup inside a registered project: %d %#v", status, result)
	}
	assertPrivate(raw)
	if strings.Contains(raw, repo) {
		t.Fatalf("the refusal named the path: %s", raw)
	}
	status, result, raw = post("heldout", setup)
	counts(status, result, raw, 0, 2, 4)
	if info, err := os.Stat(dir); err != nil || !info.IsDir() || info.Mode().Perm() != 0o700 {
		t.Fatalf("private directory: %v %v", info, err)
	}
	if s.Setting(heldoutSetting, "") != dir {
		t.Fatalf("setting: %q", s.Setting(heldoutSetting, ""))
	}

	// The setting row and the project's evolution view stay masked.
	status, result, raw = heldoutRequest(t, h, "GET", "/api/v1/settings", "", loopback, false)
	if status != 200 {
		t.Fatalf("settings: %d", status)
	}
	assertPrivate(raw)
	masked := false
	for _, row := range result["settings"].([]any) {
		if setting := row.(map[string]any); setting["name"] == heldoutSetting {
			masked = setting["secret"] == true && setting["value"] == privateValue
		}
	}
	if !masked {
		t.Fatal("the held-out setting row is not masked")
	}
	status, result, raw = heldoutRequest(t, h, "GET", "/api/v1/projects/my-app/evolution", "", loopback, false)
	if status != 200 || result["loop"].(map[string]any)["heldout_present"] != true {
		t.Fatalf("evolution: %d %#v", status, result["loop"])
	}
	assertPrivate(raw)

	status, result, raw = post("heldout/fixtures", `{"request":`+jsonString(requests[0])+`,"judge":"starter-hardstop-history"}`)
	counts(status, result, raw, 1, 1, 0)
	status, result, raw = post("heldout/fixtures", `{"request":`+jsonString(requests[1])+`,"scorer":"#!/bin/sh\necho 1\n"}`)
	counts(status, result, raw, 2, 0, 0)
	body, err := os.ReadFile(filepath.Join(dir, "H01.request"))
	if err != nil || string(body) != requests[0] {
		t.Fatalf("H01.request: %q %v", body, err)
	}
	info, err := os.Stat(filepath.Join(dir, "H01.score"))
	if err != nil || info.Mode().Perm() != 0o700 {
		t.Fatalf("H01.score: %v %v", info, err)
	}
	body, err = os.ReadFile(filepath.Join(dir, "H01.score"))
	if err != nil || !strings.Contains(string(body), "exec \"${PULSE_MEASURE_BINARY:-pulse}\" evolve score starter-hardstop-history") {
		t.Fatalf("H01.score: %q %v", body, err)
	}

	refusals := []struct {
		body string
		code int
		what string
	}{
		{`{"request":"PULSE_HELDOUT_TEMPLATE left in","judge":"starter-hardstop-history"}`, 400, "Private fixture set is not ready"},
		{`{"request":"Fine.","judge":"starter-nope"}`, 400, "Private fixture set is not ready"},
		{`{"request":"Fine.","judge":"starter-hardstop-history","scorer":"#!/bin/sh\necho 1\n"}`, 400, "Private fixture set is not ready"},
		{`{"request":"Fine."}`, 400, "Private fixture set is not ready"},
		{`{"request":"Fine.","judge":"starter-hardstop-history","slot":5}`, 400, "Private fixture set is not ready"},
		{`{"request":"Fine.","judge":"starter-hardstop-history","slot":"1"}`, 400, "Invalid slot"},
		{`{"request":"Fine.","judge":"starter-hardstop-history","slot":1.5}`, 400, "Invalid slot"},
		{`{"request":"Fine.","judge":"starter-hardstop-history","slot":1001}`, 400, "Invalid slot"},
		{`{"judge":"starter-hardstop-history"}`, 400, "Invalid request"},
		{`{"request":"Fine.","judge":7}`, 400, "Invalid judge"},
	}
	for _, c := range refusals {
		status, result, raw := post("heldout/fixtures", c.body)
		if status != c.code || result["error"] != c.what {
			t.Fatalf("%s: %d %#v", c.body, status, result)
		}
		assertPrivate(raw)
	}
	status, result, raw = heldoutRequest(t, h, "POST", "/api/v1/heldout/fixtures", `{"request":"Fine.","judge":"starter-hardstop-history"}`, loopback, false)
	if status != 403 || result["error"] != "missing X-Pulse-Heldout: yes" {
		t.Fatalf("unconfirmed fixture: %d %#v", status, result)
	}
	assertPrivate(raw)
	status, result, raw = heldoutRequest(t, h, "POST", "/api/v1/heldout/fixtures", `{"request":"Fine.","judge":"starter-hardstop-history"}`, "evil.example:8765", true)
	if status != 403 || result["error"] != "Held-out fixtures are written only on loopback" {
		t.Fatalf("fixture on a foreign host: %d %#v", status, result)
	}
	assertPrivate(raw)

	status, result, raw = heldoutRequest(t, h, "GET", "/api/v1/heldout", "", loopback, false)
	if status != 200 || result["configured"] != true || result["present"] != true || result["count"] != 2.0 || result["pending"] != 0.0 || result["problem"] != "" {
		t.Fatalf("status after two fixtures: %d %#v", status, result)
	}
	assertPrivate(raw)

	status, result, raw = post("heldout/fixtures", `{"request":`+jsonString(requests[2])+`,"judge":"starter-injection-note","slot":1}`)
	counts(status, result, raw, 2, 0, 0)
	if body, err := os.ReadFile(filepath.Join(dir, "H01.request")); err != nil || string(body) != requests[2] {
		t.Fatalf("rewritten H01.request: %q %v", body, err)
	}

	for _, c := range []struct {
		method, path string
		code         int
	}{
		{"GET", "/api/v1/heldout/fixtures", 405},
		{"DELETE", "/api/v1/heldout", 405},
		{"GET", "/api/v1/heldout/other", 404},
		{"POST", "/api/v1/heldout/fixtures/1", 404},
	} {
		if status, result, _ := heldoutRequest(t, h, c.method, c.path, "{}", loopback, true); status != c.code {
			t.Fatalf("%s %s: %d %#v", c.method, c.path, status, result)
		}
	}
	var events int
	if err := s.DB.QueryRow("SELECT count(*) FROM events WHERE subject=? AND actor='human'", heldoutSetting).Scan(&events); err != nil || events != 1 {
		t.Fatalf("setting events: %d %v", events, err)
	}
}

func TestHeldoutUnavailableStore(t *testing.T) {
	h := New(nil, nil)
	if status, result, _ := heldoutRequest(t, h, "GET", "/api/v1/heldout", "", "127.0.0.1:8765", false); status != 503 || result["error"] == nil {
		t.Fatalf("unavailable status: %d %#v", status, result)
	}
	for _, path := range []string{"heldout", "heldout/fixtures"} {
		if status, _, _ := heldoutRequest(t, h, "POST", "/api/v1/"+path, "{}", "127.0.0.1:8765", true); status != 409 {
			t.Fatalf("unavailable write %s: %d", path, status)
		}
	}
}
