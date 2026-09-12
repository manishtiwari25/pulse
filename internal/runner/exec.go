package runner

import (
	"context"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// An executor is where one run happens: this machine (localExec) or a remote
// pair of hands over ssh (sshExec). The steps mirror LocalExecutor / SshExecutor.
type executor interface {
	name() string
	remote() bool
	clone() string
	keyPath() string
	started() bool
	workerDirs() []string
	prepare(ctx context.Context) (string, error)
	runWorker(ctx context.Context, cmd []string, env map[string]string, minutes int) (outcome, error)
	resultText() *string
	commitCount(base string) int
	dirty() string
	ownerPathsTouched(base string) []string
	verify(ctx context.Context, cmd string, minutes int) (*int, string)
	fetchSource() string
	fetchEnv() []string
	cleanup()
}

type runLog struct{ lines []string }

func (l *runLog) add(format string, args ...any) {
	if len(args) == 0 {
		l.lines = append(l.lines, format)
		return
	}
	l.lines = append(l.lines, fmt.Sprintf(format, args...))
}

func (l *runLog) put(line string) { l.lines = append(l.lines, line) }

// Write lets a measurement's progress lines land in the run log.
func (l *runLog) Write(p []byte) (int, error) {
	for _, line := range strings.Split(strings.TrimRight(string(p), "\n"), "\n") {
		l.put(line)
	}
	return len(p), nil
}

func (l *runLog) text() string {
	var kept []string
	for _, x := range l.lines {
		if x != "" {
			kept = append(kept, x)
		}
	}
	return strings.Join(kept, "\n")
}

func tailStr(s string, n int) string { return tail(s, n) }

// ---- local -------------------------------------------------------------------------

type localExec struct {
	row      row
	s        *store.Store
	home     string
	job      row
	project  row
	agent    string
	log      *runLog
	root     string
	cloneDir string
	key      string
	begun    bool
	lastOut  *string
}

func newLocalExec(r row, s *store.Store, home string, job, project row, agent string, log *runLog) *localExec {
	return &localExec{row: r, s: s, home: home, job: job, project: project, agent: agent, log: log,
		root: project.str("path"), cloneDir: filepath.Join(home, "work", job.str("id")),
		key: filepath.Join(home, "work", job.str("id")+".key")}
}

func (e *localExec) name() string         { return e.row.str("name") }
func (e *localExec) remote() bool         { return false }
func (e *localExec) clone() string        { return e.cloneDir }
func (e *localExec) keyPath() string      { return e.key }
func (e *localExec) started() bool        { return e.begun }
func (e *localExec) workerDirs() []string { return workerDirs(e.home) }

func (e *localExec) branchToClone() string {
	if d := e.project.str("default_branch"); d != "" && d != "HEAD" {
		return d
	}
	return ""
}

func (e *localExec) prepare(ctx context.Context) (string, error) {
	_ = os.RemoveAll(e.cloneDir)
	args := []string{"clone", "-q", "--no-tags"}
	if b := e.branchToClone(); b != "" {
		args = append(args, "-b", b)
	}
	out, err := runGroup(ctx, append(append([]string{"git"}, args...), e.root, e.cloneDir), "", environ(), nil, 300*time.Second)
	e.log.add("$ git %s\n%s%s", strings.Join(args, " "), out.Stdout, out.Stderr)
	if err != nil {
		return "", fail(1, "clone failed: %v", err)
	}
	if out.ExitCode != 0 {
		return "", fail(1, "clone failed: %s", strings.TrimSpace(out.Stderr))
	}
	git(e.cloneDir, "remote", "remove", "origin")
	if err := configureCloneIdentity(e.cloneDir, e.agent, e.key, provenanceHook(e.agent, e.job.str("id"), e.name())); err != nil {
		return "", err
	}
	head, err := gitCheck(e.cloneDir, "rev-parse", "HEAD")
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(head.Stdout), nil
}

// preflightRunKey fails before the worker starts when the per-run signing key
// is gone: without it every commit the worker makes is unsignable and the
// finished work is lost at the end (ADR-022). Failing here is a setup error
// that sends the job back to the queue with nothing spent.
func preflightRunKey(path string) error {
	if _, err := os.Stat(path); err != nil {
		return fail(1, "setup-error: the run's signing key %s is missing (%v); the worker was not started so no work is lost (ADR-022)", path, err)
	}
	return nil
}

func (e *localExec) runWorker(ctx context.Context, cmd []string, env map[string]string, minutes int) (outcome, error) {
	if err := preflightRunKey(e.key); err != nil {
		return outcome{}, err
	}
	e.begun = true
	budget := time.Duration(minutes) * time.Minute
	if minutes <= 0 {
		budget = time.Second
	}
	out, err := runGroupObserved(ctx, cmd, e.cloneDir, workerEnvWithCLIPath(environ(), env), nil, budget, e.observer(e.execution(env)))
	if err != nil {
		return out, fail(1, "worker command not found: %s (setting runner_cmd=%s)", cmd[0], e.s.Setting("runner_cmd", ""))
	}
	text := out.Stdout
	e.lastOut = &text
	return out, nil
}

// resultText is the harness output (D-L: no result file); nil before the worker ran.
func (e *localExec) resultText() *string { return e.lastOut }

func (e *localExec) commitCount(base string) int {
	rev := git(e.cloneDir, "rev-list", "--count", base+"..HEAD")
	if !rev.ok() {
		return 0
	}
	n, _ := strconv.Atoi(strings.TrimSpace(rev.Stdout))
	return n
}

func (e *localExec) dirty() string {
	return strings.TrimSpace(git(e.cloneDir, "status", "--porcelain", "--untracked-files=no").Stdout)
}

func (e *localExec) ownerPathsTouched(base string) []string {
	return ownerPathsTouched(e.cloneDir, base)
}

func (e *localExec) verify(ctx context.Context, cmd string, minutes int) (*int, string) {
	budget := time.Duration(minutes) * time.Minute
	if budget < 60*time.Second {
		budget = 60 * time.Second
	}
	out, err := runGroup(ctx, []string{"/bin/sh", "-c", cmd}, e.cloneDir, environ(), nil, budget)
	if err != nil {
		rc := 127
		return &rc, err.Error()
	}
	if out.Timeout {
		return nil, ""
	}
	rc := out.ExitCode
	return &rc, tailStr(out.Stdout, 20000) + tailStr(out.Stderr, 20000)
}

func (e *localExec) fetchSource() string { return e.cloneDir }
func (e *localExec) fetchEnv() []string  { return environ() }

func (e *localExec) cleanup() {
	_ = os.RemoveAll(e.cloneDir)
	removeRunKey(e.key)
}

// ---- ssh ----------------------------------------------------------------------------

type sshExec struct {
	*localExec
	jobDir string
	bundle string
}

func newSSHExec(r row, s *store.Store, home string, job, project row, agent string, log *runLog) *sshExec {
	l := newLocalExec(r, s, home, job, project, agent, log)
	jobDir := strings.TrimRight(r.str("workdir"), "/") + "/" + job.str("id")
	l.cloneDir = jobDir + "/clone"
	return &sshExec{localExec: l, jobDir: jobDir, bundle: filepath.Join(home, "work", job.str("id")+".bundle")}
}

func (e *sshExec) remote() bool         { return true }
func (e *sshExec) workerDirs() []string { return []string{e.jobDir, "/tmp"} }

func (e *sshExec) call(args []string, stdin []byte, timeout time.Duration) (int, string, string, error) {
	rc, out, errText, err := sshRun(e.row, args, stdin, timeout)
	if err != nil {
		return rc, out, errText, err
	}
	shown := args
	if len(shown) > 4 {
		shown = shown[:4]
	}
	e.log.add("$ ssh %s %s\n%s%s", e.row.str("address"), shellJoin(shown), tailStr(out, 4000), tailStr(errText, 4000))
	src := errText
	if src == "" {
		src = out
	}
	if rc != 0 && strings.Contains(src, "verb not allowed") {
		verb := "?"
		if len(args) > 0 {
			verb = args[0]
		}
		return rc, out, errText, &ExecutorFault{fmt.Sprintf("hands on %s refuse `%s` (%s) - they predate this hub; refresh them with `pulse pair <admin@host> --name %s`",
			e.name(), verb, tail(strings.Join(strings.Fields(src), " "), 120), e.name())}
	}
	return rc, out, errText, nil
}

var shaRe = regexp.MustCompile(`^[0-9a-f]{40,64}$`)

func (e *sshExec) prepare(ctx context.Context) (string, error) {
	branch := e.branchToClone()
	if branch == "" {
		if head := git(e.root, "symbolic-ref", "--short", "-q", "HEAD"); head.ok() {
			branch = strings.TrimSpace(head.Stdout)
		}
	}
	ref := branch
	if ref == "" {
		ref = "HEAD"
	}
	made := gitTimeout(e.root, 300*time.Second, "bundle", "create", "-q", e.bundle, ref)
	if !made.ok() {
		return "", fail(1, "bundle failed: %s", strings.TrimSpace(made.Stderr))
	}
	data, err := os.ReadFile(e.bundle)
	if err != nil {
		return "", err
	}
	rc, out, errText, err := e.call([]string{"prepare", e.job.str("id"), ref, e.agent}, data, 600*time.Second)
	if err != nil {
		return "", err
	}
	src := errText
	if src == "" {
		src = out
	}
	if rc != 0 {
		return "", fail(1, "remote prepare failed on %s (exit %d): %s", e.name(), rc, tail(strings.Join(strings.Fields(src), " "), 300))
	}
	base := ""
	if f := strings.Fields(out); len(f) > 0 {
		base = f[len(f)-1]
	}
	if !shaRe.MatchString(base) {
		return "", fail(1, "remote prepare on %s returned no base commit: %s", e.name(), pyRepr(tail(out, 200)))
	}
	keyData, err := os.ReadFile(e.key)
	if err != nil {
		return "", err
	}
	rc, out, errText, err = e.call([]string{"key", e.job.str("id"), e.name()}, keyData, 120*time.Second)
	if err != nil {
		return "", err
	}
	if rc != 0 {
		src = errText
		if src == "" {
			src = out
		}
		return "", &ExecutorFault{fmt.Sprintf("remote key failed on %s (exit %d): %s - the machine cannot take a signing key; probe it with `pulse executor probe %s`",
			e.name(), rc, tail(strings.Join(strings.Fields(src), " "), 300), e.name())}
	}
	// Same non-empty check as preflightRunKey on the hub side: the hands wrote the
	// key under the job dir, so confirm the host-side copy exists and is not empty
	// before the worker starts and signs with it (ADR-022).
	if err := e.preflightRemoteRunKey("after upload"); err != nil {
		return "", err
	}
	return base, nil
}

// preflightRemoteRunKey runs `test -s` on the host-side run key through the
// hands `verify` verb; when is a short phrase naming the moment of the check.
func (e *sshExec) preflightRemoteRunKey(when string) error {
	rc, out, errText, err := e.call([]string{"verify", e.job.str("id"), "1", "--", remoteKeyCheckCmd(e.remoteKeyPath())}, nil, 120*time.Second)
	if err != nil {
		return err
	}
	return remoteKeyCheckResult(e.remoteKeyPath(), e.name(), when, rc, out, errText)
}

// remoteKeyCheckResult maps the exit of remoteKeyCheckCmd to the same
// setup-error preflightRunKey reports locally, or nil when the key is present.
func remoteKeyCheckResult(path, host, when string, rc int, out, errText string) error {
	if rc == 0 {
		return nil
	}
	src := errText
	if src == "" {
		src = out
	}
	return fail(1, "setup-error: the run's signing key %s on %s is missing or empty %s (exit %d): %s; the worker was not started so no work is lost (ADR-022)",
		path, host, when, rc, tail(strings.Join(strings.Fields(src), " "), 300))
}

// remoteKeyPath is where the hands `key` verb stores the per-run signing key.
func (e *sshExec) remoteKeyPath() string { return e.jobDir + "/run.key" }

// remoteKeyCheckCmd is the shell command run on the host after the key upload;
// `test -s` fails for a missing or empty file, mirroring preflightRunKey.
func remoteKeyCheckCmd(path string) string { return "test -s " + shellQuote(path) }

func (e *sshExec) runWorker(ctx context.Context, cmd []string, env map[string]string, minutes int) (outcome, error) {
	if ctx == nil {
		ctx = context.Background()
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	id := e.execution(env)
	identityErr := id.Validate()
	heartbeatReporter(e.log)(identityErr)
	fields := map[string]string(nil)
	if identityErr == nil {
		fields = e.probeCapabilities(ctx)
	}
	if fields != nil && fields["pulse"] == "missing" {
		// Same preflight as the local branch: a worker whose shell cannot resolve `pulse`
		// cannot claim, post or finish, so fail before the clone and the harness start.
		return outcome{}, &ExecutorFault{fmt.Sprintf("pulse is not on PATH on %s (hands probe reported pulse=missing) - install the pulse CLI for the executor user or fix its login PATH; probe it with `pulse executor probe %s`", e.name(), e.name())}
	}
	heartbeats := identityErr == nil && fields != nil && e.heartbeatCapableFrom(fields)
	steps := identityErr == nil && fields != nil && stepsCapable(fields) && env["PULSE_HARNESS_STEPS"] != ""
	if heartbeats {
		env = maps.Clone(env)
		env["PULSE_HEARTBEAT"] = "1"
	}
	budget := strconv.Itoa(minutes)
	if minutes <= 0 {
		budget = "1s"
	}
	args := []string{"run", e.job.str("id"), budget}
	for _, k := range sortedKeys(env) {
		args = append(args, k+"="+env[k])
	}
	args = append(append(args, "--"), cmd...)
	e.begun = true
	var watching chan struct{}
	if heartbeats {
		watching = make(chan struct{})
		go func() {
			defer close(watching)
			e.watchRemote(ctx, id)
		}()
	}
	var stepsMirror *remoteStepsMirror
	var mirroring chan struct{}
	if steps {
		stepsMirror = newRemoteStepsMirror(e, id.Session)
		mirroring = make(chan struct{})
		go func() {
			defer close(mirroring)
			stepsMirror.watch(ctx)
		}()
	}
	rc, out, errText, err := sshRunContext(ctx, e.row, args, nil, time.Duration(minutes*60+120)*time.Second)
	cancel()
	if watching != nil {
		<-watching
		e.stopHeartbeat(id, heartbeatReporter(e.log))
	}
	if mirroring != nil {
		<-mirroring
		stepsMirror.fetch(context.Background())
	}
	e.log.add("$ ssh %s run %s %s -- <worker>\n", e.row.str("address"), e.job.str("id"), budget)
	if ctx.Err() != nil && err != nil && (err == context.Canceled || err == context.DeadlineExceeded) {
		result := outcome{Timeout: true, ExitCode: -1, Stdout: out, Stderr: errText}
		if stepsMirror != nil {
			stepsMirror.finishIfMissing(result, err)
		}
		return result, nil
	}
	if err != nil {
		if stepsMirror != nil {
			stepsMirror.finishIfMissing(outcome{ExitCode: -1}, err)
		}
		return outcome{ExitCode: -1}, err
	}

	text := out
	e.lastOut = &text
	if rc == remoteExitTimeout {
		result := outcome{Timeout: true, ExitCode: -1, Stdout: out, Stderr: errText}
		if stepsMirror != nil {
			stepsMirror.finishIfMissing(result, nil)
		}
		return result, nil
	}
	result := outcome{ExitCode: rc, Stdout: out, Stderr: errText}
	if stepsMirror != nil {
		stepsMirror.finishIfMissing(result, nil)
	}
	return result, nil
}

// resultText prefers a PULSE_RESULT.md the hands still serve (a worker briefed by
// the Python hub) and falls back to the harness output.
func (e *sshExec) resultText() *string {
	rc, out, _, err := e.call([]string{"result", e.job.str("id")}, nil, 120*time.Second)
	if err == nil && rc == 0 {
		return &out
	}
	return e.lastOut
}

func (e *sshExec) commitCount(base string) int {
	rc, out, _, err := e.call([]string{"count", e.job.str("id"), base}, nil, 120*time.Second)
	if err != nil || rc != 0 {
		return 0
	}
	f := strings.Fields(out)
	if len(f) == 0 {
		return 0
	}
	n, _ := strconv.Atoi(f[len(f)-1])
	return n
}

func (e *sshExec) dirty() string {
	rc, out, _, err := e.call([]string{"dirty", e.job.str("id")}, nil, 120*time.Second)
	if err != nil || rc != 0 {
		return ""
	}
	return strings.TrimSpace(out)
}

func (e *sshExec) ownerPathsTouched(base string) []string {
	rc, out, _, err := e.call([]string{"changed", e.job.str("id"), base}, nil, 120*time.Second)
	if err != nil || rc != 0 {
		return nil
	}
	var hit []string
	for _, f := range strings.Fields(out) {
		for _, p := range ownerPaths {
			if f == p || strings.HasPrefix(f, p) {
				hit = append(hit, f)
				break
			}
		}
	}
	sortStrings(hit)
	return hit
}

func (e *sshExec) verify(ctx context.Context, cmd string, minutes int) (*int, string) {
	budget := strconv.Itoa(minutes)
	if minutes <= 0 {
		budget = "1s"
	}
	secs := minutes * 60
	if secs < 60 {
		secs = 60
	}
	rc, out, errText, err := e.call([]string{"verify", e.job.str("id"), budget, "--", cmd}, nil, time.Duration(secs+120)*time.Second)
	if err != nil {
		code := -1
		return &code, err.Error()
	}
	text := tailStr(out, 20000) + tailStr(errText, 20000)
	if rc == remoteExitTimeout {
		return nil, text
	}
	return &rc, text
}

func (e *sshExec) fetchSource() string {
	user, host, port, _ := parseSSHAddress(e.row.str("address"))
	return fmt.Sprintf("ssh://%s@%s:%d%s", user, host, port, e.cloneDir)
}

func (e *sshExec) fetchEnv() []string {
	return envWith(environ(), map[string]string{
		"GIT_SSH_COMMAND":     shellJoin(append([]string{"ssh"}, sshOptions...)),
		"GIT_TERMINAL_PROMPT": "0",
	})
}

func (e *sshExec) cleanup() {
	_ = os.Remove(e.bundle)
	removeRunKey(e.key)
	if _, _, _, err := e.call([]string{"clean", e.job.str("id")}, nil, 120*time.Second); err != nil {
		e.log.add("remote clean skipped: %v (the next probe reports leftovers)", err)
	}
}

func makeExecutor(r row, s *store.Store, home string, job, project row, agent string, log *runLog) (executor, error) {
	if r.str("name") == localExecutor || r.str("kind") == "local" {
		return newLocalExec(r, s, home, job, project, agent, log), nil
	}
	if r.str("kind") == "ssh" {
		return newSSHExec(r, s, home, job, project, agent, log), nil
	}
	return nil, fail(2, "Executor %s has unknown kind %s", r.str("name"), pyRepr(r.str("kind")))
}

// workerDirs is worker_dirs(): the store, the pulse binary's directory, scratch space.
func workerDirs(home string) []string {
	dirs := []string{home}
	if exe, err := os.Executable(); err == nil {
		dirs = append(dirs, filepath.Dir(exe))
	}
	dirs = append(dirs, os.TempDir())
	if st, err := os.Stat("/tmp"); err == nil && st.IsDir() {
		dirs = append(dirs, "/tmp")
	}
	var out []string
	for _, d := range dirs {
		if !contains(out, d) {
			out = append(out, d)
		}
	}
	return out
}

// workerCommand is worker_command(): the harness argv; the brief is the prompt (D-L). The model
// follows the job kind (store.RunModel): worker_model for the work that changes code, light_model
// for the rest.
func workerCommand(s *store.Store, kind, briefText, clone, workerCmd string, dirs []string, remote bool) ([]string, bool) {
	if workerCmd != "" {
		return splitShell(workerCmd), true
	}
	if s.HarnessNative() {
		bin := "pulse"
		if !remote {
			var err error
			bin, err = os.Executable()
			if err != nil {
				bin = "pulse"
			}
		}
		cmd := []string{bin, "harness", "run", "-p", briefText, "-C", clone,
			"--max-steps", s.Setting("harness.max_steps", ""),
			"--step-timeout", s.Setting("harness.step_timeout", ""),
			"--output-limit", s.Setting("harness.output_limit", ""),
			"--max-cost", s.Setting("harness.max_cost", ""),
			"--keep-observations", s.Setting("harness.keep_observations", "")}
		// One minute under the runner's kill, so the harness stops on its own
		// with a SUMMARY line and a finished session instead of a lost one.
		if minutes := settingInt(s, "minutes_per_job"); minutes > 1 {
			cmd = append(cmd, "--minutes", strconv.Itoa(minutes-1))
		}
		if model := s.RunModel(kind); model != "" {
			cmd = append(cmd, "--model", model)
		}
		return cmd, false
	}
	cmd := []string{runnerCmd(s), "-p", briefText, "-C", clone, "--allow-all-tools"}
	for _, p := range deniedTools {
		cmd = append(cmd, "--deny-tool", p)
	}
	for _, d := range dirs {
		cmd = append(cmd, "--add-dir", d)
	}
	cmd = append(cmd, "--disable-builtin-mcps", "--no-ask-user", "-s", "--max-ai-credits", s.Setting("max_ai_credits", ""))
	cmd = append(cmd, "--autopilot")
	if model := s.RunModel(kind); model != "" {
		cmd = append(cmd, "--model", model)
	}
	return cmd, false
}

// workerEnvWithCLIPath builds the worker environment from base plus extra and,
// when extra carries PULSE_CLI, prepends its directory to PATH once so `pulse`
// resolves by bare name in worker shells. Without PULSE_CLI it is envWith.
func workerEnvWithCLIPath(base []string, extra map[string]string) []string {
	cli := extra["PULSE_CLI"]
	if cli == "" {
		return envWith(base, extra)
	}
	dir := filepath.Dir(cli)
	path, ok := extra["PATH"]
	if !ok {
		for _, kv := range base {
			if strings.HasPrefix(kv, "PATH=") {
				path = kv[len("PATH="):]
				break
			}
		}
	}
	for _, p := range filepath.SplitList(path) {
		if p == dir {
			return envWith(base, extra)
		}
	}
	merged := make(map[string]string, len(extra)+1)
	for k, v := range extra {
		merged[k] = v
	}
	if path == "" {
		merged["PATH"] = dir
	} else {
		merged["PATH"] = dir + string(os.PathListSeparator) + path
	}
	return envWith(base, merged)
}
