package command

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/cred"
	"github.com/manishtiwari25/pulse/internal/cred/setup"
	"github.com/manishtiwari25/pulse/internal/store"
)

const testKey = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n"
const testToken = "ops_abcdefghijklmnopqrstuvwxyz0123456789"

func commandStore(t *testing.T) *store.Store {
	t.Helper()
	for _, key := range []string{"PULSE_JOB", "PULSE_AGENT", "PULSE_PROJECT", "PULSE_SESSION", "OP_SERVICE_ACCOUNT_TOKEN"} {
		t.Setenv(key, "")
	}
	home := t.TempDir()
	t.Setenv("PULSE_HOME", home)
	s, err := store.Open(store.Options{Home: home, Create: true, Now: "2026-09-15T06:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

// fakeTools answers op, security and ssh-keygen for the resolver; the real ones are never run here.
type fakeTools struct {
	calls   []string
	keyOut  string
	tokenIn string // what the keychain holds
}

func (f *fakeTools) runner() cred.Runner {
	return func(_ context.Context, name string, args []string, stdin string, env []string) (string, string, error) {
		f.calls = append(f.calls, name+" "+strings.Join(args, " "))
		switch name {
		case "security":
			joined := strings.Join(args, " ")
			switch {
			case joined == "-i":
				if !strings.Contains(stdin, "add-generic-password") {
					return "", "bad stdin", errors.New("exit 1")
				}
				f.tokenIn = strings.TrimSuffix(strings.TrimSpace(stdin[strings.LastIndex(stdin, "'")-len(testToken):]), "'")
				return "", "", nil
			case strings.Contains(joined, "find-generic-password -s pulse -a OP_SERVICE_ACCOUNT_TOKEN"):
				if f.tokenIn == "" {
					return "", "The specified item could not be found in the keychain.", errors.New("exit 44")
				}
				return f.tokenIn + "\n", "", nil
			case strings.Contains(joined, "delete-generic-password"):
				if f.tokenIn == "" {
					return "", "not found", errors.New("exit 44")
				}
				f.tokenIn = ""
				return "", "", nil
			}
		case "op":
			for _, kv := range env {
				if kv == "OP_SERVICE_ACCOUNT_TOKEN="+testToken {
					return f.keyOut, "", nil
				}
			}
			return "", "[ERROR] no token", errors.New("exit 1")
		case "ssh-keygen":
			if args[0] == "-y" {
				return "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakePublicKey pulse-push\n", "", nil
			}
			return "256 SHA256:FakeFingerprint0123456789 pulse-push (ED25519)\n", "", nil
		}
		return "", "not faked", errors.New("exit 1")
	}
}

type probeCall struct{ root, remote, ssh string }

func install(t *testing.T, tools *fakeTools, probeErr error) *[]probeCall {
	t.Helper()
	calls := &[]probeCall{}
	old := deps
	deps = Deps{
		Resolver: func(home string) *cred.Resolver {
			return &cred.Resolver{Home: home, Run: tools.runner(), GOOS: "darwin", Getenv: func(k string) string { return "" }}
		},
		Stdin: strings.NewReader(testToken + "\n"),
		Probe: func(_ context.Context, root, remote, ssh string) error {
			*calls = append(*calls, probeCall{root, remote, ssh})
			if _, err := os.Stat(strings.Trim(strings.TrimPrefix(ssh, "ssh -oBatchMode=yes -oIdentitiesOnly=yes -oIdentityAgent=none -i "), "'")); err != nil {
				t.Errorf("key not on disk during the probe: %v", err)
			}
			return probeErr
		},
	}
	t.Cleanup(func() { deps = old })
	return calls
}

func credCommand(t *testing.T, c *cli.Context, args ...string) (string, error) {
	t.Helper()
	cmd, ok := cli.Lookup("cred")
	if !ok || !cmd.NoStore || !cmd.Raw {
		t.Fatal("cred must be a raw command that opens its own store")
	}
	var out bytes.Buffer
	c.Out, c.Err = &out, io.Discard
	err := cmd.Run(c, nil, args)
	return out.String(), err
}

func noSecret(t *testing.T, text string) {
	t.Helper()
	for _, leak := range []string{"b3BlbnNzaC", testToken, "BEGIN OPENSSH"} {
		if strings.Contains(text, leak) {
			t.Fatalf("secret in output: %q", text)
		}
	}
}

// pushProject registers a project whose default branch tracks a bare remote.
func pushProject(t *testing.T, s *store.Store, slug string) string {
	t.Helper()
	root := filepath.Join(t.TempDir(), slug)
	bare := filepath.Join(t.TempDir(), slug+".git")
	for _, args := range [][]string{
		{"init", "-q", "--bare", bare},
		{"init", "-q", "-b", "main", root},
		{"-C", root, "-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", "init"},
		{"-C", root, "remote", "add", "origin", bare},
		{"-C", root, "push", "-q", "-u", "origin", "main"},
	} {
		if out, err := gitOut(args...); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	if _, err := s.DB.Exec("INSERT INTO projects (slug,name,path,default_branch,added_at) VALUES (?,?,?,?,?)", slug, slug, root, "main", s.Now()); err != nil {
		t.Fatal(err)
	}
	return root
}

func TestCredTokenSetStatusClear(t *testing.T) {
	s := commandStore(t)
	root := pushProject(t, s, "demo")
	tools := &fakeTools{keyOut: testKey}
	probes := install(t, tools, nil)
	ctx := &cli.Context{Home: s.Home}

	out, err := credCommand(t, ctx, "status")
	if err != nil || !strings.Contains(out, "push.credential: not set") || !strings.Contains(out, "token: none") {
		t.Fatalf("status before: %v %q", err, out)
	}

	out, err = credCommand(t, ctx, "token")
	if err != nil {
		t.Fatalf("token: %v", err)
	}
	noSecret(t, out)
	if !strings.Contains(out, "kept in the keychain") || tools.tokenIn != testToken {
		t.Fatalf("token out %q, kept %q", out, tools.tokenIn)
	}
	for _, call := range tools.calls {
		if strings.Contains(call, testToken) {
			t.Fatalf("token on argv: %s", call)
		}
	}

	ref := "op://PULSE/pulse push key/private key?ssh-format=openssh"
	out, err = credCommand(t, ctx, "set", ref)
	if err != nil {
		t.Fatalf("set: %v\n%s", err, out)
	}
	noSecret(t, out)
	for _, want := range []string{"Key read from 1Password (token from the keychain): SHA256:FakeFingerprint0123456789", "Public key: ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakePublicKey pulse-push", "demo: origin answers this key", "push.credential=" + ref, "landing.push main"} {
		if !strings.Contains(out, want) {
			t.Errorf("set output lacks %q:\n%s", want, out)
		}
	}
	if len(*probes) != 1 || (*probes)[0].root != root || (*probes)[0].remote != "origin" {
		t.Fatalf("probes %+v", *probes)
	}
	if got := s.Setting("push.credential", ""); got != ref {
		t.Fatalf("setting = %q", got)
	}
	var detail string
	if err := s.DB.QueryRow("SELECT detail FROM events WHERE action = 'cred.set'").Scan(&detail); err != nil {
		t.Fatal(err)
	}
	noSecret(t, detail)
	if !strings.Contains(detail, "SHA256:FakeFingerprint0123456789") {
		t.Errorf("event %q", detail)
	}
	entries, _ := os.ReadDir(filepath.Join(s.Home, "tmp"))
	for _, e := range entries {
		if strings.HasPrefix(e.Name(), "push-") {
			t.Errorf("key directory left after set: %s", e.Name())
		}
	}

	out, err = credCommand(t, ctx, "status", "--json")
	if err != nil {
		t.Fatal(err)
	}
	var rep setup.Report
	if err := json.Unmarshal([]byte(out), &rep); err != nil {
		t.Fatalf("json %v: %s", err, out)
	}
	if rep.Reference != ref || rep.Scheme != "op" || rep.Fingerprint != "SHA256:FakeFingerprint0123456789" || rep.Token != "the keychain" || rep.SetBy != "human" || rep.LandingPush != "off" {
		t.Fatalf("report %+v", rep)
	}

	out, err = credCommand(t, ctx, "clear", "--token")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "push.credential cleared") || !strings.Contains(out, "removed from the keychain item") || !strings.Contains(out, "Revoke") {
		t.Fatalf("clear out %q", out)
	}
	if s.Setting("push.credential", "") != "" || tools.tokenIn != "" {
		t.Fatal("clear left the setting or the token")
	}
	out, _ = credCommand(t, ctx, "clear")
	if !strings.Contains(out, "was not set") {
		t.Fatalf("second clear %q", out)
	}
}

func TestCredSetRefusesKeysBadRefsAndFailedProbes(t *testing.T) {
	s := commandStore(t)
	pushProject(t, s, "demo")
	ctx := &cli.Context{Home: s.Home}

	tools := &fakeTools{keyOut: testKey, tokenIn: testToken}
	install(t, tools, nil)
	for _, bad := range []string{testKey, "ghp_abcdefghijklmnopqrstuvwxyz0123456789"} {
		out, err := credCommand(t, ctx, "set", bad)
		if err == nil || !strings.Contains(err.Error(), "the key, not a reference") {
			t.Errorf("set(secret) = %v", err)
		}
		noSecret(t, out+err.Error())
	}
	if _, err := credCommand(t, ctx, "set", "vault://x"); err == nil || !strings.Contains(err.Error(), "Not a push credential reference") {
		t.Errorf("bad ref = %v", err)
	}
	if _, err := credCommand(t, ctx, "set"); err == nil || !strings.Contains(err.Error(), "Wrong cred arguments") {
		t.Errorf("no ref = %v", err)
	}

	// No token: nothing saved, the fix is named.
	install(t, &fakeTools{keyOut: testKey}, nil)
	_, err := credCommand(t, ctx, "set", "op://V/I/F")
	if err == nil || !strings.Contains(err.Error(), "pulse cred token") || !strings.Contains(err.Error(), "Nothing was saved") {
		t.Errorf("no token = %v", err)
	}
	if s.Setting("push.credential", "") != "" {
		t.Fatal("saved without a key")
	}

	// The remote refuses the key: nothing saved, the public key to add is in the output.
	install(t, &fakeTools{keyOut: testKey, tokenIn: testToken}, errors.New("ERROR: The key you are authenticating with has been marked as read only."))
	out, err := credCommand(t, ctx, "set", "op://V/I/F?ssh-format=openssh")
	if err == nil || !strings.Contains(err.Error(), "does not open demo's remote origin") || !strings.Contains(err.Error(), "read only") || !strings.Contains(err.Error(), "deploy key") {
		t.Errorf("probe fail = %v", err)
	}
	if !strings.Contains(out, "Public key: ssh-ed25519") {
		t.Errorf("public key not shown: %q", out)
	}
	noSecret(t, out)
	if s.Setting("push.credential", "") != "" {
		t.Fatal("saved after a failed probe")
	}

	// --no-probe saves without asking the remote.
	probes := install(t, &fakeTools{keyOut: testKey, tokenIn: testToken}, errors.New("must not run"))
	if _, err := credCommand(t, ctx, "set", "op://V/I/F?ssh-format=openssh", "--no-probe"); err != nil {
		t.Fatalf("no-probe: %v", err)
	}
	if len(*probes) != 0 || s.Setting("push.credential", "") != "op://V/I/F?ssh-format=openssh" {
		t.Fatalf("no-probe probes %d setting %q", len(*probes), s.Setting("push.credential", ""))
	}
}

func TestCredIsTheOwners(t *testing.T) {
	s := commandStore(t)
	install(t, &fakeTools{keyOut: testKey, tokenIn: testToken}, nil)
	for _, c := range []*cli.Context{{Home: s.Home, Agent: "granite"}, {Home: s.Home, Agent: "human:dashboard"}} {
		for _, args := range [][]string{{"set", "op://V/I/F"}, {"token"}, {"clear"}} {
			_, err := credCommand(t, c, args...)
			if c.Agent == "granite" && (err == nil || !strings.Contains(err.Error(), "Only the owner")) {
				t.Errorf("agent %s %v = %v", c.Agent, args, err)
			}
			if c.Agent == "human:dashboard" && err != nil && strings.Contains(err.Error(), "Only the owner") {
				t.Errorf("console owner refused: %v", err)
			}
		}
	}
	// An agent may read.
	if _, err := credCommand(t, &cli.Context{Home: s.Home, Agent: "granite"}, "status"); err != nil {
		t.Errorf("agent status: %v", err)
	}
	// A worker never, whatever it claims.
	t.Setenv("PULSE_JOB", "job-1")
	if _, err := credCommand(t, &cli.Context{Home: s.Home}, "set", "op://V/I/F"); err == nil || !strings.Contains(err.Error(), "Only the owner") {
		t.Errorf("worker = %v", err)
	}
	t.Setenv("PULSE_JOB", "")
	t.Setenv("PULSE_AGENT", "granite")
	if _, err := credCommand(t, &cli.Context{Home: s.Home, Agent: "human"}, "clear"); err == nil || !strings.Contains(err.Error(), "Only the owner") {
		t.Errorf("--agent human from an agent session = %v", err)
	}
}

func TestCredTokenFromFileAndBadTokens(t *testing.T) {
	s := commandStore(t)
	tools := &fakeTools{}
	install(t, tools, nil)
	ctx := &cli.Context{Home: s.Home}
	path := filepath.Join(t.TempDir(), "token.txt")
	if err := os.WriteFile(path, []byte(testToken+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := credCommand(t, ctx, "token", "--from-file", path); err != nil || tools.tokenIn != testToken {
		t.Fatalf("from-file: %v kept %q", err, tools.tokenIn)
	}
	deps.Stdin = strings.NewReader("not a token\n")
	_, err := credCommand(t, ctx, "token")
	if err == nil || !strings.Contains(err.Error(), "Token not stored") {
		t.Fatalf("bad token = %v", err)
	}
	if _, err := credCommand(t, ctx, "token", "--from-file", filepath.Join(t.TempDir(), "missing")); err == nil || !strings.Contains(err.Error(), "Could not read") {
		t.Fatalf("missing file = %v", err)
	}
	if _, err := credCommand(t, ctx, "frobnicate"); err == nil || !strings.Contains(err.Error(), "Unknown cred command") {
		t.Fatalf("unknown = %v", err)
	}
}

// gitOut runs git isolated from the host's global/system config so the
// fixture repo never reaches a 1Password/ssh signing agent.
func gitOut(args ...string) (string, error) {
	cmd := exec.Command("git", args...)
	cmd.Env = append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
	out, err := cmd.CombinedOutput()
	return string(out), err
}
