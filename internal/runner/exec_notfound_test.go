package runner

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The "worker command not found" failure must carry the underlying exec error
// so an operator can tell a missing binary from a permission or path problem.
func TestLocalRunWorkerReportsExecError(t *testing.T) {
	s := newStore(t)
	dir := t.TempDir()
	key := filepath.Join(dir, "run.key")
	if err := os.WriteFile(key, []byte("key"), 0o600); err != nil {
		t.Fatal(err)
	}
	e := &localExec{s: s, job: row{"id": "job-nf"}, project: row{"slug": "proj"}, log: &runLog{}, cloneDir: dir, key: key}
	missing := filepath.Join(dir, "no-such-worker-binary")
	_, err := e.runWorker(context.Background(), []string{missing}, map[string]string{}, 1)
	if err == nil {
		t.Fatal("expected error for missing worker command")
	}
	msg := err.Error()
	if !strings.Contains(msg, "worker command not found: "+missing) {
		t.Fatalf("message lacks command: %q", msg)
	}
	if !strings.Contains(msg, "no such file or directory") {
		t.Fatalf("message lacks underlying exec error: %q", msg)
	}
}
