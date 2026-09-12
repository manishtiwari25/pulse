package runner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
)

// pushFixture is a project with a bare remote as origin and main tracking origin/main, plus one done job
// on a branch with a real commit waiting to land.
func pushFixture(t *testing.T, s *store.Store) (proj row, repo, bare string) {
	t.Helper()
	proj, repo = newRepo(t, s)
	bare = filepath.Join(t.TempDir(), "origin.git")
	run(t, "", "git", "init", "--bare", "-q", bare)
	run(t, repo, "git", "remote", "add", "origin", bare)
	run(t, repo, "git", "push", "-q", "-u", "origin", "main")
	return proj, repo, bare
}

func pushBranch(t *testing.T, s *store.Store, proj row, repo, name string) {
	t.Helper()
	join(t, s, "worker")
	run(t, repo, "git", "checkout", "-q", "-b", name, "main")
	if err := os.WriteFile(filepath.Join(repo, filepath.Base(name)+".txt"), []byte(name+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(t, repo, "git", "add", "-A")
	run(t, repo, "git", "commit", "-q", "-m", "work on "+name)
	run(t, repo, "git", "checkout", "-q", "main")
	j, _, err := board.AddJob(s, board.JobInput{Project: proj.str("slug"), Kind: "task", Title: "t " + name, Brief: "b", Tier: 1, CreatedBy: "worker"})
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, `UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?`, testNow, name, j.ID)
}

func refOf(t *testing.T, dir, ref string) string {
	t.Helper()
	return strings.TrimSpace(run(t, dir, "git", "rev-parse", ref))
}

// bareRef reads a ref of a bare repository by --git-dir (safe.bareRepository may forbid discovery).
func bareRef(t *testing.T, bare, ref string) string {
	t.Helper()
	return strings.TrimSpace(run(t, "", "git", "--git-dir", bare, "rev-parse", ref))
}

func TestLandingPushesMainWhenAsked(t *testing.T) {
	skipSlow(t)
	bus.ResetDefault()
	s := newStore(t)
	proj, repo, bare := pushFixture(t, s)
	pushBranch(t, s, proj, repo, "pulse/worker/1")
	before := bareRef(t, bare, "refs/heads/main")

	// Off by default: the landing moves local main, the remote does not move, and nothing says otherwise.
	lines, cur := tendLandings(s, []row{proj}, "daemon", testNow, nil)
	if len(lines) != 1 || !strings.HasPrefix(lines[0], "landing landed") {
		t.Fatalf("lines %q", lines)
	}
	if len(cur) != 0 {
		t.Fatalf("hold notes with push off: %v", cur)
	}
	if bareRef(t, bare, "refs/heads/main") != before {
		t.Fatal("remote moved with landing.push off")
	}
	if pushOwed(s, "proj") != true {
		t.Fatal("a landing with push off is still owed once the owner turns it on (D6)")
	}

	// Turning it on pushes what is owed on the next tick, with no new landing.
	setSetting(t, s, "landing.push", "main")
	lines, cur = tendLandings(s, []row{proj}, "daemon", testNow, nil)
	if len(lines) != 1 || !strings.HasPrefix(lines[0], "landing pushed proj: main -> origin/main (1 commit, ") {
		t.Fatalf("lines %q", lines)
	}
	if len(cur) != 0 {
		t.Fatalf("hold notes after a good push: %v", cur)
	}
	if got, want := bareRef(t, bare, "refs/heads/main"), refOf(t, repo, "refs/heads/main"); got != want {
		t.Fatalf("remote main %s, local %s", got, want)
	}
	if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = 'land.pushed' AND subject = 'proj'`); n != 1 {
		t.Fatalf("land.pushed events %d", n)
	}
	if pushOwed(s, "proj") {
		t.Fatal("still owed after the push")
	}

	// Nothing owed, nothing landed: quiet.
	lines, cur = tendLandings(s, []row{proj}, "daemon", testNow, nil)
	if len(lines) != 0 || len(cur) != 0 {
		t.Fatalf("quiet tick said %q %v", lines, cur)
	}

	// A landing with the setting on pushes in the same tick.
	pushBranch(t, s, proj, repo, "pulse/worker/2")
	lines, _ = tendLandings(s, []row{proj}, "daemon", testNow, nil)
	if len(lines) != 2 || !strings.HasPrefix(lines[0], "landing landed") || !strings.HasPrefix(lines[1], "landing pushed proj:") {
		t.Fatalf("lines %q", lines)
	}
	if got, want := bareRef(t, bare, "refs/heads/main"), refOf(t, repo, "refs/heads/main"); got != want {
		t.Fatalf("remote main %s, local %s", got, want)
	}
}

func TestLandingPushFailureIsANoteNotAnUndo(t *testing.T) {
	skipSlow(t)
	bus.ResetDefault()
	s := newStore(t)
	proj, repo, bare := pushFixture(t, s)
	setSetting(t, s, "landing.push", "main")

	// Someone else moves the remote: the push is rejected (never forced), the landing stays, the note is
	// said once and kept for status and the hub, and the push is retried each tick while owed.
	other := filepath.Join(t.TempDir(), "other")
	run(t, "", "git", "clone", "-q", bare, other)
	if err := os.WriteFile(filepath.Join(other, "other.txt"), []byte("other\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(t, other, "git", "add", "-A")
	run(t, other, "git", "commit", "-q", "-m", "someone else")
	run(t, other, "git", "push", "-q", "origin", "main")
	remoteHead := bareRef(t, bare, "refs/heads/main")

	pushBranch(t, s, proj, repo, "pulse/worker/1")
	lines, cur := tendLandings(s, []row{proj}, "daemon", testNow, nil)
	if len(lines) != 2 || !strings.HasPrefix(lines[0], "landing landed") || !strings.HasPrefix(lines[1], "landing held proj: 1 commit landed but not pushed; push failed: ") {
		t.Fatalf("lines %q", lines)
	}
	if !strings.Contains(cur["proj"], "push failed") {
		t.Fatalf("note %q", cur["proj"])
	}
	if bareRef(t, bare, "refs/heads/main") != remoteHead {
		t.Fatal("remote moved on a rejected push")
	}
	if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = 'land.held' AND json_extract(detail, '$.state') = 'push-failed'`); n != 1 {
		t.Fatalf("land.held push-failed events %d", n)
	}
	if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed'`); n != 1 {
		t.Fatalf("landing undone: land.landed events %d", n)
	}

	// Same failure next tick: retried, not said again.
	lines, cur2 := tendLandings(s, []row{proj}, "daemon", testNow, cur)
	if len(lines) != 0 || cur2["proj"] != cur["proj"] {
		t.Fatalf("repeat tick said %q, note %q -> %q", lines, cur["proj"], cur2["proj"])
	}
	if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = 'land.held'`); n != 1 {
		t.Fatalf("land.held events %d", n)
	}

	// The owner reconciles; the next tick pushes what was owed and the hold clears.
	run(t, repo, "git", "pull", "-q", "--no-rebase", "--no-edit", "origin", "main")
	lines, cur3 := tendLandings(s, []row{proj}, "daemon", testNow, cur2)
	if len(lines) != 2 || !strings.HasPrefix(lines[0], "landing pushed proj: main -> origin/main (") || lines[1] != "landing resumed proj" {
		t.Fatalf("lines %q", lines)
	}
	if len(cur3) != 0 {
		t.Fatalf("notes after recovery: %v", cur3)
	}
	if got, want := bareRef(t, bare, "refs/heads/main"), refOf(t, repo, "refs/heads/main"); got != want {
		t.Fatalf("remote main %s, local %s", got, want)
	}
	if pushOwed(s, "proj") {
		t.Fatal("still owed after the push")
	}
}

func TestLandingPushSkipsWithoutUpstream(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	proj, repo := newRepo(t, s)
	setSetting(t, s, "landing.push", "main")
	pushBranch(t, s, proj, repo, "pulse/worker/1")
	lines, cur := tendLandings(s, []row{proj}, "daemon", testNow, nil)
	if len(lines) != 2 || !strings.HasPrefix(lines[0], "landing landed") || lines[1] != "landing held proj: landed but not pushed; push skipped: main has no upstream (git branch --set-upstream-to=origin/main main)" {
		t.Fatalf("lines %q", lines)
	}
	if !strings.Contains(cur["proj"], "no upstream") {
		t.Fatalf("note %q", cur["proj"])
	}
	if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = 'land.pushed'`); n != 0 {
		t.Fatalf("land.pushed events %d", n)
	}
}

func TestLandByHandPushesWhenAsked(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	proj, repo, bare := pushFixture(t, s)
	setSetting(t, s, "landing.push", "main")
	pushBranch(t, s, proj, repo, "pulse/worker/1")
	out := invokeOK(t, s, "land", "proj")
	if !strings.Contains(out, "proj landed:") || !strings.Contains(out, "landing pushed proj: main -> origin/main (1 commit, ") {
		t.Fatalf("out %q", out)
	}
	if got, want := bareRef(t, bare, "refs/heads/main"), refOf(t, repo, "refs/heads/main"); got != want {
		t.Fatalf("remote main %s, local %s", got, want)
	}
}
