package tally

import (
	"os"
	"path/filepath"
	"testing"
)

// TestSignalUnlandedUnderHookGitEnv guards the GIT_DIR class of failures: when
// tally runs from a git hook (pre-commit `go test ./...`) the inherited
// GIT_DIR/GIT_WORK_TREE/GIT_INDEX_FILE must not redirect the landed-Signal
// probes from projectPath to the hook's own repository.
func TestSignalUnlandedUnderHookGitEnv(t *testing.T) {
	f := newFixture(t, false)
	if err := os.MkdirAll(filepath.Join(f.repo, "internal", "x"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(f.repo, "internal", "x", "landed.go"), []byte("package x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitIn(t, f.repo, "init", "-q", "-b", "main")
	gitIn(t, f.repo, "add", ".")
	gitIn(t, f.repo, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "landed")

	// A second repository standing in for the hook's own checkout: its main
	// branch has a commit, but not internal/x/landed.go.
	other := t.TempDir()
	if err := os.WriteFile(filepath.Join(other, "README"), []byte("hook repo\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitIn(t, other, "init", "-q", "-b", "main")
	gitIn(t, other, "add", ".")
	gitIn(t, other, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "other")
	t.Setenv("GIT_DIR", filepath.Join(other, ".git"))
	t.Setenv("GIT_WORK_TREE", other)
	t.Setenv("GIT_INDEX_FILE", filepath.Join(other, ".git", "index"))
	t.Setenv("GIT_PREFIX", "")

	landedCache.Lock()
	landedCache.heads = map[string]landedHead{}
	landedCache.exists = map[string]bool{}
	landedCache.Unlock()

	if got := SignalUnlanded(f.st, f.repo, "card", "Signal: internal/x/landed.go"); got {
		t.Fatalf("SignalUnlanded(landed path) = true under hook GIT_* env; probe was redirected away from %s", f.repo)
	}
}
