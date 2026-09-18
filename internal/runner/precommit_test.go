package runner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestPreCommitHookScriptRunsVerifyWithRecipeEnv(t *testing.T) {
	recipe := &Recipe{Env: map[string]string{"CI": "1", "FOO": "bar baz"}}
	script := preCommitHookScript("make test", recipe, nil, "/tmp/clone")
	if !strings.Contains(script, "export CI=1 FOO='bar baz'") {
		t.Fatalf("recipe env not exported correctly:\n%s", script)
	}
	if !strings.Contains(script, "make test") {
		t.Fatalf("verify command missing:\n%s", script)
	}
	if !strings.HasPrefix(script, "#!/bin/sh\n") {
		t.Fatalf("not a shell script:\n%s", script)
	}
}

func TestPreCommitHookScriptWrapsMisePins(t *testing.T) {
	recipe := &Recipe{Env: map[string]string{"CI": "1"}}
	script := preCommitHookScript("go test ./...", recipe, []string{"go@1.22", "node@20"}, "/tmp/clone")
	if !strings.Contains(script, "export CI=1") {
		t.Fatalf("recipe env missing:\n%s", script)
	}
	if !strings.Contains(script, "mise exec go@1.22 node@20 -- sh -c 'go test ./...'") {
		t.Fatalf("mise wrap missing or wrong:\n%s", script)
	}
}

func TestWritePreCommitHookCreatesExecutableHook(t *testing.T) {
	dir := t.TempDir()
	clone := filepath.Join(dir, "clone")
	if err := os.MkdirAll(filepath.Join(clone, ".git", "hooks"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := writePreCommitHook(clone, "#!/bin/sh\nexit 0\n"); err != nil {
		t.Fatal(err)
	}
	hook := filepath.Join(clone, ".git", "hooks", "pre-commit")
	st, err := os.Stat(hook)
	if err != nil {
		t.Fatal(err)
	}
	if st.Mode()&0o111 == 0 {
		t.Fatalf("hook is not executable: %o", st.Mode())
	}
	body, err := os.ReadFile(hook)
	if err != nil {
		t.Fatal(err)
	}
	if string(body) != "#!/bin/sh\nexit 0\n" {
		t.Fatalf("unexpected hook body: %s", body)
	}
}
