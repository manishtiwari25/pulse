package runner

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/bus"
)

const fakePushKey = "-----BEGIN OPENSSH PRIVATE KEY-----\nZmFrZSBrZXkgZm9yIHRlc3Rz\n-----END OPENSSH PRIVATE KEY-----\n"

// fakeSSH puts an `ssh` first on PATH that records its arguments, checks the key file git hands it, and
// runs the git-receive-pack command locally, so a push to ssh://localhost<bare> lands in the bare repo
// without a daemon or a network. It returns the path of the record.
func fakeSSH(t *testing.T) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("fake ssh is a shell script")
	}
	dir := t.TempDir()
	record := filepath.Join(dir, "record")
	script := `#!/bin/sh
printf '%s\n' "$@" > "$PULSE_TEST_SSH_RECORD"
key=""
prev=""
for a in "$@"; do
  if [ "$prev" = "-i" ]; then key="$a"; fi
  prev="$a"
done
if [ -n "$key" ]; then
  if [ ! -f "$key" ]; then echo "fake ssh: key $key missing" >&2; exit 255; fi
  mode=$(stat -f '%Lp' "$key" 2>/dev/null || stat -c '%a' "$key")
  echo "$mode" > "$PULSE_TEST_SSH_RECORD.mode"
  head -c 30 "$key" > "$PULSE_TEST_SSH_RECORD.head"
fi
for a in "$@"; do last="$a"; done
exec sh -c "$last"
`
	if err := os.WriteFile(filepath.Join(dir, "ssh"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv("PULSE_TEST_SSH_RECORD", record)
	return record
}

func keyFile(t *testing.T, mode os.FileMode) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "push_key")
	if err := os.WriteFile(path, []byte(fakePushKey), mode); err != nil {
		t.Fatal(err)
	}
	return path
}

// ADR-041: with push.credential set the push goes through ssh with the referenced key alone - batch mode,
// no agent, no other identities - written 0600 under PULSE_HOME/tmp for the push and gone after it; the
// owner's own GIT_SSH_COMMAND is not consulted.
func TestLandingPushUsesTheCredentialAlone(t *testing.T) {
	skipSlow(t)
	bus.ResetDefault()
	s := newStore(t)
	record := fakeSSH(t)
	proj, repo, bare := pushFixture(t, s)
	run(t, repo, "git", "remote", "set-url", "--push", "origin", "ssh://localhost"+bare)
	t.Setenv("GIT_SSH_COMMAND", "false")
	setSetting(t, s, "landing.push", "main")
	setSetting(t, s, "push.credential", "file://"+keyFile(t, 0o600))
	pushBranch(t, s, proj, repo, "pulse/worker/1")

	lines, cur := tendLandings(s, []row{proj}, "daemon", testNow, nil)
	if len(lines) != 2 || !strings.HasPrefix(lines[0], "landing landed") || !strings.HasPrefix(lines[1], "landing pushed proj: main -> origin/main (1 commit, ") {
		t.Fatalf("lines %q", lines)
	}
	if len(cur) != 0 {
		t.Fatalf("hold notes after a good push: %v", cur)
	}
	if got, want := bareRef(t, bare, "refs/heads/main"), refOf(t, repo, "refs/heads/main"); got != want {
		t.Fatalf("remote main %s, local %s", got, want)
	}
	args, err := os.ReadFile(record)
	if err != nil {
		t.Fatalf("fake ssh was not used: %v", err)
	}
	text := string(args)
	for _, want := range []string{"-oBatchMode=yes\n", "-oIdentitiesOnly=yes\n", "-oIdentityAgent=none\n", "-i\n"} {
		if !strings.Contains(text, want) {
			t.Errorf("ssh args lack %q:\n%s", strings.TrimSpace(want), text)
		}
	}
	var keyPath string
	fields := strings.Split(strings.TrimSpace(text), "\n")
	for i, f := range fields {
		if f == "-i" && i+1 < len(fields) {
			keyPath = fields[i+1]
		}
	}
	if !strings.HasPrefix(keyPath, filepath.Join(s.Home, "tmp", "push-")) {
		t.Fatalf("key written at %q, want under %s", keyPath, filepath.Join(s.Home, "tmp", "push-"))
	}
	if mode, _ := os.ReadFile(record + ".mode"); strings.TrimSpace(string(mode)) != "600" {
		t.Errorf("key mode during the push = %q, want 600", strings.TrimSpace(string(mode)))
	}
	if head, _ := os.ReadFile(record + ".head"); !strings.HasPrefix(string(head), "-----BEGIN OPENSSH PRIVATE KEY") {
		t.Errorf("key content during the push = %q", head)
	}
	if _, err := os.Stat(keyPath); !os.IsNotExist(err) {
		t.Fatalf("key still on disk after the push: %v", err)
	}
	entries, _ := os.ReadDir(filepath.Join(s.Home, "tmp"))
	for _, e := range entries {
		if strings.HasPrefix(e.Name(), "push-") {
			t.Errorf("push directory left behind: %s", e.Name())
		}
	}
	if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = 'land.pushed' AND subject = 'proj'`); n != 1 {
		t.Fatalf("land.pushed events %d", n)
	}
}

// ADR-041 D3: a credential that cannot be resolved holds the push with the reason and the daemon never
// falls back to the credentials git has - here a local bare remote any plain push would reach. Clearing
// the setting pushes what is owed on the next tick.
func TestLandingPushCredentialFailureHoldsWithoutFallback(t *testing.T) {
	skipSlow(t)
	bus.ResetDefault()
	s := newStore(t)
	proj, repo, bare := pushFixture(t, s)
	before := bareRef(t, bare, "refs/heads/main")
	setSetting(t, s, "landing.push", "main")
	setSetting(t, s, "push.credential", "file:///nonexistent/pulse/push_key")
	pushBranch(t, s, proj, repo, "pulse/worker/1")

	lines, cur := tendLandings(s, []row{proj}, "daemon", testNow, nil)
	if len(lines) != 2 || !strings.HasPrefix(lines[0], "landing landed") || lines[1] != "landing held proj: 1 commit landed but not pushed; push credential: /nonexistent/pulse/push_key: no such file or directory" {
		t.Fatalf("lines %q", lines)
	}
	if !strings.Contains(cur["proj"], "push credential:") {
		t.Fatalf("note %q", cur["proj"])
	}
	if bareRef(t, bare, "refs/heads/main") != before {
		t.Fatal("remote moved: the push fell back to git's own credentials")
	}
	if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = 'land.held' AND json_extract(detail, '$.state') = 'push-failed'`); n != 1 {
		t.Fatalf("land.held push-failed events %d", n)
	}
	if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed'`); n != 1 {
		t.Fatalf("landing undone: land.landed events %d", n)
	}

	// A key anyone can read is refused the same way.
	setSetting(t, s, "push.credential", "file://"+keyFile(t, 0o644))
	lines, cur2 := tendLandings(s, []row{proj}, "daemon", testNow, cur)
	if len(lines) != 1 || !strings.Contains(lines[0], "push credential: ") || !strings.Contains(lines[0], "readable by others") {
		t.Fatalf("lines %q", lines)
	}
	if bareRef(t, bare, "refs/heads/main") != before {
		t.Fatal("remote moved with a loose key")
	}

	// The owner clears the setting: the owed push happens with what git has, and the hold clears.
	setSetting(t, s, "push.credential", "")
	lines, cur3 := tendLandings(s, []row{proj}, "daemon", testNow, cur2)
	if len(lines) != 2 || !strings.HasPrefix(lines[0], "landing pushed proj: main -> origin/main (1 commit, ") || lines[1] != "landing resumed proj" {
		t.Fatalf("lines %q", lines)
	}
	if len(cur3) != 0 {
		t.Fatalf("notes after recovery: %v", cur3)
	}
	if got, want := bareRef(t, bare, "refs/heads/main"), refOf(t, repo, "refs/heads/main"); got != want {
		t.Fatalf("remote main %s, local %s", got, want)
	}
}

// Without push.credential the push environment is what ADR-037 chose: batch-mode ssh unless the owner has
// an ssh command of their own.
func TestPushCredentialEnvDefaultsToGitsOwn(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	_, repo := newBareRepo(t, s)
	t.Setenv("GIT_SSH_COMMAND", "")
	env, cleanup, why := pushCredentialEnv(s, repo)
	cleanup()
	if why != "" || len(env) != 1 || env[0] != "GIT_SSH_COMMAND=ssh -oBatchMode=yes" {
		t.Fatalf("env %q why %q", env, why)
	}
	t.Setenv("GIT_SSH_COMMAND", "ssh -i /owner/key")
	env, cleanup, why = pushCredentialEnv(s, repo)
	cleanup()
	if why != "" || env != nil {
		t.Fatalf("owner's ssh command not left alone: env %q why %q", env, why)
	}
	// A worker never resolves the credential (ADR-041 D4).
	setSetting(t, s, "push.credential", "file://"+keyFile(t, 0o600))
	t.Setenv("PULSE_JOB", "job-1")
	env, cleanup, why = pushCredentialEnv(s, repo)
	cleanup()
	if env != nil || !strings.Contains(why, "never resolved inside a run") {
		t.Fatalf("worker env %q why %q", env, why)
	}
}
