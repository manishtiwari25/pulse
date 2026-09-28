// Package testutil holds small helpers shared by the package test suites. The
// hook GIT_* environment scrub lives in internal/gitenv; the wrappers here keep
// the test-suite call sites and the runner/board callers on one list.
package testutil

import (
	"os"

	"github.com/manishtiwari25/pulse/internal/gitenv"
)

// GitEnv returns the current environment with the host's system and global
// git config disabled, so fixture repositories created in tests never inherit
// commit signing (1Password/ssh agents), hooks paths or other host settings,
// and with the hook-exported GIT_* variables dropped so the suite behaves the
// same under a pre-commit hook as from a plain shell.
func GitEnv() []string {
	return append(StripHookGitEnv(os.Environ()), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
}

// StripHookGitEnv returns env without the GIT_* variables a running git hook
// exports; see gitenv.StripHook for the list.
func StripHookGitEnv(env []string) []string { return gitenv.StripHook(env) }

// HookGitVar reports whether kv is one of the hook-exported GIT_* variables (gitenv.HookVar).
func HookGitVar(kv string) bool { return gitenv.HookVar(kv) }
