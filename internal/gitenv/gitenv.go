// Package gitenv builds the process environment pulse hands to child git
// commands: the caller's environment with every inherited GIT_* variable
// dropped (a hook's GIT_DIR/GIT_INDEX_FILE would redirect the child) and the
// host's system/global config, credential prompts and optional locks disabled.
// It is the one home of the hook GIT_* variable list shared by hub, board,
// runner and the test helpers in internal/testutil.
package gitenv

import (
	"os"
	"strings"
)

// Env returns os.Environ() without GIT_* entries plus the hermetic settings.
func Env() []string {
	env := make([]string, 0, len(os.Environ())+4)
	for _, entry := range os.Environ() {
		key, _, _ := strings.Cut(entry, "=")
		if !strings.HasPrefix(key, "GIT_") {
			env = append(env, entry)
		}
	}
	return append(env, "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull,
		"GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0")
}

// hookVars are the GIT_* variables a running git hook (pre-commit running the
// test suite, for example) exports to its children. Inherited by a child git
// they redirect every call at the hook's index, dir, prefix and author, so
// fixture and production repositories fail or read the wrong tree.
var hookVars = []string{
	"GIT_INDEX_FILE=",
	"GIT_PREFIX=",
	"GIT_AUTHOR_NAME=",
	"GIT_AUTHOR_EMAIL=",
	"GIT_AUTHOR_DATE=",
	"GIT_COMMITTER_NAME=",
	"GIT_COMMITTER_EMAIL=",
	"GIT_COMMITTER_DATE=",
	"GIT_DIR=",
	"GIT_WORK_TREE=",
	"GIT_EDITOR=",
	"GIT_REFLOG_ACTION=",
}

// StripHook returns env without the GIT_* variables a running git hook exports
// (see hookVars); other GIT_* settings (GIT_SSH_COMMAND, ...) are kept.
func StripHook(env []string) []string {
	out := make([]string, 0, len(env))
	for _, kv := range env {
		if HookVar(kv) {
			continue
		}
		out = append(out, kv)
	}
	return out
}

// HookVar reports whether the KEY=value entry kv is one of the hook-exported GIT_* variables.
func HookVar(kv string) bool {
	for _, prefix := range hookVars {
		if strings.HasPrefix(kv, prefix) {
			return true
		}
	}
	return false
}
