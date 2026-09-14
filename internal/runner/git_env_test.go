package runner

import (
	"strings"
	"testing"
)

func TestGitEnvTakesNoOptionalLocksAndNeverPrompts(t *testing.T) {
	env := strings.Join(gitEnv([]string{"GIT_SSH_COMMAND=ssh -i key"}), "\n")
	for _, want := range []string{"\nGIT_TERMINAL_PROMPT=0\n", "\nGIT_OPTIONAL_LOCKS=0\n", "\nGIT_SSH_COMMAND=ssh -i key"} {
		if !strings.Contains("\n"+env+"\n", want) {
			t.Fatalf("git env lacks %q:\n%s", strings.TrimSpace(want), env)
		}
	}
}
