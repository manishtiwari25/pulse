package cred

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Material is a private key on disk for the seconds of one push.
type Material struct {
	Dir        string
	KeyPath    string
	SSHCommand string // GIT_SSH_COMMAND value: ssh that uses only this key and never prompts
}

// Materialize writes the key under PULSE_HOME/tmp with a random directory name (0700) and file mode 0600.
func (r *Resolver) Materialize(secret *Secret) (*Material, error) {
	base := filepath.Join(r.Home, "tmp")
	if err := os.MkdirAll(base, 0o700); err != nil {
		return nil, err
	}
	var buf [8]byte
	if _, err := rand.Read(buf[:]); err != nil {
		return nil, err
	}
	dir := filepath.Join(base, "push-"+hex.EncodeToString(buf[:]))
	if err := os.Mkdir(dir, 0o700); err != nil {
		return nil, err
	}
	keyPath := filepath.Join(dir, "key")
	if err := os.WriteFile(keyPath, secret.Key, 0o600); err != nil {
		_ = os.RemoveAll(dir)
		return nil, err
	}
	return &Material{Dir: dir, KeyPath: keyPath, SSHCommand: SSHCommand(keyPath)}, nil
}

// SSHCommand is the GIT_SSH_COMMAND that pushes with exactly one key: no agent, no other identities,
// no prompts. The path is single-quoted for the shell git hands the command to.
func SSHCommand(keyPath string) string {
	return fmt.Sprintf("ssh -oBatchMode=yes -oIdentitiesOnly=yes -oIdentityAgent=none -i '%s'",
		strings.ReplaceAll(keyPath, "'", `'\''`))
}

// Cleanup removes the key and its directory. Safe to call twice.
func (m *Material) Cleanup() {
	if m == nil || m.Dir == "" {
		return
	}
	if m.KeyPath != "" {
		// Overwrite before unlinking so a snapshot taken between the two shows no key.
		if info, err := os.Stat(m.KeyPath); err == nil {
			_ = os.WriteFile(m.KeyPath, make([]byte, info.Size()), 0o600)
		}
	}
	_ = os.RemoveAll(m.Dir)
}
