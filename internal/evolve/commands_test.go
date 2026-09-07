package evolve

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
)

func TestGoCommandRegistrationAndRetiredLegacyNames(t *testing.T) {
	for name, group := range map[string]string{"evolve": "agents", "reflect": "agents", "fleet": "agents", "usage": "inspect"} {
		cmd, ok := cli.Lookup(name)
		if !ok || cmd.Group != group || !cmd.NoStore {
			t.Errorf("%s was not registered as a native %s command", name, group)
		}
	}
	for _, name := range []string{"nudge", "links"} {
		if _, ok := cli.Lookup(name); ok {
			t.Errorf("retired command %s was unexpectedly registered", name)
		}
	}
}

func TestCommandJSONAndShadowFindingNeverArchive(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	joinTestAgent(t, s, "tester")
	fakeHarness(t, s, "test ! -e \"$PULSE_HOME/pulse.sqlite\"; exit 0")
	_, forbidden := privateSet(t, s)
	oldStdin := Stdin
	Stdin = strings.NewReader("one public request")
	t.Cleanup(func() { Stdin = oldStdin })
	out, _, err := runTestCommand(t, s, "evolve", "fixture", "add", "--name", "F01", "--request", "-",
		"--scorer", "printf 0.5", "--answer-key", "keys/answer.txt", "--json")
	if err != nil {
		t.Fatal(err)
	}
	requireJSON(t, out)
	out, progress, err := runTestCommand(t, s, "evolve", "measure", "--shadow", "--json")
	if err != nil {
		t.Fatal(err)
	}
	requireJSON(t, out)
	assertPrivateAbsent(t, out+progress, forbidden)
	if countRows(t, s, "evolution_archive") != 0 {
		t.Fatal("shadow CLI wrote an archive row")
	}
	var kind, actor, body string
	if err := s.DB.QueryRow("SELECT kind, agent, body FROM posts ORDER BY id DESC LIMIT 1").Scan(&kind, &actor, &body); err != nil ||
		kind != "finding" || actor != "tester" {
		t.Fatal("shadow scores were not posted as the calling agent's finding")
	}
	assertPrivateAbsent(t, body, forbidden)
	for _, args := range [][]string{{"status", "--json"}, {"archive", "--json"}, {"fixture", "list", "--json"}} {
		before := countRows(t, s, "events")
		out, _, err := runTestCommand(t, s, "evolve", args...)
		if err != nil {
			t.Fatal(err)
		}
		requireJSON(t, out)
		if countRows(t, s, "events") != before {
			t.Fatal("read-only evolve command wrote an event")
		}
	}
	out, _, err = runTestCommand(t, s, "fleet", "prepare", "--n", "2", "--json")
	if err != nil {
		t.Fatal(err)
	}
	var fleet Fleet
	if err := json.Unmarshal([]byte(out), &fleet); err != nil || len(fleet.Paths) != 2 {
		t.Fatal("fleet prepare JSON did not contain both clone paths")
	}
	for _, args := range [][]string{{"verify", fleet.ID, "--json"}, {"list", "--json"}, {"clean", fleet.ID, "--json"}} {
		out, _, err := runTestCommand(t, s, "fleet", args...)
		if err != nil {
			t.Fatal(err)
		}
		requireJSON(t, out)
	}
	assertNoClones(t, s)
}

func TestReflectReadOnlyBoardNoticesBallotHygieneAndHealth(t *testing.T) {
	s := cycleReady(t)
	p, err := s.Project("demo")
	if err != nil {
		t.Fatal(err)
	}
	marker := filepath.Join(t.TempDir(), "health-was-run")
	if _, err := s.DB.Exec("UPDATE projects SET verify_cmd = ? WHERE slug='demo'", "touch '"+marker+"'"); err != nil {
		t.Fatal(err)
	}
	votedSuggestion(t, s, "ballot-item", "product", "value.txt", 2)
	testGit(t, p.Path, "branch", "evolve/stray")
	testGit(t, p.Path, "branch", "pulse/stray")
	before := countRows(t, s, "events")
	out, _, err := runTestCommand(t, s, "reflect", "--json", "--area", "evolution")
	if err != nil {
		t.Fatal(err)
	}
	var reflected Reflection
	if err := json.Unmarshal([]byte(out), &reflected); err != nil {
		t.Fatal("reflect JSON did not parse")
	}
	if reflected.Level != "D" || !strings.Contains(reflected.Board, "NOTICES") ||
		len(reflected.Ballot) == 0 || len(reflected.Hygiene.Branches) != 2 || len(reflected.Hygiene.Worktrees) != 1 {
		t.Fatal("reflection omitted required board, ladder or hygiene information")
	}
	commands := strings.Join(reflected.NextCommands, "\n")
	for _, command := range []string{"pulse post ", "pulse vote ", "pulse evolve cycle --project demo"} {
		if !strings.Contains(commands, command) {
			t.Errorf("reflection did not end with %s", command)
		}
	}
	if _, err := os.Stat(marker); !os.IsNotExist(err) || countRows(t, s, "events") != before {
		t.Fatal("reflection executed the project check or wrote to the store")
	}
}

func TestReflectMissingStoreStillObserves(t *testing.T) {
	home := t.TempDir()
	t.Setenv("PULSE_HOME", home)
	t.Setenv("PULSE_JOB", "")
	t.Chdir(t.TempDir())
	var out bytes.Buffer
	if err := runReflect(&cli.Context{Home: home, Out: &out, Err: &out}, nil, []string{"--json"}); err != nil {
		t.Fatal(err)
	}
	var result Reflection
	if err := json.Unmarshal(out.Bytes(), &result); err != nil || result.Level != "A" || result.NextHumanStep != "pulse init PATH" {
		t.Fatal("missing store prevented Level A observation")
	}
	if _, err := os.Stat(filepath.Join(home, "pulse.sqlite")); !os.IsNotExist(err) {
		t.Fatal("read-only reflection created a store")
	}
}

func TestUsageCLIWithoutStoreAndStopReminder(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	for _, key := range []string{"COPILOT_STATE_DIR", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "OPENCODE_STORE"} {
		t.Setenv(key, "")
	}
	var out bytes.Buffer
	c := &cli.Context{Home: t.TempDir(), Out: &out, Err: &out}
	if err := runUsage(c, nil, []string{"--days", "7"}); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"copilot", "claude", "codex", "opencode"} {
		if !strings.Contains(out.String(), name+": not found") {
			t.Errorf("usage omitted the absence of %s", name)
		}
	}
	out.Reset()
	if err := runUsage(c, nil, []string{"--json"}); err != nil {
		t.Fatal(err)
	}
	requireJSON(t, out.String())
	if !strings.Contains(StopReminder(nil), "pulse reflect") {
		t.Fatal("uninitialized Stop hook lacks the reflection command")
	}
	s := testStore(t)
	if err := s.SetSetting("human", "stop", "1"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(StopReminder(s), "STOP is engaged") || strings.Contains(StopReminder(s), "\n") {
		t.Fatal("Stop reminder did not respect the switch or one-paragraph contract")
	}
}

func TestScorerRefusesEscapingSymlink(t *testing.T) {
	clone := t.TempDir()
	outside := filepath.Join(t.TempDir(), "preserve")
	putFile(t, outside, "keep", 0o600)
	if err := os.Symlink(outside, filepath.Join(clone, "outside")); err != nil {
		t.Fatal(err)
	}
	if _, err := scoreClone(context.Background(), clone, isolatedEnv(t.TempDir()), []string{"sh", "-c", "printf 1"}); err == nil {
		t.Fatal("read-only scoring accepted an escaping symlink")
	}
	body, err := os.ReadFile(outside)
	if err != nil || string(body) != "keep" {
		t.Fatal("scoring touched an external symlink target")
	}
}
