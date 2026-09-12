package runner

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
