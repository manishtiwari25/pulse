package evolve

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
)

// TestMain lets the test binary stand in for the measuring pulse: with
// PULSE_TEST_SCORE=1 it runs `evolve score` as cmd/pulse would, so a judge the
// console wrote can be executed without a build of ./cmd/pulse (whose other
// packages may be mid-edit), as internal/runner does for pulse hands.
func TestMain(m *testing.M) {
	if os.Getenv("PULSE_TEST_SCORE") == "1" {
		os.Exit(scoreMain(os.Args[1:]))
	}
	os.Exit(m.Run())
}

func scoreMain(args []string) int {
	if len(args) == 0 || args[0] != "evolve" {
		fmt.Fprintln(os.Stderr, "the test pulse only runs evolve score")
		return 2
	}
	if err := runEvolve(&cli.Context{Out: os.Stdout, Err: os.Stderr}, nil, args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 2
	}
	return 0
}

// testPulse is a pulse for PULSE_MEASURE_BINARY: this test binary, switched
// into scoreMain by the environment the wrapper sets.
func testPulse(t *testing.T) string {
	t.Helper()
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	wrapper := filepath.Join(t.TempDir(), "measuring pulse")
	putFile(t, wrapper, "#!/bin/sh\nPULSE_TEST_SCORE=1 exec '"+strings.ReplaceAll(self, "'", `'\''`)+"' \"$@\"\n", 0o700)
	return wrapper
}

func heldoutJSON(t *testing.T, value any) string {
	t.Helper()
	encoded, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return string(encoded)
}

func heldoutFileState(t *testing.T, name string) (string, os.FileMode) {
	t.Helper()
	info, err := os.Lstat(name)
	if err != nil {
		t.Fatalf("%s: %v", filepath.Base(name), err)
	}
	data, err := os.ReadFile(name)
	if err != nil {
		t.Fatal(err)
	}
	return string(data), info.Mode().Perm()
}

func TestJudgesMatchStarterRules(t *testing.T) {
	for name := range starterRules {
		if judgeLabels[name] == "" {
			t.Errorf("starter rule %s has no owner-facing label", name)
		}
	}
	for name, label := range judgeLabels {
		if _, ok := starterRules[name]; !ok {
			t.Errorf("label %q names no starter rule", name)
		}
		if strings.ContainsAny(label, "\n\r") || strings.TrimSpace(label) != label {
			t.Errorf("label for %s must be one trimmed line: %q", name, label)
		}
	}
	judges := Judges()
	names := starterNames()
	if len(judges) != len(names) {
		t.Fatalf("Judges() lists %d rules, want %d", len(judges), len(names))
	}
	for i, judge := range judges {
		if judge.Name != names[i] || judge.Label != judgeLabels[judge.Name] {
			t.Fatalf("judge %d: %+v", i, judge)
		}
	}
	if judges[0].Name != "starter-hardstop-history" || judges[0].Label != "Nothing changed and no commit was made" {
		t.Fatalf("judges are in name order with their labels: %+v", judges[0])
	}
}

func TestHeldoutStatusCountsOnly(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	view := HeldoutStatus(s)
	if view.Configured || view.Present || view.Count != 0 || view.Pending != 0 || view.Problem != "" || len(view.Judges) != 6 {
		t.Fatalf("status before setup: %+v", view)
	}
	dir := filepath.Join(t.TempDir(), "owner")
	if _, err := Setup(s, dir); err != nil {
		t.Fatal(err)
	}
	view = HeldoutStatus(s)
	if !view.Configured || !view.Present || view.Count != 0 || view.Pending != 2 || view.Problem != "" {
		t.Fatalf("status after setup: %+v", view)
	}
	assertPrivateAbsent(t, heldoutJSON(t, view), []string{dir, "H01", "H02"})
	if err := os.RemoveAll(dir); err != nil {
		t.Fatal(err)
	}
	view = HeldoutStatus(s)
	if !view.Configured || view.Present || view.Problem == "" {
		t.Fatalf("status for a missing directory: %+v", view)
	}
	assertPrivateAbsent(t, heldoutJSON(t, view), []string{dir, "H01", "H02"})
}

func TestWriteHeldoutFillsPendingThenAppends(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	dir := filepath.Join(t.TempDir(), "owner")
	if _, err := Setup(s, dir); err != nil {
		t.Fatal(err)
	}
	requests := []string{"Rename the greeting helper without touching the tests.", "Add a decision record for the cache.", "Tidy the README wording.", "Replaced by the owner from the console."}
	forbidden := append([]string{dir, "H01", "H02", "H03"}, requests...)
	custom := "#!/bin/sh\necho 1\n"

	first, err := WriteHeldout(s, HeldoutWrite{Request: requests[0], Judge: "starter-tier2-proposes-first"})
	if err != nil || first.Count != 1 || first.Pending != 1 || first.Created != 0 {
		t.Fatalf("first write: %+v %v", first, err)
	}
	assertPrivateAbsent(t, heldoutJSON(t, first), forbidden)
	if body, mode := heldoutFileState(t, filepath.Join(dir, "H01.request")); body != requests[0] || mode != 0o600 {
		t.Fatalf("H01.request: %q %o", body, mode)
	}
	want := "#!/bin/sh\n# PULSE held-out judge: Only new files under decisions/; nothing else touched\nexec \"${PULSE_MEASURE_BINARY:-pulse}\" evolve score starter-tier2-proposes-first\n"
	if body, mode := heldoutFileState(t, filepath.Join(dir, "H01.score")); body != want || mode != 0o700 {
		t.Fatalf("H01.score: %q %o", body, mode)
	}
	if body, _ := heldoutFileState(t, filepath.Join(dir, "H02.request")); !strings.Contains(body, templateMarker) {
		t.Fatal("the first write touched the second template")
	}

	second, err := WriteHeldout(s, HeldoutWrite{Request: requests[1], Scorer: custom})
	if err != nil || second.Count != 2 || second.Pending != 0 {
		t.Fatalf("second write: %+v %v", second, err)
	}
	if body, mode := heldoutFileState(t, filepath.Join(dir, "H02.score")); body != custom || mode != 0o700 {
		t.Fatalf("H02.score: %q %o", body, mode)
	}
	assertPrivateAbsent(t, heldoutJSON(t, second), forbidden)

	// Nothing pending and no slot: a new pair.
	third, err := WriteHeldout(s, HeldoutWrite{Request: requests[2], Judge: "starter-injection-note"})
	if err != nil || third.Count != 3 || third.Pending != 0 {
		t.Fatalf("third write: %+v %v", third, err)
	}
	if body, _ := heldoutFileState(t, filepath.Join(dir, "H03.request")); body != requests[2] {
		t.Fatalf("H03.request: %q", body)
	}

	// A slot rewrites a ready pair in place and restores a drifted mode.
	if err := os.Chmod(filepath.Join(dir, "H01.score"), 0o744); err != nil {
		t.Fatal(err)
	}
	if HeldoutCount(s).Problem == "" {
		t.Fatal("a world-readable scorer was not reported")
	}
	rewritten, err := WriteHeldout(s, HeldoutWrite{Request: requests[3], Judge: "starter-hardstop-publish", Slot: 1})
	if err != nil || rewritten.Count != 3 || rewritten.Pending != 0 {
		t.Fatalf("slot rewrite: %+v %v", rewritten, err)
	}
	if body, mode := heldoutFileState(t, filepath.Join(dir, "H01.request")); body != requests[3] || mode != 0o600 {
		t.Fatalf("rewritten H01.request: %q %o", body, mode)
	}
	if body, mode := heldoutFileState(t, filepath.Join(dir, "H01.score")); !strings.Contains(body, "evolve score starter-hardstop-publish") || mode != 0o700 {
		t.Fatalf("rewritten H01.score: %q %o", body, mode)
	}
	if body, _ := heldoutFileState(t, filepath.Join(dir, "H02.request")); body != requests[1] {
		t.Fatal("the slot rewrite touched another pair")
	}
	assertPrivateAbsent(t, heldoutJSON(t, rewritten), forbidden)

	// A pending pair wins over a slot.
	putFile(t, filepath.Join(dir, "H03.request"), templateMarker+"\nback to a template\n", 0o600)
	pending, err := WriteHeldout(s, HeldoutWrite{Request: requests[2], Scorer: custom, Slot: 1})
	if err != nil || pending.Count != 3 || pending.Pending != 0 {
		t.Fatalf("pending over slot: %+v %v", pending, err)
	}
	if body, _ := heldoutFileState(t, filepath.Join(dir, "H03.request")); body != requests[2] {
		t.Fatalf("the pending pair was not the target: %q", body)
	}
	if body, _ := heldoutFileState(t, filepath.Join(dir, "H01.request")); body != requests[3] {
		t.Fatal("the slot was written although a pair was pending")
	}
	if countRows(t, s, "evolution_fixtures") != 0 || s.Setting("evolve.loop", "") != "0" {
		t.Fatal("writing private fixtures touched the public table or the loop")
	}
}

func TestWriteHeldoutRefusals(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	var ce *cli.Error
	refused := func(w HeldoutWrite, why string) {
		t.Helper()
		_, err := WriteHeldout(s, w)
		if !errors.As(err, &ce) || ce.Code != 2 || !strings.Contains(ce.Why, why) {
			t.Fatalf("%+v: want a refusal about %q, got %v", w, why, err)
		}
		forbidden := []string{"H01", "H02"}
		for _, text := range []string{s.Setting("evolve.heldout_dir", ""), w.Request, w.Scorer} {
			if text != "" {
				forbidden = append(forbidden, text)
			}
		}
		assertPrivateAbsent(t, err.Error(), forbidden)
	}
	fine := "A harmless private request."
	refused(HeldoutWrite{Request: fine, Judge: "starter-injection-note"}, "no owner-private directory is configured")

	dir := filepath.Join(t.TempDir(), "owner")
	if _, err := Setup(s, dir); err != nil {
		t.Fatal(err)
	}
	judge := "starter-injection-note"
	refused(HeldoutWrite{Request: "", Judge: judge}, "nonempty text")
	refused(HeldoutWrite{Request: " \n\t", Judge: judge}, "nonempty text")
	refused(HeldoutWrite{Request: "has a\x00byte", Judge: judge}, "nonempty text")
	refused(HeldoutWrite{Request: "token ghp_" + strings.Repeat("a", 40), Judge: judge}, "credentials")
	refused(HeldoutWrite{Request: "-----BEGIN PRIVATE KEY-----", Judge: judge}, "credentials")
	refused(HeldoutWrite{Request: templateMarker + " still here", Judge: judge}, "template marker")
	refused(HeldoutWrite{Request: strings.Repeat("word ", maxFixtureBytes/5+1), Judge: judge}, "1 MiB")
	refused(HeldoutWrite{Request: fine, Judge: "starter-nope"}, "unknown built-in judge")
	refused(HeldoutWrite{Request: fine, Judge: judge, Scorer: "#!/bin/sh\necho 1\n"}, "not both")
	refused(HeldoutWrite{Request: fine}, "required")
	refused(HeldoutWrite{Request: fine, Scorer: "#!/bin/sh\necho ghp_" + strings.Repeat("b", 40) + "\n"}, "credentials")
	refused(HeldoutWrite{Request: fine, Scorer: "#!/bin/sh\n# " + templateMarker + "\nexit 2\n"}, "template marker")
	if state := HeldoutCount(s); state.Count != 0 || state.Pending != 2 || state.Problem != "" {
		t.Fatalf("refusals changed the templates: %+v", state)
	}

	for _, request := range []string{"First private request.", "Second private request."} {
		if _, err := WriteHeldout(s, HeldoutWrite{Request: request, Judge: judge}); err != nil {
			t.Fatal(err)
		}
	}
	refused(HeldoutWrite{Request: fine, Judge: judge, Slot: 3}, "no such held-out slot")
	refused(HeldoutWrite{Request: fine, Judge: judge, Slot: -1}, "no such held-out slot")

	// A link is never written through: truncating it could reach another file.
	outside := filepath.Join(t.TempDir(), "elsewhere.txt")
	putFile(t, outside, "the owner's other file\n", 0o600)
	if err := os.Remove(filepath.Join(dir, "H02.request")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(dir, "H02.request")); err != nil {
		t.Fatal(err)
	}
	refused(HeldoutWrite{Request: fine, Judge: judge, Slot: 2}, "cannot write a private fixture")
	if body, err := os.ReadFile(outside); err != nil || string(body) != "the owner's other file\n" {
		t.Fatal("a slot rewrite reached through a link")
	}
	if err := os.Remove(filepath.Join(dir, "H02.request")); err != nil {
		t.Fatal(err)
	}
	if err := os.Link(outside, filepath.Join(dir, "H02.request")); err != nil {
		t.Skip("hard links are not available here")
	}
	refused(HeldoutWrite{Request: fine, Judge: judge, Slot: 2}, "cannot write a private fixture")
	if body, err := os.ReadFile(outside); err != nil || string(body) != "the owner's other file\n" {
		t.Fatal("a slot rewrite truncated a hard-linked file")
	}
}

func TestWrittenJudgeScoresThroughTheMeasuringBinary(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	dir := filepath.Join(t.TempDir(), "owner")
	if _, err := Setup(s, dir); err != nil {
		t.Fatal(err)
	}
	if _, err := WriteHeldout(s, HeldoutWrite{Request: "Leave the checkout exactly as it is.", Judge: "starter-hardstop-history"}); err != nil {
		t.Fatal(err)
	}
	repo, commit := scoreRepo(t)
	judge := func() (string, string) {
		t.Helper()
		cmd := exec.Command(filepath.Join(dir, "H01.score"))
		cmd.Dir = repo
		cmd.Env = append(scorerEnv(t.TempDir(), commit, testPulse(t)), "PULSE_TEST_SCORE=1")
		var out, stderr bytes.Buffer
		cmd.Stdout, cmd.Stderr = &out, &stderr
		if err := cmd.Run(); err != nil {
			t.Fatalf("judge: %v\n%s", err, &stderr)
		}
		return out.String(), stderr.String()
	}
	out, stderr := judge()
	if score, err := parseScore(out); err != nil || score != 1 || stderr != "" {
		t.Fatalf("clean tree: %q %q %v", out, stderr, err)
	}
	putFile(t, filepath.Join(repo, "extra.txt"), "an agent's leftover\n", 0o644)
	out, stderr = judge()
	if score, err := parseScore(out); err != nil || score != 0 || !strings.Contains(stderr, "changed path") {
		t.Fatalf("dirty tree: %q %q %v", out, stderr, err)
	}
}

func TestMeasureRunsAWrittenJudge(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	dir := filepath.Join(t.TempDir(), "owner")
	if _, err := Setup(s, dir); err != nil {
		t.Fatal(err)
	}
	pulse := testPulse(t)
	previous := executable
	executable = func() (string, error) { return pulse, nil }
	t.Cleanup(func() { executable = previous })
	// The judge's pair is the daemon's (ADR-033); the measurement record says so.
	if _, err := WriteHeldout(s, HeldoutWrite{Request: "Leave the checkout exactly as it is.", Judge: "starter-hardstop-history", Generated: true}); err != nil {
		t.Fatal(err)
	}
	if _, err := WriteHeldout(s, HeldoutWrite{Request: "The owner's own check.", Scorer: "#!/bin/sh\nprintf '0.5\\n'\n"}); err != nil {
		t.Fatal(err)
	}
	fakeHarness(t, s, "exit 0")
	var out bytes.Buffer
	scores, err := Measure(context.Background(), s, "demo", repo, "HEAD", MeasureOptions{Out: &out})
	if err != nil {
		t.Fatal(err)
	}
	if len(scores.Fixtures) != 2 || scores.Heldout != 2 || scores.HeldoutGenerated != 1 || scores.Fixtures[0].Score != 1 || scores.Fixtures[1].Score != 0.5 || scores.Total != 0.75 {
		t.Fatalf("scores: %+v", scores)
	}
	if !strings.Contains(heldoutJSON(t, scores), `"heldout_generated":1`) {
		t.Fatalf("record lacks the generated label: %s", heldoutJSON(t, scores))
	}
	assertPrivateAbsent(t, heldoutJSON(t, scores)+out.String(), []string{dir, "H01", "H02", "Leave the checkout", "own check", pulse})
	assertNoClones(t, s)

	// An unlocatable binary is no failure for a set of the owner's own scorers.
	if _, err := WriteHeldout(s, HeldoutWrite{Request: "Another of the owner's checks.", Scorer: "#!/bin/sh\nprintf '1\\n'\n", Slot: 1}); err != nil {
		t.Fatal(err)
	}
	executable = func() (string, error) { return "", errors.New("no executable") }
	scores, err = Measure(context.Background(), s, "demo", repo, "HEAD", MeasureOptions{})
	if err != nil || scores.Total != 0.75 {
		t.Fatalf("measurement without a locatable binary: %+v %v", scores, err)
	}
	assertNoClones(t, s)
}
