package runner

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/gitenv"
)

// gitEnv must not inherit the GIT_* variables a git hook exports, so runner git
// stays hermetic when pulse runs inside a hook.
func TestGitEnvDropsHookVariables(t *testing.T) {
	t.Setenv("GIT_INDEX_FILE", "/elsewhere/index")
	t.Setenv("GIT_PREFIX", "sub/")
	t.Setenv("GIT_AUTHOR_NAME", "someone else")
	t.Setenv("GIT_DIR", "/elsewhere/.git")
	t.Setenv("KEEP_ME", "yes")
	env := gitEnv([]string{"EXTRA=1"})
	for _, kv := range env {
		if gitenv.HookVar(kv) {
			t.Fatalf("gitEnv leaked %q", kv)
		}
	}
	for _, want := range []string{"KEEP_ME=yes", "EXTRA=1", "GIT_TERMINAL_PROMPT=0"} {
		found := false
		for _, kv := range env {
			if kv == want {
				found = true
			}
		}
		if !found {
			t.Fatalf("gitEnv missing %q", want)
		}
	}
}
