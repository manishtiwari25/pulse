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
// running binary, the worker env built by the production helpers run.go and exec.go use for
// localExec.runWorker (workerCLIEnv + workerEnvWithCLIPath) leads with the directory of os.Executable()
// and keeps the inherited PATH entries behind it.
func TestCLIOnPathExecutableDir(t *testing.T) {
	exe, err := os.Executable()
	if err != nil {
		t.Skip("os.Executable:", err)
	}
	inherited := t.TempDir()
	t.Setenv("PATH", inherited)
	env := map[string]string{}
	workerCLIEnv(env, exe)
	if env["PULSE_CLI"] != exe {
		t.Fatalf("PULSE_CLI = %q, want %q", env["PULSE_CLI"], exe)
	}
	var got string
	for _, kv := range workerEnvWithCLIPath(environ(), env) {
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

// TestCLIOnPathFallback locks the fallback localCLI takes when os.Executable()
// cannot be used: the binary named by PULSE_CLI, then <home>/bin/pulse, and
// only executables count. Here os.Executable() resolves (the test binary), so
// the fallbacks are exercised directly through localCLIFallback.
func TestCLIOnPathFallback(t *testing.T) {
	home := t.TempDir()
	t.Setenv("PULSE_CLI", "")
	if got := localCLIFallback(home); got != "" {
		t.Fatalf("no candidates: got %q", got)
	}
	bin := filepath.Join(home, "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	homeCLI := filepath.Join(bin, "pulse")
	if err := os.WriteFile(homeCLI, []byte("#!/bin/sh\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := localCLIFallback(home); got != "" {
		t.Fatalf("non-executable ~/.pulse/bin/pulse accepted: %q", got)
	}
	if err := os.Chmod(homeCLI, 0o755); err != nil {
		t.Fatal(err)
	}
	if got := localCLIFallback(home); got != homeCLI {
		t.Fatalf("home fallback: got %q want %q", got, homeCLI)
	}
	other := filepath.Join(t.TempDir(), "pulse")
	if err := os.WriteFile(other, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PULSE_CLI", other)
	if got := localCLIFallback(home); got != other {
		t.Fatalf("PULSE_CLI fallback: got %q want %q", got, other)
	}
	if p := cliOnPath(t.TempDir(), other); p == "" || filepath.SplitList(p)[0] != filepath.Dir(other) {
		t.Fatalf("worker PATH does not lead with fallback dir: %q", p)
	}
	if exe, err := os.Executable(); err == nil {
		if got := localCLI(home); got != exe {
			t.Fatalf("localCLI prefers os.Executable: got %q want %q", got, exe)
		}
	}
}
