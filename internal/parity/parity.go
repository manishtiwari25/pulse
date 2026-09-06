package parity

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// SnapshotDir is the read-only copy of the live store used for golden tests.
const SnapshotDir = "/tmp/pulse-parity"

// DefaultOracleDir is where the Python oracle (pulse.py from 02691f8) lives
// unless PULSE_ORACLE_DIR says otherwise.
const DefaultOracleDir = "/tmp/pulse-oracle"

// FixedNow is the clock every golden test passes to both sides.
const FixedNow = "2026-09-06T21:00:00Z"

// OracleDir is the directory holding pulse.py: $PULSE_ORACLE_DIR, else
// DefaultOracleDir.
func OracleDir() string {
	if d := os.Getenv("PULSE_ORACLE_DIR"); d != "" {
		return d
	}
	return DefaultOracleDir
}

// OracleScript is the path of the Python oracle, "" when it is missing.
func OracleScript() string {
	p := filepath.Join(OracleDir(), "pulse.py")
	if st, err := os.Stat(p); err != nil || st.IsDir() {
		return ""
	}
	return p
}

// Python is the python3 interpreter on PATH, "" when absent.
func Python() string {
	p, err := exec.LookPath("python3")
	if err != nil {
		return ""
	}
	return p
}

// Snapshot returns SnapshotDir when it holds a pulse.sqlite, else skips t.
func Snapshot(t *testing.T) string {
	t.Helper()
	if _, err := os.Stat(filepath.Join(SnapshotDir, "pulse.sqlite")); err != nil {
		t.Skipf("no parity snapshot at %s/pulse.sqlite", SnapshotDir)
	}
	return SnapshotDir
}

// RequireOracle skips t unless python3 and the oracle script are available.
func RequireOracle(t *testing.T) {
	t.Helper()
	if Python() == "" {
		t.Skip("python3 not on PATH")
	}
	if OracleScript() == "" {
		t.Skipf("no oracle at %s/pulse.py (set PULSE_ORACLE_DIR)", OracleDir())
	}
}

// OracleEnv is the environment the oracle runs with: the caller's, with
// PULSE_HOME set to home (the Python tool prints the default home's store path
// unless the environment names it) and the identity variables cleared so the
// oracle sees exactly the arguments it is given.
func OracleEnv(home string) []string {
	var env []string
	for _, kv := range os.Environ() {
		k := strings.SplitN(kv, "=", 2)[0]
		switch k {
		case "PULSE_HOME", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT", "PULSE_NOW", "PULSE_JOB":
			continue
		}
		env = append(env, kv)
	}
	return append(env, "PULSE_HOME="+home)
}

// Oracle runs `python3 <oracle>/pulse.py --home <home> args...` and returns
// its stdout. It skips t when python3, the script or the home's store is
// missing and fails t when the oracle exits non-zero.
func Oracle(t *testing.T, home string, args ...string) string {
	t.Helper()
	out, code, stderr := OracleRun(t, home, args...)
	if code != 0 {
		t.Fatalf("oracle %v exited %d: %s", args, code, stderr)
	}
	return out
}

// OracleRun is Oracle without the exit-code assertion: stdout, exit code and
// stderr, for commands whose failure is the point.
func OracleRun(t *testing.T, home string, args ...string) (string, int, string) {
	t.Helper()
	RequireOracle(t)
	if _, err := os.Stat(filepath.Join(home, "pulse.sqlite")); err != nil {
		t.Skipf("no store at %s/pulse.sqlite", home)
	}
	cmd := exec.Command(Python(), append([]string{OracleScript(), "--home", home}, args...)...)
	cmd.Env = OracleEnv(home)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	err := cmd.Run()
	code := 0
	if ee, ok := err.(*exec.ExitError); ok {
		code = ee.ExitCode()
	} else if err != nil {
		t.Fatalf("oracle %v: %v", args, err)
	}
	return stdout.String(), code, stderr.String()
}

// Normalize trims trailing whitespace from every line and drops trailing
// blank lines, the only tolerance the golden comparisons allow.
func Normalize(text string) string {
	lines := strings.Split(text, "\n")
	for i, l := range lines {
		lines[i] = strings.TrimRight(l, " \t\r")
	}
	for len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	return strings.Join(lines, "\n")
}

// Diff is a small line diff for failure messages: the first differing lines
// with context, "" when the normalized texts match.
func Diff(want, got string) string {
	w, g := strings.Split(Normalize(want), "\n"), strings.Split(Normalize(got), "\n")
	n := len(w)
	if len(g) > n {
		n = len(g)
	}
	var b strings.Builder
	shown := 0
	for i := 0; i < n && shown < 12; i++ {
		var wl, gl string
		if i < len(w) {
			wl = w[i]
		}
		if i < len(g) {
			gl = g[i]
		}
		if wl == gl {
			continue
		}
		if shown == 0 && i > 0 {
			b.WriteString("  ...\n")
		}
		b.WriteString("- " + wl + "\n+ " + gl + "\n")
		shown++
	}
	if b.Len() == 0 && len(w) != len(g) {
		b.WriteString("line counts differ")
	}
	if b.Len() > 0 {
		b.WriteString("(oracle " + itoa(len(w)) + " lines, go " + itoa(len(g)) + " lines)")
	}
	return b.String()
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	neg := n < 0
	if neg {
		n = -n
	}
	var d []byte
	for n > 0 {
		d = append([]byte{byte('0' + n%10)}, d...)
		n /= 10
	}
	if neg {
		return "-" + string(d)
	}
	return string(d)
}

// Rewrites are the intentional text differences between the Python oracle and
// the Go tool: the oracle still points agents at Markdown files that no longer
// exist; Go points at the command or record that replaced them. Each pair is
// applied to the oracle's output before a golden comparison, so every other
// byte must still match.
var Rewrites = [][2]string{
	{"no self-votes - docs/workflows/pulse.md)", "no self-votes - pulse guide)"},
	{"docs/evolution/STOP is present: the loop and ratification are paused until a human removes it",
		"STOP is present: the loop and ratification are paused until the owner lifts it"},
	{"passed: ratify per docs/workflows/amend.md", "passed: ratify with pulse amend apply"},
	{"unchecked: no docs/evolution/constitution.md", "unchecked: no constitution record"},
	{"(pulse pair; docs/workflows/executors.md)", "(pulse pair; pulse record show WF-executors)"},
}

// Expected is the oracle's output with Rewrites applied: what the Go tool
// must print.
func Expected(oracle string) string {
	for _, r := range Rewrites {
		oracle = strings.ReplaceAll(oracle, r[0], r[1])
	}
	return oracle
}

// Same compares oracle and Go output after Rewrites and Normalize and reports
// a readable failure on t when they differ.
func Same(t *testing.T, label, want, got string) bool {
	t.Helper()
	want = Expected(want)
	if Normalize(want) == Normalize(got) {
		return true
	}
	t.Errorf("%s: Go output differs from the oracle\n%s", label, Diff(want, got))
	return false
}
