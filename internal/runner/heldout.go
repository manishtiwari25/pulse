package runner

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

// The daemon's held-out phase (ADR-033): while evolve.heldout is auto and the
// private set is short, one sealed model call per tick writes a fixture
// through evolve.Generate. The call runs like the login check - the runner
// command alone, in an empty temporary directory, with a scrubbed
// environment - and its reply goes to the 0600 file and nowhere else; the log
// and the event carry counts and the judge's public name.

const heldoutRetryMinutes = 60
const heldoutCallTimeout = 3 * time.Minute

// heldoutGen is the single in-flight guard: one generation at a time, its
// outcome reported on the tick after it finishes.
var heldoutGen struct {
	sync.Mutex
	inflight      bool
	lastFailureAt string
	lines         []string
}

// sealedCall runs the runner command once with the prompt and returns its
// trimmed stdout. Tests replace it; runSealedCall is the real one.
var sealedCall = runSealedCall

func runSealedCall(ctx context.Context, s *store.Store, prompt string) (string, error) {
	dir, err := os.MkdirTemp("", "pulse-heldout-")
	if err != nil {
		return "", errors.New("cannot create an empty working directory")
	}
	defer os.RemoveAll(dir)
	args := []string{"-p", prompt, "--no-ask-user", "--disable-builtin-mcps", "-s"}
	// The sealed call reads one held-out record and answers a line; light work, light_model.
	if model := s.LightModel(); model != "" {
		args = append(args, "--model", model)
	}
	cmd := exec.CommandContext(ctx, runnerCmd(s), args...)
	cmd.Dir = dir
	cmd.Env = sealedEnv(os.Environ())
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Run(); err != nil {
		// stderr is the only text that may be reported; stdout is never a reason.
		line := faultLine(stderr.String(), 200)
		switch {
		case errors.Is(ctx.Err(), context.DeadlineExceeded):
			line = "the model call timed out"
		case line == "":
			line = err.Error()
		}
		return "", errors.New(cut(redactSecrets(line), 200))
	}
	return strings.TrimSpace(stdout.String()), nil
}

// sealedEnv keeps what the runner needs to start and log in - PATH, HOME,
// TMPDIR, LANG and the Copilot variables - and nothing that names a project,
// a job or the store.
func sealedEnv(environ []string) []string {
	env := []string{}
	for _, entry := range environ {
		key, _, _ := strings.Cut(entry, "=")
		switch {
		case key == "PATH" || key == "HOME" || key == "TMPDIR" || key == "LANG",
			key == "GH_TOKEN" || key == "GITHUB_TOKEN", strings.HasPrefix(key, "COPILOT_"):
			env = append(env, entry)
		}
	}
	return env
}

// tendHeldout is the daemon's phase: it reports the previous generation's
// outcome, then starts one more when the set is short, the mode is auto, no
// generation is running and the last failure is at least heldoutRetryMinutes
// old. It never blocks the tick on the model.
func tendHeldout(s *store.Store, now string) []string {
	heldoutGen.Lock()
	defer heldoutGen.Unlock()
	lines := heldoutGen.lines
	heldoutGen.lines = nil
	if heldoutGen.inflight || evolve.HeldoutMode(s) != "auto" {
		return lines
	}
	if heldoutGen.lastFailureAt != "" && mustISO(now).Before(mustISO(plusMinutes(heldoutGen.lastFailureAt, heldoutRetryMinutes))) {
		return lines
	}
	if evolve.HeldoutComplete(evolve.HeldoutCount(s)) {
		return lines
	}
	heldoutGen.inflight = true
	go generateHeldout(s, now)
	return lines
}

func generateHeldout(s *store.Store, now string) {
	ctx, cancel := context.WithTimeout(context.Background(), heldoutCallTimeout)
	defer cancel()
	result, err := evolve.Generate(ctx, s, func(ctx context.Context, prompt string) (string, error) {
		return sealedCall(ctx, s, prompt)
	}, now)
	heldoutGen.Lock()
	defer heldoutGen.Unlock()
	heldoutGen.inflight = false
	switch {
	case err != nil:
		heldoutGen.lastFailureAt = now
		heldoutGen.lines = append(heldoutGen.lines, "held-out: generation failed: "+heldoutFailure(err))
	case result.Wrote:
		heldoutGen.lastFailureAt = ""
		heldoutGen.lines = append(heldoutGen.lines, fmt.Sprintf("held-out: generated fixture %d of %d (judge %s)",
			result.Count, max(2, result.Count+result.Pending), result.Judge))
		eventAt(s.DB, now, "daemon", "evolve.heldout", "", map[string]any{"generated": result.Generated, "count": result.Count, "pending": result.Pending})
	}
}

// heldoutFailure is the one redacted line the log gets: a refusal's what and
// why, else the first line of whatever the call reported.
func heldoutFailure(err error) string {
	var ce *cli.Error
	text := errMessage(err)
	if errors.As(err, &ce) {
		text = ce.What
		if ce.Why != "" {
			text += ": " + ce.Why
		}
	}
	return cut(redactSecrets(faultLine(text, 200)), 200)
}
