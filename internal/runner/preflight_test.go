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

// TestSetupErrorsSharePrefix: every preflight check listed in the doc table at
// the top of preflight.go reports its failure with the "setup-error:" prefix,
// so the runner classifies all of them as setup failures that requeue the job.
func TestSetupErrorsSharePrefix(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "absent")
	empty := filepath.Join(t.TempDir(), "empty")
	if err := os.WriteFile(empty, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	prev := freeSpace
	freeSpace = func(string) (uint64, error) { return 0, nil }
	defer func() { freeSpace = prev }()
	cases := map[string]error{
		"preflightPulseCLI no cli":   preflightPulseCLI("", ""),
		"preflightPulseCLI bad cli":  preflightPulseCLI("", missing),
		"preflightRunKey missing":    preflightRunKey(missing),
		"preflightRunKey empty":      preflightRunKey(empty),
		"preflightTmpSpace unusable": preflightTmpSpace(missing+"/x", 1),
		"preflightTmpSpace low":      preflightTmpSpace(t.TempDir(), 1),
		"remoteKeyCheckResult":       remoteKeyCheckResult("/j/run.key", "host", "after upload", 1, "", "no such file"),
		"remoteCLICheckResult":       remoteCLICheckResult("/opt/pulse", "host", 1, "", ""),
	}
	for name, err := range cases {
		if err == nil {
			t.Errorf("%s: want a setup-error, got nil", name)
			continue
		}
		if !strings.HasPrefix(err.Error(), "setup-error:") {
			t.Errorf("%s: %q does not start with \"setup-error:\"", name, err.Error())
		}
	}
}
