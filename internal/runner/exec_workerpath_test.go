package runner

import (
	"os"
	"strings"
	"testing"
	"time"
)

func lookupEnv(env []string, key string) string {
	for _, kv := range env {
		if strings.HasPrefix(kv, key+"=") {
			return kv[len(key)+1:]
		}
	}
	return ""
}

func TestLocalExecWorkerPath(t *testing.T) {
	sep := string(os.PathListSeparator)
	base := []string{"PATH=/usr/bin" + sep + "/bin", "HOME=/h"}

	got := workerEnvWithCLIPath(base, map[string]string{"PULSE_CLI": "/opt/pulse/bin/pulse"})
	if p := lookupEnv(got, "PATH"); p != "/opt/pulse/bin"+sep+"/usr/bin"+sep+"/bin" {
		t.Fatalf("PATH not prefixed with dir(PULSE_CLI): %q", p)
	}
	if lookupEnv(got, "PULSE_CLI") != "/opt/pulse/bin/pulse" {
		t.Fatalf("PULSE_CLI lost: %v", got)
	}

	// Already on PATH: no duplicate.
	got = workerEnvWithCLIPath(base, map[string]string{"PULSE_CLI": "/usr/bin/pulse"})
	if p := lookupEnv(got, "PATH"); p != "/usr/bin"+sep+"/bin" {
		t.Fatalf("PATH changed although dir already present: %q", p)
	}

	// No PULSE_CLI: plain envWith.
	got = workerEnvWithCLIPath(base, map[string]string{"X": "1"})
	if p := lookupEnv(got, "PATH"); p != "/usr/bin"+sep+"/bin" {
		t.Fatalf("PATH changed without PULSE_CLI: %q", p)
	}

	// No PATH at all: dir becomes PATH.
	got = workerEnvWithCLIPath([]string{"HOME=/h"}, map[string]string{"PULSE_CLI": "/opt/p/pulse"})
	if p := lookupEnv(got, "PATH"); p != "/opt/p" {
		t.Fatalf("PATH without base: %q", p)
	}
}

// TestSSHExecWorkerPath: the remote branch launches the worker through hands
// `run`, so the same dir(PULSE_CLI) prefix must apply there when run.go passes
// PULSE_CLI as a K=V argument.
func TestSSHExecWorkerPath(t *testing.T) {
	dir := t.TempDir()
	cli := dir + "/pulse"
	if err := os.WriteFile(cli, []byte("#!/bin/sh\necho ok\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	var out strings.Builder
	h := &hands{stdout: &out, stderr: &out}
	rc := h.withTimeout(time.Minute, dir, []string{"/bin/sh", "-c", "printf %s \"$PATH\""}, map[string]string{"PULSE_CLI": cli})
	if rc != 0 {
		t.Fatalf("rc=%d out=%q", rc, out.String())
	}
	if !strings.HasPrefix(out.String(), dir+string(os.PathListSeparator)) {
		t.Fatalf("remote worker PATH not prefixed with dir(PULSE_CLI): %q", out.String())
	}
}
