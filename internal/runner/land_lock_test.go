package runner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
)

// An index.lock nobody owns holds every landing of a checkout (ADR-024 D1 fast-forwards that checkout and defers
// otherwise). The landing sets an abandoned one aside - only under its own mutex, on a clean tree, past the
// mutex's own staleness, with no open holder and the same file rechecked - quarantines it beside the index, says
// so once in land.stale-lock, and lands. A young lock or one a process holds defers with a note that names the
// lock, before any merge is tried.
func TestLandingSetsAbandonedIndexLockAside(t *testing.T) {
	skipSlow(t)
	bus.ResetDefault()
	s := newStore(t)
	proj, dir := newBareRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")
	run(t, dir, "git", "checkout", "-q", "-b", "pulse/aurora/one", "main")
	os.WriteFile(filepath.Join(dir, "one.txt"), []byte("one\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "add one")
	run(t, dir, "git", "checkout", "-q", "main")
	j, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "task", Title: "t one", Brief: "b", Tier: 1, CreatedBy: "aurora"})
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?", testNow, "pulse/aurora/one", j.ID)
	lock := filepath.Join(dir, ".git", "index.lock")
	projects := []row{proj}

	// Fresh: some git may be mid-write. Deferred by name, nothing merged, the lock untouched.
	os.WriteFile(lock, nil, 0o644)
	lines, held := tendLandings(s, projects, "daemon", testNow, map[string]string{})
	if len(lines) != 1 || !strings.Contains(lines[0], "is mid-write (.git/index.lock is") || !strings.Contains(held["proj"], "mid-write") {
		t.Fatalf("fresh lock: lines=%q held=%v", lines, held)
	}
	if _, err := os.Stat(lock); err != nil {
		t.Fatalf("fresh lock was touched: %v", err)
	}

	// Old but open in a live process: still deferred, still untouched.
	old := time.Now().Add(-(indexLockMinutes + 1) * time.Minute)
	os.Chtimes(lock, old, old)
	holders := lockHolders
	lockHolders = func(string) ([]int, bool) { return []int{4242}, true }
	lines, held = tendLandings(s, projects, "daemon", testNow, map[string]string{})
	lockHolders = holders
	if len(lines) != 1 || !strings.Contains(lines[0], ".git/index.lock open in process 4242") {
		t.Fatalf("held lock: lines=%q held=%v", lines, held)
	}
	if _, err := os.Stat(lock); err != nil {
		t.Fatalf("held lock was touched: %v", err)
	}

	// Old and unowned: set aside, recorded, landed.
	lockHolders = func(string) ([]int, bool) { return nil, true }
	defer func() { lockHolders = holders }()
	lines, held = tendLandings(s, projects, "daemon", testNow, held)
	joined := strings.Join(lines, "\n")
	if len(held) != 0 || !strings.Contains(joined, "landing landed: pulse/aurora/one") {
		t.Fatalf("abandoned lock: lines=%q held=%v", lines, held)
	}
	if _, err := os.Stat(lock); !os.IsNotExist(err) {
		t.Fatalf("abandoned lock still there: %v", err)
	}
	aside, _ := filepath.Glob(filepath.Join(dir, ".git", "pulse-quarantine-index.lock-*"))
	if len(aside) != 1 {
		t.Fatalf("quarantined lock: %v", aside)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.stale-lock' AND subject = 'pulse/aurora/one'"); n != 1 {
		t.Fatalf("land.stale-lock events: %d", n)
	}
	ev := mustOne(s.DB, "SELECT detail FROM events WHERE action = 'land.stale-lock'").str("detail")
	if !strings.Contains(ev, filepath.Base(aside[0])) || !strings.Contains(strings.ReplaceAll(ev, " ", ""), `"size":0`) {
		t.Fatalf("land.stale-lock detail: %s", ev)
	}
	if head := strings.TrimSpace(run(t, dir, "git", "log", "-1", "--format=%s", "main")); head != "add one" {
		t.Fatalf("main after landing: %q", head)
	}
}

// A machine that cannot name a lock's holders leaves it alone and says so, instead of guessing from age.
func TestLandingLeavesIndexLockWhenHoldersUnknown(t *testing.T) {
	dir := t.TempDir()
	run(t, dir, "git", "init", "-q", "-b", "main", ".")
	lock := filepath.Join(dir, ".git", "index.lock")
	os.WriteFile(lock, nil, 0o644)
	old := time.Now().Add(-(indexLockMinutes + 1) * time.Minute)
	os.Chtimes(lock, old, old)
	holders := lockHolders
	lockHolders = func(string) ([]int, bool) { return nil, false }
	defer func() { lockHolders = holders }()
	note, moved := freeIndexLock(dir, time.Now())
	if moved != nil || !strings.Contains(note, "cannot say whose it is") || !strings.Contains(note, "remove it by hand") {
		t.Fatalf("unknown holders: note=%q moved=%v", note, moved)
	}
	if _, err := os.Stat(lock); err != nil {
		t.Fatalf("lock was touched: %v", err)
	}
	if note, moved := freeIndexLock(t.TempDir(), time.Now()); note != "" || moved != nil {
		t.Fatalf("no lock: note=%q moved=%v", note, moved)
	}
	if !lockedCheckout("fatal: Unable to create '/x/.git/index.lock': File exists.") || lockedCheckout("fatal: Not possible to fast-forward, aborting.") {
		t.Fatal("lockedCheckout misreads git's stderr")
	}
}
