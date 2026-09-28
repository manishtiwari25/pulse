package testutil

import (
	"os"
	"testing"
)

// TestGitEnvDropsHookVariables guards the GIT_DIR class of failures: a test suite
// run under a git hook (or with GIT_DIR=/tmp/x in the shell) must not hand the
// hook's repository location to fixture git commands.
func TestGitEnvDropsHookVariables(t *testing.T) {
	t.Setenv("GIT_DIR", "/tmp/x")
	t.Setenv("GIT_WORK_TREE", "/tmp/x")
	t.Setenv("GIT_INDEX_FILE", "/tmp/x/index")
	t.Setenv("GIT_AUTHOR_NAME", "hook")
	t.Setenv("PULSE_TESTUTIL_KEEP", "1")

	env := GitEnv()
	for _, kv := range env {
		if HookGitVar(kv) {
			t.Errorf("GitEnv leaked hook variable %q", kv)
		}
	}
	want := map[string]bool{
		"PULSE_TESTUTIL_KEEP=1":           false,
		"GIT_CONFIG_NOSYSTEM=1":           false,
		"GIT_CONFIG_GLOBAL=" + os.DevNull: false,
	}
	for _, kv := range env {
		if _, ok := want[kv]; ok {
			want[kv] = true
		}
	}
	for kv, seen := range want {
		if !seen {
			t.Errorf("GitEnv missing %q", kv)
		}
	}
}
