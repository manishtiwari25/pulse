package board

import (
	"bytes"
	"errors"
	"flag"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

const testNow = "2026-09-06T10:00:00Z"

// oracleDir is where the Python oracle lives (PULSE_ORACLE_DIR, default /tmp/pulse-oracle).
func oracleDir() string {
	if d := os.Getenv("PULSE_ORACLE_DIR"); d != "" {
		return d
	}
	return "/tmp/pulse-oracle"
}

func oracleAvailable(t *testing.T) string {
	t.Helper()
	script := filepath.Join(oracleDir(), "pulse.py")
	if _, err := os.Stat(script); err != nil {
		t.Skip("no oracle at " + script)
	}
	if _, err := exec.LookPath("python3"); err != nil {
		t.Skip("no python3")
	}
	return script
}

// oracle runs the Python tool on home; env holds extra KEY=VALUE pairs.
func oracle(t *testing.T, home string, env []string, args ...string) (stdout, stderr string, code int) {
	t.Helper()
	script := oracleAvailable(t)
	cmd := exec.Command("python3", append([]string{script, "--home", home}, args...)...)
	cmd.Env = append(os.Environ(), "PULSE_HOME="+home, "PULSE_AGENT=", "PULSE_SESSION=", "PULSE_PROJECT=")
	cmd.Env = append(cmd.Env, env...)
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	cmd.Stdin = strings.NewReader("")
	err := cmd.Run()
	code = 0
	var ee *exec.ExitError
	if errors.As(err, &ee) {
		code = ee.ExitCode()
	} else if err != nil {
		t.Fatalf("oracle: %v", err)
	}
	return out.String(), errb.String(), code
}

func oracleMessage(stderr string) string {
	return strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(stderr), "pulse: "))
}

func openHome(t *testing.T, home string) *store.Store {
	t.Helper()
	// Fixture stores and identities are independent of the parent runner.
	t.Setenv("PULSE_HOME", home)
	t.Setenv("PULSE_JOB", "")
	t.Setenv("PULSE_AGENT", "")
	t.Setenv("PULSE_SESSION", "")
	t.Setenv("PULSE_PROJECT", "")
	s, err := store.Open(store.Options{Home: home, Create: true, Now: testNow})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func newHome(t *testing.T) (string, *store.Store) {
	t.Helper()
	home := t.TempDir()
	return home, openHome(t, home)
}

func TestNewHomeWithRunnerEnvironment(t *testing.T) {
	t.Setenv("PULSE_HOME", t.TempDir())
	names := []string{"PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT"}
	for _, name := range names {
		t.Setenv(name, "parent")
	}
	home, _ := newHome(t)
	if got := os.Getenv("PULSE_HOME"); got != home {
		t.Fatalf("PULSE_HOME = %q, want fixture home %q", got, home)
	}
	for _, name := range names {
		if got := os.Getenv(name); got != "" {
			t.Errorf("%s = %q, want empty fixture default", name, got)
		}
	}
}

// snapshotHome copies /tmp/pulse-parity/pulse.sqlite into a temp home.
func snapshotHome(t *testing.T) (string, *store.Store) {
	t.Helper()
	src := "/tmp/pulse-parity/pulse.sqlite"
	if _, err := os.Stat(src); err != nil {
		t.Skip("no snapshot at " + src)
	}
	home := t.TempDir()
	copyFile(t, src, filepath.Join(home, store.StoreName))
	return home, openHome(t, home)
}

func copyFile(t *testing.T, src, dst string) {
	t.Helper()
	data, err := os.ReadFile(src)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dst, data, 0o600); err != nil {
		t.Fatal(err)
	}
}

// cloneHome closes nothing; it copies the store files of home into a new temp home for the oracle.
func cloneHome(t *testing.T, home string) string {
	t.Helper()
	dst := t.TempDir()
	for _, suffix := range []string{"", "-wal", "-shm"} {
		src := filepath.Join(home, store.StoreName+suffix)
		if _, err := os.Stat(src); err == nil {
			copyFile(t, src, filepath.Join(dst, store.StoreName+suffix))
		}
	}
	return dst
}

// gitRepo makes a git checkout with one commit.
func gitRepo(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	if real, err := filepath.EvalSymlinks(dir); err == nil {
		dir = real
	}
	run := func(args ...string) {
		cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
		cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@x", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@x",
			"GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-q", "-b", "main")
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("# t\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-q", "-m", "init")
	return dir
}

// fixture: a store with one project and two agents (alice, bob).
func fixture(t *testing.T) (string, *store.Store, *store.Project) {
	t.Helper()
	bus.ResetDefault()
	home, s := newHome(t)
	repo := gitRepo(t)
	p, _, err := RegisterProject(s, repo, "demo", "", false, "", "human", "")
	if err != nil {
		t.Fatal(err)
	}
	for _, a := range []string{"alice", "bob"} {
		if _, _, err := JoinAgent(s, JoinInput{Name: a, Runner: "copilot", Model: "m", Session: "sess-" + a}); err != nil {
			t.Fatal(err)
		}
	}
	return home, s, p
}

// runCmd runs a registered command the way main does: flags interleaved with positionals.
func runCmd(t *testing.T, s *store.Store, c *cli.Context, argv ...string) (string, error) {
	t.Helper()
	cmd, ok := cli.Lookup(argv[0])
	if !ok {
		t.Fatalf("no command %s", argv[0])
	}
	fs := flag.NewFlagSet(argv[0], flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	if cmd.Flags != nil {
		cmd.Flags(fs)
	}
	var project, agent, session string
	if fs.Lookup("project") == nil {
		fs.StringVar(&project, "project", "", "")
	}
	if fs.Lookup("agent") == nil {
		fs.StringVar(&agent, "agent", "", "")
	}
	if fs.Lookup("session") == nil {
		fs.StringVar(&session, "session", "", "")
	}
	var positional []string
	args := argv[1:]
	for len(args) > 0 {
		a := args[0]
		if !strings.HasPrefix(a, "-") || a == "-" {
			positional = append(positional, a)
			args = args[1:]
			continue
		}
		if err := fs.Parse(args); err != nil {
			return "", err
		}
		args = fs.Args()
	}
	if c == nil {
		c = &cli.Context{}
	}
	var out bytes.Buffer
	c.Store, c.Home, c.Out, c.Err, c.Args = s, s.Home, &out, io.Discard, positional
	if project != "" {
		c.Project = project
	}
	if agent != "" {
		c.Agent = agent
	}
	if session != "" {
		c.Session = session
	}
	err := cmd.Run(c, fs, positional)
	return out.String(), err
}

func what(err error) string {
	var ce *cli.Error
	if errors.As(err, &ce) {
		return ce.What
	}
	if err == nil {
		return ""
	}
	return err.Error()
}

func code(err error) int {
	var ce *cli.Error
	if errors.As(err, &ce) {
		return ce.ExitCode()
	}
	if err == nil {
		return 0
	}
	return 1
}

func countRows(t *testing.T, s *store.Store, table, where string, args ...any) int {
	t.Helper()
	q := "SELECT COUNT(*) FROM " + table
	if where != "" {
		q += " WHERE " + where
	}
	var n int
	if err := s.DB.QueryRow(q, args...).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

func noTTY(t *testing.T) {
	t.Helper()
	old := StdinIsTTY
	StdinIsTTY = func() bool { return false }
	oldIn := Stdin
	Stdin = strings.NewReader("")
	t.Cleanup(func() { StdinIsTTY = old; Stdin = oldIn })
}
