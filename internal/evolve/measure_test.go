package evolve

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestMeasureWeightedIsolatedAndPrivate(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	fixture(t, s, "demo", "F01", "printf 'score: 0.2\\n'", 1)
	fixture(t, s, "demo", "F02", "printf '0.6\\n'", 3)
	_, forbidden := privateSet(t, s)
	log := filepath.Join(t.TempDir(), "invocations")
	fakeHarness(t, s, `
test "$1" = "-p"
test -n "$2"
test ! -e "$PULSE_HOME/pulse.sqlite"
test "$PULSE_JOB" = measurement
test -z "${PULSE_AGENT:-}"
test ! -e .pulse
test ! -e keys/answer.txt
test -z "$(git remote)"
test -z "$(git tag --list)"
if git symbolic-ref -q HEAD >/dev/null; then exit 3; fi
printf '%s\n' "$PWD" >> '`+log+`'
printf 'changed in the clone\n' > value.txt
printf 'HIDDEN_HARNESS_OUTPUT\n'
`)
	var out bytes.Buffer
	scores, err := Measure(context.Background(), s, "demo", repo, "HEAD", MeasureOptions{Out: &out})
	if err != nil {
		t.Fatal(err)
	}
	if len(scores.Fixtures) != 4 || scores.Heldout != 2 || math.Abs(scores.Total-0.6) > 1e-9 {
		t.Fatal("weighted fixture scores were not aggregated correctly")
	}
	if scores.Fixtures[2].Name != "H1" || scores.Fixtures[3].Name != "H2" {
		t.Fatal("private scores did not receive opaque sequential indexes")
	}
	body, _ := json.Marshal(scores)
	assertPrivateAbsent(t, string(body)+out.String(), append(forbidden, "HIDDEN_HARNESS_OUTPUT"))
	original, err := os.ReadFile(filepath.Join(repo, "value.txt"))
	if err != nil || string(original) != "original\n" {
		t.Fatal("measurement changed the real project")
	}
	calls, err := os.ReadFile(log)
	if err != nil || len(lines(string(calls))) != 4 {
		t.Fatal("measurement did not make exactly one clone per fixture")
	}
	unique := map[string]bool{}
	for _, name := range lines(string(calls)) {
		unique[name] = true
		if _, err := os.Stat(name); !os.IsNotExist(err) {
			t.Fatal("measurement clone survived completion")
		}
	}
	if len(unique) != 4 || countRows(t, s, "evolution_archive") != 0 {
		t.Fatal("clones were reused or measurement wrote an archive")
	}
	assertNoClones(t, s)
}

func TestMeasureSubsetAlwaysIncludesPrivateSet(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	fixture(t, s, "demo", "F01", "printf 0.2", 1)
	fixture(t, s, "demo", "F02", "printf 0.6", 1)
	privateSet(t, s)
	fakeHarness(t, s, "exit 0")
	scores, err := Measure(context.Background(), s, "demo", repo, "HEAD", MeasureOptions{Fixtures: []string{"F02"}, Shadow: true})
	if err != nil || len(scores.Fixtures) != 3 || scores.Heldout != 2 || !scores.Shadow {
		t.Fatal("public subset silently excluded the private set")
	}
	if _, err := Measure(context.Background(), s, "demo", repo, "HEAD", MeasureOptions{Fixtures: []string{"missing"}}); err == nil {
		t.Fatal("unknown fixture restriction silently selected something else")
	}
	assertNoClones(t, s)
}

func TestMeasureReadOnlyScorerViolation(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	fixture(t, s, "demo", "F01", "chmod u+w value.txt; printf changed > value.txt; printf 1", 1)
	fakeHarness(t, s, "exit 0")
	_, err := Measure(context.Background(), s, "demo", repo, "HEAD", MeasureOptions{})
	var violation *ViolationError
	if !errors.As(err, &violation) {
		t.Fatalf("mutating scorer did not fail as a constraint: %v", err)
	}
	assertNoClones(t, s)
	if countRows(t, s, "evolution_archive") != 0 {
		t.Fatal("scorer violation wrote an archive")
	}
}

func TestMeasureFailureAndCancellationCleanup(t *testing.T) {
	for _, mode := range []string{"harness failure", "bad scorer", "cancel"} {
		t.Run(mode, func(t *testing.T) {
			s := testStore(t)
			repo := testProject(t, s, "demo")
			scorer := "printf 0.5"
			if mode == "bad scorer" {
				scorer = "printf 'invalid score\\n'"
			}
			fixture(t, s, "demo", "F01", scorer, 1)
			body := "exit 0"
			if mode == "harness failure" {
				body = "printf 'HIDDEN_FAILURE\\n' >&2; exit 7"
			}
			ctx := context.Background()
			cancel := func() {}
			if mode == "cancel" {
				body = "sleep 30"
				ctx, cancel = context.WithTimeout(ctx, 250*time.Millisecond)
			}
			defer cancel()
			fakeHarness(t, s, body)
			var out bytes.Buffer
			_, err := Measure(ctx, s, "demo", repo, "HEAD", MeasureOptions{Out: &out})
			if err == nil {
				t.Fatal("failed measurement reported success")
			}
			if mode == "cancel" && !errors.Is(err, context.DeadlineExceeded) {
				t.Fatalf("deadline was not preserved: %v", err)
			}
			if strings.Contains(err.Error()+out.String(), "HIDDEN_FAILURE") {
				t.Fatal("harness diagnostics escaped")
			}
			assertNoClones(t, s)
		})
	}
}

func TestScoreProtocol(t *testing.T) {
	for _, item := range []struct {
		text string
		want float64
		ok   bool
	}{
		{"0", 0, true}, {"1\n", 1, true}, {"notes\nscore: .25\n", .25, true},
		{"NaN", 0, false}, {"Inf", 0, false}, {"-0.1", 0, false}, {"1.1", 0, false},
		{"score: .5\nscore: .5\n", 0, false}, {"no score", 0, false},
	} {
		got, err := parseScore(item.text)
		if (err == nil) != item.ok || (item.ok && got != item.want) {
			t.Errorf("score protocol case %q: got %v, error %v", item.text, got, err)
		}
	}
}

func TestHarnessPromptIsOneLiteralArgument(t *testing.T) {
	request := "literal '; $(do-not-execute) \"\nrequest"
	for _, command := range []string{"'/tmp/runner with spaces' --flag", "fake -p {prompt} --allow-all-tools", "fake -p"} {
		args, err := harnessArgs(command, request)
		if err != nil {
			t.Fatal(err)
		}
		count := 0
		for _, arg := range args {
			if arg == request {
				count++
			}
		}
		if count != 1 {
			t.Fatal("fixture request was split or interpreted as command syntax")
		}
	}
	if _, err := harnessArgs("fake -p old-request", request); err == nil {
		t.Fatal("ambiguous configured prompt accepted")
	}
}
