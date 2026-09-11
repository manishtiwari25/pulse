package harness

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/provider"
)

func readStepEvents(t *testing.T, path string) []map[string]any {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(string(data)), "\n")
	events := make([]map[string]any, 0, len(lines))
	for _, line := range lines {
		var ev map[string]any
		if err := json.Unmarshal([]byte(line), &ev); err != nil {
			t.Fatalf("bad JSONL line %q: %v", line, err)
		}
		events = append(events, ev)
	}
	return events
}

func TestHarnessRunStepsFileNormalRun(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{})
	path := filepath.Join(t.TempDir(), "nested", "steps.jsonl")
	out, stderr, code := invokeHarness(t, s.Home, "run", "-p", "do it", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--session", "hs-steps", "--steps-file", path, "--trajectory", "none", "--quiet")
	if code != 0 || !strings.Contains(out, "SUMMARY: fake complete") || stderr != "" {
		t.Fatalf("run = code %d stdout %q stderr %q", code, out, stderr)
	}
	events := readStepEvents(t, path)
	if len(events) < 3 {
		t.Fatalf("events = %#v", events)
	}
	begin := events[0]
	if begin["event"] != "begin" || begin["session"] != "hs-steps" || begin["provider"] != fakeProviderName ||
		begin["model"] != "fake-model" || begin["cwd"] == "" || begin["at"] == "" {
		t.Fatalf("bad begin: %#v", begin)
	}
	step := events[1]
	usage, _ := step["usage"].(map[string]any)
	if step["event"] != "step" || step["n"] == nil || step["kind"] == nil || step["content"] == nil ||
		usage["prompt_tokens"] == nil || usage["completion_tokens"] == nil {
		t.Fatalf("bad step: %#v", step)
	}
	finish := events[len(events)-1]
	if finish["event"] != "finish" || finish["exit_status"] != ExitSubmitted ||
		!strings.Contains(finish["submission"].(string), "SUMMARY: fake complete") || finish["error"] != "" ||
		finish["cost"] == nil || finish["steps"] == nil || finish["at"] == "" {
		t.Fatalf("bad finish: %#v", finish)
	}
}

func TestHarnessRunStepsFileEarlyNotLoggedIn(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{err: provider.ErrNotLoggedIn})
	path := filepath.Join(t.TempDir(), "steps.jsonl")
	out, stderr, code := invokeHarness(t, s.Home, "run", "-p", "do it", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--session", "hs-login-steps", "--steps-file", path, "--quiet")
	if code != 4 || !strings.Contains(stderr, "not logged in:") || !strings.Contains(out, "SUMMARY: harness stopped: not_logged_in") {
		t.Fatalf("run = code %d stdout %q stderr %q", code, out, stderr)
	}
	events := readStepEvents(t, path)
	if len(events) != 2 {
		t.Fatalf("events = %#v", events)
	}
	if events[0]["event"] != "begin" || events[0]["session"] != "hs-login-steps" || events[0]["model"] != "" {
		t.Fatalf("bad begin: %#v", events[0])
	}
	if events[1]["event"] != "finish" || events[1]["exit_status"] != ExitNotLoggedIn ||
		!strings.Contains(events[1]["error"].(string), "not logged in") {
		t.Fatalf("bad finish: %#v", events[1])
	}
}
