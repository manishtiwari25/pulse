package evolve

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func testStore(t *testing.T) *store.Store {
	t.Helper()
	home := t.TempDir()
	for _, name := range []string{"PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT", "PULSE_NOW"} {
		t.Setenv(name, "")
	}
	t.Setenv("PULSE_HOME", home)
	t.Setenv("GIT_CONFIG_NOSYSTEM", "1")
	t.Setenv("GIT_CONFIG_GLOBAL", os.DevNull)
	bus.ResetDefault()
	if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(store.Options{Home: home})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func putFile(t *testing.T, name, body string, mode os.FileMode) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(name), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(name, []byte(body), mode); err != nil {
		t.Fatal(err)
	}
}

func testGit(t *testing.T, repo string, args ...string) string {
	t.Helper()
	argv := append([]string{"-c", "core.hooksPath=" + os.DevNull, "-c", "commit.gpgsign=false",
		"-c", "user.name=EVO fixture", "-c", "user.email=evo@example.invalid"}, args...)
	cmd := exec.Command("git", argv...)
	cmd.Dir = repo
	cmd.Env = gitEnv()
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("temporary repository git operation failed: %v (%s)", err, out)
	}
	return strings.TrimSpace(string(out))
}

func testProject(t *testing.T, s *store.Store, slug string) string {
	t.Helper()
	repo := t.TempDir()
	testGit(t, repo, "init", "--quiet", "--initial-branch=main")
	putFile(t, filepath.Join(repo, "value.txt"), "original\n", 0o644)
	putFile(t, filepath.Join(repo, ".pulse", "answer"), "withheld answer\n", 0o600)
	putFile(t, filepath.Join(repo, "keys", "answer.txt"), "another answer\n", 0o600)
	testGit(t, repo, "add", ".")
	testGit(t, repo, "commit", "--quiet", "-m", "temporary fixture")
	if _, err := board.AddProject(s, repo, slug, "", false, "this-command-must-not-run", "human", slug); err != nil {
		t.Fatal(err)
	}
	return repo
}

func joinTestAgent(t *testing.T, s *store.Store, name string) {
	t.Helper()
	if _, _, err := board.JoinAgent(s, board.JoinInput{Name: name, Runner: "fake", Model: "test", Session: name + "-session"}); err != nil {
		t.Fatal(err)
	}
}

func fixture(t *testing.T, s *store.Store, project, name, scorer string, weight float64) *Fixture {
	t.Helper()
	f, err := AddFixture(s, FixtureInput{
		Project: project, Name: name, Request: "Improve the value in the clone.", Scorer: scorer,
		Weight: weight, Actor: "human", AnswerKeys: []string{"keys/answer.txt"},
	})
	if err != nil {
		t.Fatal(err)
	}
	return f
}

func fakeHarness(t *testing.T, s *store.Store, body string) string {
	t.Helper()
	file := filepath.Join(t.TempDir(), "fake harness")
	putFile(t, file, "#!/bin/sh\nset -eu\n"+body+"\n", 0o700)
	if err := s.SetSetting("human", "runner_cmd", "'"+file+"'"); err != nil {
		t.Fatal(err)
	}
	return file
}

func privateSet(t *testing.T, s *store.Store) (string, []string) {
	t.Helper()
	dir := t.TempDir()
	if err := os.Chmod(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"owner-private-first", "owner-private-second"} {
		putFile(t, filepath.Join(dir, name+".request"), "This private request is supplied only to its isolated worker.", 0o600)
		putFile(t, filepath.Join(dir, name+".score"), "#!/bin/sh\nprintf '0.8\\n'\n", 0o700)
	}
	if err := s.SetSetting("human", "evolve.heldout_dir", dir); err != nil {
		t.Fatal(err)
	}
	return dir, []string{"owner-private-first", "owner-private-second", dir,
		"This private request is supplied only to its isolated worker."}
}

func countRows(t *testing.T, s *store.Store, table string) int {
	t.Helper()
	var n int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM " + table).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

func assertPrivateAbsent(t *testing.T, value string, private []string) {
	t.Helper()
	for _, forbidden := range private {
		if strings.Contains(value, forbidden) {
			t.Fatal("owner-private data escaped into observable output")
		}
	}
}

func assertNoClones(t *testing.T, s *store.Store) {
	t.Helper()
	entries, err := os.ReadDir(filepath.Join(s.Home, "fleet"))
	if err != nil && !os.IsNotExist(err) {
		t.Fatal(err)
	}
	if len(entries) != 0 {
		t.Fatal("measurement left disposable clones behind")
	}
}

func runTestCommand(t *testing.T, s *store.Store, name string, args ...string) (string, string, error) {
	t.Helper()
	var out, stderr bytes.Buffer
	command, ok := cli.Lookup(name)
	if !ok {
		t.Fatalf("command %s was not registered", name)
	}
	c := &cli.Context{Store: s, Home: s.Home, Out: &out, Err: &stderr, Project: "demo", Agent: "tester", Session: "tester-session"}
	err := command.Run(c, nil, args)
	return out.String(), stderr.String(), err
}

func requireJSON(t *testing.T, text string) {
	t.Helper()
	if !json.Valid([]byte(text)) {
		t.Fatal("command did not return one valid JSON document")
	}
}

func withinTime(t *testing.T, wait <-chan error) error {
	t.Helper()
	select {
	case err := <-wait:
		return err
	case <-time.After(8 * time.Second):
		t.Fatal("temporary runner did not complete")
		return context.DeadlineExceeded
	}
}
