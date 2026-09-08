package runner

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
)

// ADR-035: the daemon rebuilds and installs itself after a landing in the brain.
//
// Once per tick, after landings, tendBinary compares the brain checkout's HEAD with the revision this
// binary was built from. When they differ, the checkout is clean and on its default branch, and
// daemon.self_update is on, it builds pulse.new from that checkout, smoke-tests it, keeps the running
// binary as pulse.prev and renames pulse.new over pulse. The next tick's stamp check exits 3 after the
// runs in flight, exactly as a hand-installed build does today. Nothing here pulls, fetches or pushes.

const (
	selfUpdateArea   = "runner"
	selfUpdateThread = "release"
	selfUpdateEvent  = "daemon.self_update"
	// selfUpdateBuildTimeout bounds `go build` and each smoke command.
	selfUpdateBuildTimeout = 10 * time.Minute
	// restoredMarker is what the launcher leaves next to the binary after it put pulse.prev back (rule 5).
	restoredMarker = ".restored"
)

// selfUpdateState is the phase's memory between ticks: the build runs in a goroutine and reports on
// the next tick, so the tick is never held by the compiler.
var selfUpdateState struct {
	sync.Mutex
	inflight      bool
	lines         []string
	lastFailureAt string
	lastSkip      string // the last "waiting" reason said, so each state is logged once
	installed     string // revision already installed and awaiting the restart
}

// selfUpdateEnv is what the phase needs from its host; tests point it at fakes.
type selfUpdateEnv struct {
	Binary   string        // the running binary's path (pulseBinary())
	GoCmd    string        // "go"
	Timeout  time.Duration // selfUpdateBuildTimeout
	Revision string        // cli.Revision() of the running binary; "" when unstamped
	Actor    string        // the daemon's agent name for the finding
	Inflight int           // runs in flight when the finding is written
}

func shortRev(rev string) string {
	if len(rev) > 12 {
		return rev[:12]
	}
	if rev == "" {
		return "unstamped"
	}
	return rev
}

// tendBinary is the daemon's phase (rules 1-4). It returns the lines to log this tick.
func tendBinary(s *store.Store, now string, env selfUpdateEnv) []string {
	st := &selfUpdateState
	st.Lock()
	defer st.Unlock()
	lines := st.lines
	st.lines = nil
	if st.inflight {
		return lines
	}
	say := func(reason string) []string {
		if reason == st.lastSkip {
			return lines
		}
		st.lastSkip = reason
		return append(lines, "self-update: "+reason)
	}
	if s.Setting("daemon.self_update", "") != "on" {
		return lines
	}
	if s.Setting("stop", "") == "1" {
		return say("stop is set; no build")
	}
	if st.lastFailureAt != "" {
		retry, _ := strconv.Atoi(s.Setting("daemon.self_update_retry_minutes", "60"))
		if retry < 1 {
			retry = 60
		}
		if mustISO(now).Before(mustISO(plusMinutes(st.lastFailureAt, float64(retry)))) {
			return lines
		}
	}
	brain := brainProject(s.DB)
	if brain == nil {
		return say("no brain project; nothing to build from")
	}
	repo := brain.str("path")
	head := git(repo, "rev-parse", "HEAD")
	branch := git(repo, "rev-parse", "--abbrev-ref", "HEAD")
	status := git(repo, "status", "--porcelain")
	if !head.ok() || !branch.ok() || !status.ok() {
		return say(fmt.Sprintf("%s is not a readable git checkout: %s", repo, gitReason(head, branch, status)))
	}
	rev := strings.TrimSpace(head.Stdout)
	if rev == st.installed || rev == env.Revision {
		return lines
	}
	def := brain.str("default_branch")
	if def == "" {
		def = "main"
	}
	if got := strings.TrimSpace(branch.Stdout); got != def {
		return say(fmt.Sprintf("brain checkout is on %s, not %s; waiting", got, def))
	}
	if strings.TrimSpace(status.Stdout) != "" {
		return say("brain checkout is dirty; waiting for it to be clean")
	}
	if _, err := exec.LookPath(env.GoCmd); err != nil {
		return say(fmt.Sprintf("%s is not on the daemon's PATH; builds are yours until it is", env.GoCmd))
	}
	st.lastSkip = ""
	st.inflight = true
	lines = append(lines, fmt.Sprintf("self-update: brain HEAD %s differs from the running %s; building", shortRev(rev), shortRev(env.Revision)))
	go func() {
		outcome := buildAndInstall(s, repo, rev, env)
		st.Lock()
		defer st.Unlock()
		st.inflight = false
		st.lines = append(st.lines, outcome.line)
		if outcome.installed {
			st.installed = rev
			st.lastFailureAt = ""
		} else {
			st.lastFailureAt = s.Now()
		}
	}()
	return lines
}

type selfUpdateOutcome struct {
	line      string
	installed bool
}

// buildAndInstall is rules 2-4 for one revision; it runs off the tick and never touches pulse unless
// the smoke passed.
func buildAndInstall(s *store.Store, repo, rev string, env selfUpdateEnv) selfUpdateOutcome {
	timeout := env.Timeout
	if timeout <= 0 {
		timeout = selfUpdateBuildTimeout
	}
	newBinary := env.Binary + ".new"
	_ = os.Remove(newBinary)
	if out, err := runIn(repo, timeout, env.GoCmd, "build", "-trimpath", "-o", newBinary, "./cmd/pulse"); err != nil {
		return selfUpdateFailure(s, rev, env, "build", out, err)
	}
	note, err := signNewBuild(s, newBinary)
	if err != nil {
		return selfUpdateFailure(s, rev, env, "sign", "", err)
	}
	version, err := runIn(repo, timeout, newBinary, "version")
	if err != nil {
		return selfUpdateFailure(s, rev, env, "smoke", version, err)
	}
	if !strings.Contains(version, shortRev(rev)) {
		return selfUpdateFailure(s, rev, env, "smoke", version, fmt.Errorf("pulse.new version printed %q, not the revision %s", strings.TrimSpace(version), shortRev(rev)))
	}
	if out, err := runIn(repo, timeout, newBinary, "help"); err != nil {
		return selfUpdateFailure(s, rev, env, "smoke", out, err)
	}
	prev := env.Binary + ".prev"
	_ = os.Remove(prev)
	if err := os.Rename(env.Binary, prev); err != nil {
		return selfUpdateFailure(s, rev, env, "install", "", err)
	}
	if err := os.Rename(newBinary, env.Binary); err != nil {
		_ = os.Rename(prev, env.Binary)
		return selfUpdateFailure(s, rev, env, "install", "", err)
	}
	now := s.Now()
	detail := map[string]any{"from": env.Revision, "to": rev, "binary": env.Binary, "inflight": env.Inflight}
	if _, err := bus.Emit(s, env.Actor, selfUpdateEvent, "pulse", detail); err != nil {
		return selfUpdateOutcome{installed: true, line: fmt.Sprintf("self-update: installed %s; event not written: %s", shortRev(rev), errMessage(err))}
	}
	summary := fmt.Sprintf("pulse rebuilt from %s: restarts after %d run(s) in flight", shortRev(rev), env.Inflight)
	body := fmt.Sprintf("The brain's default branch landed at `%s` while the daemon ran `%s`. The daemon built `%s` from the clean checkout at %s, "+
		"checked that it prints its revision and answers `help`, kept the running binary as `%s` and installed the new one at %s.\n\n"+
		"The daemon and the hub restart on the changed file: the daemon after the %d run(s) in flight, the hub within a minute. "+
		"Roll back with `mv %s %s` (the daemon restarts again); `pulse settings set daemon.self_update off` stops these builds (ADR-035).\n",
		rev, firstNonEmpty(env.Revision, "an unstamped build"), env.Binary, repo, prev, now, env.Inflight, prev, env.Binary)
	if err := selfUpdateFinding(s, env.Actor, summary, body); err != nil {
		return selfUpdateOutcome{installed: true, line: fmt.Sprintf("self-update: installed %s; finding not posted: %s", shortRev(rev), errMessage(err))}
	}
	if note != "" {
		return selfUpdateOutcome{installed: true, line: "self-update: " + summary + "\n" + note}
	}
	return selfUpdateOutcome{installed: true, line: "self-update: " + summary}
}

func selfUpdateFailure(s *store.Store, rev string, env selfUpdateEnv, stage, output string, err error) selfUpdateOutcome {
	tail := strings.TrimSpace(redactSecrets(strings.Join(lastLines(output, 20), "\n")))
	line := fmt.Sprintf("self-update: %s of %s failed: %s", stage, shortRev(rev), errMessage(err))
	retry := s.Setting("daemon.self_update_retry_minutes", "60")
	summary := fmt.Sprintf("pulse %s of %s failed; the running binary is untouched", stage, shortRev(rev))
	body := fmt.Sprintf("The daemon tried to rebuild itself from the brain's `%s` and the %s step failed: %s\n\n"+
		"`%s` was not touched; `%s.new` is kept for reading when it exists. The daemon tries again in %s minutes "+
		"(daemon.self_update_retry_minutes); `pulse settings set daemon.self_update off` stops it (ADR-035).\n\n```\n%s\n```\n",
		rev, stage, errMessage(err), env.Binary, env.Binary, retry, tail)
	if perr := selfUpdateFinding(s, env.Actor, summary, body); perr != nil {
		line += "; finding not posted: " + errMessage(perr)
	}
	return selfUpdateOutcome{line: line}
}

func selfUpdateFinding(s *store.Store, actor, summary, body string) error {
	brain := brainProject(s.DB)
	if brain == nil {
		return fmt.Errorf("no brain project")
	}
	_, err := createPost(s, s.Now(), postOpts{Kind: "finding", Project: brain.str("slug"), Area: selfUpdateArea, Thread: selfUpdateThread,
		Summary: trunc(summary, 230), Body: body, Agent: actorAgent(s, actor)})
	return err
}

// runIn runs one command in dir with the daemon's own environment and a deadline; the combined output
// is returned with the error so a failure can be read in the finding.
func runIn(dir string, timeout time.Duration, name string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, name, args...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GIT_TERMINAL_PROMPT=0")
	var out bytes.Buffer
	cmd.Stdout = &out
	cmd.Stderr = &out
	err := cmd.Run()
	if ctx.Err() == context.DeadlineExceeded {
		err = fmt.Errorf("%s timed out after %s", filepath.Base(name), timeout)
	}
	return out.String(), err
}

// gitReason names why the first failed git call failed: its stderr, else its error, else the exit
// code - a git killed by the timeout (macOS holding a folder behind a dialog) has neither.
func gitReason(results ...gitResult) string {
	for _, r := range results {
		if r.ok() {
			continue
		}
		if text := strings.TrimSpace(r.Stderr); text != "" {
			return text
		}
		if r.Err != nil {
			return r.Err.Error()
		}
		if r.Code < 0 {
			return "git was killed before it answered (a folder still held behind a dialog, or a timeout)"
		}
		return fmt.Sprintf("git exited %d without a message", r.Code)
	}
	return ""
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if strings.TrimSpace(v) != "" {
			return v
		}
	}
	return ""
}

// checkRestored is rule 5's second half, run once at daemon start: when the launcher left the marker
// after putting pulse.prev back, the daemon names the failed build in a finding, turns self-update off
// until a human reads it, and removes the marker.
func checkRestored(s *store.Store, env selfUpdateEnv) []string {
	marker := env.Binary + restoredMarker
	data, err := os.ReadFile(marker)
	if err != nil {
		return nil
	}
	when := strings.TrimSpace(string(data))
	failed := "an unknown build"
	var detail string
	if err := s.DB.QueryRow("SELECT detail FROM events WHERE action = ? ORDER BY seq DESC LIMIT 1", selfUpdateEvent).Scan(&detail); err == nil {
		if to := jsonField(detail, "to"); to != "" {
			failed = "the build of " + shortRev(to)
		}
	}
	var lines []string
	if err := s.SetSetting(env.Actor, "daemon.self_update", "off"); err != nil {
		lines = append(lines, "self-update: could not turn itself off: "+errMessage(err))
	}
	summary := fmt.Sprintf("restored the previous pulse after 3 early exits: %s did not start; self-update is off", failed)
	body := fmt.Sprintf("The launcher saw pulse exit three times in a row within a minute of starting (at %s) and put `%s.prev` back over `%s`; "+
		"the failed build is kept as `%s.failed`. The daemon now running is the previous build, `%s`.\n\n"+
		"`daemon.self_update` is off until you turn it on in Settings: read the daemon log for the failure, fix the brain's default branch, "+
		"then `pulse settings set daemon.self_update on` (ADR-035).\n", when, env.Binary, env.Binary, env.Binary, firstNonEmpty(env.Revision, "unstamped"))
	if err := selfUpdateFinding(s, env.Actor, summary, body); err != nil {
		lines = append(lines, "self-update: restore finding not posted: "+errMessage(err))
	}
	_ = os.Remove(marker)
	return append(lines, "self-update: "+summary)
}

// jsonField reads one string field from a small JSON object without a struct.
func jsonField(text, key string) string {
	var m map[string]any
	if err := json.Unmarshal([]byte(text), &m); err != nil {
		return ""
	}
	v, _ := m[key].(string)
	return v
}
