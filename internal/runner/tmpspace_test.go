package runner

import (
	"errors"
	"strings"
	"testing"
)

func TestPreflightTmpSpaceFullDiskIsSetupError(t *testing.T) {
	orig := freeSpace
	defer func() { freeSpace = orig }()
	freeSpace = func(string) (uint64, error) { return 10 << 20, nil }
	err := preflightTmpSpace("/scratch", minTmpSpace)
	if err == nil {
		t.Fatal("expected a setup error when TMPDIR is nearly full")
	}
	for _, want := range []string{"setup-error", "/scratch", "10.0 MiB", "512.0 MiB"} {
		if !strings.Contains(err.Error(), want) {
			t.Fatalf("error should mention %q, got %q", want, err.Error())
		}
	}
	freeSpace = func(string) (uint64, error) { return 0, errors.New("no such file") }
	if err := preflightTmpSpace("/gone", minTmpSpace); err == nil || !strings.Contains(err.Error(), "unusable") {
		t.Fatalf("unstat-able TMPDIR must be a setup-error naming 'unusable', got %v", err)
	}
	freeSpace = func(string) (uint64, error) { return minTmpSpace, nil }
	if err := preflightTmpSpace("/ok", minTmpSpace); err != nil {
		t.Fatalf("enough space must pass: %v", err)
	}
}

func TestPreflightTmpSpaceUsesWorkerTMPDIR(t *testing.T) {
	if got := workerTmpDir([]string{"PATH=/bin", "TMPDIR=/w/tmp"}); got != "/w/tmp" {
		t.Fatalf("TMPDIR from env, got %q", got)
	}
	if got := workerTmpDir([]string{"PATH=/bin"}); got == "" {
		t.Fatal("default TMPDIR must not be empty")
	}
	if err := preflightTmpSpace(t.TempDir(), 1); err != nil {
		t.Fatalf("real Statfs on a temp dir must pass: %v", err)
	}
}
