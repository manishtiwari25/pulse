package runner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// -- ADR-035: the daemon rebuilds and installs itself after a landing in the brain ---------------------------

// fakeGo is a `go` stand-in on PATH: `go build ... -o OUT ./cmd/pulse` writes OUT as a script that
// answers `version` with the checkout's revision (or a wrong one under FAKE_GO_BADVERSION) and `help`
// with exit 0; FAKE_GO_FAIL makes the build fail.
const fakeGo = `#!/bin/sh
[ -n "$FAKE_GO_FAIL" ] && { echo "# github.com/x/pulse"; echo "./cmd/pulse/main.go:9:2: undefined: nope"; exit 1; }
out=""
while [ $# -gt 0 ]; do [ "$1" = "-o" ] && out=$2; shift; done
rev=$(git rev-parse HEAD | cut -c1-12)
[ -n "$FAKE_GO_BADVERSION" ] && rev=cafebabe0000
printf '#!/bin/sh\ncase "$1" in version) echo dev-%s;; help) exit 0;; *) exit 1;; esac\n' "$rev" > "$out"
chmod +x "$out"
`

func resetSelfUpdate() {
	selfUpdateState.Lock()
	defer selfUpdateState.Unlock()
	selfUpdateState.inflight = false
	selfUpdateState.lines = nil
	selfUpdateState.lastFailureAt = ""
	selfUpdateState.lastSkip = ""
	selfUpdateState.installed = ""
}

func waitSelfUpdateIdle(t *testing.T) {
	t.Helper()
	deadline := time.Now().Add(20 * time.Second)
	for time.Now().Before(deadline) {
		selfUpdateState.Lock()
		busy := selfUpdateState.inflight
		selfUpdateState.Unlock()
		if !busy {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("the build did not finish")
}

// selfUpdateFixture: a brain checkout on main, a fake go first on PATH, an "old" binary to replace.
func selfUpdateFixture(t *testing.T) (*store.Store, string, selfUpdateEnv) {
	t.Helper()
	resetSelfUpdate()
	t.Cleanup(resetSelfUpdate)
	// Signing is macOS-only (codesign); these tests are about the build.
	signingSystem = "linux"
	t.Cleanup(func() { signingSystem = "" })
	s := newStore(t)
	_, repo := newBareRepo(t, s)
	exec1(s.DB, "UPDATE projects SET brain = 1 WHERE slug = 'proj'")
	tools := t.TempDir()
	if err := os.WriteFile(filepath.Join(tools, "go"), []byte(fakeGo), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", tools+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("FAKE_GO_FAIL", "")
	t.Setenv("FAKE_GO_BADVERSION", "")
	bin := filepath.Join(t.TempDir(), "pulse")
	if err := os.WriteFile(bin, []byte("old binary"), 0o755); err != nil {
		t.Fatal(err)
	}
	setSetting(t, s, "daemon.self_update", "on")
	return s, repo, selfUpdateEnv{Binary: bin, GoCmd: "go", Timeout: 10 * time.Second, Revision: "0000000000000000000000000000000000000000", Actor: "daemon", Inflight: 2}
}

func headOf(t *testing.T, repo string) string {
	t.Helper()
	return strings.TrimSpace(run(t, repo, "git", "rev-parse", "HEAD"))
}

func TestSelfUpdateBuildsSmokesInstallsAndReports(t *testing.T) {
	s, repo, env := selfUpdateFixture(t)
	rev := headOf(t, repo)
	lines := tendBinary(s, testNow, env)
	if len(lines) != 1 || !strings.Contains(lines[0], "brain HEAD "+rev[:12]+" differs from the running 000000000000; building") {
		t.Fatalf("first tick: %v", lines)
	}
	if lines := tendBinary(s, testNow, env); len(lines) != 0 {
		t.Fatalf("a second tick during the build said %v", lines)
	}
	waitSelfUpdateIdle(t)
	lines = tendBinary(s, testNow, env)
	if len(lines) != 1 || lines[0] != "self-update: pulse rebuilt from "+rev[:12]+": restarts after 2 run(s) in flight" {
		t.Fatalf("after the build: %v", lines)
	}
	if data, _ := os.ReadFile(env.Binary + ".prev"); string(data) != "old binary" {
		t.Fatalf("pulse.prev = %q", data)
	}
	if data, _ := os.ReadFile(env.Binary); !strings.Contains(string(data), "dev-"+rev[:12]) {
		t.Fatalf("pulse was not replaced by the build:\n%s", data)
	}
	if fileExists(env.Binary + ".new") {
		t.Fatal("pulse.new left behind")
	}
	if n := count(t, s, "SELECT count(*) AS n FROM events WHERE action = ? AND detail LIKE ?", selfUpdateEvent, "%"+rev+"%"); n != 1 {
		t.Fatalf("self_update events = %d", n)
	}
	post, err := queryOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND area = ? AND thread = ? ORDER BY id DESC LIMIT 1", selfUpdateArea, selfUpdateThread)
	if err != nil || post == nil || post.str("project") != "proj" || !strings.Contains(post.str("summary"), "pulse rebuilt from "+rev[:12]) || !strings.Contains(post.str("body"), env.Binary+".prev") {
		t.Fatalf("finding: %v %v", post, err)
	}
	// The installed revision waits for the restart: no second build of the same HEAD.
	if lines := tendBinary(s, testNow, env); len(lines) != 0 || fileExists(env.Binary+".new") {
		t.Fatalf("rebuilt the installed revision: %v", lines)
	}
}

func TestSelfUpdateWaitsOutEveryOtherStateAndSaysSoOnce(t *testing.T) {
	s, repo, env := selfUpdateFixture(t)
	rev := headOf(t, repo)
	// Same revision: nothing to do, nothing said.
	same := env
	same.Revision = rev
	if lines := tendBinary(s, testNow, same); len(lines) != 0 {
		t.Fatalf("same revision: %v", lines)
	}
	// Dirty checkout: one line, then silence while the state lasts.
	os.WriteFile(filepath.Join(repo, "scratch.txt"), []byte("x"), 0o644)
	lines := tendBinary(s, testNow, env)
	if len(lines) != 1 || !strings.Contains(lines[0], "brain checkout is dirty") {
		t.Fatalf("dirty: %v", lines)
	}
	if lines := tendBinary(s, testNow, env); len(lines) != 0 {
		t.Fatalf("dirty again: %v", lines)
	}
	os.Remove(filepath.Join(repo, "scratch.txt"))
	// Another branch.
	run(t, repo, "git", "checkout", "-q", "-b", "feature")
	lines = tendBinary(s, testNow, env)
	if len(lines) != 1 || !strings.Contains(lines[0], "brain checkout is on feature, not main; waiting") {
		t.Fatalf("branch: %v", lines)
	}
	run(t, repo, "git", "checkout", "-q", "main")
	// Stop set.
	setSetting(t, s, "stop", "1")
	lines = tendBinary(s, testNow, env)
	if len(lines) != 1 || !strings.Contains(lines[0], "stop is set; no build") {
		t.Fatalf("stop: %v", lines)
	}
	setSetting(t, s, "stop", "0")
	// Setting off: silent.
	setSetting(t, s, "daemon.self_update", "off")
	if lines := tendBinary(s, testNow, env); len(lines) != 0 {
		t.Fatalf("off: %v", lines)
	}
	setSetting(t, s, "daemon.self_update", "on")
	// No go on PATH.
	noGo := env
	noGo.GoCmd = "go-that-does-not-exist"
	lines = tendBinary(s, testNow, noGo)
	if len(lines) != 1 || !strings.Contains(lines[0], "go-that-does-not-exist is not on the daemon's PATH") {
		t.Fatalf("no go: %v", lines)
	}
	// An unreadable checkout names a reason even when git said nothing.
	exec1(s.DB, "UPDATE projects SET path = ? WHERE slug = 'proj'", filepath.Join(t.TempDir(), "gone"))
	lines = tendBinary(s, testNow, env)
	if len(lines) != 1 || !strings.Contains(lines[0], "is not a readable git checkout: ") || strings.HasSuffix(strings.TrimSpace(lines[0]), ":") {
		t.Fatalf("unreadable checkout: %v", lines)
	}
	if got := gitReason(gitResult{Code: -1}); !strings.Contains(got, "killed before it answered") {
		t.Fatalf("a killed git needs a reason: %q", got)
	}
	if got := gitReason(gitResult{Code: 128}); got != "git exited 128 without a message" {
		t.Fatalf("a silent exit needs a reason: %q", got)
	}
	exec1(s.DB, "UPDATE projects SET path = ? WHERE slug = 'proj'", repo)
	if fileExists(env.Binary+".new") || fileExists(env.Binary+".prev") {
		t.Fatal("something was built while waiting")
	}
	if data, _ := os.ReadFile(env.Binary); string(data) != "old binary" {
		t.Fatal("pulse was touched while waiting")
	}
}

func TestSelfUpdateFailureLeavesPulseAloneAndRetriesLater(t *testing.T) {
	s, repo, env := selfUpdateFixture(t)
	rev := headOf(t, repo)
	t.Setenv("FAKE_GO_FAIL", "1")
	if lines := tendBinary(s, testNow, env); len(lines) != 1 || !strings.Contains(lines[0], "building") {
		t.Fatalf("start: %v", lines)
	}
	waitSelfUpdateIdle(t)
	lines := tendBinary(s, testNow, env)
	if len(lines) != 1 || !strings.Contains(lines[0], "self-update: build of "+rev[:12]+" failed") {
		t.Fatalf("after the failed build: %v", lines)
	}
	if data, _ := os.ReadFile(env.Binary); string(data) != "old binary" || fileExists(env.Binary+".prev") {
		t.Fatal("pulse was touched by a failed build")
	}
	post, err := queryOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1", selfUpdateThread)
	if err != nil || post == nil || !strings.Contains(post.str("summary"), "build of "+rev[:12]+" failed; the running binary is untouched") || !strings.Contains(post.str("body"), "undefined: nope") {
		t.Fatalf("failure finding: %v %v", post, err)
	}
	// Within the retry window nothing starts; after it the build runs again and, passing now, installs.
	t.Setenv("FAKE_GO_FAIL", "")
	if lines := tendBinary(s, plusMinutes(testNow, 30), env); len(lines) != 0 {
		t.Fatalf("retried early: %v", lines)
	}
	setSetting(t, s, "daemon.self_update_retry_minutes", "15")
	if lines := tendBinary(s, plusMinutes(testNow, 30), env); len(lines) != 1 || !strings.Contains(lines[0], "building") {
		t.Fatalf("retry after the window: %v", lines)
	}
	waitSelfUpdateIdle(t)
	if lines := tendBinary(s, plusMinutes(testNow, 31), env); len(lines) != 1 || !strings.Contains(lines[0], "pulse rebuilt from "+rev[:12]) {
		t.Fatalf("retry outcome: %v", lines)
	}
	// A smoke failure (wrong revision printed) keeps pulse.new for the reader and pulse untouched.
	resetSelfUpdate()
	run(t, repo, "git", "commit", "-q", "--allow-empty", "-m", "next")
	next := headOf(t, repo)
	t.Setenv("FAKE_GO_BADVERSION", "1")
	tendBinary(s, testNow, env)
	waitSelfUpdateIdle(t)
	lines = tendBinary(s, testNow, env)
	if len(lines) != 1 || !strings.Contains(lines[0], "smoke of "+next[:12]+" failed") || !strings.Contains(lines[0], "not the revision "+next[:12]) {
		t.Fatalf("smoke: %v", lines)
	}
	if !fileExists(env.Binary + ".new") {
		t.Fatal("pulse.new was not kept after the smoke failure")
	}
	if data, _ := os.ReadFile(env.Binary); !strings.Contains(string(data), "dev-"+rev[:12]) {
		t.Fatal("pulse changed after a failed smoke")
	}
}

func TestCheckRestoredTurnsSelfUpdateOffAndNamesTheBuild(t *testing.T) {
	s, _, env := selfUpdateFixture(t)
	// Nothing to do without the marker.
	if lines := checkRestored(s, env); len(lines) != 0 {
		t.Fatalf("no marker: %v", lines)
	}
	exec1(s.DB, "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'daemon', ?, 'pulse', ?)", testNow, selfUpdateEvent, `{"from":"aaa","to":"deadbeefcafe0000"}`)
	if err := os.WriteFile(env.Binary+restoredMarker, []byte("2026-09-08T10:00:00Z\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	lines := checkRestored(s, env)
	if len(lines) != 1 || !strings.Contains(lines[0], "restored the previous pulse after 3 early exits: the build of deadbeefcafe did not start; self-update is off") {
		t.Fatalf("restored: %v", lines)
	}
	if got := s.Setting("daemon.self_update", ""); got != "off" {
		t.Fatalf("self_update = %q after a restore", got)
	}
	if fileExists(env.Binary + restoredMarker) {
		t.Fatal("marker kept")
	}
	post, err := queryOne(s.DB, "SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1", selfUpdateThread)
	if err != nil || post == nil || !strings.Contains(post.str("body"), "2026-09-08T10:00:00Z") || !strings.Contains(post.str("body"), "pulse settings set daemon.self_update on") {
		t.Fatalf("restore finding: %v %v", post, err)
	}
	// With self-update off, the phase stays quiet.
	if lines := tendBinary(s, testNow, env); len(lines) != 0 {
		t.Fatalf("after the restore: %v", lines)
	}
}
