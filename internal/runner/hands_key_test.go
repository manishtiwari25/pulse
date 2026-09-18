package runner

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// TestHandsKeyRefusesSigningKeyWithoutKeyFile: hands.key must not point user.signingkey at a
// run.key that was never written (or is not a key), mirroring configureCloneIdentity (ADR-022).
func TestHandsKeyRefusesSigningKeyWithoutKeyFile(t *testing.T) {
	if os.Getuid() == 0 {
		t.Skip("root ignores directory permissions")
	}
	const job = "job-20260918T000000Z-abcdef"
	cases := []struct {
		name     string
		stdin    string
		readonly bool
		wantErr  string
		wantKey  bool
	}{
		{name: "write fails", stdin: "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n", readonly: true, wantErr: "permission denied"},
		{name: "not a key", stdin: "junk", wantErr: "not an openssh private key"},
		{name: "ok", stdin: "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n", wantKey: true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			workdir := t.TempDir()
			dir := filepath.Join(workdir, job)
			clone := filepath.Join(dir, "clone")
			if err := os.MkdirAll(clone, 0o755); err != nil {
				t.Fatal(err)
			}
			for _, args := range [][]string{{"init", "-q"}, {"config", "user.name", "aurora"}} {
				cmd := exec.Command("git", args...)
				cmd.Dir = clone
				if out, err := cmd.CombinedOutput(); err != nil {
					t.Fatalf("git %v: %v %s", args, err, out)
				}
			}
			if tc.readonly {
				if err := os.Chmod(dir, 0o555); err != nil {
					t.Fatal(err)
				}
				t.Cleanup(func() { _ = os.Chmod(dir, 0o755) })
			}
			h := &hands{workdir: workdir, home: t.TempDir(), stdin: strings.NewReader(tc.stdin), stdout: &bytes.Buffer{}, stderr: &bytes.Buffer{}}
			err := h.key(job, "exec")
			if tc.wantErr != "" && (err == nil || !strings.Contains(err.Error(), tc.wantErr)) {
				t.Fatalf("err = %v, want %q", err, tc.wantErr)
			}
			if tc.wantErr == "" && err != nil {
				t.Fatalf("key: %v", err)
			}
			cmd := exec.Command("git", "config", "--local", "--get", "user.signingkey")
			cmd.Dir = clone
			out, _ := cmd.Output()
			got := strings.TrimSpace(string(out))
			if tc.wantKey && got != filepath.Join(dir, "run.key") {
				t.Fatalf("user.signingkey = %q, want run.key", got)
			}
			if !tc.wantKey && got != "" {
				t.Fatalf("user.signingkey set to %q despite missing key", got)
			}
			if !tc.wantKey && fileExists(filepath.Join(dir, "run.key")) {
				t.Fatalf("run.key left behind")
			}
		})
	}
}
