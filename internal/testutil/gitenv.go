// Package testutil holds small helpers shared by the package test suites. The
// hook GIT_* environment scrub lives in internal/gitenv; callers that only
// need the scrub use gitenv.StripHook directly.
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
	return append(gitenv.StripHook(os.Environ()), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
}
