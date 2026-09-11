package runner

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Executors (ADR-020): this machine and the remote hands the human paired.

const (
	localExecutor     = "local"
	unreachableAfter  = 3
	sshExitUnreach    = 255
	remoteExitTimeout = 124
	loginCheckPrompt  = "Reply with exactly the word ok and nothing else."
	nativeLoginPrompt = "Reply with exactly one line: SUMMARY: ok"
	loginCheckMinutes = 2
	loginExpiryWarn   = 7
	githubAPI         = "https://api.github.com"
)

var (
	executorStatuses  = []string{"active", "drained", "unreachable"}
	loginBackoffMins  = [2]int{15, 360}
	sshOptions        = []string{"-o", "BatchMode=yes", "-o", "ConnectTimeout=15", "-o", "ClearAllForwardings=yes"}
	loginFaultRe      = regexp.MustCompile(`(?i)(^|\n)not logged in:|not_logged_in|re-?authenticat|not logged in|authenticat|unauthori[sz]ed|\b401\b|Copilot Requests|log ?in`)
	tokenRe           = regexp.MustCompile(`(github_pat_|gho_|ghu_|ghp_|ghs_|ghr_)[A-Za-z0-9_]+`)
	jobDirRe          = regexp.MustCompile(`^[A-Za-z0-9._-]+$`)
	sshAddressRe      = regexp.MustCompile(`^([A-Za-z0-9._-]+)@([A-Za-z0-9._-]+|\[[0-9A-Fa-f:.]+\])(?::([0-9]{1,5}))?$`)
	workerEnv         = [][2]string{{"COPILOT_SUBAGENT_MAX_CONCURRENT", "256"}, {"COPILOT_SUBAGENT_MAX_DEPTH", "128"}}
	hookIsUnreachable = errors.New("unreachable")
	_                 = hookIsUnreachable
)

// ExecutorUnreachable: the ssh transport failed (exit 255).
type ExecutorUnreachable struct{ Msg string }

func (e *ExecutorUnreachable) Error() string { return e.Msg }

// ExecutorFault: the machine answered and a human must fix it.
type ExecutorFault struct{ Msg string }

func (e *ExecutorFault) Error() string { return e.Msg }

// ExecutorUnfit: the machine lacks a tool one project needs.
type ExecutorUnfit struct{ Msg, Tool, Project string }

func (e *ExecutorUnfit) Error() string { return e.Msg }

func loginFault(text string) bool { return loginFaultRe.MatchString(text) }

func redactTokens(text string) string { return tokenRe.ReplaceAllString(text, "[redacted]") }

func faultLine(text string, limit int) string {
	var lines []string
	for _, l := range strings.Split(text, "\n") {
		if l = strings.Join(strings.Fields(l), " "); l != "" {
			lines = append(lines, l)
		}
	}
	for _, l := range lines {
		if loginFaultRe.MatchString(l) {
			return cut(redactTokens(l), limit)
		}
	}
	if len(lines) == 0 {
		return ""
	}
	return cut(redactTokens(lines[0]), limit)
}

func cut(s string, n int) string {
	r := []rune(s)
	if len(r) > n {
		return string(r[:n])
	}
	return s
}

func tail(s string, n int) string {
	r := []rune(s)
	if len(r) > n {
		return string(r[len(r)-n:])
	}
	return s
}

func parseSSHAddress(text string) (string, string, int, error) {
	m := sshAddressRe.FindStringSubmatch(strings.TrimSpace(text))
	if m == nil {
		return "", "", 0, fail(2, "ssh address must be user@host[:port], got %s", pyRepr(text))
	}
	port := 22
	if m[3] != "" {
		port, _ = strconv.Atoi(m[3])
	}
	if port <= 0 || port >= 65536 {
		return "", "", 0, fail(2, "ssh port out of range in %s", pyRepr(text))
	}
	return m[1], m[2], port, nil
}

func normalizeProjects(text string) string {
	raw := strings.TrimSpace(text)
	if raw == "" || raw == "*" || raw == "all" {
		return "*"
	}
	set := map[string]bool{}
	for _, p := range strings.Split(raw, ",") {
		if strings.TrimSpace(p) != "" {
			set[slugify(p, 40)] = true
		}
	}
	var slugs []string
	for s := range set {
		if s != "" {
			slugs = append(slugs, s)
		}
	}
	sort.Strings(slugs)
	return strings.Join(slugs, ",")
}

func executorAllows(r row, slug string) bool {
	projects := "*"
	if r != nil && r.str("projects") != "" {
		projects = r.str("projects")
	}
	return projects == "*" || contains(strings.Split(projects, ","), slug)
}

func executorProjects(r row) []string {
	if r == nil || r.str("projects") == "" || r.str("projects") == "*" {
		return nil
	}
	return strings.Split(r.str("projects"), ",")
}

func localExecutorRow(s *store.Store) row {
	workers := settingInt(s, "workers")
	if workers < 0 {
		workers = 0
	}
	return row{"name": localExecutor, "kind": "local", "address": "", "workdir": "", "workers": int64(workers),
		"projects": "*", "status": "active", "last_probe_at": "", "probe_note": "this machine", "errors": int64(0),
		"backoff_minutes": int64(0), "next_probe_at": "", "runs_done": int64(0), "runs_failed": int64(0), "created_at": "", "created_by": ""}
}

// executorRows lists the registered hands; a removed executor (board.RemoveExecutor
// keeps its row for the runs it made) is no longer one of them.
func executorRows(s *store.Store, includeLocal bool) []row {
	rows := mustRows(s.DB, "SELECT * FROM executors WHERE status != 'removed' ORDER BY name")
	if includeLocal {
		return append([]row{localExecutorRow(s)}, rows...)
	}
	return rows
}

func executorRow(s *store.Store, name string) row {
	if name == "" || name == localExecutor {
		return localExecutorRow(s)
	}
	return mustOne(s.DB, "SELECT * FROM executors WHERE name = ? AND status != 'removed'", name)
}

func addExecutor(s *store.Store, now, name, address, workdir string, workers int, projects, createdBy string) (row, error) {
	name = slugify(name, 40)
	if name == "" || name == localExecutor || name == "x" {
		return nil, fail(2, "Executor name must be lowercase words with dashes and not %s", pyRepr(localExecutor))
	}
	if _, _, _, err := parseSSHAddress(address); err != nil {
		return nil, err
	}
	workdir = strings.TrimSpace(workdir)
	if !strings.HasPrefix(workdir, "/") || contains(strings.Split(workdir, "/"), "..") {
		return nil, fail(2, "--workdir must be an absolute path on the executor (e.g. /srv/pulse)")
	}
	if executorRow(s, name) != nil {
		return nil, fail(2, "Executor %s already exists", name)
	}
	if workers < 1 {
		return nil, fail(2, "--workers must be at least 1")
	}
	wd := strings.TrimRight(workdir, "/")
	if wd == "" {
		wd = "/"
	}
	if mustOne(s.DB, "SELECT 1 FROM executors WHERE name = ? AND status = 'removed'", name) != nil {
		// The row of removed hands stays for its history; pairing the name again revives it.
		exec1(s.DB, "UPDATE executors SET kind = 'ssh', address = ?, workdir = ?, workers = ?, projects = ?, status = 'drained', "+
			"created_at = ?, created_by = ?, last_probe_at = '', probe_note = '', next_probe_at = '' WHERE name = ?",
			strings.TrimSpace(address), wd, workers, normalizeProjects(projects), now, createdBy, name)
	} else {
		exec1(s.DB, "INSERT INTO executors (name, kind, address, workdir, workers, projects, status, created_at, created_by) "+
			"VALUES (?, 'ssh', ?, ?, ?, ?, 'drained', ?, ?)", name, strings.TrimSpace(address), wd, workers, normalizeProjects(projects), now, createdBy)
	}
	eventAt(s.DB, now, createdBy, "executor.add", name, map[string]any{"address": address, "workdir": workdir, "workers": workers, "projects": normalizeProjects(projects)})
	return executorRow(s, name), nil
}

func executorRunning(s *store.Store, name, now string) int {
	horizon := plusMinutes(now, -float64(2*settingInt(s, "minutes_per_job")+15))
	r := mustOne(s.DB, "SELECT COUNT(*) AS n FROM runs WHERE status = 'running' AND executor = ? AND started_at > ?", name, horizon)
	return r.int("n")
}

func executorFreeSlots(s *store.Store, r row, now string, inflight int) int {
	if r.str("status") != "active" {
		return 0
	}
	running := executorRunning(s, r.str("name"), now)
	if inflight > running {
		running = inflight
	}
	free := r.int("workers") - running
	if free < 0 {
		return 0
	}
	return free
}

func pickExecutor(s *store.Store, slug, now string) row {
	var best row
	bestRunning := 0
	for _, r := range executorRows(s, false) {
		if r.str("status") != "active" || !executorAllows(r, slug) {
			continue
		}
		if executorFreeSlots(s, r, now, 0) <= 0 {
			continue
		}
		running := executorRunning(s, r.str("name"), now)
		if best == nil || running < bestRunning || (running == bestRunning && r.str("name") < best.str("name")) {
			best, bestRunning = r, running
		}
	}
	if best != nil {
		return best
	}
	local := localExecutorRow(s)
	if executorFreeSlots(s, local, now, 0) > 0 {
		return local
	}
	return nil
}

func setExecutorStatus(s *store.Store, now, name, status, actor string, note *string) (row, error) {
	if name == localExecutor {
		return nil, fail(2, "The local executor is this machine; use `pulse settings set workers N` to size it")
	}
	if !contains(executorStatuses, status) {
		return nil, fail(2, "Status must be one of: %s", strings.Join(executorStatuses, ", "))
	}
	if executorRow(s, name) == nil {
		return nil, fail(2, "No executor %s", name)
	}
	var noteArg any
	if note != nil {
		noteArg = *note
	}
	exec1(s.DB, "UPDATE executors SET status = ?, probe_note = COALESCE(?, probe_note), errors = CASE WHEN ? = 'active' THEN 0 ELSE errors END, "+
		"backoff_minutes = CASE WHEN ? = 'active' THEN 0 ELSE backoff_minutes END, next_probe_at = CASE WHEN ? = 'active' THEN '' ELSE next_probe_at END, "+
		"healed_at = CASE WHEN ? = 'active' THEN '' ELSE healed_at END WHERE name = ?", status, noteArg, status, status, status, status, name)
	text := ""
	if note != nil {
		text = *note
	}
	eventAt(s.DB, now, actor, "executor."+status, name, text)
	return executorRow(s, name), nil
}

// noteExecutorRun is note_executor_run(): counters, transport errors, faults.
func noteExecutorRun(s *store.Store, now, name, status, note string, transportError, fault bool) {
	if name == localExecutor {
		return
	}
	r := executorRow(s, name)
	if r == nil {
		return
	}
	if fault {
		exec1(s.DB, "UPDATE executors SET errors = errors + 1, probe_note = ?, status = CASE WHEN status = 'active' THEN 'drained' ELSE status END "+
			"WHERE name = ?", trunc("fault: "+note, 300), name)
		eventAt(s.DB, now, "runner", "executor.drained", name, map[string]any{"fault": trunc(note, 200)})
		if loginFault(note) {
			exec1(s.DB, "UPDATE executors SET backoff_minutes = ?, next_probe_at = ? WHERE name = ?",
				loginBackoffMins[0], plusMinutes(now, float64(loginBackoffMins[0])), name)
		}
		return
	}
	if transportError {
		errs := r.int("errors") + 1
		failed := 1
		if status == "error" {
			failed = 0
		}
		if errs >= unreachableAfter && r.str("status") == "active" {
			exec1(s.DB, "UPDATE executors SET errors = ?, runs_failed = runs_failed + ?, status = 'unreachable', backoff_minutes = 1, "+
				"next_probe_at = ?, probe_note = ? WHERE name = ?", errs, failed, plusMinutes(now, 1), trunc(note, 200), name)
			eventAt(s.DB, now, "runner", "executor.unreachable", name, map[string]any{"errors": errs, "note": trunc(note, 200)})
		} else {
			exec1(s.DB, "UPDATE executors SET errors = ?, runs_failed = runs_failed + ? WHERE name = ?", errs, failed, name)
		}
		return
	}
	if status == "error" {
		return
	}
	done := 0
	if status == "done" || status == "dry-run" {
		done = 1
	}
	failed := 1
	if done == 1 || status == "stale" {
		failed = 0
	}
	exec1(s.DB, "UPDATE executors SET errors = 0, runs_done = runs_done + ?, runs_failed = runs_failed + ? WHERE name = ?", done, failed, name)
}

// ---- the ssh transport ---------------------------------------------------------

// sshCommand is ssh_command(): argv travels NUL-separated in base64.
func sshCommand(r row, args []string) ([]string, error) {
	user, host, port, err := parseSSHAddress(r.str("address"))
	if err != nil {
		return nil, err
	}
	var payload bytes.Buffer
	for _, a := range args {
		payload.WriteString(a)
		payload.WriteByte(0)
	}
	cmd := append([]string{"ssh"}, sshOptions...)
	cmd = append(cmd, "-p", strconv.Itoa(port), user+"@"+host, "b64 "+base64.StdEncoding.EncodeToString(payload.Bytes()))
	return cmd, nil
}

// sshRunner is what ssh_run does; tests swap it for a local pulse-hands.
type sshRunner func(ctx context.Context, argv []string, stdin []byte, timeout time.Duration) (int, string, string, error)

var runSSH sshRunner = runProcess

func runProcess(ctx context.Context, argv []string, stdin []byte, timeout time.Duration) (int, string, string, error) {
	out, err := runGroup(ctx, argv, "", environ(), stdin, timeout)
	if err != nil {
		if errors.Is(err, exec.ErrNotFound) {
			return -1, "", "", exec.ErrNotFound
		}
		return -1, out.Stdout, out.Stderr, err
	}
	if out.Timeout {
		if err := ctx.Err(); err != nil {
			return -1, out.Stdout, out.Stderr, err
		}
		return -1, out.Stdout, out.Stderr, context.DeadlineExceeded
	}
	return out.ExitCode, out.Stdout, out.Stderr, nil
}

func sshRun(r row, args []string, stdin []byte, timeout time.Duration) (int, string, string, error) {
	return sshRunContext(context.Background(), r, args, stdin, timeout)
}

func sshRunContext(ctx context.Context, r row, args []string, stdin []byte, timeout time.Duration) (int, string, string, error) {
	argv, err := sshCommand(r, args)
	if err != nil {
		return 0, "", "", err
	}
	rc, out, errText, err := runSSH(ctx, argv, stdin, timeout)
	if err != nil {
		if ctx.Err() != nil {
			return rc, out, errText, ctx.Err()
		}
		if errors.Is(err, exec.ErrNotFound) {
			return 0, "", "", &ExecutorUnreachable{"ssh is not installed on this machine"}
		}
		if errors.Is(err, context.DeadlineExceeded) {
			verb := ""
			if len(args) > 0 {
				verb = args[0]
			}
			return 0, "", "", &ExecutorUnreachable{fmt.Sprintf("ssh %s timed out after %ds", verb, int(timeout.Seconds()))}
		}
		return 0, "", "", err
	}
	if rc == sshExitUnreach {
		msg := tail(strings.TrimSpace(errText), 300)
		if msg == "" {
			msg = "ssh exit 255"
		}
		return 0, "", "", &ExecutorUnreachable{msg}
	}
	return rc, out, errText, nil
}

func adminSSHCommand(address, remoteCommand, controlPath string) ([]string, error) {
	user, host, port, err := parseSSHAddress(address)
	if err != nil {
		return nil, err
	}
	cmd := append([]string{"ssh"}, sshOptions...)
	if controlPath != "" {
		cmd = append(cmd, "-o", "ControlPath="+controlPath)
	}
	return append(cmd, "-p", strconv.Itoa(port), user+"@"+host, remoteCommand), nil
}

func adminSSHRun(address, remoteCommand string, stdin []byte, timeout time.Duration, controlPath string) (int, string, string, error) {
	argv, err := adminSSHCommand(address, remoteCommand, controlPath)
	if err != nil {
		return 0, "", "", err
	}
	rc, out, errText, err := runSSH(context.Background(), argv, stdin, timeout)
	if err != nil {
		if errors.Is(err, exec.ErrNotFound) {
			return 0, "", "", &ExecutorUnreachable{"ssh is not installed on this machine"}
		}
		if errors.Is(err, context.DeadlineExceeded) {
			user, _, _ := strings.Cut(address, "@")
			return 0, "", "", &ExecutorUnreachable{fmt.Sprintf("ssh to %s@... timed out after %ds", user, int(timeout.Seconds()))}
		}
		return 0, "", "", err
	}
	if rc == sshExitUnreach {
		msg := tail(strings.TrimSpace(errText), 300)
		if msg == "" {
			msg = "ssh exit 255"
		}
		return 0, "", "", &ExecutorUnreachable{msg}
	}
	return rc, out, errText, nil
}

// ---- login ------------------------------------------------------------------------

const loginPrepScript = `set -e; d="$HOME/.copilot"; f="$d/settings.json"; umask 077; mkdir -p "$d"; chmod 700 "$d"; ` +
	`if [ ! -s "$f" ]; then printf "{\n  \"storeTokenPlaintext\": true\n}\n" > "$f"; echo settings=written; ` +
	`elif grep -q "\"storeTokenPlaintext\"[[:space:]]*:[[:space:]]*true" "$f"; then echo settings=present; ` +
	`elif command -v node >/dev/null 2>&1; then node -e "const fs=require(\"fs\");const p=process.argv[1];` +
	`const c=JSON.parse(fs.readFileSync(p,\"utf8\"));c.storeTokenPlaintext=true;` +
	`fs.writeFileSync(p,JSON.stringify(c,null,2)+\"\n\")" "$f"; echo settings=merged; ` +
	`else echo settings=unmergeable; exit 4; fi`

var (
	loginPrepCommand = "sudo -n -u pulse -H sh -c '" + loginPrepScript + "'"
	loginCommand     = "sudo -n -u pulse -H timeout 90 copilot login --with-token"
)

func classifyToken(token string) (string, error) {
	if token == "" {
		return "", fail(2, "no token on standard input; pipe it, e.g. `op read op://vault/item/field | pulse executor login ...`")
	}
	if strings.ContainsAny(token, " \t\r\n\v\f") {
		return "", fail(2, "the token must be one line with no whitespace inside")
	}
	switch {
	case strings.HasPrefix(token, "github_pat_"):
		return "pat", nil
	case strings.HasPrefix(token, "gho_"):
		return "oauth", nil
	case strings.HasPrefix(token, "ghu_"):
		return "user", nil
	case strings.HasPrefix(token, "ghp_"):
		return "", fail(2, "a classic personal access token (ghp_) is rejected by the Copilot CLI; create a fine-grained token with the Copilot Requests permission (github_pat_)")
	}
	return "", fail(2, "unsupported token prefix; use a fine-grained token (github_pat_), device-flow token (gho_), or user-to-server token (ghu_)")
}

type tokenFacts struct {
	OK      *bool
	User    string
	Expires string
	Note    string
}

// githubHTTP carries every call to the GitHub API; tests point it at a fake transport.
var githubHTTP = &http.Client{Timeout: 15 * time.Second}

func githubAPIBase(api string) string {
	if api == "" {
		api = os.Getenv("PULSE_GITHUB_API")
	}
	if api == "" {
		api = githubAPI
	}
	return strings.TrimRight(api, "/")
}

// githubGet is one authenticated GET; the token travels in the header and nowhere else.
func githubGet(token, url string) (*http.Response, []byte, error) {
	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return nil, nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("User-Agent", "pulse")
	req.Header.Set("Accept", "application/vnd.github+json")
	resp, err := githubHTTP.Do(req)
	if err != nil {
		return nil, nil, err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	return resp, body, nil
}

func githubTokenFacts(token, api string) tokenFacts {
	resp, body, err := githubGet(token, githubAPIBase(api)+"/user")
	if err != nil {
		return tokenFacts{Note: fmt.Sprintf("GitHub not reached (%s); the token's validity is unknown", cut(redactTokens(err.Error()), 80))}
	}
	t, f := true, false
	switch {
	case resp.StatusCode >= 200 && resp.StatusCode < 300:
		var parsed map[string]any
		_ = json.Unmarshal(body, &parsed)
		user := ""
		if v, ok := parsed["login"].(string); ok {
			user = v
		}
		expires := strings.TrimSpace(resp.Header.Get("github-authentication-token-expiration"))
		note := "GitHub accepts the token"
		if user != "" {
			note += " for " + user
		}
		if expires != "" {
			note += ", expires " + expires
		}
		return tokenFacts{OK: &t, User: user, Expires: expires, Note: note}
	case resp.StatusCode == 401 || resp.StatusCode == 403:
		return tokenFacts{OK: &f, Note: fmt.Sprintf("GitHub refuses the token (%d): invalid, expired, or revoked - the token was not sent", resp.StatusCode)}
	}
	return tokenFacts{Note: fmt.Sprintf("GitHub answered %d; the token's validity is unknown", resp.StatusCode)}
}

// noRepositoryRefusal is the executor token rule (ADR-034 rule 1): a token that reaches any repository
// could push around the land gate from a machine that runs untrusted agent output, so it never travels.
const noRepositoryRefusal = "the executor token must reach no repository: mint a fine-grained token with only the Copilot Requests permission"

// githubNoRepos asks GitHub for one repository the token can see in any role; ok is true only when the
// answer is 200 with an empty list. The note explains any other answer without the token.
func githubNoRepos(token, api string) (bool, string) {
	resp, body, err := githubGet(token, githubAPIBase(api)+"/user/repos?per_page=1&affiliation=owner,collaborator,organization_member")
	if err != nil {
		return false, fmt.Sprintf("GitHub not reached (%s); the token's reach is unknown", cut(redactTokens(err.Error()), 80))
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return false, fmt.Sprintf("GitHub answered %d to the repository check; the token's reach is unknown", resp.StatusCode)
	}
	var repos []json.RawMessage
	if err := json.Unmarshal(body, &repos); err != nil {
		return false, "GitHub's repository answer was not a list; the token's reach is unknown"
	}
	if len(repos) > 0 {
		return false, noRepositoryRefusal
	}
	return true, "the token reaches no repository"
}

var expiryDateRe = regexp.MustCompile(`^(\d{4}-\d{2}-\d{2})`)

func expiryWarning(expires, now string, days int) string {
	if expires == "" {
		return ""
	}
	m := expiryDateRe.FindStringSubmatch(expires)
	if m == nil {
		return ""
	}
	when, err1 := time.Parse("2006-01-02", m[1])
	ref, err2 := time.Parse("2006-01-02", now[:10])
	if err1 != nil || err2 != nil {
		return ""
	}
	left := int(when.Sub(ref).Hours() / 24)
	if left < 0 {
		return "the token expired on " + m[1]
	}
	if left <= days {
		return fmt.Sprintf("the token expires in %d day(s), on %s", left, m[1])
	}
	return ""
}

// loginCheckCommand is the one-word run that proves the runner is logged in; it is light work,
// so it runs with light_model (store.LightModel).
func loginCheckCommand(s *store.Store) []string {
	if s.HarnessNative() {
		cmd := []string{"pulse", "harness", "run", "-p", nativeLoginPrompt, "--max-steps", "2", "--no-store", "--quiet"}
		if model := s.LightModel(); model != "" {
			cmd = append(cmd, "--model", model)
		}
		return cmd
	}
	cmd := []string{runnerCmd(s), "-p", loginCheckPrompt, "--no-ask-user", "--disable-builtin-mcps", "-s"}
	if model := s.LightModel(); model != "" {
		cmd = append(cmd, "--model", model)
	}
	return cmd
}

func runnerCmd(s *store.Store) string {
	if v := s.Setting("runner_cmd", ""); v != "" {
		return v
	}
	return "copilot"
}

func tinyBundle() ([]byte, error) {
	tmp, err := os.MkdirTemp("", "pulse-login-check-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(tmp)
	repo := filepath.Join(tmp, "repo")
	if _, err := gitCheck(tmp, "init", "-q", "--initial-branch=main", repo); err != nil {
		return nil, err
	}
	if _, err := gitCheck(repo, "-c", "user.name=pulse", "-c", "user.email=pulse@pulse.local", "-c", "commit.gpgsign=false",
		"commit", "-q", "--allow-empty", "-m", "login check"); err != nil {
		return nil, err
	}
	bundle := filepath.Join(tmp, "check.bundle")
	if _, err := gitCheck(repo, "bundle", "create", "-q", bundle, "main"); err != nil {
		return nil, err
	}
	return os.ReadFile(bundle)
}

// loginCheck is login_check(): one real model call through the hands.
func loginCheck(s *store.Store, name, actor, now string, apply bool) (bool, string, error) {
	r := executorRow(s, name)
	if r == nil || name == localExecutor {
		return false, "", fail(2, "No executor %s", name)
	}
	job := "login-check-" + regexp.MustCompile(`[^0-9A-Za-z]`).ReplaceAllString(now, "")
	started := time.Now()
	ok, kind, said := false, "", ""
	rc := 0
	bundle, err := tinyBundle()
	if err != nil {
		return false, "", err
	}
	var out, errText string
	rc, out, errText, err = sshRun(r, []string{"prepare", job, "main", "pulse"}, bundle, 120*time.Second)
	var unreach *ExecutorUnreachable
	if errors.As(err, &unreach) {
		kind, said = "unreachable", unreach.Msg
	} else if err != nil {
		return false, "", err
	} else if rc != 0 {
		src := errText
		if src == "" {
			src = out
		}
		kind, said = "hands", fmt.Sprintf("prepare failed (exit %d): %s", rc, tail(strings.Join(strings.Fields(src), " "), 200))
	} else {
		args := append([]string{"run", job, strconv.Itoa(loginCheckMinutes), "--"}, loginCheckCommand(s)...)
		rc, out, errText, err = sshRun(r, args, nil, time.Duration(loginCheckMinutes*60+60)*time.Second)
		if errors.As(err, &unreach) {
			kind, said = "unreachable", unreach.Msg
		} else if err != nil {
			return false, "", err
		} else {
			said = faultLine(errText+"\n"+out, 200)
			if s.HarnessNative() {
				switch {
				case rc == 0 && strings.Contains(out, "SUMMARY:"):
					ok, kind = true, "ok"
				case rc == 4 || strings.HasPrefix(strings.ToLower(strings.TrimSpace(errText)), "not logged in:") || loginFault(said):
					kind = "refused"
				default:
					kind = "failed"
				}
			} else {
				switch {
				case rc == 0 && !loginFault(said):
					ok, kind = true, "ok"
				case loginFault(said):
					kind = "refused"
				default:
					kind = "failed"
				}
			}
		}
		_, _, _, _ = sshRun(r, []string{"clean", job}, nil, 60*time.Second)
	}
	seconds := int(time.Since(started).Seconds())
	var note string
	worker := "copilot"
	if s.HarnessNative() {
		worker = "harness"
	}
	switch {
	case ok:
		note = fmt.Sprintf("login ok: %s answered in %ds", worker, seconds)
	case kind == "refused":
		note = fmt.Sprintf("%s refuses the login on %s: %s - GitHub accepts the token but Copilot does not: a fine-grained token needs "+
			"the Copilot Requests permission, and the account needs Copilot; fix the token on GitHub or place a new one with "+
			"`pulse executor login %s --admin <admin@host> --with-token`", worker, name, said, name)
	case kind == "failed":
		note = fmt.Sprintf("%s failed on %s (exit %d): %s", worker, name, rc, said)
	default:
		note = fmt.Sprintf("%s: %s", kind, cut(redactTokens(said), 200))
	}
	eventAt(s.DB, now, actor, "executor.login-check", name, map[string]any{"ok": ok, "kind": kind, "note": trunc(note, 300), "seconds": seconds})
	if apply {
		r = executorRow(s, name)
		switch {
		case ok && r.str("status") == "drained":
			n := trunc("ok: "+note, 300)
			if _, err := setExecutorStatus(s, now, name, "active", actor, &n); err != nil {
				return ok, note, err
			}
			exec1(s.DB, "UPDATE executors SET backoff_minutes = 0, next_probe_at = '', errors = 0 WHERE name = ?", name)
			eventAt(s.DB, now, actor, "executor.recovered", name, map[string]any{"note": trunc(note, 200)})
		case ok:
			exec1(s.DB, "UPDATE executors SET backoff_minutes = 0, next_probe_at = '' WHERE name = ?", name)
		default:
			wait := r.int("backoff_minutes") * 2
			if wait < loginBackoffMins[0] {
				wait = loginBackoffMins[0]
			}
			if wait > loginBackoffMins[1] {
				wait = loginBackoffMins[1]
			}
			if kind == "refused" {
				exec1(s.DB, "UPDATE executors SET probe_note = ?, backoff_minutes = ?, next_probe_at = ?, "+
					"status = CASE WHEN status = 'active' THEN 'drained' ELSE status END WHERE name = ?",
					trunc("fault: "+note, 300), wait, plusMinutes(now, float64(wait)), name)
				if r.str("status") == "active" {
					eventAt(s.DB, now, actor, "executor.drained", name, map[string]any{"fault": trunc(note, 200)})
				}
			} else if r.str("status") == "drained" {
				exec1(s.DB, "UPDATE executors SET backoff_minutes = ?, next_probe_at = ? WHERE name = ?", wait, plusMinutes(now, float64(wait)), name)
			}
		}
	}
	return ok, note, nil
}

// setExecutorAdmin remembers the admin ssh address a login travelled over (ADR-034 rule 3), so the daemon
// can heal the executor there later.
func setExecutorAdmin(s *store.Store, name, admin string) {
	if admin = strings.TrimSpace(admin); admin != "" {
		exec1(s.DB, "UPDATE executors SET admin = ? WHERE name = ?", admin, name)
	}
}

func loginDrained(r row) bool {
	return r.str("status") == "drained" && loginFault(r.str("probe_note")) && r.str("next_probe_at") != ""
}

type loginResult struct {
	OK    bool     `json:"ok"`
	Kind  string   `json:"kind"`
	RC    int      `json:"rc"`
	Lines []string `json:"lines"`
}

func lastLines(text string, n int) []string {
	lines := strings.Split(strings.TrimRight(redactTokens(text), "\n"), "\n")
	if len(lines) == 1 && lines[0] == "" {
		return nil
	}
	if len(lines) > n {
		lines = lines[len(lines)-n:]
	}
	return lines
}

// placeLogin is executorLogin as the daemon calls it (heal, Provision); tests swap it for a stub.
var placeLogin = executorLogin

// executorLogin is executor_login(): place or remove the worker login over the admin session.
func executorLogin(s *store.Store, now, name, adminAddress string, token io.Reader, isTTY, remove bool, actor string) (loginResult, error) {
	r := executorRow(s, name)
	if r == nil || name == localExecutor {
		return loginResult{}, fail(2, "No executor %s", name)
	}
	if _, _, _, err := parseSSHAddress(adminAddress); err != nil {
		return loginResult{}, err
	}
	if (token == nil) == !remove {
		return loginResult{}, fail(2, "executor login needs exactly one of --with-token or --remove")
	}
	setExecutorAdmin(s, name, adminAddress)
	res := loginResult{Lines: []string{}}
	logEvent := func(extra map[string]any) {
		d := map[string]any{"kind": res.Kind, "rc": res.RC, "ok": res.OK}
		for k, v := range extra {
			d[k] = v
		}
		eventAt(s.DB, now, actor, "executor.login", name, d)
	}
	var unreach *ExecutorUnreachable
	if remove {
		res.Kind = "removed"
		removeCommand := `sudo -n -u pulse -H sh -c 'rm -rf -- "$HOME/.copilot"'`
		if s.HarnessNative() {
			removeCommand = `sudo -n -u pulse -H sh -c 'rm -f -- "$HOME/.pulse/private/providers/copilot.json"'`
		}
		rc, out, errText, err := adminSSHRun(adminAddress, removeCommand, nil, 120*time.Second, "")
		if errors.As(err, &unreach) {
			res.RC = sshExitUnreach
			res.Lines = append(res.Lines, fmt.Sprintf("%s unreachable: %s", name, unreach.Msg))
			logEvent(nil)
			return res, nil
		} else if err != nil {
			return res, err
		}
		res.RC, res.OK = rc, rc == 0
		if res.OK {
			res.Lines = append(res.Lines, "login removed for user pulse on "+name)
			before := r.str("status")
			probeOK, note, err := probeExecutor(s, name, actor, now)
			if err != nil {
				return res, err
			}
			status := executorRow(s, name).str("status")
			if !probeOK && before == "active" {
				n := note
				st, err := setExecutorStatus(s, now, name, "drained", actor, &n)
				if err != nil {
					return res, err
				}
				status = st.str("status")
			}
			if probeOK {
				res.Lines = append(res.Lines, fmt.Sprintf("%s: ok, %s: %s", name, status, note))
			} else {
				res.Lines = append(res.Lines, fmt.Sprintf("%s: probe FAILED (%s): %s", name, status, note))
			}
		} else {
			res.Lines = append(res.Lines, fmt.Sprintf("login removal failed (exit %d)", rc))
			res.Lines = append(res.Lines, lastLines(out+errText, 5)...)
		}
		logEvent(nil)
		return res, nil
	}
	if isTTY {
		return res, fail(2, "the token must be piped, not typed")
	}
	raw, _ := io.ReadAll(token)
	tok := strings.TrimSpace(string(raw))
	kind, err := classifyToken(tok)
	if err != nil {
		return res, err
	}
	res.Kind = kind
	facts := githubTokenFacts(tok, "")
	res.Lines = append(res.Lines, facts.Note)
	if facts.OK != nil && !*facts.OK {
		res.RC = 1
		logEvent(map[string]any{"step": "github"})
		return res, nil
	}
	if warn := expiryWarning(facts.Expires, now, loginExpiryWarn); warn != "" {
		res.Lines = append(res.Lines, "warning: "+warn)
	}
	settingsNote := "settings=unknown"
	loginCmd := loginCommand
	if s.HarnessNative() {
		settingsNote = "settings=native"
		loginCmd = "sudo -n -u pulse -H timeout 90 pulse harness login --with-token"
	} else {
		prepRC, prepOut, prepErr, err := adminSSHRun(adminAddress, loginPrepCommand, nil, 120*time.Second, "")
		if errors.As(err, &unreach) {
			res.RC = sshExitUnreach
			res.Lines = append(res.Lines, fmt.Sprintf("%s unreachable: %s", name, unreach.Msg))
			logEvent(nil)
			return res, nil
		} else if err != nil {
			return res, err
		}
		if prepRC != 0 {
			res.RC = prepRC
			res.Lines = append(res.Lines, fmt.Sprintf("preparing ~/.copilot/settings.json for user pulse on %s failed (exit %d); the token was not sent", name, prepRC))
			res.Lines = append(res.Lines, lastLines(prepOut+prepErr, 5)...)
			logEvent(map[string]any{"step": "settings"})
			return res, nil
		}
		for _, l := range strings.Split(prepOut, "\n") {
			if strings.HasPrefix(l, "settings=") {
				settingsNote = strings.TrimSpace(l)
				break
			}
		}
	}
	rc, out, errText, err := adminSSHRun(adminAddress, loginCmd, append([]byte(tok), '\n'), 120*time.Second, "")
	if errors.As(err, &unreach) {
		res.RC = sshExitUnreach
		res.Lines = append(res.Lines, fmt.Sprintf("%s unreachable: %s", name, unreach.Msg))
		logEvent(nil)
		return res, nil
	} else if err != nil {
		return res, err
	}
	res.RC, res.OK = rc, rc == 0
	if res.OK {
		res.Lines = append(res.Lines, fmt.Sprintf("login stored for user pulse on %s (kind %s, %s)", name, kind, settingsNote))
		probeOK, note, err := probeExecutor(s, name, actor, now)
		if err != nil {
			return res, err
		}
		if probeOK {
			checkOK, checkNote, err := loginCheck(s, name, actor, now, true)
			if err != nil {
				return res, err
			}
			status := executorRow(s, name).str("status")
			if checkOK {
				res.Lines = append(res.Lines, fmt.Sprintf("%s: ok, %s: %s; %s", name, status, checkNote, note))
			} else {
				res.OK = false
				res.Lines = append(res.Lines, fmt.Sprintf("%s: login check FAILED (%s): %s", name, status, checkNote))
			}
		} else {
			status := executorRow(s, name).str("status")
			res.Lines = append(res.Lines, fmt.Sprintf("%s: probe FAILED (%s): %s", name, status, note))
		}
	} else {
		res.Lines = append(res.Lines, fmt.Sprintf("login failed (exit %d)", rc))
		res.Lines = append(res.Lines, lastLines(out+errText, 5)...)
	}
	extra := map[string]any{}
	if facts.Expires != "" {
		extra["expires"] = facts.Expires
	}
	if facts.User != "" {
		extra["user"] = facts.User
	}
	logEvent(extra)
	return res, nil
}

// ---- probe --------------------------------------------------------------------------

func parseProbe(text string) map[string]string {
	fields := map[string]string{}
	for _, tok := range strings.Fields(text) {
		if k, v, ok := strings.Cut(tok, "="); ok {
			fields[strings.TrimSpace(k)] = strings.TrimSpace(v)
		}
	}
	return fields
}

func probeExecutor(s *store.Store, name, actor, now string) (bool, string, error) {
	r := executorRow(s, name)
	if r == nil || name == localExecutor {
		return false, "", fail(2, "No executor %s", name)
	}
	drainedForLogin := loginDrained(r)
	ok, note := false, ""
	rc, out, errText, err := sshRun(r, []string{"probe"}, nil, 60*time.Second)
	var unreach *ExecutorUnreachable
	if errors.As(err, &unreach) {
		note = "unreachable: " + unreach.Msg
	} else if err != nil {
		return false, "", err
	} else {
		fields := parseProbe(out)
		var problems []string
		if rc != 0 || fields["hands"] != "ok" {
			src := errText
			if src == "" {
				src = out
			}
			said := tail(strings.Join(strings.Fields(src), " "), 160)
			if said == "" {
				said = "no output"
			}
			problems = append(problems, fmt.Sprintf("pulse-hands did not answer (exit %d: %s)", rc, said))
		} else {
			get := func(k, def string) string {
				if v, ok := fields[k]; ok {
					return v
				}
				return def
			}
			if get("git", "missing") == "missing" {
				problems = append(problems, "git missing")
			}
			if s.HarnessNative() {
				if get("harness", "no") != "yes" {
					problems = append(problems, fmt.Sprintf("harness login missing (run `pulse executor login %s --admin <admin@host> --with-token`)", name))
				}
			} else if get("copilot", "missing") == "missing" {
				problems = append(problems, "copilot missing")
			}
			if fields["keys_ok"] != "yes" {
				problems = append(problems, "authorized key lacks command=/restrict")
			}
			if get("remotes", "0") != "0" {
				problems = append(problems, fmt.Sprintf("%s clone(s) with remotes", fields["remotes"]))
			}
			if fields["workdir"] != "" && strings.TrimRight(fields["workdir"], "/") != strings.TrimRight(r.str("workdir"), "/") {
				problems = append(problems, fmt.Sprintf("workdir is %s not %s", fields["workdir"], r.str("workdir")))
			}
			if !s.HarnessNative() && get("logged_in", "yes") != "yes" {
				problems = append(problems, "copilot is not logged in (run `copilot login` as the executor user)")
			}
			if _, has := fields["keys"]; !has {
				problems = append(problems, fmt.Sprintf("hands predate ADR-022 (no keys= field); refresh them with `pulse pair <admin@host> --name %s`", name))
			}
			if minFree := settingInt(s, "daemon.min_free_mb"); minFree > 0 {
				if n, err := strconv.Atoi(get("disk_free_mb", "0")); err == nil && n < minFree {
					want := fmt.Sprintf("%d MB", minFree)
					if minFree%1024 == 0 {
						want = fmt.Sprintf("%d GiB", minFree/1024)
					}
					// disk_free_mb is measured at the hand's workdir (df on it), so that is the path named
					where := fields["workdir"]
					if where == "" {
						where = r.str("workdir")
					}
					problems = append(problems, fmt.Sprintf("under %s free on %s", want, where))
				}
			}
		}
		ok = len(problems) == 0
		if ok {
			swept := sweepExecutorJobs(s, now, r, fields["jobs"])
			if len(swept) > 0 {
				fields["jobs"] = fmt.Sprintf("swept:%d", len(swept))
			} else if _, has := fields["jobs"]; !has {
				fields["jobs"] = "-"
			}
			var stale []string
			keys := fields["keys"]
			if keys == "" {
				keys = "-"
			}
			for _, jobID := range strings.Split(keys, ",") {
				if jobID == "" || jobID == "-" || !jobDirRe.MatchString(jobID) {
					continue
				}
				live := mustOne(s.DB, "SELECT 1 FROM runs WHERE job_id = ? AND executor = ? AND status = 'running'", jobID, name)
				if live == nil && !contains(swept, jobID) {
					stale = append(stale, jobID)
				}
			}
			if len(stale) > 0 {
				problems = append(problems, fmt.Sprintf("%d signing key(s) left without a live run: %s", len(stale), strings.Join(stale, ", ")))
				ok = false
			}
		}
		if ok {
			var keys []string
			for k := range fields {
				if k != "hands" {
					keys = append(keys, k)
				}
			}
			sort.Strings(keys)
			parts := make([]string, len(keys))
			for i, k := range keys {
				parts[i] = k + "=" + fields[k]
			}
			note = "ok: " + strings.Join(parts, " ")
		} else {
			note = strings.Join(problems, "; ")
		}
	}
	note = trunc(note, 300)
	switch {
	case ok && drainedForLogin:
		exec1(s.DB, "UPDATE executors SET last_probe_at = ?, probe_note = ?, errors = 0 WHERE name = ?",
			now, trunc("fault: login refused, awaiting the login check; "+note, 300), name)
	case ok:
		exec1(s.DB, "UPDATE executors SET last_probe_at = ?, probe_note = ?, errors = 0, backoff_minutes = 0, next_probe_at = '', "+
			"status = CASE WHEN status = 'drained' AND ? = 'human' THEN 'active' WHEN status = 'unreachable' THEN 'active' ELSE status END "+
			"WHERE name = ?", now, note, actor, name)
		if actor == humanAgent {
			clearUnfit(s.DB, now, name)
		}
	default:
		exec1(s.DB, "UPDATE executors SET last_probe_at = ?, probe_note = ? WHERE name = ?", now, note, name)
	}
	eventAt(s.DB, now, actor, "executor.probe", name, map[string]any{"ok": ok, "note": note})
	return ok, note, nil
}

func sweepExecutorJobs(s *store.Store, now string, r row, jobs string) []string {
	var swept []string
	for _, jobID := range strings.Split(jobs, ",") {
		if jobID == "" || jobID == "-" || !jobDirRe.MatchString(jobID) {
			continue
		}
		if mustOne(s.DB, "SELECT 1 FROM runs WHERE job_id = ? AND executor = ? AND status = 'running'", jobID, r.str("name")) != nil {
			continue
		}
		rc, _, _, err := sshRun(r, []string{"clean", jobID}, nil, 60*time.Second)
		if err != nil {
			break
		}
		if rc == 0 {
			swept = append(swept, jobID)
			eventAt(s.DB, now, "runner", "executor.sweep", r.str("name"), map[string]any{"job": jobID})
		}
	}
	return swept
}

func lastLoginCheck(s *store.Store, name string) map[string]any {
	r := mustOne(s.DB, "SELECT at, detail FROM events WHERE action = 'executor.login-check' AND subject = ? ORDER BY seq DESC LIMIT 1", name)
	if r == nil {
		return nil
	}
	d := jsonMap(r.str("detail"))
	ok, _ := d["ok"].(bool)
	note, _ := d["note"].(string)
	return map[string]any{"at": r.str("at"), "ok": ok, "note": note}
}

func loginExpiry(s *store.Store, name string) string {
	r := mustOne(s.DB, "SELECT detail FROM events WHERE action = 'executor.login' AND subject = ? ORDER BY seq DESC LIMIT 1", name)
	if r == nil {
		return ""
	}
	v, _ := jsonMap(r.str("detail"))["expires"].(string)
	return v
}
