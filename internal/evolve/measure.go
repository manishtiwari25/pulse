package evolve

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func init() { Measure = measure }

// executable locates this binary for starter scorers; tests substitute it.
var executable = os.Executable

type measuredFixture struct {
	name, kind, request string
	scorer              []string
	weight              float64
}

// pulseScorer reports whether a scorer's first word is pulse.
func pulseScorer(scorer string) bool {
	rest, ok := strings.CutPrefix(strings.TrimLeft(scorer, " \t"), "pulse")
	return ok && (rest == "" || rest[0] == ' ' || rest[0] == '\t')
}

// selfScorer rewrites a scorer whose first word is pulse so it runs this very
// executable: a measured clone must not depend on PATH to find the starter
// scorers, and a differently installed pulse must not judge them. Any other
// command is the owner's and stays as written.
func selfScorer(scorer, exe string) string {
	if !pulseScorer(scorer) {
		return scorer
	}
	rest := strings.TrimPrefix(strings.TrimLeft(scorer, " \t"), "pulse")
	return "'" + strings.ReplaceAll(exe, "'", `'\''`) + "'" + rest
}

// scorerEnv is the harness's isolated environment plus the measured commit, so
// a scorer can tell the tree it judges from wherever the harness moved HEAD,
// and the measuring binary, so a private judge the console wrote finds pulse
// without PATH (ADR-032).
func scorerEnv(home, commit, exe string) []string {
	env := append(isolatedEnv(home), "PULSE_MEASURE_COMMIT="+commit)
	if exe != "" {
		env = append(env, "PULSE_MEASURE_BINARY="+exe)
	}
	return env
}

func measure(ctx context.Context, s *store.Store, project, repo, ref string, opts MeasureOptions) (scores *Scores, retErr error) {
	if _, err := validProject(s, project); err != nil {
		return nil, err
	}
	fixtures, err := selectFixtures(s, project, opts.Fixtures)
	if err != nil {
		return nil, err
	}
	private, state, err := loadPrivate(s)
	if err != nil {
		return nil, err
	}
	if state.Pending != 0 {
		return nil, privateError("private templates are pending; the owner must fill them before measurement")
	}
	// An unlocatable binary only matters to a public pulse scorer; a private
	// judge falls back to PATH and a private-only set still measures.
	exe, err := executable()
	if err != nil {
		exe = ""
	}
	matrix := []measuredFixture{}
	for _, f := range fixtures {
		if !finite(f.Weight) || f.Weight <= 0 || strings.TrimSpace(f.Request) == "" ||
			store.SecretShaped(f.Request) || store.SecretShaped(f.Scorer) {
			return nil, inputError("Cannot measure fixture set", "a public fixture has invalid weight or unsafe text", "pulse evolve fixture list --project "+project)
		}
		scorer := f.Scorer
		if pulseScorer(scorer) {
			if exe == "" {
				return nil, inputError("Cannot run measurement", "the pulse executable behind the starter scorers cannot be located", "pulse evolve fixture list --project "+project)
			}
			scorer = selfScorer(scorer, exe)
		}
		matrix = append(matrix, measuredFixture{f.Name, f.Kind, f.Request, []string{"sh", "-c", scorer}, f.Weight})
	}
	for i, f := range private {
		matrix = append(matrix, measuredFixture{fmt.Sprintf("H%d", i+1), "heldout", f.request, []string{f.scorer}, 1})
	}
	if len(matrix) == 0 {
		return nil, inputError("No fixtures to measure", "this project has no enabled evaluators", "pulse evolve fixture seed --project "+project+"  (starter set), or pulse evolve fixture add --project "+project+" --name N --request FILE --scorer CMD")
	}
	repo, commit, err := resolveRevision(ctx, repo, ref)
	if err != nil {
		return nil, err
	}
	if value := s.Setting("evolve.heldout_dir", ""); value != "" {
		dir, err := privateDir(s, value)
		if err != nil {
			return nil, err
		}
		if within(dir, repo) {
			return nil, privateError("private fixtures must be outside the measured tree as well as registered projects")
		}
	}
	minutes, err := strconv.ParseFloat(s.Setting("minutes_per_job", ""), 64)
	if err != nil || !finite(minutes) || minutes <= 0 || minutes > float64((1<<63-1)/int64(time.Minute)) {
		return nil, inputError("Cannot run measurement", "minutes_per_job must allow a positive bounded deadline", "pulse settings set minutes_per_job 10")
	}
	timeout := time.Duration(minutes * float64(time.Minute))
	keys, err := projectAnswerKeys(s, project)
	if err != nil {
		return nil, err
	}
	base, err := fleetRoot(s, true)
	if err != nil {
		return nil, err
	}
	id := board.NewID(s, "measure")
	root := filepath.Join(base, id)
	if within(root, repo) {
		return nil, &ViolationError{Reason: "measurement clones would be inside the measured checkout"}
	}
	if err := os.Mkdir(root, 0o700); err != nil {
		return nil, err
	}
	defer func() {
		if err := removeManaged(s, id); err != nil {
			retErr = errors.Join(retErr, fmt.Errorf("measurement cleanup failed: %w", err))
			scores = nil
		}
	}()
	scores = &Scores{
		Project: project, Repo: repo, Ref: commit, Fixtures: []FixtureScore{},
		Heldout: len(private), HeldoutGenerated: state.Generated, Sandbox: root, Shadow: opts.Shadow,
	}
	maxWeight := 0.0
	for _, f := range matrix {
		maxWeight = max(maxWeight, f.weight)
	}
	total, weights := 0.0, 0.0
	for i, f := range matrix {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		label := f.name
		if f.kind == "heldout" {
			label = "held-out fixture"
		}
		if opts.Out != nil {
			fmt.Fprintf(opts.Out, "[%d/%d] %s: isolated measurement\n", i+1, len(matrix), label)
		}
		clone := filepath.Join(root, strconv.Itoa(i+1))
		if err := strippedClone(ctx, repo, commit, clone, keys); err != nil {
			return nil, err
		}
		home := filepath.Join(root, "homes", strconv.Itoa(i+1))
		if err := os.MkdirAll(home, 0o700); err != nil {
			return nil, err
		}
		argv, err := harnessArgs(s.Setting("runner_cmd", ""), f.request)
		if err != nil {
			return nil, err
		}
		jobCtx, cancel := context.WithTimeout(ctx, timeout)
		err = runProcess(jobCtx, clone, isolatedEnv(home), argv, io.Discard)
		cancel()
		if err != nil {
			return nil, measurementFailure(ctx, label, "harness", err)
		}
		if err := checkClone(ctx, clone, keys); err != nil {
			return nil, err
		}
		scoreCtx, cancel := context.WithTimeout(ctx, timeout)
		value, reason, err := scoreClone(scoreCtx, clone, scorerEnv(home, commit, exe), f.scorer)
		cancel()
		if err != nil {
			return nil, measurementFailure(ctx, label, "read-only scorer", err)
		}
		scores.Fixtures = append(scores.Fixtures, FixtureScore{Name: f.name, Kind: f.kind, Score: value, Weight: f.weight})
		total += value * (f.weight / maxWeight)
		weights += f.weight / maxWeight
		if opts.Out != nil {
			// A held-out scorer's words could name what must stay private; only a
			// public fixture's reason is shown, and never a secret-shaped one.
			if reason != "" && f.kind != "heldout" && !store.SecretShaped(reason) {
				fmt.Fprintf(opts.Out, "[%d/%d] %s: %.4f (%s)\n", i+1, len(matrix), label, value, reason)
			} else {
				fmt.Fprintf(opts.Out, "[%d/%d] %s: %.4f\n", i+1, len(matrix), label, value)
			}
		}
	}
	scores.Total = total / weights
	return scores, nil
}

func measurementFailure(ctx context.Context, label, step string, err error) error {
	if ctx.Err() != nil {
		return ctx.Err()
	}
	var violation *ViolationError
	if errors.As(err, &violation) {
		return violation
	}
	why := step + " failed; command output is withheld"
	if errors.Is(err, context.DeadlineExceeded) {
		why = step + " exceeded minutes_per_job"
	}
	return inputError("Measurement failed for "+label, why, "pulse evolve fixture list (owner: review the scorer and runner_cmd)")
}

// scoreClone runs the scorer inside the frozen clone and returns its score and
// the first line it wrote to stderr - a starter scorer's reason for a 0 - cut
// to one short line. The reason is for the owner's progress output only.
func scoreClone(ctx context.Context, clone string, env, scorer []string) (value float64, reason string, retErr error) {
	root, err := os.OpenRoot(clone)
	if err != nil {
		return 0, "", err
	}
	defer root.Close()
	restore, err := freezeTree(root)
	if err != nil {
		return 0, "", err
	}
	defer func() { retErr = errors.Join(retErr, restore()) }()
	before, err := treeFingerprint(root)
	if err != nil {
		return 0, "", err
	}
	out := &boundedOutput{limit: 64 << 10}
	stderr := &boundedOutput{limit: 4 << 10}
	runErr := runProcessTo(ctx, clone, env, scorer, out, stderr)
	after, err := treeFingerprint(root)
	if err != nil || before != after {
		return 0, "", &ViolationError{Reason: "scorer changed the measured tree"}
	}
	if runErr != nil {
		return 0, "", runErr
	}
	if out.overflow {
		return 0, "", errors.New("scorer output exceeds its limit")
	}
	value, err = parseScore(out.buf.String())
	return value, firstLine(stderr.buf.String(), 200), err
}

// firstLine is the first non-blank line of text, cut to limit characters.
func firstLine(text string, limit int) string {
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		if runes := []rune(line); len(runes) > limit {
			line = string(runes[:limit-1]) + "…"
		}
		return line
	}
	return ""
}

func parseScore(text string) (float64, error) {
	if v, err := strconv.ParseFloat(strings.TrimSpace(text), 64); err == nil && scoreValue(v) {
		return v, nil
	}
	var found bool
	var result float64
	for _, line := range strings.Split(text, "\n") {
		key, value, ok := strings.Cut(strings.TrimSpace(line), ":")
		if !ok || !strings.EqualFold(strings.TrimSpace(key), "score") {
			continue
		}
		v, err := strconv.ParseFloat(strings.TrimSpace(value), 64)
		if found || err != nil || !scoreValue(v) {
			return 0, errors.New("scorer must print exactly one finite score in [0,1]")
		}
		found, result = true, v
	}
	if !found {
		return 0, errors.New("scorer must print a number or score: X in [0,1]")
	}
	return result, nil
}
