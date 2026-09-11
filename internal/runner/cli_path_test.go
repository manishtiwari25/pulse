package runner

import (
	"os"
	"path/filepath"
	"testing"
)

func TestCLIOnPath(t *testing.T) {
	bin := t.TempDir()
	exe := filepath.Join(bin, "pulse")
	if err := os.WriteFile(exe, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	other := t.TempDir()
	if got := cliOnPath(other, exe); got != bin+string(os.PathListSeparator)+other {
		t.Fatalf("missing pulse: got %q", got)
	}
	if got := cliOnPath("", exe); got != bin {
		t.Fatalf("empty PATH: got %q", got)
	}
	if got := cliOnPath(bin+string(os.PathListSeparator)+other, exe); got != "" {
		t.Fatalf("already on PATH: got %q, want no change", got)
	}
	if got := cliOnPath(other, ""); got != "" {
		t.Fatalf("no executable: got %q", got)
	}
}
