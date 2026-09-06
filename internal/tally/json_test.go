package tally

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestJSONAndHashInputs(t *testing.T) {
	f := newFixture(t, true)
	for _, cmd := range []string{"board", "queue", "ballot", "proposals", "tally"} {
		out, err := runCLI(t, f.st, cmd, "--project", "fixture", "--now", fixtureNow, "--json")
		if err != nil {
			t.Fatalf("%s --json: %v", cmd, err)
		}
		var v any
		if err := json.Unmarshal([]byte(out), &v); err != nil {
			t.Errorf("%s --json is not JSON: %v\n%s", cmd, err, out)
		}
		if cmd == "tally" && (!strings.Contains(out, `"stale_days"`) || !strings.Contains(out, `"suggestions"`)) {
			t.Errorf("tally json keys:\n%s", out[:200])
		}
	}
	// hash: a post id, a file, stdin
	want, _ := runCLI(t, f.st, "hash", f.ids["amendment"])
	path := filepath.Join(t.TempDir(), "amend.md")
	if err := os.WriteFile(path, []byte(fixtureAmendmentBody), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := runCLI(t, f.st, "hash", path)
	if err != nil || got != want {
		t.Errorf("hash file: %v %q != %q", err, got, want)
	}
	r, w, _ := os.Pipe()
	w.WriteString(fixtureAmendmentBody)
	w.Close()
	old := os.Stdin
	os.Stdin = r
	got, err = runCLI(t, f.st, "hash", "-")
	os.Stdin = old
	if err != nil || got != want {
		t.Errorf("hash stdin: %v %q != %q", err, got, want)
	}
	if _, err := runCLI(t, f.st, "hash", f.ids["eligible"]); err == nil || !strings.Contains(err.Error(), "Proposed text") {
		t.Errorf("hash of a suggestion: %v", err)
	}
	if _, err := runCLI(t, f.st, "tally", "--project", "fixture", "--amend-majority", "most"); err == nil {
		t.Error("bad --amend-majority accepted")
	}
	if _, err := runCLI(t, f.st, "board", "--project", "nope"); err == nil {
		t.Error("unknown project accepted")
	}
}
