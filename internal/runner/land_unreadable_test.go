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

// A checkout git cannot read (a folder macOS has not let the daemon open, a path that is gone, a broken
// repository) is not the same as a branch that does not exist: the job keeps its branch, no land.missing
// event is written, and the project's other landings wait for the same reason.
func TestLandingKeepsBranchWhenCheckoutUnreadable(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	proj, dir := newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")

	exists, why := branchExists(dir, "nope")
	if exists || why != "" {
		t.Fatalf("absent branch in a readable repo: exists=%v why=%q", exists, why)
	}
	run(t, dir, "git", "branch", "pulse/delta/x")
	if exists, why = branchExists(dir, "pulse/delta/x"); !exists || why != "" {
		t.Fatalf("present branch: exists=%v why=%q", exists, why)
	}
	notRepo := t.TempDir()
	if exists, why = branchExists(notRepo, "pulse/delta/x"); exists || !strings.Contains(why, "cannot read the repository") {
		t.Fatalf("not a repository: exists=%v why=%q", exists, why)
	}
	if exists, why = branchExists(filepath.Join(notRepo, "gone"), "pulse/delta/x"); exists || !strings.Contains(why, "cannot read") {
		t.Fatalf("missing path: exists=%v why=%q", exists, why)
	}

	for _, kind := range []string{"task", "experiment"} {
		j, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: kind, Title: "t " + kind, Brief: "Suggestion: 20260101T000000Z-x\nHypothesis: h\n",
			Tier: 1, CreatedBy: "aurora", Branch: "pulse/delta/" + kind})
		if err != nil {
			t.Fatal(err)
		}
		exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?", testNow, "pulse/delta/"+kind, j.ID)
	}
	// The checkout disappears (or macOS refuses it): both landings are held, nothing is forgotten.
	if err := os.Rename(dir, dir+".away"); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Rename(dir+".away", dir) })
	res := landPending(s, proj, "daemon", testNow, true, 0, false)
	if len(res) != 1 || res[0].State != "unreadable" || !strings.Contains(res[0].Note, "kept: cannot read") {
		t.Fatalf("unreadable checkout: %+v", res)
	}
	for _, kind := range []string{"task", "experiment"} {
		if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE project = 'proj' AND kind = ? AND status = 'done' AND branch = ?", kind, "pulse/delta/"+kind); n != 1 {
			t.Fatalf("%s job lost its branch", kind)
		}
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.missing'"); n != 0 {
		t.Fatalf("land.missing written for an unreadable checkout: %d", n)
	}
	// Back in place, a branch that truly does not exist is still reported and cleared.
	if err := os.Rename(dir+".away", dir); err != nil {
		t.Fatal(err)
	}
	res = landPending(s, proj, "daemon", testNow, true, 0, false)
	states := []string{}
	for _, r := range res {
		states = append(states, r.State)
	}
	if len(res) != 2 || (states[0] != "missing" && states[1] != "missing") {
		t.Fatalf("readable again: %+v", res)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE project = 'proj' AND status = 'done' AND branch != ''"); n != 0 {
		t.Fatalf("missing branches not cleared: %d", n)
	}
}

func TestProtectedFoldersNote(t *testing.T) {
	s := newStore(t)
	home := t.TempDir()
	for _, p := range []struct{ slug, path string }{{"desk", filepath.Join(home, "Desktop", "work", "desk")}, {"docs", filepath.Join(home, "Documents")}, {"src", filepath.Join(home, "src", "Desktop")}} {
		if err := os.MkdirAll(p.path, 0o755); err != nil {
			t.Fatal(err)
		}
		run(t, p.path, "git", "init", "-q", "-b", "main")
		if _, _, err := addProject(s, p.path, p.slug, "", false, "", humanAgent, p.slug); err != nil {
			t.Fatal(err)
		}
	}
	note := protectedFoldersNote(s, "darwin", home)
	if !strings.Contains(note, "desk (") || !strings.Contains(note, "docs (") || strings.Contains(note, "src (") || !strings.Contains(note, "answer Allow") {
		t.Fatalf("note:\n%s", note)
	}
	if protectedFoldersNote(s, "linux", home) != "" || protectedFoldersNote(nil, "darwin", home) != "" {
		t.Fatal("note outside macOS or without a store")
	}
}

// A project whose checkout does not answer is held for the loop: no landing verdict, no lease against it,
// its queued job untouched; a readable project in the same store is still served.
func TestDaemonHoldsUnreadableCheckout(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	_, dir := newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")
	setSetting(t, s, "jobs_per_day", "20")
	j, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "task", Title: "held", Brief: "b", Tier: 1, CreatedBy: "aurora"})
	if err != nil {
		t.Fatal(err)
	}
	probes := &checkoutProbes{}
	okRows, held, lines := probes.ready(projectRows(s.DB, false), time.Second)
	if len(okRows) != 1 || len(held) != 0 || len(lines) != 0 {
		t.Fatalf("readable: ok=%d held=%v lines=%v", len(okRows), held, lines)
	}
	if err := os.Rename(dir, dir+".away"); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Rename(dir+".away", dir) })
	okRows, held, lines = probes.ready(projectRows(s.DB, false), time.Second)
	if len(okRows) != 0 || !strings.Contains(held["proj"], "cannot read") || len(lines) != 1 || !strings.HasPrefix(lines[0], "holding proj this loop: cannot read") {
		t.Fatalf("gone: ok=%d held=%v lines=%v", len(okRows), held, lines)
	}
	if _, _, l := probes.ready(projectRows(s.DB, false), time.Second); len(l) != 0 {
		t.Fatalf("same reason noted twice: %v", l)
	}
	if job, _, why, err := takeJob(s, "aurora", "", "", false, testNow, localExecutor, 0); err != nil || job != nil || !strings.Contains(why, "cannot read the checkout for proj") || !strings.Contains(why, "permission dialog") {
		t.Fatalf("takeJob while held: job=%v why=%q err=%v", job, why, err)
	}
	holdCheckouts(nil)

	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: ran'"})
	if !strings.Contains(log, "holding proj this loop: cannot read") || !strings.Contains(log, "cannot read the checkout for proj") || strings.Contains(log, "landing") {
		t.Fatalf("daemon log:\n%s", log)
	}
	if r := jobRow(s.DB, j.ID); r.str("status") != "queued" || r.int("attempts") != 0 {
		t.Fatalf("held job touched: %v", r)
	}
	if len(heldCheckouts()) != 0 {
		t.Fatal("hold not cleared when the daemon left")
	}
	// The checkout is back: the same job runs on the next loop.
	if err := os.Rename(dir+".away", dir); err != nil {
		t.Fatal(err)
	}
	log = onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: ran'"})
	if strings.Contains(log, "holding proj") || !strings.Contains(log, j.ID) {
		t.Fatalf("daemon log after return:\n%s", log)
	}
	if r := jobRow(s.DB, j.ID); r.str("status") != "done" {
		t.Fatalf("job after return: %v", r)
	}
}
