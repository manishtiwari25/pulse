// Package testutil holds small helpers shared by the package test suites.
package testutil

import "os"

// GitEnv returns the current environment with the host's system and global
// git config disabled, so fixture repositories created in tests never inherit
// commit signing (1Password/ssh agents), hooks paths or other host settings.
func GitEnv() []string {
	return append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
}
