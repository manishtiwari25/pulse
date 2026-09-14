package runner

import (
	"context"
	"encoding/base64"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/user"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// `pulse hands`: the forced command a PULSE executor answers with (ADR-020), ported from hosted/pulse-hands.sh
// so the same binary, cross-compiled and copied to the machine, serves. The wire protocol is unchanged: the hub
// sends `b64 <base64 of NUL-separated argv>` in SSH_ORIGINAL_COMMAND (never eval'd) or git's own
// `git-upload-pack '<path>'`; only the verbs below run; every path stays under the work directory.

const handsDefaultWorkdir = "/srv/pulse"

var (
	handsJobRe      = regexp.MustCompile(`^[A-Za-z0-9._-]+$`)
	handsBranchRe   = regexp.MustCompile(`^[A-Za-z0-9._/-]+$`)
	handsEnvRe      = regexp.MustCompile(`^(PULSE_[A-Z_]+|GIT_TERMINAL_PROMPT)=.*$`)
	handsB64Re      = regexp.MustCompile(`^b64 ([A-Za-z0-9+/=]+)$`)
	handsUploadRe   = regexp.MustCompile(`^git-upload-pack '([^'[:cntrl:]]+)'$`)
	handsPinnedRe   = regexp.MustCompile(`^command="[^"]*pulse-hands[^"]*",([^ ]*,)?restrict`)
	handsLoggedInRe = regexp.MustCompile(`"(authTokens":\{"|loggedInUsers":\[\{")`)
)

// handsError is die(): "pulse-hands: <msg>" on stderr, exit 2.
type handsError struct{ msg string }

func (e *handsError) Error() string { return "pulse-hands: " + e.msg }

func handsDie(format string, args ...any) error { return &handsError{fmt.Sprintf(format, args...)} }

// handsExit carries a worker's or check's exit code through.
type handsExit struct{ code int }

func (e *handsExit) Error() string { return "exit " + strconv.Itoa(e.code) }

type hands struct {
	workdir string
	home    string
	stdin   io.Reader
	stdout  io.Writer
	stderr  io.Writer
}

func handsAllowedWorker(rest []string) bool {
	switch filepath.Base(rest[0]) {
	case "copilot":
		return true
	case "pulse":
		return len(rest) >= 3 && rest[1] == "harness" && rest[2] == "run"
	}
	return false
}

func newHands(stdin io.Reader, stdout, stderr io.Writer) *hands {
	wd := os.Getenv("PULSE_HANDS_WORKDIR")
	if wd == "" {
		wd = handsDefaultWorkdir
	}
	home := os.Getenv("HOME")
	if home == "" {
		home = "/nonexistent"
	}
	// sshd starts a forced command with a minimal PATH; user-local tool homes join it (WU-021).
	os.Setenv("PATH", strings.Join([]string{filepath.Join(home, ".local", "bin"), filepath.Join(home, ".cargo", "bin"), filepath.Join(home, ".bun", "bin"), os.Getenv("PATH")}, ":"))
	return &hands{workdir: wd, home: home, stdin: stdin, stdout: stdout, stderr: stderr}
}

func (h *hands) jobDir(job string) (string, error) {
	if !handsJobRe.MatchString(job) || job == "." || job == ".." {
		return "", handsDie("bad job id")
	}
	return h.workdir + "/" + job, nil
}

func (h *hands) cloneDir(job string) (string, error) {
	dir, err := h.jobDir(job)
	if err != nil {
		return "", err
	}
	clone := dir + "/clone"
	if st, err := os.Stat(clone + "/.git"); err != nil || !st.IsDir() {
		return "", handsDie("no clone for job %s", job)
	}
	return clone, nil
}

// handsBudget parses MINUTES (integer) or a trailing `s` for seconds.
func handsBudget(budget string) (time.Duration, error) {
	if strings.HasSuffix(budget, "s") {
		n, err := strconv.Atoi(strings.TrimSuffix(budget, "s"))
		if err != nil || n < 0 {
			return 0, handsDie("bad minutes")
		}
		if n < 1 {
			return 0, handsDie("the budget must be at least one second")
		}
		return time.Duration(n) * time.Second, nil
	}
	n, err := strconv.Atoi(budget)
	if err != nil || n < 0 {
		return 0, handsDie("bad minutes")
	}
	if n < 1 {
		return 0, handsDie("the budget must be at least one second")
	}
	return time.Duration(n) * time.Minute, nil
}

// withTimeout is timeout(1) --kill-after=30: the command in its own process group, TERM then KILL, exit 124.
func (h *hands) withTimeout(budget time.Duration, dir string, argv []string, extra map[string]string) int {
	return h.withTimeoutObserved(budget, dir, argv, extra, nil)
}

func (h *hands) withTimeoutObserved(budget time.Duration, dir string, argv []string, extra map[string]string, observer *processObserver) int {
	cmd := exec.Command(argv[0], argv[1:]...)
	cmd.Dir = dir
	// Same PATH prefix as localExec.runWorker: when run.go passed PULSE_CLI, its
	// directory leads PATH so the remote worker resolves `pulse` by bare name.
	env := workerEnvWithCLIPath(os.Environ(), extra)
	var kept []string
	for _, kv := range env {
		k, _, _ := strings.Cut(kv, "=")
		if k == "SSH_ORIGINAL_COMMAND" || k == "SSH_CONNECTION" || k == "SSH_CLIENT" {
			continue
		}
		kept = append(kept, kv)
	}
	cmd.Env = kept
	cmd.Stdin = h.stdin
	cmd.Stdout, cmd.Stderr = h.stdout, h.stderr
	err, timedOut, started := waitCommand(context.Background(), cmd, budget, handsKillAfter, observer)
	if !started {
		fmt.Fprintf(h.stderr, "%s: %v\n", argv[0], err)
		return 127
	}
	if timedOut {
		return remoteExitTimeout
	}
	return finish(err, "", "", false).ExitCode
}

var handsKillAfter = 30 * time.Second

// probeVersion reports the copilot CLI version for `pulse hands probe`: "present" when the binary
// resolves but prints no version, or an error when it is not on PATH. It is a package-level hook so
// tests (newTestHands) can pin it in-process instead of relying on PATH shims: the real CLI unpacks
// a ~130MB runtime into $HOME on first run, which is exactly what the test guard must never trigger.
var probeVersion = copilotVersion

func copilotVersion() (string, error) {
	path, err := exec.LookPath("copilot")
	if err != nil {
		return "", err
	}
	out, err := exec.Command(path, "--version").Output()
	if err != nil {
		return "present", nil
	}
	first, _, _ := strings.Cut(string(out), "\n")
	v := strings.Trim(regexp.MustCompile(`[^0-9.]`).ReplaceAllString(first, ""), ".")
	if v == "" {
		return "present", nil
	}
	return v, nil
}

func (h *hands) probe() error {
	gitv := "missing"
	if out, err := exec.Command("git", "--version").Output(); err == nil {
		if f := strings.Fields(string(out)); len(f) >= 3 {
			gitv = f[2]
		}
	}
	copv := "missing"
	if v, err := probeVersion(); err == nil {
		copv = v
	}
	loggedIn := "no"
	if copv != "missing" {
		if os.Getenv("COPILOT_GITHUB_TOKEN")+os.Getenv("GH_TOKEN")+os.Getenv("GITHUB_TOKEN") != "" {
			loggedIn = "yes"
		} else if data, err := os.ReadFile(filepath.Join(h.home, ".copilot", "config.json")); err == nil && len(data) > 0 {
			flat := strings.NewReplacer(" ", "", "\t", "", "\r", "", "\n", "").Replace(string(data))
			if handsLoggedInRe.MatchString(flat) {
				loggedIn = "yes"
			}
		}
	}
	harnessLogin := "no"
	if os.Getenv("COPILOT_GITHUB_TOKEN")+os.Getenv("GH_TOKEN")+os.Getenv("GITHUB_TOKEN") != "" {
		harnessLogin = "yes"
	} else if st, err := os.Stat(filepath.Join(h.home, ".pulse", "private", "providers", "copilot.json")); err == nil && !st.IsDir() {
		harnessLogin = "yes"
	}
	// pulse= carries the resolved CLI path so the runner can hand it to workers as PULSE_CLI
	// and the doctor can say where it lives; pulse_lookup= names the PATH searched when it is
	// missing so the remote fault can say where the CLI was looked for.
	pulsev := "missing"
	pulseLookup := ""
	if path, err := exec.LookPath("pulse"); err == nil {
		if abs, err := filepath.Abs(path); err == nil {
			path = abs
		}
		pulsev = path
	} else {
		pulseLookup = strings.ReplaceAll(os.Getenv("PATH"), " ", "%20")
		if pulseLookup == "" {
			pulseLookup = "-"
		}
	}
	if os.Getenv("PULSE_HANDS_ALLOW_ANY_WORKER") == "1" {
		if copv == "missing" {
			copv = "test"
		}
		loggedIn = "yes"
		if pulsev == "missing" {
			pulsev = "test"
		}
		harnessLogin = "yes"
	}
	remotes := 0
	entries, _ := os.ReadDir(h.workdir)
	var jobs, keys []string
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		jobs = append(jobs, e.Name())
		d := filepath.Join(h.workdir, e.Name())
		if fileExists(filepath.Join(d, "run.key")) {
			keys = append(keys, e.Name())
		}
		clone := filepath.Join(d, "clone")
		if st, err := os.Stat(filepath.Join(clone, ".git")); err == nil && st.IsDir() {
			if out, err := exec.Command("git", "-C", clone, "remote").Output(); err == nil && strings.TrimSpace(string(out)) != "" {
				remotes++
			}
		}
	}
	keysOK := "no"
	if data, err := os.ReadFile(filepath.Join(h.home, ".ssh", "authorized_keys")); err == nil {
		total, pinned := 0, 0
		for _, line := range strings.Split(string(data), "\n") {
			if strings.TrimSpace(line) == "" || strings.HasPrefix(line, "#") {
				continue
			}
			total++
			if handsPinnedRe.MatchString(line) {
				pinned++
			}
		}
		if total > 0 && total == pinned {
			keysOK = "yes"
		}
	}
	if v := os.Getenv("PULSE_HANDS_KEYS_OK"); v != "" {
		keysOK = v
	}
	_ = os.MkdirAll(h.workdir, 0o755)
	freeMB := "0"
	if out, err := exec.Command("df", "-Pm", h.workdir).Output(); err == nil {
		lines := strings.Split(strings.TrimSpace(string(out)), "\n")
		if len(lines) >= 2 {
			if f := strings.Fields(lines[1]); len(f) >= 4 {
				freeMB = f[3]
			}
		}
	}
	name := "?"
	if u, err := user.Current(); err == nil {
		name = u.Username
	}
	lookup := ""
	if pulseLookup != "" {
		lookup = " pulse_lookup=" + pulseLookup
	}
	dash := func(list []string) string {
		if len(list) == 0 {
			return "-"
		}
		return strings.Join(list, ",")
	}
	fmt.Fprintf(h.stdout, "hands=ok user=%s workdir=%s git=%s copilot=%s logged_in=%s harness=%s pulse=%s%s remotes=%d keys_ok=%s disk_free_mb=%s jobs=%s keys=%s heartbeat=1 steps=1\n",
		name, h.workdir, gitv, copv, loggedIn, harnessLogin, pulsev, lookup, remotes, keysOK, freeMB, dash(jobs), dash(keys))
	return nil
}

func (h *hands) prepare(job, branch, agent string) error {
	if !handsJobRe.MatchString(agent) {
		return handsDie("bad agent name")
	}
	if !handsBranchRe.MatchString(branch) {
		return handsDie("bad branch")
	}
	dir, err := h.jobDir(job)
	if err != nil {
		return err
	}
	_ = os.RemoveAll(dir)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return handsDie("%v", err)
	}
	bundle := dir + "/source.bundle"
	f, err := os.Create(bundle)
	if err != nil {
		return handsDie("%v", err)
	}
	if _, err := io.Copy(f, h.stdin); err != nil {
		f.Close()
		return handsDie("%v", err)
	}
	f.Close()
	if !git("", "init", "-q", dir+"/verify").ok() {
		return handsDie("cannot make the scratch repository")
	}
	if !git(dir+"/verify", "bundle", "verify", bundle).ok() {
		_ = os.RemoveAll(dir + "/verify")
		return handsDie("the bundle does not verify")
	}
	_ = os.RemoveAll(dir + "/verify")
	var cl gitResult
	if branch == "HEAD" {
		cl = git("", "-c", "advice.detachedHead=false", "clone", "-q", "--no-tags", bundle, dir+"/clone")
	} else {
		cl = git("", "clone", "-q", "--no-tags", "-b", branch, bundle, dir+"/clone")
	}
	if !cl.ok() {
		return handsDie("%s", strings.TrimSpace(cl.Stderr))
	}
	_ = os.Remove(bundle)
	clone := dir + "/clone"
	git(clone, "remote", "remove", "origin")
	git(clone, "config", "user.name", agent)
	git(clone, "config", "user.email", agent+"@pulse.local")
	git(clone, "config", "commit.gpgsign", "false")
	git(clone, "config", "tag.gpgsign", "false")
	head := git(clone, "rev-parse", "HEAD")
	fmt.Fprint(h.stdout, head.Stdout)
	return nil
}

func (h *hands) key(job, executor string) error {
	if !handsJobRe.MatchString(executor) {
		return handsDie("bad executor name")
	}
	dir, err := h.jobDir(job)
	if err != nil {
		return err
	}
	clone, err := h.cloneDir(job)
	if err != nil {
		return err
	}
	key := dir + "/run.key"
	data, _ := io.ReadAll(h.stdin)
	if err := os.WriteFile(key, data, 0o600); err != nil {
		return handsDie("%v", err)
	}
	_ = os.Chmod(key, 0o600)
	if !strings.Contains(string(data), "-----BEGIN OPENSSH PRIVATE KEY-----") {
		_ = os.Remove(key)
		return handsDie("not an openssh private key")
	}
	agent := strings.TrimSpace(git(clone, "config", "user.name").Stdout)
	if !handsJobRe.MatchString(agent) {
		return handsDie("the clone has no agent identity (prepare first)")
	}
	hooks := clone + "/.git/hooks"
	git(clone, "config", "gpg.format", "ssh")
	git(clone, "config", "gpg.ssh.program", "ssh-keygen")
	git(clone, "config", "user.signingkey", key)
	git(clone, "config", "commit.gpgsign", "true")
	git(clone, "config", "tag.gpgsign", "false")
	git(clone, "config", "core.hooksPath", hooks)
	_ = os.MkdirAll(hooks, 0o755)
	if err := os.WriteFile(hooks+"/prepare-commit-msg", []byte(provenanceHook(agent, job, executor)), 0o755); err != nil {
		return handsDie("%v", err)
	}
	_ = os.Chmod(hooks+"/prepare-commit-msg", 0o755)
	fmt.Fprintln(h.stdout, "keyed")
	return nil
}

func (h *hands) brief(job string) error {
	dir, err := h.jobDir(job)
	if err != nil {
		return err
	}
	if st, err := os.Stat(dir); err != nil || !st.IsDir() {
		return handsDie("no job dir for %s (prepare first)", job)
	}
	data, _ := io.ReadAll(h.stdin)
	if err := os.WriteFile(dir+"/brief.md", data, 0o644); err != nil {
		return handsDie("%v", err)
	}
	return nil
}

func (h *hands) run(job, minutes string, rest []string) error {
	clone, err := h.cloneDir(job)
	if err != nil {
		return err
	}
	extra := map[string]string{}
	for len(rest) > 0 && rest[0] != "--" {
		kv := rest[0]
		rest = rest[1:]
		if !handsEnvRe.MatchString(kv) {
			k, _, _ := strings.Cut(kv, "=")
			return handsDie("env key not allowed: %s", k)
		}
		k, v, _ := strings.Cut(kv, "=")
		extra[k] = v
	}
	if len(rest) == 0 || rest[0] != "--" {
		return handsDie("run needs -- before the worker command")
	}
	rest = rest[1:]
	if len(rest) == 0 {
		return handsDie("run needs a worker command")
	}
	if os.Getenv("PULSE_HANDS_ALLOW_ANY_WORKER") != "1" && !handsAllowedWorker(rest) {
		return handsDie("only the copilot or pulse harness worker may run here")
	}
	budget, err := handsBudget(minutes)
	if err != nil {
		return err
	}
	if os.Getenv("COPILOT_SUBAGENT_MAX_CONCURRENT") == "" {
		extra["COPILOT_SUBAGENT_MAX_CONCURRENT"] = "256"
	}
	if os.Getenv("COPILOT_SUBAGENT_MAX_DEPTH") == "" {
		extra["COPILOT_SUBAGENT_MAX_DEPTH"] = "128"
	}
	var observer *processObserver
	if extra["PULSE_HEARTBEAT"] == "1" {
		observer, err = h.heartbeatObserver(job, extra)
		if err != nil {
			fmt.Fprintf(h.stderr, "pulse-hands: presence unavailable: %v\n", err)
		}
	}
	return &handsExit{h.withTimeoutObserved(budget, clone, rest, extra, observer)}
}

func (h *hands) result(job string) error {
	clone, err := h.cloneDir(job)
	if err != nil {
		return err
	}
	data, err := os.ReadFile(clone + "/" + resultFile)
	if err != nil {
		return &handsExit{3}
	}
	_, _ = h.stdout.Write(data)
	return nil
}

func (h *hands) count(job, base string) error {
	clone, err := h.cloneDir(job)
	if err != nil {
		return err
	}
	if !shaRe.MatchString(base) {
		return handsDie("bad base")
	}
	r := git(clone, "rev-list", "--count", base+"..HEAD")
	fmt.Fprint(h.stdout, r.Stdout)
	if !r.ok() {
		fmt.Fprint(h.stderr, r.Stderr)
		return &handsExit{r.Code}
	}
	return nil
}

func (h *hands) dirty(job string) error {
	clone, err := h.cloneDir(job)
	if err != nil {
		return err
	}
	fmt.Fprint(h.stdout, git(clone, "status", "--porcelain", "--untracked-files=no").Stdout)
	return nil
}

func (h *hands) changed(job, base string) error {
	clone, err := h.cloneDir(job)
	if err != nil {
		return err
	}
	if !shaRe.MatchString(base) {
		return handsDie("bad base")
	}
	r := git(clone, "diff", "--name-only", base+"..HEAD")
	fmt.Fprint(h.stdout, r.Stdout)
	if !r.ok() {
		fmt.Fprint(h.stderr, r.Stderr)
		return &handsExit{r.Code}
	}
	return nil
}

func (h *hands) dropResult(job string) error {
	clone, err := h.cloneDir(job)
	if err != nil {
		return err
	}
	if git(clone, "ls-files", "--error-unmatch", resultFile).ok() {
		git(clone, "rm", "-q", "--cached", resultFile)
		git(clone, "commit", "-q", "-m", fmt.Sprintf("runner: drop %s (never committed)", resultFile))
		fmt.Fprintln(h.stdout, "dropped")
	} else {
		fmt.Fprintln(h.stdout, "clean")
	}
	return nil
}

func (h *hands) verify(job, minutes string, rest []string) error {
	if len(rest) == 0 || rest[0] != "--" {
		return handsDie("verify needs -- before the command")
	}
	rest = rest[1:]
	if len(rest) != 1 {
		return handsDie("verify takes one shell command string")
	}
	clone, err := h.cloneDir(job)
	if err != nil {
		return err
	}
	budget, err := handsBudget(minutes)
	if err != nil {
		return err
	}
	return &handsExit{h.withTimeout(budget, clone, []string{"bash", "-c", rest[0]}, nil)}
}

func (h *hands) clean(job string) error {
	dir, err := h.jobDir(job)
	if err != nil {
		return err
	}
	return os.RemoveAll(dir)
}

func (h *hands) uploadPack(path string) error {
	path = strings.TrimSuffix(strings.TrimPrefix(path, "'"), "'")
	if !strings.HasPrefix(path, h.workdir+"/") || !strings.HasSuffix(path, "/clone") || strings.Count(strings.TrimPrefix(path, h.workdir+"/"), "/") != 1 {
		return handsDie("upload-pack path must be a job clone under %s", h.workdir)
	}
	if strings.Contains(path, "..") {
		return handsDie("bad path")
	}
	if st, err := os.Stat(path + "/.git"); err != nil || !st.IsDir() {
		return handsDie("no such clone")
	}
	cmd := exec.Command("git", "upload-pack", path)
	cmd.Stdin, cmd.Stdout, cmd.Stderr = h.stdin, h.stdout, h.stderr
	if err := cmd.Run(); err != nil {
		return &handsExit{finish(err, "", "", false).ExitCode}
	}
	return nil
}

// handsMain dispatches one verb; the exit code is 0, the worker's, 2 on die, 3 for an absent result.
func (h *hands) handsMain(args []string) error {
	if cmd := os.Getenv("SSH_ORIGINAL_COMMAND"); cmd != "" {
		if m := handsUploadRe.FindStringSubmatch(cmd); m != nil {
			args = []string{"git-upload-pack", m[1]}
		} else if m := handsB64Re.FindStringSubmatch(cmd); m != nil {
			raw, err := base64.StdEncoding.DecodeString(m[1])
			if err != nil {
				return handsDie("refused: commands arrive as 'b64 ...' from pulse.py or as git-upload-pack")
			}
			args = nil
			for _, item := range strings.Split(string(raw), "\x00") {
				if item != "" || len(args) > 0 {
					args = append(args, item)
				}
			}
			if len(args) > 0 && args[len(args)-1] == "" {
				args = args[:len(args)-1]
			}
			if len(args) == 0 {
				return handsDie("empty command")
			}
		} else {
			return handsDie("refused: commands arrive as 'b64 ...' from pulse.py or as git-upload-pack")
		}
	}
	if len(args) == 0 {
		return handsDie("no command (this key only runs pulse-hands verbs)")
	}
	verb, rest := args[0], args[1:]
	need := func(n int, usage string) error {
		if len(rest) != n {
			return handsDie("%s", usage)
		}
		return nil
	}
	switch verb {
	case "probe":
		return h.probe()
	case "heartbeat":
		if err := need(2, "heartbeat JOB SESSION"); err != nil {
			return err
		}
		return h.heartbeat(rest[0], rest[1])
	case "steps":
		if err := need(3, "steps JOB SESSION OFFSET"); err != nil {
			return err
		}
		return h.steps(rest[0], rest[1], rest[2])
	case "prepare":
		if err := need(3, "prepare JOB BRANCH AGENT"); err != nil {
			return err
		}
		return h.prepare(rest[0], rest[1], rest[2])
	case "key":
		if err := need(2, "key JOB EXECUTOR"); err != nil {
			return err
		}
		return h.key(rest[0], rest[1])
	case "brief":
		if err := need(1, "brief JOB"); err != nil {
			return err
		}
		return h.brief(rest[0])
	case "run":
		if len(rest) < 4 {
			return handsDie("run JOB MINUTES [K=V ...] -- CMD ...")
		}
		return h.run(rest[0], rest[1], rest[2:])
	case "result":
		if err := need(1, "result JOB"); err != nil {
			return err
		}
		return h.result(rest[0])
	case "count":
		if err := need(2, "count JOB BASE"); err != nil {
			return err
		}
		return h.count(rest[0], rest[1])
	case "dirty":
		if err := need(1, "dirty JOB"); err != nil {
			return err
		}
		return h.dirty(rest[0])
	case "changed":
		if err := need(2, "changed JOB BASE"); err != nil {
			return err
		}
		return h.changed(rest[0], rest[1])
	case "drop-result":
		if err := need(1, "drop-result JOB"); err != nil {
			return err
		}
		return h.dropResult(rest[0])
	case "verify":
		if len(rest) < 4 {
			return handsDie("verify JOB MINUTES -- CMD")
		}
		return h.verify(rest[0], rest[1], rest[2:])
	case "clean":
		if err := need(1, "clean JOB"); err != nil {
			return err
		}
		return h.clean(rest[0])
	case "git-upload-pack":
		if err := need(1, "git-upload-pack PATH"); err != nil {
			return err
		}
		return h.uploadPack(rest[0])
	}
	return handsDie("verb not allowed: %s", verb)
}
