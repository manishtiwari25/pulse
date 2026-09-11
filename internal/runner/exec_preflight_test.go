package runner

import (
	"os"
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
