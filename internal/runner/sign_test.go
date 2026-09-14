package runner

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
)

// Off macOS signing is silent and does nothing. On macOS every build gets the same designated requirement,
// `identifier "pulse"`, whether it is the daemon's pulse.new (signed in place) or the running binary at
// `pulse service install` (a signed copy renamed over it), and CodeSigned tells the two apart from a
// plain ad-hoc build (ADR-035 rule 8).
func TestSigningGivesEveryBuildTheSameRequirement(t *testing.T) {
	testDeadline(t, 60*time.Second)
	dir := t.TempDir()
	binary := filepath.Join(dir, "pulse")
	if err := os.WriteFile(binary, []byte("not a mach-o"), 0o755); err != nil {
		t.Fatal(err)
	}

	signingSystem = "linux"
	t.Cleanup(func() { signingSystem = "" })
	if signed(binary) {
		t.Fatal("nothing is signed off macOS")
	}
	if err := sign(binary, true); err != nil {
		t.Fatalf("off macOS sign is a no-op: %v", err)
	}
	var out bytes.Buffer
	ensureSigned(&out, binary)
	if out.Len() != 0 {
		t.Fatalf("off macOS ensureSigned says nothing: %q", out.String())
	}
	if data, _ := os.ReadFile(binary); string(data) != "not a mach-o" {
		t.Fatal("the file was touched off macOS")
	}

	if runtime.GOOS != "darwin" {
		return
	}
	if _, err := exec.LookPath("codesign"); err != nil {
		t.Skip("no codesign")
	}
	signingSystem = "darwin"
	missing := filepath.Join(dir, "missing")
	if err := sign(missing, true); err == nil {
		t.Fatal("a missing file cannot be signed")
	}
	if fileExists(missing + ".signing") {
		t.Fatal("a failed sign left its copy behind")
	}
	// A real ad-hoc Go build: the linker's requirement is its cdhash until pulse signs it.
	build := filepath.Join(dir, "hello")
	src := filepath.Join(dir, "hello.go")
	if err := os.WriteFile(src, []byte("package main\nfunc main() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if out, err := exec.Command("go", "build", "-o", build, src).CombinedOutput(); err != nil {
		t.Skipf("go build: %v %s", err, out)
	}
	if ok, applicable := cli.CodeSigned(build); ok || !applicable {
		t.Fatalf("a fresh build is not pulse-signed: %v %v", ok, applicable)
	}
	if err := sign(build, false); err != nil {
		t.Fatal(err)
	}
	if !signed(build) {
		t.Fatal("signed in place: the requirement is pulse's")
	}
	req, _ := exec.Command("codesign", "-d", "-r-", build).CombinedOutput()
	if text := strings.ReplaceAll(string(req), `"`, ""); !strings.Contains(text, "designated => identifier pulse") {
		t.Fatalf("requirement: %s", req)
	}
	if out, err := exec.Command("codesign", "--verify", "--strict", build).CombinedOutput(); err != nil {
		t.Fatalf("the signed build does not verify: %v %s", err, out)
	}
	out.Reset()
	ensureSigned(&out, build)
	if !strings.Contains(out.String(), "is signed as pulse") {
		t.Fatalf("a signed binary is left alone: %q", out.String())
	}
	// The running-binary path: a copy is signed and renamed over the original.
	if out, err := exec.Command("codesign", "--force", "-s", "-", build).CombinedOutput(); err != nil {
		t.Fatalf("re-sign ad-hoc: %v %s", err, out)
	}
	out.Reset()
	ensureSigned(&out, build)
	if !strings.Contains(out.String(), "signed "+build+" as pulse") || !signed(build) || fileExists(build+".signing") {
		t.Fatalf("running-binary sign: %q signed=%v", out.String(), signed(build))
	}
}

// TestSigningKeyMustExistBeforeIdentity: pointing user.signingkey at a missing key would only
// surface when the worker commits, losing finished work; the runner must fail early instead.
func TestSigningKeyMustExistBeforeIdentity(t *testing.T) {
	testDeadline(t, 60*time.Second)
	clone := t.TempDir()
	run(t, clone, "git", "init", "-q")
	missing := filepath.Join(t.TempDir(), "missing.key")
	err := configureCloneIdentity(clone, "aurora", missing, provenanceHook("aurora", "JOB-1", localExecutor))
	if err == nil {
		t.Fatal("identity configured with a missing signing key")
	}
	if !strings.Contains(err.Error(), "signing key") {
		t.Fatalf("unclear error: %v", err)
	}
	cmd := exec.Command("git", "config", "--local", "--get", "user.signingkey")
	cmd.Dir = clone
	if out, err := cmd.Output(); err == nil {
		t.Fatalf("user.signingkey set despite missing key: %q", out)
	}
}

// TestSigningKeyCreatesWorkDir: makeRunKey writes the key even when PULSE_HOME/work is not there yet.
func TestSigningKeyCreatesWorkDir(t *testing.T) {
	if _, err := exec.LookPath("ssh-keygen"); err != nil {
		t.Skip("no ssh-keygen")
	}
	keyPath := filepath.Join(t.TempDir(), "work", "job.key")
	if _, err := makeRunKey(keyPath, "aurora", "run-1"); err != nil {
		t.Fatal(err)
	}
	if !fileExists(keyPath) {
		t.Fatal("key not written")
	}
}

// configureCloneIdentity writes one include file instead of eight `git config` calls; the values
// must still read back through git, including paths with spaces and quotes.
func TestConfigureCloneIdentityIncludeFile(t *testing.T) {
	dir := filepath.Join(t.TempDir(), `with space "q"`)
	run(t, "", "git", "init", "-q", dir)
	key := filepath.Join(dir, `key "x" y`)
	if err := os.WriteFile(key, []byte("k"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := configureCloneIdentity(dir, "aurora", key, "#!/bin/sh\n"); err != nil {
		t.Fatal(err)
	}
	for k, want := range map[string]string{"user.name": "aurora", "user.email": "aurora@pulse.local",
		"gpg.format": "ssh", "gpg.ssh.program": "ssh-keygen", "user.signingkey": key,
		"commit.gpgsign": "true", "tag.gpgsign": "false", "core.hooksPath": filepath.Join(dir, ".git", "hooks")} {
		if got := git(dir, "config", "--get", k); got.Stdout != want+"\n" {
			t.Errorf("%s = %q want %q (%s)", k, got.Stdout, want, got.Stderr)
		}
	}
}
