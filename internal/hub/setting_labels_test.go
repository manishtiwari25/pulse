package hub

import (
	"os"
	"regexp"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

// TestSettingLabelsCoverCatalogue keeps the humans-first settings page
// honest (ADR-042 D5): every key the store knows must carry a human label
// in web/hub/src/lib/setting-labels.ts, otherwise the page falls back to a
// derived name (the raw key's last segment) without anyone noticing.
func TestSettingLabelsCoverCatalogue(t *testing.T) {
	src, err := os.ReadFile("../../web/hub/src/lib/setting-labels.ts")
	if err != nil {
		t.Skipf("setting-labels.ts not in this checkout: %v", err)
	}
	labelRe := regexp.MustCompile(`(?m)^\s*"?([a-z_][a-z0-9_.]*)"?:\s*"[^"]+",?$`)
	labels := map[string]bool{}
	for _, m := range labelRe.FindAllStringSubmatch(string(src), -1) {
		labels[m[1]] = true
	}
	if len(labels) < 10 {
		t.Fatalf("parsed only %d labels from setting-labels.ts; the LABELS map changed shape", len(labels))
	}
	var missing []string
	for _, name := range append(store.SettingNames(), store.ExtraSettingNames()...) {
		if !labels[name] {
			missing = append(missing, name)
		}
	}
	if len(missing) > 0 {
		t.Fatalf("settings without a human label in web/hub/src/lib/setting-labels.ts: %s", strings.Join(missing, ", "))
	}
}
