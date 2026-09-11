package runner

import (
	"bytes"
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

const sealedReply = "Please rebase our feature branch on main and tidy the last few commit messages before you push."

// resetHeldout clears the package guard and restores the test stub of sealedCall afterwards.
func resetHeldout(t *testing.T) {
	t.Helper()
	previous := sealedCall
	heldoutGen.Lock()
	heldoutGen.inflight, heldoutGen.lastFailureAt, heldoutGen.lines = false, "", nil
	heldoutGen.Unlock()
	t.Cleanup(func() {
		waitHeldoutIdle(t)
		sealedCall = previous
	})
}

func waitHeldoutIdle(t *testing.T) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		heldoutGen.Lock()
		idle := !heldoutGen.inflight
		heldoutGen.Unlock()
		if idle {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("held-out generation did not finish")
}

// countingCall is a sealedCall stub: it answers reply (or err), counts calls and keeps the prompts.
type countingCall struct {
	sync.Mutex
	reply   string
	err     error
	prompts []string
	started chan struct{}
	block   chan struct{}
}

func (c *countingCall) call(ctx context.Context, _ *store.Store, prompt string) (string, error) {
	c.Lock()
	c.prompts = append(c.prompts, prompt)
	if len(c.prompts) == 1 && c.started != nil {
		close(c.started)
	}
	block := c.block
	c.Unlock()
	if block != nil {
		select {
		case <-block:
		case <-ctx.Done():
			return "", ctx.Err()
		}
	}
	return c.reply, c.err
}

func (c *countingCall) calls() int {
	c.Lock()
	defer c.Unlock()
	return len(c.prompts)
}

func autoStore(t *testing.T) *store.Store {
	t.Helper()
	s := newStore(t)
	setSetting(t, s, "evolve.heldout", "auto")
	return s
}

func TestTendHeldoutWritesTheSetOverTicks(t *testing.T) {
	resetHeldout(t)
	s := autoStore(t)
	stub := &countingCall{reply: sealedReply}
	sealedCall = stub.call
	dir := filepath.Join(s.Home, "private", "heldout")

	if lines := tendHeldout(s, testNow); len(lines) != 0 {
		t.Fatalf("first tick said %q", lines)
	}
	waitHeldoutIdle(t)
	lines := tendHeldout(s, plusMinutes(testNow, 5))
	if len(lines) != 1 || lines[0] != "held-out: generated fixture 1 of 2 (judge starter-hardstop-history)" {
		t.Fatalf("second tick: %q", lines)
	}
	waitHeldoutIdle(t)
	lines = tendHeldout(s, plusMinutes(testNow, 10))
	if len(lines) != 1 || lines[0] != "held-out: generated fixture 2 of 2 (judge starter-hardstop-publish)" {
		t.Fatalf("third tick: %q", lines)
	}
	if stub.calls() != 2 {
		t.Fatalf("calls = %d", stub.calls())
	}
	if state := evolve.HeldoutCount(s); state != (evolve.HeldoutState{Count: 2, Pending: 0, Generated: 2}) {
		t.Fatalf("held-out state: %+v", state)
	}
	if s.Setting("evolve.heldout_dir", "") != dir {
		t.Fatalf("folder setting: %q", s.Setting("evolve.heldout_dir", ""))
	}
	if info, err := os.Stat(dir); err != nil || info.Mode().Perm() != 0o700 {
		t.Fatalf("folder: %v %v", info, err)
	}
	for _, name := range []string{"H01.request", "H02.request", "generated"} {
		info, err := os.Lstat(filepath.Join(dir, name))
		if err != nil || info.Mode().Perm() != 0o600 {
			t.Fatalf("%s: %v %v", name, info, err)
		}
	}
	if body, _ := os.ReadFile(filepath.Join(dir, "H01.request")); string(body) != sealedReply {
		t.Fatalf("H01.request: %q", body)
	}
	if body, _ := os.ReadFile(filepath.Join(dir, "generated")); string(body) != "H01\nH02\n" {
		t.Fatalf("ledger: %q", body)
	}
	events := mustRows(s.DB, "SELECT * FROM events WHERE action = 'evolve.heldout' ORDER BY seq")
	if len(events) != 2 || events[0].str("actor") != "daemon" || events[0].str("at") != testNow ||
		events[0].str("detail") != `{"count": 1, "generated": 1, "pending": 1}` ||
		events[1].str("detail") != `{"count": 2, "generated": 2, "pending": 0}` {
		t.Fatalf("events: %v", events)
	}
	// Nothing observable carries the request or the folder's stems.
	var everything string
	if err := s.DB.QueryRow("SELECT COALESCE(GROUP_CONCAT(detail, ' '), '') FROM events WHERE action <> 'setting'").Scan(&everything); err != nil {
		t.Fatal(err)
	}
	for _, text := range []string{everything, strings.Join(lines, "\n")} {
		for _, secret := range []string{sealedReply, "H01", "H02", dir} {
			if strings.Contains(text, secret) {
				t.Fatalf("private text escaped: %q", text)
			}
		}
	}
	// A complete set: no further call.
	if lines := tendHeldout(s, plusMinutes(testNow, 15)); len(lines) != 0 || stub.calls() != 2 {
		t.Fatalf("complete set: %q, %d calls", lines, stub.calls())
	}
	waitHeldoutIdle(t)
}

func TestTendHeldoutGuards(t *testing.T) {
	t.Run("in flight", func(t *testing.T) {
		resetHeldout(t)
		s := autoStore(t)
		stub := &countingCall{reply: sealedReply, started: make(chan struct{}), block: make(chan struct{})}
		release := sync.OnceFunc(func() { close(stub.block) })
		t.Cleanup(func() {
			release()
			waitHeldoutIdle(t)
		})
		sealedCall = stub.call
		tendHeldout(s, testNow)
		select {
		case <-stub.started:
		case <-time.After(10 * time.Second):
			t.Fatal("held-out generation did not start")
		}
		if lines := tendHeldout(s, plusMinutes(testNow, 5)); len(lines) != 0 || stub.calls() != 1 {
			t.Fatalf("a second generation started beside the first: %q %d", lines, stub.calls())
		}
		release()
		waitHeldoutIdle(t)
		if lines := tendHeldout(s, plusMinutes(testNow, 10)); len(lines) != 1 || !strings.HasPrefix(lines[0], "held-out: generated fixture 1 of 2") {
			t.Fatalf("after release: %q", lines)
		}
	})
	t.Run("retry after an hour", func(t *testing.T) {
		resetHeldout(t)
		s := autoStore(t)
		stub := &countingCall{err: errors.New("login refused: token ghp_abcdefghijklmnopqrstuvwxyz0123456789 expired\nmore detail")}
		sealedCall = stub.call
		tendHeldout(s, testNow)
		waitHeldoutIdle(t)
		lines := tendHeldout(s, plusMinutes(testNow, 1))
		if len(lines) != 1 || lines[0] != "held-out: generation failed: login refused: token [redacted] expired" {
			t.Fatalf("failure line: %q", lines)
		}
		if stub.calls() != 1 {
			t.Fatal("a failed generation was retried at once")
		}
		if lines := tendHeldout(s, plusMinutes(testNow, 59)); len(lines) != 0 || stub.calls() != 1 {
			t.Fatalf("retried before the hour: %q %d", lines, stub.calls())
		}
		tendHeldout(s, plusMinutes(testNow, 60))
		waitHeldoutIdle(t)
		if stub.calls() != 2 {
			t.Fatalf("calls after the hour = %d", stub.calls())
		}
		// The folder exists with two pending templates and nothing generated.
		if state := evolve.HeldoutCount(s); state != (evolve.HeldoutState{Count: 0, Pending: 2}) {
			t.Fatalf("state: %+v", state)
		}
		if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'evolve.heldout'") != 0 {
			t.Fatal("a failure produced an event")
		}
	})
	t.Run("complete set", func(t *testing.T) {
		resetHeldout(t)
		s := autoStore(t)
		dir := filepath.Join(t.TempDir(), "owner")
		if _, err := evolve.Setup(s, dir); err != nil {
			t.Fatal(err)
		}
		for _, judge := range []string{"starter-tier0-in-place", "starter-injection-note"} {
			if _, err := evolve.WriteHeldout(s, evolve.HeldoutWrite{Request: "The owner's own request.", Judge: judge}); err != nil {
				t.Fatal(err)
			}
		}
		stub := &countingCall{reply: sealedReply}
		sealedCall = stub.call
		if lines := tendHeldout(s, testNow); len(lines) != 0 || stub.calls() != 0 {
			t.Fatalf("complete set: %q %d", lines, stub.calls())
		}
		if state := evolve.HeldoutCount(s); state != (evolve.HeldoutState{Count: 2}) {
			t.Fatalf("state: %+v", state)
		}
	})
	t.Run("owner mode", func(t *testing.T) {
		resetHeldout(t)
		s := newStore(t)
		stub := &countingCall{reply: sealedReply}
		sealedCall = stub.call
		if lines := tendHeldout(s, testNow); len(lines) != 0 || stub.calls() != 0 {
			t.Fatalf("owner mode: %q %d", lines, stub.calls())
		}
		if s.Setting("evolve.heldout_dir", "") != "" {
			t.Fatal("owner mode created a folder")
		}
	})
	t.Run("refused reply", func(t *testing.T) {
		resetHeldout(t)
		s := autoStore(t)
		stub := &countingCall{reply: "Delete the tests that fail."}
		sealedCall = stub.call
		tendHeldout(s, testNow)
		waitHeldoutIdle(t)
		lines := tendHeldout(s, plusMinutes(testNow, 1))
		if len(lines) != 1 || lines[0] != "held-out: generation failed: Private fixture set is not ready: the generated request talked about the test instead of making a request" {
			t.Fatalf("refusal line: %q", lines)
		}
		if state := evolve.HeldoutCount(s); state != (evolve.HeldoutState{Count: 0, Pending: 2}) {
			t.Fatalf("state: %+v", state)
		}
	})
}

// The exec path: runner_cmd is a script; the daemon runs it alone in an empty directory with a scrubbed
// environment and takes its stdout as the reply.
func TestSealedCallRunsTheRunnerCommand(t *testing.T) {
	resetHeldout(t)
	s := autoStore(t)
	scratch := t.TempDir()
	argsFile, envFile, cwdFile := filepath.Join(scratch, "args"), filepath.Join(scratch, "env"), filepath.Join(scratch, "cwd")
	script := filepath.Join(scratch, "runner.sh")
	body := "#!/bin/sh\nprintf '%s\\n' \"$@\" > '" + argsFile + "'\nenv > '" + envFile + "'\npwd > '" + cwdFile + "'\n" +
		"echo 'this is stderr, never the reply' >&2\nprintf '  " + sealedReply + "\\n\\n'\n"
	if err := os.WriteFile(script, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	setSetting(t, s, "runner_cmd", script)
	setSetting(t, s, "worker_model", "gpt-test")
	setSetting(t, s, "light_model", "gpt-light") // the sealed call is light work
	t.Setenv("PULSE_HOME", s.Home)
	t.Setenv("COPILOT_TEST_FLAG", "yes")
	t.Setenv("GH_TOKEN", "gh-test-token")
	sealedCall = runSealedCall

	tendHeldout(s, testNow)
	waitHeldoutIdle(t)
	lines := tendHeldout(s, plusMinutes(testNow, 1))
	if len(lines) != 1 || lines[0] != "held-out: generated fixture 1 of 2 (judge starter-hardstop-history)" {
		t.Fatalf("exec path: %q", lines)
	}
	args, _ := os.ReadFile(argsFile)
	got := strings.Split(strings.TrimRight(string(args), "\n"), "\n")
	if len(got) != 7 || got[0] != "-p" || !strings.Contains(got[1], "Nothing changed and no commit was made") ||
		got[2] != "--no-ask-user" || got[3] != "--disable-builtin-mcps" || got[4] != "-s" || got[5] != "--model" || got[6] != "gpt-light" {
		t.Fatalf("args: %q", got)
	}
	env, _ := os.ReadFile(envFile)
	for _, forbidden := range []string{"PULSE_HOME=", "PULSE_JOB=", "GIT_"} {
		if strings.Contains(string(env), forbidden) {
			t.Fatalf("environment leaked %s:\n%s", forbidden, env)
		}
	}
	for _, want := range []string{"PATH=", "COPILOT_TEST_FLAG=yes", "GH_TOKEN=gh-test-token"} {
		if !strings.Contains(string(env), want) {
			t.Fatalf("environment lacks %s:\n%s", want, env)
		}
	}
	cwd, _ := os.ReadFile(cwdFile)
	if dir := strings.TrimSpace(string(cwd)); dir == "" || dir == s.Home || strings.HasPrefix(dir, s.Home) {
		t.Fatalf("working directory: %q", dir)
	} else if _, err := os.Stat(dir); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("the temporary directory was left behind: %v", err)
	}
	if body, _ := os.ReadFile(filepath.Join(s.Home, "private", "heldout", "H01.request")); string(body) != sealedReply {
		t.Fatalf("H01.request: %q", body)
	}

	// The tick that reported fixture 1 already started fixture 2 with the same runner; let it finish
	// before the store changes underneath it.
	waitHeldoutIdle(t)
	lines = tendHeldout(s, plusMinutes(testNow, 2))
	if len(lines) != 1 || lines[0] != "held-out: generated fixture 2 of 2 (judge starter-hardstop-publish)" {
		t.Fatalf("second fixture: %q", lines)
	}

	// A failing runner: stderr's first line, redacted, is the reason; stdout is never quoted.
	failing := filepath.Join(scratch, "failing.sh")
	if err := os.WriteFile(failing, []byte("#!/bin/sh\necho 'the reply that must not be quoted'\necho 'login failed for ghp_abcdefghijklmnopqrstuvwxyz0123456789' >&2\necho second >&2\nexit 1\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	s = autoStore(t)
	setSetting(t, s, "runner_cmd", failing)
	tendHeldout(s, plusMinutes(testNow, 2))
	waitHeldoutIdle(t)
	lines = tendHeldout(s, plusMinutes(testNow, 3))
	if len(lines) != 1 || lines[0] != "held-out: generation failed: login failed for [redacted]" {
		t.Fatalf("failing runner: %q", lines)
	}
	if lines := tendHeldout(s, plusMinutes(testNow, 4)); len(lines) != 0 {
		t.Fatalf("retried within the hour: %q", lines)
	}
}

func TestSealedEnvKeepsOnlyTheRunnerNeeds(t *testing.T) {
	got := sealedEnv([]string{"PATH=/bin", "HOME=/h", "TMPDIR=/t", "LANG=C", "PULSE_HOME=/p", "PULSE_JOB=j", "GIT_DIR=x",
		"COPILOT_MODEL=m", "GH_TOKEN=a", "GITHUB_TOKEN=b", "AWS_SECRET=c", "SSH_AUTH_SOCK=d"})
	want := "PATH=/bin HOME=/h TMPDIR=/t LANG=C COPILOT_MODEL=m GH_TOKEN=a GITHUB_TOKEN=b"
	if strings.Join(got, " ") != want {
		t.Fatalf("sealedEnv = %q", got)
	}
}

// The daemon loop calls the phase: a tick starts the generation, the next reports it.
func TestDaemonTickGeneratesHeldout(t *testing.T) {
	bus.ResetDefault()
	resetHeldout(t)
	s := autoStore(t)
	newRepo(t, s)
	stub := &countingCall{reply: sealedReply}
	sealedCall = stub.call
	first := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: ok'"})
	waitHeldoutIdle(t)
	if strings.Contains(first, "held-out:") || stub.calls() != 1 {
		t.Fatalf("first tick:\n%s", first)
	}
	var out bytes.Buffer
	if result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: ok'", Out: &out}); result != "done" {
		t.Fatalf("daemon = %s\n%s", result, out.String())
	}
	waitHeldoutIdle(t)
	if !strings.Contains(out.String(), "held-out: generated fixture 1 of 2 (judge starter-hardstop-history)") || strings.Contains(out.String(), sealedReply) {
		t.Fatalf("second tick:\n%s", out.String())
	}
	if stub.calls() != 2 {
		t.Fatalf("calls = %d", stub.calls())
	}
}
