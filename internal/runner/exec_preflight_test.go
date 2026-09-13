package runner

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestPreflightRunKeyMissingIsSetupError(t *testing.T) {
	dir := t.TempDir()
	missing := filepath.Join(dir, "job.key")
	err := preflightRunKey(missing)
	if err == nil {
		t.Fatal("expected an error for a missing run key")
	}
	if !strings.Contains(err.Error(), "setup-error") || !strings.Contains(err.Error(), missing) {
		t.Fatalf("error should name setup-error and the key path, got %q", err.Error())
	}
	if err := os.WriteFile(missing, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := preflightRunKey(missing); err == nil || !strings.Contains(err.Error(), "empty") {
		t.Fatalf("empty key must be a setup-error naming 'empty', got %v", err)
	}
	if err := os.WriteFile(missing, []byte("k"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := preflightRunKey(missing); err != nil {
		t.Fatalf("present key must pass: %v", err)
	}
}

func TestPreflightRunKeyRemoteCheckCmd(t *testing.T) {
	dir := t.TempDir()
	key := filepath.Join(dir, "run.key")
	run := func() error {
		return exec.Command("/bin/sh", "-c", remoteKeyCheckCmd(key)).Run()
	}
	if err := run(); err == nil {
		t.Fatal("missing remote key must fail test -s")
	}
	if err := os.WriteFile(key, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := run(); err == nil {
		t.Fatal("empty remote key must fail test -s")
	}
	if err := os.WriteFile(key, []byte("k"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := run(); err != nil {
		t.Fatalf("non-empty remote key must pass: %v", err)
	}
	if cmd := remoteKeyCheckCmd("/w/job 1/run.key"); !strings.Contains(cmd, "'/w/job 1/run.key'") {
		t.Fatalf("path must be shell-quoted, got %q", cmd)
	}
}

func TestPreflightRunKeyRemoteResult(t *testing.T) {
	if err := remoteKeyCheckResult("/w/j/run.key", "box", "before the worker starts", 0, "", ""); err != nil {
		t.Fatalf("exit 0 must pass: %v", err)
	}
	err := remoteKeyCheckResult("/w/j/run.key", "box", "before the worker starts", 1, "", "no such file")
	if err == nil {
		t.Fatal("non-zero exit must be a setup error")
	}
	for _, want := range []string{"setup-error", "/w/j/run.key", "box", "before the worker starts", "no such file"} {
		if !strings.Contains(err.Error(), want) {
			t.Fatalf("error %q should contain %q", err.Error(), want)
		}
	}
}

// TestPreflightRunKeyAfterMakeRunKey: the key makeRunKey writes passes the
// preflight that run.go applies right after it, before clone creation.
func TestPreflightRunKeyAfterMakeRunKey(t *testing.T) {
	path := filepath.Join(t.TempDir(), "run.key")
	if _, err := makeRunKey(path, "juniper", "run-1"); err != nil {
		t.Fatal(err)
	}
	if err := preflightRunKey(path); err != nil {
		t.Fatalf("fresh run key rejected: %v", err)
	}
}

func TestPreflightRemoteCLICheckCmd(t *testing.T) {
	dir := t.TempDir()
	cli := filepath.Join(dir, "pulse")
	run := func() error {
		return exec.Command("/bin/sh", "-c", remoteCLICheckCmd(cli)).Run()
	}
	if err := run(); err == nil {
		t.Fatal("missing PULSE_CLI must fail test -x")
	}
	if err := os.WriteFile(cli, []byte("#!/bin/sh\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := run(); err == nil {
		t.Fatal("non-executable PULSE_CLI must fail test -x")
	}
	if err := os.Chmod(cli, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := run(); err != nil {
		t.Fatalf("executable PULSE_CLI must pass: %v", err)
	}
	if cmd := remoteCLICheckCmd("/usr/local/bin/my pulse"); !strings.Contains(cmd, "'/usr/local/bin/my pulse'") {
		t.Fatalf("path must be shell-quoted, got %q", cmd)
	}
}

func TestPreflightRemoteCLIResult(t *testing.T) {
	if err := remoteCLICheckResult("/usr/local/bin/pulse", "box", 0, "", ""); err != nil {
		t.Fatalf("exit 0 must pass: %v", err)
	}
	err := remoteCLICheckResult("/usr/local/bin/pulse", "box", 1, "", "no such file")
	if err == nil {
		t.Fatal("non-zero exit must be a setup error")
	}
	for _, want := range []string{"setup-error", "PULSE_CLI", "/usr/local/bin/pulse", "box", "no such file", "not started"} {
		if !strings.Contains(err.Error(), want) {
			t.Fatalf("error %q should contain %q", err.Error(), want)
		}
	}
}

// stageLocalPulse points the remote worker's PULSE_CLI fallback at bin for the test:
// the fake hands run on this machine, where nothing is staged at pairPulsePath, and
// sshExec's `test -x $PULSE_CLI` preflight would otherwise fail at setup.
func stageLocalPulse(t *testing.T, bin string) {
	t.Helper()
	prev := stagedPulsePath
	stagedPulsePath = bin
	t.Cleanup(func() { stagedPulsePath = prev })
}

func TestPreflightRunKeyLocalPrepareFailsEarly(t *testing.T) {
	s := newStore(t)
	project, _ := newRepo(t, s)
	home := t.TempDir()
	e := newLocalExec(row{"name": "local"}, s, home, row{"id": "job-prep"}, project, "cinder", &runLog{})
	if err := os.MkdirAll(filepath.Dir(e.key), 0o755); err != nil {
		t.Fatal(err)
	}
	_, err := e.prepare(context.Background())
	if err == nil || !strings.Contains(err.Error(), "setup-error") || !strings.Contains(err.Error(), e.key) {
		t.Fatalf("prepare without a run key must fail with setup-error naming the key, got %v", err)
	}
	if e.started() {
		t.Fatal("prepare must not mark the run as begun")
	}
}
