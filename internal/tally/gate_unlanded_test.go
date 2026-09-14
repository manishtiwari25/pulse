package tally

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func gitIn(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@t", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@t", "GIT_CONFIG_GLOBAL=/dev/null", "GIT_CONFIG_NOSYSTEM=1")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

func TestSignalUnlanded(t *testing.T) {
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
	gitIn(t, f.repo, "checkout", "-q", "-b", "pulse/agent/job")
	if err := os.WriteFile(filepath.Join(f.repo, "internal", "x", "unlanded.go"), []byte("package x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitIn(t, f.repo, "add", ".")
	gitIn(t, f.repo, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "unlanded")

	cases := map[string]bool{
		"Signal: internal/x/landed.go":                                 false,
		"Signal: internal/x/unlanded.go":                               true,
		"Signal: internal/x/unlanded.go\nSignal: internal/x/landed.go": false, // any landed path is enough
		"Signal: check:go-test":                                        false,
		"Signal: internal/x/missing.go":                                false, // not resolving at all: NoEvidence gate handles it
	}
	for body, want := range cases {
		if got := SignalUnlanded(f.st, f.repo, "card", body); got != want {
			t.Errorf("SignalUnlanded(%q) = %v, want %v", body, got, want)
		}
	}
	// Without a default branch on the project the gate never fires.
	if _, err := f.st.DB.Exec("UPDATE projects SET default_branch = '' WHERE slug = 'fixture'"); err != nil {
		t.Fatal(err)
	}
	if SignalUnlanded(f.st, f.repo, "card", "Signal: internal/x/unlanded.go") {
		t.Error("SignalUnlanded fired with no default branch configured")
	}
	// A non-git directory never fires either.
	if SignalUnlanded(f.st, t.TempDir(), "card", "Signal: internal/x/unlanded.go") {
		t.Error("SignalUnlanded fired outside a git repository")
	}
}
