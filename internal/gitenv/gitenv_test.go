package gitenv

import (
	"strings"
	"testing"
)

func TestEnvDropsInheritedGitVars(t *testing.T) {
	t.Setenv("GIT_DIR", "/nonexistent/.git")
	t.Setenv("GIT_INDEX_FILE", "/nonexistent/index")
	t.Setenv("PULSE_GITENV_KEEP", "1")
	var keep, prompt bool
	for _, kv := range Env() {
		if strings.HasPrefix(kv, "GIT_DIR=") || strings.HasPrefix(kv, "GIT_INDEX_FILE=") {
			t.Fatalf("inherited %q leaked", kv)
		}
		keep = keep || kv == "PULSE_GITENV_KEEP=1"
		prompt = prompt || kv == "GIT_TERMINAL_PROMPT=0"
	}
	if !keep || !prompt {
		t.Fatalf("missing kept var (%v) or hermetic setting (%v)", keep, prompt)
	}
}
