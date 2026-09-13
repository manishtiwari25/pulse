package runner

// Preflight checks refuse to start a worker whose run would fail late. Every
// one of them returns an error whose text starts with "setup-error:" (see
// TestSetupErrorsSharePrefix) so the runner classifies it as a setup failure:
// the job goes back to the queue and no worker minutes are spent.
//
//	Check                     Where                                   Stage
//	preflightRunKey           run.go (both executors)                 after the run key is written, before anything is spent
//	preflightRunKey           exec.go localExec.prepare               clone prepare
//	preflightRunKey           exec.go localExec.runWorker             last moment before the worker starts
//	preflightRunKey           exec.go sshExec.runWorker               hub-side copy, last moment before the worker starts
//	preflightTmpSpace         exec.go localExec.runWorker             last moment before the worker starts
//	preflightPulseCLI         run.go (local executor only)            after the worker env is built, before launch
//	preflightRemoteRunKey     exec.go sshExec.prepare                 after the key upload (remoteKeyCheckResult)
//	preflightRemoteCLI        exec.go sshExec.runWorker               before the remote worker starts (remoteCLICheckResult)
//
// Adding a check: return fail(1, "setup-error: ...") and add its message
// constructor to TestSetupErrorsSharePrefix.

import (
	"os"
	"path/filepath"
	"strings"
)

// pulseOnPath resolves the `pulse` binary the worker will see: it walks the
// PATH the worker is launched with (base process env overlaid with extra) and
// returns the first executable named pulse, or "" when none resolves. The
// runner logs this before every launch so a run whose brief says
// `pulse claim` but whose shell has no pulse is visible in the run log
// instead of being rediscovered by the worker.
func pulseOnPath(env []string) string {
	pathEnv := ""
	for _, kv := range env {
		if strings.HasPrefix(kv, "PATH=") {
			pathEnv = kv[len("PATH="):]
		}
	}
	for _, dir := range filepath.SplitList(pathEnv) {
		if dir == "" {
			continue
		}
		p := filepath.Join(dir, "pulse")
		if st, err := os.Stat(p); err == nil && !st.IsDir() && st.Mode()&0o111 != 0 {
			return p
		}
	}
	return ""
}

// preflightLine is the run-log line for pulseOnPath's answer.
func preflightLine(found, cli string) string {
	if found != "" {
		return "preflight: pulse on worker PATH: " + found
	}
	if cli != "" {
		return "preflight: pulse missing from worker PATH; worker must use $PULSE_CLI=" + cli
	}
	return "preflight: pulse missing from worker PATH and no PULSE_CLI set"
}

// preflightPulseCLI is the fail-fast partner of preflightLine: it returns nil
// when `pulse` resolved on the worker PATH or PULSE_CLI names an executable
// file, and a setup-error otherwise so the worker is never launched with a
// brief it cannot follow (no `pulse claim`, `pulse vote` or `pulse job finish`).
func preflightPulseCLI(found, cli string) error {
	if found != "" {
		return nil
	}
	if cli != "" {
		if st, err := os.Stat(cli); err == nil && !st.IsDir() && st.Mode()&0o111 != 0 {
			return nil
		}
		return fail(1, "setup-error: pulse is missing from the worker PATH and PULSE_CLI=%s is not an executable file; the worker was not started so no work is lost", cli)
	}
	return fail(1, "setup-error: pulse is missing from the worker PATH and no PULSE_CLI is set; the worker was not started so no work is lost")
}
