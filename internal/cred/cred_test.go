package cred

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const testKey = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n"

func TestParse(t *testing.T) {
	cases := []struct {
		in      string
		wantErr string
		check   func(Ref) bool
	}{
		{"op://PULSE/pulse push key/private key?ssh-format=openssh", "", func(r Ref) bool {
			return r.Scheme == "op" && r.Vault == "PULSE" && r.Item == "pulse push key" && r.Field == "private key" && r.Query == "ssh-format=openssh"
		}},
		{"op://V/I/section/field", "", func(r Ref) bool { return r.Field == "section/field" }},
		{"OP://V/I/F", "", func(r Ref) bool { return r.Scheme == "op" }},
		{"op://V/I", "needs a vault, an item and a field", nil},
		{"op://V//F", "needs a vault, an item and a field", nil},
		{"op://V/I/F?%zz", "malformed query", nil},
		{"keychain://pulse/push-key", "", func(r Ref) bool { return r.Service == "pulse" && r.Account == "push-key" }},
		{"keychain://pulse", "needs a service and an account", nil},
		{"keychain://pulse/a/b", "needs a service and an account", nil},
		{"file:///Users/me/.pulse/secrets/push_key", "", func(r Ref) bool { return r.Path == "/Users/me/.pulse/secrets/push_key" }},
		{"file://relative/key", "needs an absolute path", nil},
		{"file:///a/../b", "", func(r Ref) bool { return r.Path == "/b" }},
		{"", "empty", nil},
		{"   ", "empty", nil},
		{"ssh-key-here", "is not a reference", nil},
		{"vault://x/y", "unknown scheme", nil},
		{"op://V/I/F\nfile:///x", "one line", nil},
	}
	for _, c := range cases {
		ref, err := Parse(c.in)
		if c.wantErr != "" {
			if err == nil || !strings.Contains(err.Error(), c.wantErr) {
				t.Errorf("Parse(%q) error = %v, want containing %q", c.in, err, c.wantErr)
			}
			continue
		}
		if err != nil {
			t.Errorf("Parse(%q) unexpected error %v", c.in, err)
			continue
		}
		if !c.check(ref) {
			t.Errorf("Parse(%q) = %+v", c.in, ref)
		}
		if ref.String() != strings.TrimSpace(c.in) && !strings.HasPrefix(c.in, "OP") {
			t.Errorf("Parse(%q).String() = %q", c.in, ref.String())
		}
	}
}

type call struct {
	name  string
	args  []string
	stdin string
	env   []string
}

type fake struct {
	calls []call
	// reply is keyed by name; the function decides the output from the args.
	reply map[string]func(args []string, stdin string, env []string) (string, string, error)
}

func (f *fake) runner() Runner {
	return func(_ context.Context, name string, args []string, stdin string, env []string) (string, string, error) {
		f.calls = append(f.calls, call{name, args, stdin, env})
		if fn, ok := f.reply[name]; ok {
			return fn(args, stdin, env)
		}
		return "", "not faked: " + name, errors.New("exit 1")
	}
}

func envValue(env []string, key string) (string, bool) {
	for _, kv := range env {
		if strings.HasPrefix(kv, key+"=") {
			return kv[len(key)+1:], true
		}
	}
	return "", false
}

func newResolver(t *testing.T, f *fake, goos string, getenv map[string]string) *Resolver {
	t.Helper()
	return &Resolver{
		Home:   t.TempDir(),
		Run:    f.runner(),
		GOOS:   goos,
		Getenv: func(k string) string { return getenv[k] },
	}
}

func TestResolveOpUsesServiceAccountToken(t *testing.T) {
	t.Setenv("OP_CONNECT_HOST", "https://connect.example")
	t.Setenv("OP_CONNECT_TOKEN", "connect-secret")
	t.Setenv(TokenEnv, "from-process-env-should-be-replaced")
	f := &fake{reply: map[string]func([]string, string, []string) (string, string, error){
		"security": func(args []string, _ string, _ []string) (string, string, error) {
			if strings.Join(args, " ") == "find-generic-password -s pulse -a OP_SERVICE_ACCOUNT_TOKEN -w" {
				return "ops_abcdefghijklmnopqrstuvwxyz0123456789\n", "", nil
			}
			return "", "unexpected security call", errors.New("exit 1")
		},
		"op": func(args []string, _ string, env []string) (string, string, error) {
			if strings.Join(args, " ") != "read -n op://PULSE/pulse push key/private key?ssh-format=openssh" {
				return "", "bad args", errors.New("exit 1")
			}
			tok, _ := envValue(env, TokenEnv)
			if tok != "ops_abcdefghijklmnopqrstuvwxyz0123456789" {
				return "", "token not in env", errors.New("exit 1")
			}
			if _, ok := envValue(env, "OP_CONNECT_HOST"); ok {
				return "", "connect host leaked", errors.New("exit 1")
			}
			if _, ok := envValue(env, "OP_CONNECT_TOKEN"); ok {
				return "", "connect token leaked", errors.New("exit 1")
			}
			return testKey, "", nil
		},
	}}
	r := newResolver(t, f, "darwin", map[string]string{})
	ref, _ := Parse("op://PULSE/pulse push key/private key?ssh-format=openssh")
	sec, err := r.Resolve(context.Background(), ref)
	if err != nil {
		t.Fatalf("Resolve: %v", err)
	}
	if string(sec.Key) != testKey {
		t.Errorf("key = %q", sec.Key)
	}
	if sec.Source != "1Password (token from the keychain)" {
		t.Errorf("source = %q", sec.Source)
	}
	for _, c := range f.calls {
		for _, a := range c.args {
			if strings.Contains(a, "ops_") {
				t.Errorf("token passed on argv: %v", c.args)
			}
		}
	}
}

func TestResolveOpWithoutTokenSaysHow(t *testing.T) {
	f := &fake{reply: map[string]func([]string, string, []string) (string, string, error){}}
	r := newResolver(t, f, "linux", map[string]string{})
	ref, _ := Parse("op://V/I/F")
	_, err := r.Resolve(context.Background(), ref)
	if err == nil || !strings.Contains(err.Error(), "pulse cred token") {
		t.Fatalf("err = %v", err)
	}
	for _, c := range f.calls {
		if c.name == "op" {
			t.Error("op was run without a token")
		}
	}
}

func TestResolveOpTokenFromFileAndErrorRedaction(t *testing.T) {
	f := &fake{reply: map[string]func([]string, string, []string) (string, string, error){
		"op": func(_ []string, _ string, env []string) (string, string, error) {
			tok, _ := envValue(env, TokenEnv)
			return "", "[ERROR] 2026/09/15 token " + tok + " rejected: " + testKey, errors.New("exit 1")
		},
	}}
	r := newResolver(t, f, "linux", map[string]string{})
	if err := os.MkdirAll(filepath.Dir(r.tokenFile()), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(r.tokenFile(), []byte("ops_filetoken_0123456789abcdefghij\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	ref, _ := Parse("op://V/I/F")
	_, err := r.Resolve(context.Background(), ref)
	if err == nil {
		t.Fatal("expected error")
	}
	msg := err.Error()
	if strings.Contains(msg, "ops_filetoken") || strings.Contains(msg, "b3BlbnNzaC") {
		t.Fatalf("secret leaked into error: %q", msg)
	}
	if !strings.Contains(msg, "op read failed") {
		t.Errorf("msg = %q", msg)
	}
}

func TestResolveOpRefusesNonKeyWithHint(t *testing.T) {
	f := &fake{reply: map[string]func([]string, string, []string) (string, string, error){
		"op": func(_ []string, _ string, _ []string) (string, string, error) {
			return "ghp_not_a_key_at_all", "", nil
		},
	}}
	r := newResolver(t, f, "linux", map[string]string{TokenEnv: "ops_envtoken_0123456789abcdefghij"})
	ref, _ := Parse("op://V/I/private key")
	_, err := r.Resolve(context.Background(), ref)
	if err == nil || !strings.Contains(err.Error(), "not an SSH private key") || !strings.Contains(err.Error(), "ssh-format=openssh") {
		t.Fatalf("err = %v", err)
	}
	if strings.Contains(err.Error(), "ghp_") {
		t.Fatalf("value leaked: %v", err)
	}
}

func TestResolveKeychainDecodesHex(t *testing.T) {
	f := &fake{reply: map[string]func([]string, string, []string) (string, string, error){
		"security": func(args []string, _ string, _ []string) (string, string, error) {
			if strings.Join(args, " ") != "find-generic-password -s pulse -a push-key -w" {
				return "", "bad args", errors.New("exit 1")
			}
			// security -w prints multi-line secrets as hex.
			hexKey := ""
			for _, b := range []byte(testKey) {
				hexKey += string("0123456789abcdef"[b>>4]) + string("0123456789abcdef"[b&15])
			}
			return hexKey + "\n", "", nil
		},
	}}
	r := newResolver(t, f, "darwin", map[string]string{})
	ref, _ := Parse("keychain://pulse/push-key")
	sec, err := r.Resolve(context.Background(), ref)
	if err != nil {
		t.Fatalf("Resolve: %v", err)
	}
	if string(sec.Key) != testKey {
		t.Errorf("key = %q", sec.Key)
	}
	lin := newResolver(t, f, "linux", map[string]string{})
	if _, err := lin.Resolve(context.Background(), ref); err == nil || !strings.Contains(err.Error(), "macOS") {
		t.Errorf("linux keychain err = %v", err)
	}
}

func TestResolveFile(t *testing.T) {
	r := newResolver(t, &fake{}, "linux", map[string]string{})
	dir := t.TempDir()
	good := filepath.Join(dir, "key")
	if err := os.WriteFile(good, []byte(testKey), 0o600); err != nil {
		t.Fatal(err)
	}
	ref, _ := Parse("file://" + good)
	sec, err := r.Resolve(context.Background(), ref)
	if err != nil {
		t.Fatalf("Resolve: %v", err)
	}
	if string(sec.Key) != testKey || sec.Source != "file" {
		t.Errorf("secret = %+v", sec)
	}

	loose := filepath.Join(dir, "loose")
	if err := os.WriteFile(loose, []byte(testKey), 0o644); err != nil {
		t.Fatal(err)
	}
	ref, _ = Parse("file://" + loose)
	if _, err := r.Resolve(context.Background(), ref); err == nil || !strings.Contains(err.Error(), "readable by others") {
		t.Errorf("loose mode err = %v", err)
	}

	link := filepath.Join(dir, "link")
	if err := os.Symlink(good, link); err != nil {
		t.Fatal(err)
	}
	ref, _ = Parse("file://" + link)
	if _, err := r.Resolve(context.Background(), ref); err == nil || !strings.Contains(err.Error(), "symlinks are refused") {
		t.Errorf("symlink err = %v", err)
	}

	ref, _ = Parse("file://" + filepath.Join(dir, "missing"))
	if _, err := r.Resolve(context.Background(), ref); err == nil || !strings.Contains(err.Error(), "no such file") {
		t.Errorf("missing err = %v", err)
	}

	notKey := filepath.Join(dir, "notkey")
	if err := os.WriteFile(notKey, []byte("hello\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	ref, _ = Parse("file://" + notKey)
	if _, err := r.Resolve(context.Background(), ref); err == nil || !strings.Contains(err.Error(), "not an SSH private key") {
		t.Errorf("not-key err = %v", err)
	}
}

func TestResolveRefusedInsideWorker(t *testing.T) {
	dir := t.TempDir()
	good := filepath.Join(dir, "key")
	if err := os.WriteFile(good, []byte(testKey), 0o600); err != nil {
		t.Fatal(err)
	}
	ref, _ := Parse("file://" + good)
	r := newResolver(t, &fake{}, "linux", map[string]string{"PULSE_JOB": "job-x"})
	if _, err := r.Resolve(context.Background(), ref); !errors.Is(err, ErrWorker) {
		t.Errorf("PULSE_JOB err = %v", err)
	}
	r = newResolver(t, &fake{}, "linux", map[string]string{})
	r.Worker = true
	if _, err := r.Resolve(context.Background(), ref); !errors.Is(err, ErrWorker) {
		t.Errorf("Worker err = %v", err)
	}
}

func TestMaterializeAndCleanup(t *testing.T) {
	r := newResolver(t, &fake{}, "linux", map[string]string{})
	m, err := r.Materialize(&Secret{Key: []byte(testKey)})
	if err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(m.KeyPath)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Errorf("key mode = %04o", info.Mode().Perm())
	}
	dinfo, _ := os.Stat(m.Dir)
	if dinfo.Mode().Perm() != 0o700 {
		t.Errorf("dir mode = %04o", dinfo.Mode().Perm())
	}
	if !strings.HasPrefix(m.Dir, filepath.Join(r.Home, "tmp", "push-")) {
		t.Errorf("dir = %s", m.Dir)
	}
	want := "ssh -oBatchMode=yes -oIdentitiesOnly=yes -oIdentityAgent=none -i '" + m.KeyPath + "'"
	if m.SSHCommand != want {
		t.Errorf("ssh command = %q", m.SSHCommand)
	}
	m.Cleanup()
	if _, err := os.Stat(m.Dir); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("dir still there after cleanup: %v", err)
	}
	m.Cleanup() // idempotent
	var nilMaterial *Material
	nilMaterial.Cleanup()
}

func TestSSHCommandQuotesPath(t *testing.T) {
	got := SSHCommand("/tmp/it's here/key")
	if got != `ssh -oBatchMode=yes -oIdentitiesOnly=yes -oIdentityAgent=none -i '/tmp/it'\''s here/key'` {
		t.Errorf("got %q", got)
	}
}

func TestStoreToken(t *testing.T) {
	f := &fake{reply: map[string]func([]string, string, []string) (string, string, error){
		"security": func(args []string, stdin string, _ []string) (string, string, error) {
			if strings.Join(args, " ") != "-i" {
				return "", "token on argv", errors.New("exit 1")
			}
			if stdin != "add-generic-password -a OP_SERVICE_ACCOUNT_TOKEN -s pulse -U -w 'ops_abcdefghijklmnopqrstuvwxyz0123456789'\n" {
				return "", "bad stdin: " + stdin, errors.New("exit 1")
			}
			return "", "", nil
		},
	}}
	r := newResolver(t, f, "darwin", map[string]string{})
	where, err := r.StoreToken(context.Background(), "  ops_abcdefghijklmnopqrstuvwxyz0123456789\n")
	if err != nil {
		t.Fatalf("StoreToken: %v", err)
	}
	if !strings.Contains(where, "keychain") {
		t.Errorf("where = %q", where)
	}

	lin := newResolver(t, &fake{}, "linux", map[string]string{})
	where, err = lin.StoreToken(context.Background(), "ops_abcdefghijklmnopqrstuvwxyz0123456789")
	if err != nil {
		t.Fatalf("StoreToken linux: %v", err)
	}
	info, err := os.Stat(where)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("token file %s: %v mode %v", where, err, info)
	}
	tok, src, err := lin.Token(context.Background())
	if err != nil || tok != "ops_abcdefghijklmnopqrstuvwxyz0123456789" || src != "the token file" {
		t.Errorf("Token = %q %q %v", tok, src, err)
	}
	removed, err := lin.ClearToken(context.Background())
	if err != nil || len(removed) != 1 {
		t.Errorf("ClearToken = %v %v", removed, err)
	}
	if _, _, err := lin.Token(context.Background()); err == nil {
		t.Error("token still resolves after clear")
	}

	for _, bad := range []string{"", "short", "has space in it and is long enough", "quote'inside_and_long_enough_x", "two\nlines_long_enough_abcdefgh"} {
		if _, err := lin.StoreToken(context.Background(), bad); err == nil {
			t.Errorf("StoreToken(%q) accepted", bad)
		}
	}
}

func TestTokenPrefersEnvironment(t *testing.T) {
	f := &fake{}
	r := newResolver(t, f, "darwin", map[string]string{TokenEnv: "ops_envtoken_0123456789abcdefghij"})
	tok, src, err := r.Token(context.Background())
	if err != nil || tok != "ops_envtoken_0123456789abcdefghij" || src != "the environment" {
		t.Errorf("Token = %q %q %v", tok, src, err)
	}
	if len(f.calls) != 0 {
		t.Errorf("security was called: %v", f.calls)
	}
}

func TestInspect(t *testing.T) {
	f := &fake{reply: map[string]func([]string, string, []string) (string, string, error){
		"ssh-keygen": func(args []string, _ string, _ []string) (string, string, error) {
			switch args[0] {
			case "-y":
				return "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample pulse-push\n", "", nil
			case "-l":
				return "256 SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG pulse-push (ED25519)\n", "", nil
			}
			return "", "bad args", errors.New("exit 1")
		},
	}}
	r := newResolver(t, f, "linux", map[string]string{})
	pub, fp, err := r.Inspect(context.Background(), "/tmp/key")
	if err != nil {
		t.Fatal(err)
	}
	if pub != "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample pulse-push" || fp != "SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG" {
		t.Errorf("pub=%q fp=%q", pub, fp)
	}
}

func TestRedact(t *testing.T) {
	in := "failed: " + testKey + " token ops_abcdefghijklmnopqrstuvwxyz0123456789 and plain-secret end"
	out := Redact(in, "plain-secret")
	for _, leak := range []string{"b3BlbnNzaC", "ops_abc", "plain-secret"} {
		if strings.Contains(out, leak) {
			t.Errorf("leaked %q in %q", leak, out)
		}
	}
	if !strings.Contains(out, "[private key]") || !strings.Contains(out, "[token]") || !strings.Contains(out, "[redacted]") || !strings.HasSuffix(out, "end") {
		t.Errorf("out = %q", out)
	}
	if Redact("clean text") != "clean text" {
		t.Error("clean text changed")
	}
	// An unterminated key block is still removed.
	if got := Redact("x -----BEGIN RSA PRIVATE KEY-----\nabc"); strings.Contains(got, "abc") {
		t.Errorf("unterminated block leaked: %q", got)
	}
}

func TestIsPrivateKey(t *testing.T) {
	for _, ok := range []string{testKey, "-----BEGIN RSA PRIVATE KEY-----\nx\n-----END RSA PRIVATE KEY-----", "  -----BEGIN PRIVATE KEY-----\n", "-----BEGIN EC PRIVATE KEY-----"} {
		if !IsPrivateKey([]byte(ok)) {
			t.Errorf("IsPrivateKey(%q) = false", ok)
		}
	}
	for _, no := range []string{"", "ssh-ed25519 AAAA", "-----BEGIN CERTIFICATE-----", "ghp_token", "-----BEGIN PUBLIC KEY-----"} {
		if IsPrivateKey([]byte(no)) {
			t.Errorf("IsPrivateKey(%q) = true", no)
		}
	}
}
