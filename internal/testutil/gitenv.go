// Package testutil holds small helpers shared by the package test suites.
package testutil

import (
	"os"
	"strings"
)

// hookGitVars are the GIT_* variables a running git hook (pre-commit running
// the test suite, for example) exports to its children. Inherited by a test's
// git child they redirect every call at the hook's index, dir, prefix and
// author, so fixture repositories fail or read the wrong tree. The list
// mirrors runner.hookGitEnvPrefixes.
var hookGitVars = []string{
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

// GitEnv returns the current environment with the host's system and global
// git config disabled, so fixture repositories created in tests never inherit
// commit signing (1Password/ssh agents), hooks paths or other host settings,
// and with the hook-exported GIT_* variables dropped so the suite behaves the
// same under a pre-commit hook as from a plain shell.
func GitEnv() []string {
	env := make([]string, 0, len(os.Environ())+2)
	for _, kv := range os.Environ() {
		if isHookGitVar(kv) {
			continue
		}
		env = append(env, kv)
	}
	return append(env, "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
}

func isHookGitVar(kv string) bool {
	for _, prefix := range hookGitVars {
		if strings.HasPrefix(kv, prefix) {
			return true
		}
	}
	return false
}
