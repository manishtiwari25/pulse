// Package gitenv builds the process environment pulse hands to child git
// commands: the caller's environment with every inherited GIT_* variable
// dropped (a hook's GIT_DIR/GIT_INDEX_FILE would redirect the child) and the
// host's system/global config, credential prompts and optional locks disabled.
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
