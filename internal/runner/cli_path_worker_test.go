package runner

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// TestCLIOnPathWorkerProcess locks the pulse-CLI-on-PATH fix at the variable that
// matters: the PATH the process spawned by localExec.runWorker actually observes.
// The env is built the way run.go builds it (PULSE_CLI + cliOnPath) and the worker
// itself prints $PATH, so a regression in runWorker's env plumbing fails here even
// if the helpers still pass their unit tests.
func TestCLIOnPathWorkerProcess(t *testing.T) {
	exe, err := os.Executable()
	if err != nil {
		t.Skip("os.Executable:", err)
	}
	inherited := t.TempDir()
	t.Setenv("PATH", inherited)
	s := newStore(t)
	e, _, env := runnerHeartbeatFixture(t, s, "local")
	env["PULSE_CLI"] = exe
	if p := cliOnPath(os.Getenv("PATH"), exe); p != "" {
		env["PATH"] = p
	}
	out, err := e.runWorker(context.Background(), []string{"/bin/sh", "-c", `printf 'PATH=%s\n' "$PATH"`}, env, 1)
	if err != nil || out.ExitCode != 0 {
		t.Fatalf("worker: %+v %v", out, err)
	}
	var got string
	for _, line := range strings.Split(out.Stdout, "\n") {
		if strings.HasPrefix(line, "PATH=") {
			got = strings.TrimPrefix(line, "PATH=")
		}
	}
	dirs := filepath.SplitList(got)
	if len(dirs) == 0 || dirs[0] != filepath.Dir(exe) {
		t.Fatalf("spawned worker PATH %q does not lead with %q", got, filepath.Dir(exe))
	}
	if dirs[len(dirs)-1] != inherited {
		t.Fatalf("spawned worker PATH %q dropped inherited %q", got, inherited)
	}
}
