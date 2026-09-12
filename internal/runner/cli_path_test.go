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

// TestCLIOnPathExecutableDir locks the PATH fix for real: with a PATH that does not resolve the
// running binary, the worker env built the way run.go builds it for localExec.runWorker
// (cliOnPath + envWith over the inherited environ) leads with the directory of os.Executable()
// and keeps the inherited PATH entries behind it.
func TestCLIOnPathExecutableDir(t *testing.T) {
	exe, err := os.Executable()
	if err != nil {
		t.Skip("os.Executable:", err)
	}
	inherited := t.TempDir()
	t.Setenv("PATH", inherited)
	env := map[string]string{}
	if p := cliOnPath(os.Getenv("PATH"), exe); p != "" {
		env["PATH"] = p
	}
	var got string
	for _, kv := range envWith(environ(), env) {
		if len(kv) > 5 && kv[:5] == "PATH=" {
			got = kv[5:]
		}
	}
	dirs := filepath.SplitList(got)
	if len(dirs) < 2 || dirs[0] != filepath.Dir(exe) {
		t.Fatalf("worker PATH %q does not lead with %q", got, filepath.Dir(exe))
	}
	if dirs[len(dirs)-1] != inherited {
		t.Fatalf("worker PATH %q dropped the inherited entry %q", got, inherited)
	}
}
