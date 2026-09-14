package tally

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
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

// TestSignalUnlandedCacheFollowsLanding checks that a path landing on the default branch is seen once
// the resolved-head memo expires: the exists memo is keyed by commit, so a moved branch is never stale.
func TestSignalUnlandedCacheFollowsLanding(t *testing.T) {
	f := newFixture(t, false)
	if err := os.MkdirAll(filepath.Join(f.repo, "internal", "x"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(f.repo, "internal", "x", "a.go"), []byte("package x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	gitIn(t, f.repo, "init", "-q", "-b", "main")
	gitIn(t, f.repo, "add", ".")
	gitIn(t, f.repo, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "base")
	if err := os.WriteFile(filepath.Join(f.repo, "internal", "x", "b.go"), []byte("package x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	resetLandedCache()
	if !SignalUnlanded(f.st, f.repo, "card", "Signal: internal/x/b.go") {
		t.Fatal("uncommitted path should read as unlanded")
	}
	gitIn(t, f.repo, "add", ".")
	gitIn(t, f.repo, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "land b")
	// Expire the head memo without waiting for the TTL.
	landedCache.Lock()
	for k, h := range landedCache.heads {
		h.at = h.at.Add(-2 * landedHeadTTL)
		landedCache.heads[k] = h
	}
	landedCache.Unlock()
	if SignalUnlanded(f.st, f.repo, "card", "Signal: internal/x/b.go") {
		t.Fatal("landed path still reads as unlanded after the branch moved")
	}
}

// BenchmarkSignalUnlanded measures one tally pass asking the same question for many cards.
func BenchmarkSignalUnlanded(b *testing.B) {
	dir := b.TempDir()
	run := func(args ...string) {
		cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
		cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@t", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@t", "GIT_CONFIG_GLOBAL=/dev/null", "GIT_CONFIG_NOSYSTEM=1")
		if out, err := cmd.CombinedOutput(); err != nil {
			b.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	if err := os.WriteFile(filepath.Join(dir, "a.go"), []byte("package x\n"), 0o644); err != nil {
		b.Fatal(err)
	}
	run("init", "-q", "-b", "main")
	run("add", ".")
	run("-c", "commit.gpgsign=false", "commit", "-q", "-m", "base")
	if err := os.WriteFile(filepath.Join(dir, "b.go"), []byte("package x\n"), 0o644); err != nil {
		b.Fatal(err)
	}
	home := b.TempDir()
	if err := store.Create(filepath.Join(home, "pulse.sqlite")); err != nil {
		b.Fatal(err)
	}
	st, err := store.Open(store.Options{Home: home, Now: fixtureNow})
	if err != nil {
		b.Fatal(err)
	}
	defer st.Close()
	if _, err := st.DB.Exec("INSERT INTO projects (slug, name, path, default_branch, added_at) VALUES ('bench', 'B', ?, 'main', ?)", dir, fixtureNow); err != nil {
		b.Fatal(err)
	}
	resetLandedCache()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		for j := 0; j < 20; j++ { // one pass over 20 cards naming the same unlanded path
			if !SignalUnlanded(st, dir, "card", "Signal: b.go") {
				b.Fatal("expected unlanded")
			}
		}
	}
}

// TestPlanGateHoldsUnlandedSignal drives the GateUnlanded verdict through PlanGate with the checkout
// sitting on a pulse/* branch: the Signal path is judged against `<default_branch>:<path>`, not HEAD,
// so a file that exists only on the unlanded branch holds the card while a landed one is planned.
func TestPlanGateHoldsUnlandedSignal(t *testing.T) {
	f := newFixture(t, true)
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
	resetLandedCache()

	unlanded := addEligibleSuggestion(t, f, "6", "runner-worker/model", "worker-unlanded", "Worker unlanded card", "Signal: internal/x/unlanded.go", "", []string{"up", "up"})
	landed := addEligibleSuggestion(t, f, "7", "runner-worker/model", "worker-landed", "Worker landed card", "Signal: internal/x/landed.go", "", []string{"up", "up"})
	if _, err := f.st.DB.Exec("UPDATE agents SET kind = 'worker' WHERE name = 'runner-worker/model'"); err != nil {
		t.Fatal(err)
	}
	res, err := Compute(f.st, "fixture", "", Thresholds{Now: fixtureNow})
	if err != nil {
		t.Fatal(err)
	}
	suggestions := map[string]Suggestion{}
	posts := map[string]*Post{}
	for _, sg := range res.Suggestions {
		suggestions[sg.ID] = sg
	}
	for i := range res.Posts {
		posts[res.Posts[i].ID] = &res.Posts[i]
	}
	for id, want := range map[string]string{unlanded: GateUnlanded, landed: ""} {
		if got := PlanGate(f.st, res.Repo, suggestions[id], posts[id]); got != want {
			t.Fatalf("PlanGate(%s) = %q, want %q (HEAD on pulse/* branch)", id, got, want)
		}
	}
}
