package hub

import (
	"net/http"
	"strings"
	"testing"
)

// GET /api/v1/service reads the user services without changing anything; a
// dry-run POST previews the units. Neither touches the service manager, so
// the test needs no launchctl (ADR-038).
func TestServiceRoute(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	s := additionsStore(t, false)
	h := New(s, nil)
	h.Port = 8765
	handovers := 0
	h.Handover = func(func() error) { handovers++ }

	status, result := additionsRequest(t, h, http.MethodGet, "/api/v1/service", "")
	if status != 200 || result["hub_is_service"] != false || result["hub_port"] != float64(8765) || result["command"] != "pulse service install" ||
		result["hub_url"] != "http://127.0.0.1:8765" {
		t.Fatalf("GET service: %d %#v", status, result)
	}
	state, ok := result["service"].(map[string]any)
	if !ok || state["installed"] != false || state["kind"] == "" || state["supported"] != true {
		t.Fatalf("service state: %#v", result["service"])
	}
	units, ok := state["units"].([]any)
	if !ok || len(units) != 2 {
		t.Fatalf("units: %#v", state["units"])
	}
	for i, name := range []string{"daemon", "hub"} {
		unit := units[i].(map[string]any)
		if unit["name"] != name || unit["present"] != false || unit["loaded"] != false || !strings.Contains(unit["path"].(string), "pulse") {
			t.Fatalf("unit %d: %#v", i, unit)
		}
	}

	// A missing dry_run previews, like every owner write.
	status, result = additionsRequest(t, h, http.MethodPost, "/api/v1/service", `{}`)
	if status != 200 || result["dry_run"] != true || result["installed"] != false || result["handover"] != false || handovers != 0 {
		t.Fatalf("preview: %d %#v", status, result)
	}
	output, _ := result["output"].(string)
	if !strings.Contains(output, "pulse-daemon.sh") || !strings.Contains(output, "pulse-hub.sh") || !strings.Contains(output, "--port 8765") {
		t.Fatalf("preview output:\n%s", output)
	}
	if daemonPath, _ := result["daemon_path"].(string); daemonPath == "" || !strings.Contains(daemonPath, "pulse") {
		t.Fatalf("preview paths: %#v", result)
	}
	if status, result = additionsRequest(t, h, http.MethodGet, "/api/v1/service", ""); status != 200 || result["service"].(map[string]any)["installed"] != false {
		t.Fatalf("a preview installed something: %#v", result)
	}

	for _, path := range []string{"/api/v1/service/extra"} {
		if status, _ := additionsRequest(t, h, http.MethodGet, path, ""); status != 404 {
			t.Fatalf("%s: %d", path, status)
		}
	}
	if status, _ := additionsRequest(t, h, http.MethodDelete, "/api/v1/service", ""); status != 405 {
		t.Fatalf("DELETE service: %d", status)
	}
	none := New(nil, nil)
	if status, _ := additionsRequest(t, none, http.MethodGet, "/api/v1/service", ""); status != 503 {
		t.Fatalf("no store: %d", status)
	}
}
