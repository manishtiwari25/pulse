// Package runner is the Go port of the PULSE runner: the planner, the worker
// harness (`pulse work`), landing, the daemon loop, the service files, the
// executors (local and ssh hands), `pulse hands` (the forced command on a
// paired machine), `pulse pair`, `pulse env` and `pulse onboard`.
//
// The rules, messages and store rows mirror legacy/pulse.py; behaviour that
// intentionally differs is called out at the site (see the D-E, D-J, D-K and
// D-L decisions in the build brief).
package runner

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

const isoLayout = "2006-01-02T15:04:05Z"

// Constants shared with legacy/pulse.py.
const (
	humanAgent  = store.HumanAgent
	commonsSlug = store.CommonsSlug
)

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

func fail(code int, format string, args ...any) error {
	return cli.Errorf(code, fmt.Sprintf(format, args...), "", "")
}

func stampOf(iso string) string {
	return strings.NewReplacer("-", "", ":", "").Replace(iso)
}

func parseISO(value string) (time.Time, error) {
	t, err := time.Parse(isoLayout, value)
	if err != nil {
		return time.Time{}, fail(2, "Not a UTC time like 2026-09-05T08:00:00Z: %q", value)
	}
	return t, nil
}

func mustISO(value string) time.Time {
	t, err := parseISO(value)
	if err != nil {
		return time.Time{}
	}
	return t
}

func formatISO(t time.Time) string { return t.UTC().Format(isoLayout) }

func plusHours(iso string, hours float64) string {
	return formatISO(mustISO(iso).Add(time.Duration(hours * float64(time.Hour))))
}

func plusMinutes(iso string, minutes float64) string {
	return formatISO(mustISO(iso).Add(time.Duration(minutes * float64(time.Minute))))
}

func dayNumber(value string) int {
	if len(value) < 10 {
		return 0
	}
	t, err := time.Parse("2006-01-02", value[:10])
	if err != nil {
		return 0
	}
	return int(t.Unix()/86400) + 719163 // proleptic Gregorian ordinal like date.toordinal()
}

var nonSlug = regexp.MustCompile(`[^a-z0-9]+`)

func slugify(name string, limit int) string {
	slug := strings.Trim(nonSlug.ReplaceAllString(strings.ToLower(name), "-"), "-")
	if len(slug) > limit {
		slug = slug[:limit]
	}
	if slug == "" {
		return "x"
	}
	slug = strings.TrimRight(slug, "-")
	if slug == "" {
		return "x"
	}
	return slug
}

func sha256Text(text string) string {
	sum := sha256.Sum256([]byte(text))
	return hex.EncodeToString(sum[:])
}

// trunc is Python trunc(): cut to n-1 runes and add "~" when longer than n.
func trunc(text string, n int) string {
	r := []rune(text)
	if len(r) > n {
		return string(r[:n-1]) + "~"
	}
	return text
}

func plural(n int, word string) string {
	if n == 1 {
		return word
	}
	return word + "s"
}

func isInside(path, root string) bool {
	p, err1 := filepath.Abs(path)
	r, err2 := filepath.Abs(root)
	if err1 != nil || err2 != nil {
		return false
	}
	if rp, err := filepath.EvalSymlinks(p); err == nil {
		p = rp
	}
	if rr, err := filepath.EvalSymlinks(r); err == nil {
		r = rr
	}
	rel, err := filepath.Rel(r, p)
	if err != nil {
		return false
	}
	return rel == "." || (rel != ".." && !strings.HasPrefix(rel, "../"))
}

func newID(prefix, now string) string {
	b := make([]byte, 3)
	_, _ = rand.Read(b)
	return fmt.Sprintf("%s-%s-%s", prefix, stampOf(now), hex.EncodeToString(b))
}

// gitResult mirrors subprocess.CompletedProcess for git calls.
type gitResult struct {
	Code   int
	Stdout string
	Stderr string
	Err    error
}

func (g gitResult) ok() bool { return g.Err == nil && g.Code == 0 }

func gitTimeout(repo string, timeout time.Duration, args ...string) gitResult {
	return gitTimeoutEnv(repo, timeout, nil, args...)
}

// gitTimeoutEnv is gitTimeout with extra environment entries (KEY=value) for the one command.
func gitTimeoutEnv(repo string, timeout time.Duration, env []string, args ...string) gitResult {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	if repo != "" {
		args = append([]string{"-C", repo}, args...)
	}
	cmd := exec.CommandContext(ctx, "git", args...)
	cmd.Env = gitEnv(env)
	var out, errb strings.Builder
	cmd.Stdout = &out
	cmd.Stderr = &errb
	err := cmd.Run()
	res := gitResult{Stdout: out.String(), Stderr: errb.String()}
	if err != nil {
		if ee, ok := err.(*exec.ExitError); ok {
			res.Code = ee.ExitCode()
		} else {
			res.Code = -1
			res.Err = err
		}
	}
	return res
}

// gitEnv is the environment every runner git command runs with: no credential prompts, and no optional
// locks - a read (status, diff) does not take .git/index.lock to refresh the index on the side, so a
// probe the timeout kills on a loaded disk cannot leave a lock that holds every later landing and
// self-update ("Unable to create index.lock: File exists"). Writes lock as before.
func gitEnv(extra []string) []string {
	// Isolate PULSE's git from the host's git configuration and macOS Xcode license
	// prompts. The host may have 1Password SSH signing, GPG signing, or a global
	// config that conflicts with PULSE's per-clone identity setup. PULSE sets every
	// git option it needs explicitly (user.name/email, commit.gpgsign, etc.).
	base := []string{
		"GIT_TERMINAL_PROMPT=0",
		"GIT_OPTIONAL_LOCKS=0",
		"GIT_CONFIG_NOSYSTEM=1",
		"GIT_CONFIG_GLOBAL=/dev/null",
	}
	if runtime.GOOS == "darwin" {
		// Use the command-line tools git instead of /usr/bin/git, which prompts for
		// the Xcode license on a fresh macOS install or after updates.
		base = append(base, "DEVELOPER_DIR=/Library/Developer/CommandLineTools")
	}
	return append(append(os.Environ(), base...), extra...)
}

func git(repo string, args ...string) gitResult {
	return gitTimeout(repo, 60*time.Second, args...)
}

func gitCheck(repo string, args ...string) (gitResult, error) {
	res := git(repo, args...)
	if !res.ok() {
		return res, fmt.Errorf("git %s failed in %s: %s", strings.Join(args, " "), repo, strings.TrimSpace(res.Stderr))
	}
	return res, nil
}

func gitBranch(repo string) string {
	res := gitTimeout(repo, 10*time.Second, "rev-parse", "--abbrev-ref", "HEAD")
	if !res.ok() {
		return ""
	}
	return strings.TrimSpace(res.Stdout)
}

// row is a map-shaped SQL row, the equivalent of sqlite3.Row turned into a dict.
type row map[string]any

func (r row) str(key string) string {
	switch v := r[key].(type) {
	case nil:
		return ""
	case string:
		return v
	case []byte:
		return string(v)
	case int64:
		return fmt.Sprint(v)
	case float64:
		return fmt.Sprint(v)
	default:
		return fmt.Sprint(v)
	}
}

func (r row) int(key string) int {
	switch v := r[key].(type) {
	case int64:
		return int(v)
	case int:
		return v
	case float64:
		return int(v)
	case string:
		var n int
		fmt.Sscanf(v, "%d", &n)
		return n
	case []byte:
		var n int
		fmt.Sscanf(string(v), "%d", &n)
		return n
	}
	return 0
}

func (r row) float(key string) float64 {
	switch v := r[key].(type) {
	case int64:
		return float64(v)
	case float64:
		return v
	case string:
		var f float64
		fmt.Sscanf(v, "%g", &f)
		return f
	}
	return 0
}

type querier interface {
	Query(query string, args ...any) (*sql.Rows, error)
	QueryRow(query string, args ...any) *sql.Row
	Exec(query string, args ...any) (sql.Result, error)
}

func queryRows(q querier, query string, args ...any) ([]row, error) {
	rs, err := q.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rs.Close()
	cols, err := rs.Columns()
	if err != nil {
		return nil, err
	}
	var out []row
	for rs.Next() {
		vals := make([]any, len(cols))
		ptrs := make([]any, len(cols))
		for i := range vals {
			ptrs[i] = &vals[i]
		}
		if err := rs.Scan(ptrs...); err != nil {
			return nil, err
		}
		r := row{}
		for i, c := range cols {
			if b, ok := vals[i].([]byte); ok {
				vals[i] = string(b)
			}
			r[c] = vals[i]
		}
		out = append(out, r)
	}
	return out, rs.Err()
}

func queryOne(q querier, query string, args ...any) (row, error) {
	rows, err := queryRows(q, query, args...)
	if err != nil || len(rows) == 0 {
		return nil, err
	}
	return rows[0], nil
}

func mustRows(q querier, query string, args ...any) []row {
	rows, err := queryRows(q, query, args...)
	if err != nil {
		panic(err)
	}
	return rows
}

func mustOne(q querier, query string, args ...any) row {
	r, err := queryOne(q, query, args...)
	if err != nil {
		panic(err)
	}
	return r
}

func exec1(q querier, query string, args ...any) {
	if _, err := q.Exec(query, args...); err != nil {
		panic(err)
	}
}

// eventAt writes an event with an explicit clock (Python event(con, ...) uses utc_now(),
// which tests pin through PULSE_NOW; the store's Now honours the same pin).
func eventAt(q querier, at, actor, action, subject string, detail any) {
	text := ""
	switch d := detail.(type) {
	case nil:
	case string:
		text = d
	default:
		text, _ = store.PyJSON(d)
	}
	exec1(q, "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)", at, actor, action, subject, text)
}

// recover1 turns the panics of the must* helpers back into an error at a
// command boundary.
func recover1(err *error) {
	if r := recover(); r != nil {
		if e, ok := r.(error); ok {
			*err = e
			return
		}
		*err = fmt.Errorf("%v", r)
	}
}

func jsonList(text string) []string {
	var out []string
	if text == "" {
		return out
	}
	_ = json.Unmarshal([]byte(text), &out)
	return out
}

func jsonMap(text string) map[string]any {
	out := map[string]any{}
	if text == "" {
		return out
	}
	_ = json.Unmarshal([]byte(text), &out)
	return out
}

func pyJSON(v any) string {
	s, _ := store.PyJSON(v)
	return s
}

func fileExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

func readFile(path string) string {
	b, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	return string(b)
}

func homeDir() string {
	h, err := os.UserHomeDir()
	if err != nil {
		return os.Getenv("HOME")
	}
	return h
}

// shellQuote is shlex.quote().
func shellQuote(s string) string {
	if s == "" {
		return "''"
	}
	if safe.MatchString(s) {
		return s
	}
	return "'" + strings.ReplaceAll(s, "'", `'"'"'`) + "'"
}

var safe = regexp.MustCompile(`^[a-zA-Z0-9@%+=:,./_-]+$`)

func shellJoin(args []string) string {
	parts := make([]string, len(args))
	for i, a := range args {
		parts[i] = shellQuote(a)
	}
	return strings.Join(parts, " ")
}

// splitShell is a small shlex.split (POSIX mode) for worker commands.
func splitShell(s string) []string {
	var out []string
	var cur strings.Builder
	inWord := false
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c == '\'':
			inWord = true
			j := strings.IndexByte(s[i+1:], '\'')
			if j < 0 {
				cur.WriteString(s[i+1:])
				i = len(s)
			} else {
				cur.WriteString(s[i+1 : i+1+j])
				i += j + 1
			}
		case c == '"':
			inWord = true
			i++
			for i < len(s) && s[i] != '"' {
				if s[i] == '\\' && i+1 < len(s) && strings.ContainsRune("\"\\$`\n", rune(s[i+1])) {
					i++
				}
				cur.WriteByte(s[i])
				i++
			}
		case c == '\\' && i+1 < len(s):
			inWord = true
			i++
			cur.WriteByte(s[i])
		case c == ' ' || c == '\t' || c == '\n':
			if inWord {
				out = append(out, cur.String())
				cur.Reset()
				inWord = false
			}
		default:
			inWord = true
			cur.WriteByte(c)
		}
	}
	if inWord {
		out = append(out, cur.String())
	}
	return out
}

func settingInt(s *store.Store, key string) int { return s.SettingInt(key) }
