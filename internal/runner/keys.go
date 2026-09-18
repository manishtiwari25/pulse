package runner

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Provenance (ADR-022): one signing key per run, a clone-local hook that adds
// the Pulse-Agent/Pulse-Job/Pulse-Executor trailers, and the landing check.

var provenanceTrailers = []string{"Pulse-Agent", "Pulse-Job", "Pulse-Executor"}

func agentEmail(agent string) string { return agent + "@pulse.local" }

// makeRunKey is make_run_key(): the private half at path (0600), the public half returned.
func makeRunKey(path, agent, runID string) (string, error) {
	pub := path + ".pub"
	for _, stale := range []string{path, pub} {
		_ = os.Remove(stale)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return "", fail(1, "cannot make the directory for the run's signing key: %v", err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", agent+" "+runID, "-f", path)
	var errb strings.Builder
	cmd.Stderr = &errb
	if err := cmd.Run(); err != nil {
		if _, lookErr := exec.LookPath("ssh-keygen"); lookErr != nil {
			return "", fail(1, "ssh-keygen is not installed; the runner needs it to give each run its signing key (ADR-022)")
		}
		return "", fail(1, "cannot make the run's signing key: %s", strings.TrimSpace(errb.String()))
	}
	_ = os.Chmod(path, 0o600)
	if !fileExists(path) {
		return "", fail(1, "the run's signing key was not written at %s; commits would fail at commit time (ADR-022)", path)
	}
	fields := strings.Fields(readFile(pub))
	if len(fields) > 2 {
		fields = fields[:2]
	}
	return strings.Join(fields, " "), nil
}

func removeRunKey(path string) {
	_ = os.Remove(path)
	_ = os.Remove(path + ".pub")
}

func provenanceHook(agent, jobID, executor string) string {
	values := []string{agent, jobID, executor}
	parts := make([]string, len(provenanceTrailers))
	for i, k := range provenanceTrailers {
		parts[i] = "--trailer " + shellQuote(k+": "+values[i])
	}
	return "#!/bin/sh\n" +
		"# pulse: provenance trailers for every commit of this run (ADR-022). Local to this clone, never committed.\n" +
		"exec git interpret-trailers --in-place --if-exists doNothing " + strings.Join(parts, " ") + " \"$1\"\n"
}

// preCommitHookScript returns a shell script that runs the project's verify
// command with the recipe environment and optional mise pins. It is installed
// as .git/hooks/pre-commit so a worker cannot commit changes that fail verify.
func preCommitHookScript(verifyCmd string, recipe *Recipe, pins []string, clone string) string {
	var b strings.Builder
	b.WriteString("#!/bin/sh\n")
	b.WriteString("# pulse: verify-before-commit gate (auto-installed by runner)\n")
	if exports := recipeEnvExports(recipe); exports != "" {
		b.WriteString(strings.TrimSuffix(exports, "; "))
		b.WriteString("\n")
	}
	if len(pins) > 0 {
		b.WriteString(miseWrapShell(pins, verifyCmd, clone))
	} else {
		b.WriteString(verifyCmd)
	}
	b.WriteString("\n")
	return b.String()
}

func configureCloneIdentity(clone, agent, keyPath, hookText string) error {
	// The key must exist before user.signingkey points at it: otherwise every commit the worker
	// makes fails at commit time and finished work is lost (ADR-022). Fail the run early instead.
	if !fileExists(keyPath) {
		return fail(1, "the run's signing key %s does not exist; refusing to configure user.signingkey (ADR-022)", keyPath)
	}
	hooks := filepath.Join(clone, ".git", "hooks")
	// One git process instead of eight: the identity keys live in .git/pulse-identity and
	// .git/config includes that file, which git resolves exactly like inline keys.
	var identity strings.Builder
	for _, kv := range [][2]string{{"user.name", agent}, {"user.email", agentEmail(agent)},
		{"gpg.format", "ssh"}, {"gpg.ssh.program", "ssh-keygen"},
		{"user.signingkey", keyPath}, {"commit.gpgsign", "true"}, {"tag.gpgsign", "false"},
		{"core.hooksPath", hooks}} {
		fmt.Fprintf(&identity, "%s\t%s = %s\n", gitConfigSection(kv[0]), kv[0][strings.LastIndexByte(kv[0], '.')+1:], gitConfigQuote(kv[1]))
	}
	identityPath := filepath.Join(clone, ".git", "pulse-identity")
	if err := os.WriteFile(identityPath, []byte(identity.String()), 0o644); err != nil {
		return err
	}
	if _, err := gitCheck(clone, "config", "include.path", identityPath); err != nil {
		return err
	}
	if err := os.MkdirAll(hooks, 0o755); err != nil {
		return err
	}
	hook := filepath.Join(hooks, "prepare-commit-msg")
	if err := os.WriteFile(hook, []byte(hookText), 0o755); err != nil {
		return err
	}
	return os.Chmod(hook, 0o755)
}

func writePreCommitHook(clone, script string) error {
	hook := filepath.Join(clone, ".git", "hooks", "pre-commit")
	if err := os.WriteFile(hook, []byte(script), 0o755); err != nil {
		return err
	}
	return os.Chmod(hook, 0o755)
}

// gitConfigSection renders the section header for a dotted config key: gpg.ssh.program
// becomes [gpg "ssh"], user.name becomes [user].
func gitConfigSection(key string) string {
	section, rest, _ := strings.Cut(key, ".")
	if i := strings.LastIndexByte(rest, '.'); i >= 0 {
		return fmt.Sprintf("[%s %s]\n", section, gitConfigQuote(rest[:i]))
	}
	return "[" + section + "]\n"
}

// gitConfigQuote double-quotes a config value so paths with spaces, quotes or backslashes survive.
func gitConfigQuote(v string) string {
	v = strings.NewReplacer(`\`, `\\`, `"`, `\"`, "\n", `\n`).Replace(v)
	return `"` + v + `"`
}

// SignersLine is the one-line allowed-signers content for a run (`pulse signers` prints the same shape).
func SignersLine(agent, pubkey string) string {
	return fmt.Sprintf("%s namespaces=\"git\" %s\n", agentEmail(agent), pubkey)
}

// SignersText is the allowed-signers file for every run the store knows a public key of (ADR-022 §3).
func SignersText(s *store.Store) string {
	var b strings.Builder
	for _, r := range mustRows(s.DB, "SELECT agent, pubkey FROM runs WHERE pubkey != '' ORDER BY started_at, id") {
		b.WriteString(SignersLine(r.str("agent"), r.str("pubkey")))
	}
	return b.String()
}

// landingProblems is landing_problems(): the plain reasons that refuse a landing; empty lands.
func landingProblems(root, base, ref, agent, jobID, pubkey, signersPath string) []string {
	if err := os.WriteFile(signersPath, []byte(SignersLine(agent, pubkey)), 0o600); err != nil {
		return []string{"could not write the allowed-signers file: " + err.Error()}
	}
	format := "%H%x1f%G?%x1f%an%x1f%ae%x1f%(trailers:key=Pulse-Agent,valueonly,separator=%x1d)%x1f%(trailers:key=Pulse-Job,valueonly,separator=%x1d)%x1e"
	out := gitTimeout(root, 120*time.Second, "-c", "gpg.ssh.program=ssh-keygen", "-c", "gpg.ssh.allowedSignersFile="+signersPath,
		"log", "--format="+format, base+".."+ref)
	_ = os.Remove(signersPath)
	if !out.ok() {
		return []string{"could not read the fetched commits: " + strings.TrimSpace(out.Stderr)}
	}
	var problems []string
	why := map[string]string{"N": "is not signed", "U": "is signed by a key that is not this run's", "B": "has a bad signature",
		"E": "has a signature that cannot be checked"}
	for _, record := range strings.Split(out.Stdout, "\x1e") {
		record = strings.TrimSpace(record)
		if record == "" {
			continue
		}
		fields := strings.Split(record, "\x1f")
		for len(fields) < 6 {
			fields = append(fields, "")
		}
		for i := range fields {
			fields[i] = strings.TrimSpace(fields[i])
		}
		sha, status, an, ae, tAgent, tJob := fields[0], fields[1], fields[2], fields[3], fields[4], fields[5]
		shortSHA := short(sha, 12)
		if status != "G" {
			w, ok := why[status]
			if !ok {
				w = "has signature status " + status
			}
			problems = append(problems, fmt.Sprintf("commit %s %s", shortSHA, w))
		}
		if an != agent || ae != agentEmail(agent) {
			problems = append(problems, fmt.Sprintf("commit %s is authored by %s <%s>, not %s", shortSHA, an, ae, agent))
		}
		agents := splitTrailer(tAgent)
		jobs := splitTrailer(tJob)
		switch {
		case len(agents) == 0:
			problems = append(problems, fmt.Sprintf("commit %s lacks its Pulse-Agent trailer", shortSHA))
		case len(agents) != 1 || agents[0] != agent:
			problems = append(problems, fmt.Sprintf("commit %s names %s in its Pulse-Agent trailer, not %s", shortSHA, strings.Join(agents, ", "), agent))
		}
		switch {
		case len(jobs) == 0:
			problems = append(problems, fmt.Sprintf("commit %s lacks its Pulse-Job trailer", shortSHA))
		case len(jobs) != 1 || jobs[0] != jobID:
			problems = append(problems, fmt.Sprintf("commit %s names job %s in its Pulse-Job trailer, not %s", shortSHA, strings.Join(jobs, ", "), jobID))
		}
	}
	return problems
}

func splitTrailer(s string) []string {
	var out []string
	for _, v := range strings.Split(s, "\x1d") {
		if v = strings.TrimSpace(v); v != "" {
			out = append(out, v)
		}
	}
	return out
}

// reprobeDue is reprobe_due(): unreachable executors probed again with a doubling wait.
func reprobeDue(s *store.Store, interval int, now string) [][2]string {
	var back [][2]string
	capMin := interval / 60
	if capMin < 1 {
		capMin = 1
	}
	for _, r := range executorRows(s, false) {
		if r.str("next_probe_at") != "" && r.str("next_probe_at") > now {
			continue
		}
		if loginDrained(r) {
			ok, note, err := loginCheck(s, r.str("name"), "runner", now, true)
			if err == nil && ok {
				back = append(back, [2]string{r.str("name"), note})
			}
			continue
		}
		if r.str("status") != "unreachable" {
			continue
		}
		ok, note, err := probeExecutor(s, r.str("name"), "runner", now)
		if err != nil {
			continue
		}
		if ok {
			back = append(back, [2]string{r.str("name"), note})
		} else {
			bo := r.int("backoff_minutes")
			if bo < 1 {
				bo = 1
			}
			wait := bo * 2
			if wait > capMin {
				wait = capMin
			}
			if wait < 1 {
				wait = 1
			}
			exec1(s.DB, "UPDATE executors SET backoff_minutes = ?, next_probe_at = ? WHERE name = ?", wait, plusMinutes(now, float64(wait)), r.str("name"))
		}
	}
	return back
}
