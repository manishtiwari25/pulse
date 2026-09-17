package runner

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
)

// A checkout with uncommitted changes holds every landing of its project (ADR-024). The daemon says so once
// - one log line, one land.held event, one meta.landing note for pulse status and the hub - and not again
// on the next tick while nothing changed; when the checkout is clean again the branches land, the note is
// cleared and land.resumed is written once.
func TestLandingHoldSaidOncePerChange(t *testing.T) {
	skipSlow(t)
	bus.ResetDefault()
	s := newStore(t)
	proj, dir := newBareRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")
	setSetting(t, s, "landing.auto_stash", "off")

	// Two verified branches wait; the owner has uncommitted work in the checkout.
	for _, name := range []string{"one", "two"} {
		run(t, dir, "git", "checkout", "-q", "-b", "pulse/aurora/"+name, "main")
		os.WriteFile(filepath.Join(dir, name+".txt"), []byte(name+"\n"), 0o644)
		run(t, dir, "git", "add", "-A")
		run(t, dir, "git", "commit", "-q", "-m", "add "+name)
		run(t, dir, "git", "checkout", "-q", "main")
		j, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "task", Title: "t " + name, Brief: "b", Tier: 1, CreatedBy: "aurora"})
		if err != nil {
			t.Fatal(err)
		}
		exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?", testNow, "pulse/aurora/"+name, j.ID)
	}
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# proj\n\nowner's work in progress\n"), 0o644)

	projects := []row{proj}
	lines, held := tendLandings(s, projects, "daemon", testNow, map[string]string{})
	want := "landing held proj: 2 pending landing jobs; deferred: the checkout of " + dir + " has uncommitted changes"
	if len(lines) != 1 || lines[0] != want {
		t.Fatalf("first tick lines: %q", lines)
	}
	if held["proj"] != strings.TrimPrefix(want, "landing held proj: ") {
		t.Fatalf("held: %v", held)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.held' AND subject = 'proj'"); n != 1 {
		t.Fatalf("land.held events after the first tick: %d", n)
	}
	var stored map[string]struct{ At, Note string }
	raw, ok, err := s.Meta("landing")
	if err != nil || !ok || json.Unmarshal([]byte(raw), &stored) != nil || stored["proj"].Note != held["proj"] || stored["proj"].At != testNow {
		t.Fatalf("meta.landing: ok=%v err=%v %q", ok, err, raw)
	}

	// The same hold on the next tick: nothing new is said or written, the note stays.
	lines, held2 := tendLandings(s, projects, "daemon", testNow, held)
	if len(lines) != 0 || held2["proj"] != held["proj"] {
		t.Fatalf("second tick: lines=%q held=%v", lines, held2)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action LIKE 'land.%'"); n != 1 {
		t.Fatalf("events after the second tick: %d", n)
	}

	// A project whose checkout did not answer this tick keeps its note; nothing is said about it.
	lines, held3 := tendLandings(s, nil, "daemon", testNow, held2)
	if len(lines) != 0 || held3["proj"] != held["proj"] {
		t.Fatalf("unswept tick: lines=%q held=%v", lines, held3)
	}

	// Committed: both branches land, the hold clears once, and status shows no hold.
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "owner's work")
	lines, held4 := tendLandings(s, projects, "daemon", testNow, held3)
	joined := strings.Join(lines, "\n")
	if len(held4) != 0 || len(lines) != 3 || !strings.Contains(joined, "landing landed: pulse/aurora/one") ||
		!strings.Contains(joined, "landing landed: pulse/aurora/two") || lines[2] != "landing resumed proj" {
		t.Fatalf("after the commit: lines=%q held=%v", lines, held4)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.resumed' AND subject = 'proj'"); n != 1 {
		t.Fatalf("land.resumed events: %d", n)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE project = 'proj' AND status = 'done' AND branch != ''"); n != 0 {
		t.Fatalf("branches still waiting: %d", n)
	}
	raw, _, _ = s.Meta("landing")
	if raw != "{}" {
		t.Fatalf("meta.landing after the hold cleared: %q", raw)
	}
	// Quiet again: nothing waits, nothing is held, nothing is said.
	if lines, held5 := tendLandings(s, projects, "daemon", testNow, held4); len(lines) != 0 || len(held5) != 0 {
		t.Fatalf("quiet tick: lines=%q held=%v", lines, held5)
	}
}

// The daemon loop carries the hold between ticks and logs it through tendLandings: a dirty checkout is one
// "landing held" line in the log, not a "landing deferred" line per waiting branch per tick.
func TestDaemonLogsLandingHoldOnce(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	_, dir := newBareRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")
	setSetting(t, s, "landing.auto_stash", "off")
	setSetting(t, s, "jobs_per_day", "20")
	run(t, dir, "git", "checkout", "-q", "-b", "pulse/aurora/x", "main")
	os.WriteFile(filepath.Join(dir, "x.txt"), []byte("x\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "add x")
	run(t, dir, "git", "checkout", "-q", "main")
	j, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "task", Title: "x", Brief: "b", Tier: 1, CreatedBy: "aurora"})
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ?, branch = 'pulse/aurora/x' WHERE id = ?", testNow, j.ID)
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# proj\n\nedited\n"), 0o644)

	log := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: ran'"})
	if strings.Count(log, "landing held proj: 1 pending landing job; deferred: the checkout of "+dir+" has uncommitted changes") != 1 || strings.Contains(log, "landing deferred") {
		t.Fatalf("daemon log:\n%s", log)
	}
	if r := jobRow(s.DB, j.ID); r.str("branch") != "pulse/aurora/x" {
		t.Fatalf("held job lost its branch: %v", r)
	}
}

// With landing.auto_stash on (the default), a dirty checkout is stashed before
// landing so the verified branch can land. The stash keeps the owner's changes
// with a clear message and the landing proceeds.
func TestLandingAutoStashLands(t *testing.T) {
	skipSlow(t)
	bus.ResetDefault()
	s := newStore(t)
	proj, dir := newBareRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")

	branch := "pulse/aurora/auto"
	run(t, dir, "git", "checkout", "-q", "-b", branch, "main")
	os.WriteFile(filepath.Join(dir, "feature.txt"), []byte("feature\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "add feature")
	run(t, dir, "git", "checkout", "-q", "main")

	j, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "task", Title: "auto stash", Brief: "b", Tier: 1, CreatedBy: "aurora"})
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?", testNow, branch, j.ID)

	// Owner's uncommitted work on main.
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# proj\n\nowner's work\n"), 0o644)

	projects := []row{proj}
	lines, held := tendLandings(s, projects, "daemon", testNow, map[string]string{})
	joined := strings.Join(lines, "\n")
	if len(held) != 0 || !strings.Contains(joined, "landing landed: "+branch) {
		t.Fatalf("expected landing to succeed; lines=%q held=%v", lines, held)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed' AND subject = ?", branch); n != 1 {
		t.Fatalf("land.landed events: %d", n)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE id = ? AND status = 'done' AND branch = ''", j.ID); n != 1 {
		t.Fatalf("job not finished: %v", jobRow(s.DB, j.ID))
	}

	// The owner's changes were stashed, not committed or lost.
	stashList := run(t, dir, "git", "stash", "list")
	if !strings.Contains(stashList, "pulse auto-stash before landing "+j.ID) {
		t.Fatalf("stash list missing expected message: %q", stashList)
	}
	status := run(t, dir, "git", "status", "--porcelain", "--untracked-files=no")
	if strings.TrimSpace(status) != "" {
		t.Fatalf("checkout not clean after landing: %q", status)
	}
}

// With landing.auto_stash off, a dirty checkout defers landing exactly as it
// did before the self-healing behavior existed.
func TestLandingAutoStashOffDefers(t *testing.T) {
	skipSlow(t)
	bus.ResetDefault()
	s := newStore(t)
	proj, dir := newBareRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")
	setSetting(t, s, "landing.auto_stash", "off")

	branch := "pulse/aurora/manual"
	run(t, dir, "git", "checkout", "-q", "-b", branch, "main")
	os.WriteFile(filepath.Join(dir, "feature.txt"), []byte("feature\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "add feature")
	run(t, dir, "git", "checkout", "-q", "main")

	j, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "task", Title: "manual stash", Brief: "b", Tier: 1, CreatedBy: "aurora"})
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?", testNow, branch, j.ID)
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# proj\n\nowner's work\n"), 0o644)

	projects := []row{proj}
	lines, held := tendLandings(s, projects, "daemon", testNow, map[string]string{})
	want := "landing held proj: 1 pending landing job; deferred: the checkout of " + dir + " has uncommitted changes"
	if len(lines) != 1 || lines[0] != want {
		t.Fatalf("expected deferral; lines=%q held=%v", lines, held)
	}
	if r := jobRow(s.DB, j.ID); r.str("branch") != branch {
		t.Fatalf("deferred job lost its branch: %v", r)
	}
	if stashList := run(t, dir, "git", "stash", "list"); strings.TrimSpace(stashList) != "" {
		t.Fatalf("unexpected stash created: %q", stashList)
	}
}

// When auto-stash is on but git cannot stash the working tree (e.g. an
// unresolved merge), the landing is blocked with a clear reason instead of
// silently deferred.
func TestLandingAutoStashFailedBlocked(t *testing.T) {
	skipSlow(t)
	bus.ResetDefault()
	s := newStore(t)
	proj, dir := newBareRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")

	branch := "pulse/aurora/conflict"
	run(t, dir, "git", "checkout", "-q", "-b", branch, "main")
	os.WriteFile(filepath.Join(dir, "feature.txt"), []byte("feature\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "add feature")
	run(t, dir, "git", "checkout", "-q", "main")

	j, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "task", Title: "stash fails", Brief: "b", Tier: 1, CreatedBy: "aurora"})
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?", testNow, branch, j.ID)

	// Create an unresolved merge on main so git stash cannot save the state.
	run(t, dir, "git", "checkout", "-q", "-b", "side", "main")
	os.WriteFile(filepath.Join(dir, "a.txt"), []byte("side\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "side commit")
	run(t, dir, "git", "checkout", "-q", "main")
	os.WriteFile(filepath.Join(dir, "a.txt"), []byte("main\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "main commit")
	// The merge is expected to conflict; do not use run() because it fails on non-zero exit.
	exec.Command("git", "-C", dir, "merge", "side").CombinedOutput()
	// The merge is left conflicted.

	projects := []row{proj}
	lines, held := tendLandings(s, projects, "daemon", testNow, map[string]string{})
	if len(lines) != 1 || !strings.Contains(lines[0], "blocked") || !strings.Contains(lines[0], "could not be auto-stashed") {
		t.Fatalf("expected blocked with auto-stash reason; lines=%q held=%v", lines, held)
	}
	if !strings.Contains(held["proj"], "blocked") {
		t.Fatalf("expected hold to say blocked: %v", held)
	}
	if r := jobRow(s.DB, j.ID); r.str("branch") != branch {
		t.Fatalf("blocked job lost its branch: %v", r)
	}
}
