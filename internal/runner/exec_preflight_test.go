package runner

import (
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
