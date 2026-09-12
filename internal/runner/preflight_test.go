package runner

import (
	"os"
	"path/filepath"
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
