package runner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestPulseOnPathFindsExecutableOnWorkerPath(t *testing.T) {
	dir := t.TempDir()
	bin := filepath.Join(dir, "pulse")
	if err := os.WriteFile(bin, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	empty := t.TempDir()
	env := envWith([]string{"PATH=" + empty, "HOME=/x"}, map[string]string{"PATH": empty + string(os.PathListSeparator) + dir})
	if got := pulseOnPath(env); got != bin {
		t.Fatalf("pulseOnPath = %q, want %q", got, bin)
	}
	if got := pulseOnPath([]string{"PATH=" + empty}); got != "" {
		t.Fatalf("pulseOnPath on empty dir = %q, want none", got)
	}
	if got := pulseOnPath([]string{"PATH=" + empty}); got == "" {
		line := preflightLine(got, "/opt/pulse")
		if line != "preflight: pulse missing from worker PATH; worker must use $PULSE_CLI=/opt/pulse" {
			t.Fatalf("line = %q", line)
		}
	}
	if line := preflightLine(bin, ""); line != "preflight: pulse on worker PATH: "+bin {
		t.Fatalf("line = %q", line)
	}
}

// TestPulseOnPathMissingIsSetupError: when neither `pulse` on the worker PATH nor
// PULSE_CLI resolves to an executable, the launch is refused with a setup-error
// instead of only logging; a resolvable one on either side passes.
func TestPulseOnPathMissingIsSetupError(t *testing.T) {
	dir := t.TempDir()
	exe := filepath.Join(dir, "pulse")
	if err := os.WriteFile(exe, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := preflightPulseCLI(exe, ""); err != nil {
		t.Fatalf("pulse on PATH: %v", err)
	}
	if err := preflightPulseCLI("", exe); err != nil {
		t.Fatalf("PULSE_CLI executable: %v", err)
	}
	err := preflightPulseCLI("", "")
	if err == nil || !strings.Contains(err.Error(), "setup-error") || !strings.Contains(err.Error(), "no PULSE_CLI") {
		t.Fatalf("nothing resolves: got %v", err)
	}
	err = preflightPulseCLI("", filepath.Join(dir, "nope"))
	if err == nil || !strings.Contains(err.Error(), "setup-error") || !strings.Contains(err.Error(), "not an executable") {
		t.Fatalf("dangling PULSE_CLI: got %v", err)
	}
	if err := preflightPulseCLI("", dir); err == nil {
		t.Fatal("directory PULSE_CLI accepted")
	}
}
