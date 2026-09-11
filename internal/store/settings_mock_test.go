package store

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The hub mock (web/hub/mock/api/settings.json) must list every DaemonSettings key the store
// knows (daemon.* and landing.push), so a new DaemonSettings key without a mock row fails here rather than in the browser.
func TestMockSettingsListEveryDaemonSetting(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "..", "web", "hub", "mock", "api", "settings.json"))
	if err != nil {
		t.Skipf("hub mock not present: %v", err)
	}
	var doc struct {
		Settings []struct {
			Name    string `json:"name"`
			Kind    string `json:"kind"`
			Default string `json:"default"`
		} `json:"settings"`
	}
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatalf("parse mock: %v", err)
	}
	mock := map[string]struct{ kind, def string }{}
	for _, r := range doc.Settings {
		mock[r.Name] = struct{ kind, def string }{r.Kind, r.Default}
	}
	for _, name := range DaemonSettingNames() {
		got, ok := mock[name]
		if !ok {
			t.Errorf("mock lacks daemon setting %s", name)
			continue
		}
		if want := DaemonRules[name].Kind; want != "" && got.kind != want {
			t.Errorf("%s: mock kind %q, store %q", name, got.kind, want)
		}
		if want := DaemonSettings[name]; got.def != want {
			t.Errorf("%s: mock default %q, store %q", name, got.def, want)
		}
	}
	for name := range mock {
		if _, ok := DaemonSettings[name]; !ok && strings.HasPrefix(name, "daemon.") {
			t.Errorf("mock lists %s which the store does not know", name)
		}
	}
}
