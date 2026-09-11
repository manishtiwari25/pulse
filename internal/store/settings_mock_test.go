package store

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

// mockSettingGroups are the store setting groups the hub mock must list in full.
var mockSettingGroups = []struct {
	group    string
	settings map[string]string
	rules    map[string]SettingRule
}{
	{"daemon", DaemonSettings, DaemonRules},
	{"evolve", EvolveSettings, EvolveRules},
	{"lead", LeadSettings, LeadRules},
	{"owner", OwnerSettings, OwnerRules},
	{"harness", HarnessSettings, HarnessRules},
}

// The hub mock (web/hub/mock/api/settings.json) must list every key of every settings group the
// store knows (daemon.*, evolve.*, lead.*, owner.*, harness), with the store's kind and default, so a
// new setting without a mock row fails here rather than in the browser. The mock name set is the one
// variable: a mock row under a group prefix the store does not know fails too.
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
		if _, dup := mock[r.Name]; dup {
			t.Errorf("mock lists %s twice", r.Name)
		}
		mock[r.Name] = struct{ kind, def string }{r.Kind, r.Default}
	}
	for _, g := range mockSettingGroups {
		for _, name := range sortedKeys(g.settings) {
			got, ok := mock[name]
			if !ok {
				t.Errorf("mock lacks %s setting %s", g.group, name)
				continue
			}
			if want := g.rules[name].Kind; want != "" && got.kind != want {
				t.Errorf("%s: mock kind %q, store %q", name, got.kind, want)
			}
			if want := g.settings[name]; got.def != want {
				t.Errorf("%s: mock default %q, store %q", name, got.def, want)
			}
		}
	}
	known := map[string]bool{}
	for name := range DefaultSettings {
		known[name] = true
	}
	for _, name := range ExtraSettingNames() {
		known[name] = true
	}
	for name := range mock {
		if !known[name] {
			t.Errorf("mock lists %s which the store does not know", name)
		}
	}
}
