package setup

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cred"
	"github.com/manishtiwari25/pulse/internal/store"
)

const testKey = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n"
const testToken = "ops_abcdefghijklmnopqrstuvwxyz0123456789"

func setupStore(t *testing.T) *store.Store {
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

// fakeOp answers op with canned stdout per verb and ssh-keygen with a fixed key; it records every call.
type fakeOp struct {
	calls []string
	env   [][]string
	out   map[string]string // "vault list", "item list", "read"
	fail  string            // verb whose call fails
}

func (f *fakeOp) runner() cred.Runner {
	return func(_ context.Context, name string, args []string, _ string, env []string) (string, string, error) {
		f.calls = append(f.calls, name+" "+strings.Join(args, " "))
		f.env = append(f.env, env)
		switch name {
		case "op":
			verb := strings.Join(args[:2], " ")
			if args[0] == "read" {
				verb = "read"
			}
			if f.fail == verb {
				return "", "[ERROR] 2026/09/15 06:00:00 unauthorized: token " + testToken + " rejected", errors.New("exit 1")
			}
			return f.out[verb], "", nil
		case "ssh-keygen":
			if args[0] == "-y" {
				return "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakePublicKey pulse-push\n", "", nil
			}
			return "256 SHA256:FakeFingerprint0123456789 pulse-push (ED25519)\n", "", nil
		}
		return "", "not faked", errors.New("exit 1")
	}
}

func resolver(home string, f *fakeOp, goos string) *cred.Resolver {
	return &cred.Resolver{Home: home, Run: f.runner(), GOOS: goos, Getenv: func(k string) string {
		if k == cred.TokenEnv {
			return testToken
		}
		return ""
	}}
}

func TestOpReferenceUsesNamesWhileTheySurvive(t *testing.T) {
	v := Vault{ID: "vid1", Name: "PULSE"}
	if got := OpReference(v, "pulse push key", "iid1"); got != "op://PULSE/pulse push key/private key?ssh-format=openssh" {
		t.Fatalf("got %q", got)
	}
	if got := OpReference(Vault{ID: "vid1", Name: "ops/prod"}, "key?", "iid1"); got != "op://vid1/iid1/private key?ssh-format=openssh" {
		t.Fatalf("ids expected for names with / or ?: %q", got)
	}
	if got := OpReference(Vault{ID: "vid1", Name: " padded "}, "", "iid1"); got != "op://vid1/iid1/private key?ssh-format=openssh" {
		t.Fatalf("ids expected for padded or empty names: %q", got)
	}
	for _, ref := range []string{OpReference(v, "pulse push key", "iid1"), OpReference(Vault{ID: "vid1"}, "a/b", "iid1")} {
		if _, err := cred.Parse(ref); err != nil {
			t.Fatalf("%s does not parse: %v", ref, err)
		}
	}
}

func TestVaultsAndItemsListThroughOp(t *testing.T) {
	f := &fakeOp{out: map[string]string{
		"vault list": `[{"id":"v1","name":"PULSE"},{"id":"v2","name":"ops/shared"}]`,
		"item list":  `[{"id":"i1","title":"pulse push key","category":"SSH_KEY","updated_at":"2026-09-14T10:00:00Z","vault":{"id":"v1","name":"PULSE"}}]`,
	}}
	r := resolver(t.TempDir(), f, "linux")
	vaults, err := Vaults(context.Background(), r)
	if err != nil || len(vaults) != 2 || vaults[0].Name != "PULSE" {
		t.Fatalf("vaults %v %v", vaults, err)
	}
	items, err := Items(context.Background(), r, "v1")
	if err != nil || len(items) != 1 {
		t.Fatalf("items %v %v", items, err)
	}
	if items[0].Reference != "op://PULSE/pulse push key/private key?ssh-format=openssh" || items[0].Title != "pulse push key" {
		t.Fatalf("item %+v", items[0])
	}
	if !strings.Contains(f.calls[1], "item list --vault v1 --categories SSH Key --format json") {
		t.Fatalf("op call %q", f.calls[1])
	}
	for _, env := range f.env {
		found := false
		for _, kv := range env {
			if kv == cred.TokenEnv+"="+testToken {
				found = true
			}
			if strings.HasPrefix(kv, "OP_CONNECT_") {
				t.Fatalf("connect variables must be stripped: %s", kv)
			}
		}
		if !found {
			t.Fatal("token missing from op's environment")
		}
	}
	if _, err := Items(context.Background(), r, " "); err == nil {
		t.Fatal("an empty vault must be refused")
	}
	f.out["vault list"] = "not json"
	if _, err := Vaults(context.Background(), r); err == nil || strings.Contains(err.Error(), "not json") {
		t.Fatalf("bad JSON must be named without echoing it: %v", err)
	}
	f.fail = "vault list"
	_, err = Vaults(context.Background(), r)
	if err == nil || strings.Contains(err.Error(), testToken) {
		t.Fatalf("op failure must be redacted: %v", err)
	}
	empty := &fakeOp{out: map[string]string{"vault list": "[]"}}
	vaults, err = Vaults(context.Background(), resolver(t.TempDir(), empty, "linux"))
	if err != nil || vaults == nil || len(vaults) != 0 {
		t.Fatalf("no vaults must be an empty list, not null: %v %v", vaults, err)
	}
}

func TestProvidersFollowTheMachine(t *testing.T) {
	f := &fakeOp{}
	mac := Providers(resolver("/h", f, "darwin"))
	if len(mac) != 3 || mac[0].ID != "op" || !mac[0].Available || !mac[0].Browse || !mac[0].NeedsToken {
		t.Fatalf("op provider %+v", mac[0])
	}
	if !mac[1].Available || mac[1].ID != "keychain" || len(mac[1].Fields) != 2 {
		t.Fatalf("keychain on macOS %+v", mac[1])
	}
	if !mac[2].Generate || mac[2].Example != "file:///h/secrets/push_key" {
		t.Fatalf("file provider %+v", mac[2])
	}
	lin := Providers(resolver("/h", f, "linux"))
	if lin[1].Available || lin[1].Unavailable == "" {
		t.Fatalf("keychain must be unavailable on linux with a reason: %+v", lin[1])
	}
}

func TestGenerateThenApplyAFileKey(t *testing.T) {
	if _, err := exec.LookPath("ssh-keygen"); err != nil {
		t.Skip("ssh-keygen not installed")
	}
	s := setupStore(t)
	r := cred.New(s.Home)
	r.GOOS = "linux"
	g, err := Generate(context.Background(), r, false, Deps{})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(g.PublicKey, "ssh-ed25519 ") || !strings.HasPrefix(g.Fingerprint, "SHA256:") || g.Existed {
		t.Fatalf("generated %+v", g)
	}
	if g.Reference != "file://"+filepath.Join(s.Home, "secrets", "push_key") {
		t.Fatalf("reference %q", g.Reference)
	}
	info, err := os.Stat(g.Path)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("key mode %v %v", info, err)
	}
	again, err := Generate(context.Background(), r, false, Deps{})
	if err != nil || !again.Existed || again.Fingerprint != g.Fingerprint {
		t.Fatalf("an existing key must be kept and described: %+v %v", again, err)
	}
	replaced, err := Generate(context.Background(), r, true, Deps{})
	if err != nil || replaced.Existed || replaced.Fingerprint == g.Fingerprint {
		t.Fatalf("replace must mint a new key: %+v %v", replaced, err)
	}

	// Apply through a probe that records the ssh command and passes, then one that refuses.
	var probes []string
	ok := Deps{Probe: func(_ context.Context, root, remote, ssh string) error {
		probes = append(probes, remote+" "+ssh)
		return nil
	}}
	root := pushProject(t, s, "alpha")
	res, err := Apply(context.Background(), s, r, "human", replaced.Reference, true, ok)
	if err != nil {
		t.Fatal(err)
	}
	if res.Fingerprint != replaced.Fingerprint || res.Source != "file" || len(res.Probes) != 1 || !res.Probes[0].OK || res.Probes[0].Project != "alpha" {
		t.Fatalf("result %+v", res)
	}
	if s.Setting("push.credential", "") != replaced.Reference {
		t.Fatal("reference not saved")
	}
	if len(probes) != 1 || !strings.Contains(probes[0], "-oIdentityAgent=none -i '") {
		t.Fatalf("probe %v", probes)
	}
	if _, err := os.Stat(root); err != nil {
		t.Fatal(err)
	}
	rep := Status(context.Background(), s, r)
	if rep.Reference != replaced.Reference || rep.Fingerprint != replaced.Fingerprint || rep.Scheme != "file" || rep.SetBy != "human" {
		t.Fatalf("status %+v", rep)
	}

	// A refusal carries the public key and saves nothing.
	cleared, err := Clear(context.Background(), s, r, "human", false)
	if err != nil || cleared.Was != replaced.Reference {
		t.Fatalf("clear %+v %v", cleared, err)
	}
	refuse := Deps{Probe: func(context.Context, string, string, string) error {
		return errors.New("ERROR: The key you are authenticating with has been marked as read only.")
	}}
	_, err = Apply(context.Background(), s, r, "human", replaced.Reference, true, refuse)
	var ref *Refusal
	if !errors.As(err, &ref) || ref.PublicKey != replaced.PublicKey || ref.Code != 1 || !strings.Contains(ref.Why, "read only") {
		t.Fatalf("refusal %+v", err)
	}
	if len(ref.Probes) != 1 || ref.Probes[0].OK || ref.Probes[0].Why == "" {
		t.Fatalf("refusal probes %+v", ref.Probes)
	}
	if s.Setting("push.credential", "") != "" {
		t.Fatal("a refused key must not be saved")
	}
	// The key itself is never a reference.
	keyText, _ := os.ReadFile(replaced.Path)
	_, err = Apply(context.Background(), s, r, "human", string(keyText), false, Deps{})
	if !errors.As(err, &ref) || ref.Code != 2 || strings.Contains(ref.Error(), "PRIVATE KEY-----\n") {
		t.Fatalf("pasted key %v", err)
	}
	// No probe when asked not to: saved unprobed, the probe list empty.
	res, err = Apply(context.Background(), s, r, "human", replaced.Reference, false, refuse)
	if err != nil || res.Probed || len(res.Probes) != 0 {
		t.Fatalf("no-probe %+v %v", res, err)
	}
	// Inside a run, nothing is generated.
	t.Setenv("PULSE_JOB", "job-1")
	if _, err := Generate(context.Background(), r, true, Deps{}); !errors.Is(err, cred.ErrWorker) {
		t.Fatalf("worker guard: %v", err)
	}
}

func TestApplyThroughOpNamesTheItemNotTheKey(t *testing.T) {
	s := setupStore(t)
	f := &fakeOp{out: map[string]string{"read": testKey}}
	r := resolver(s.Home, f, "linux")
	res, err := Apply(context.Background(), s, r, "human:dashboard", "op://PULSE/pulse push key/private key?ssh-format=openssh", false, Deps{})
	if err != nil {
		t.Fatal(err)
	}
	if res.Source != "1Password (token from the environment)" || res.Fingerprint != "SHA256:FakeFingerprint0123456789" {
		t.Fatalf("result %+v", res)
	}
	if _, err := os.Stat(filepath.Join(s.Home, "tmp")); err == nil {
		if entries, _ := os.ReadDir(filepath.Join(s.Home, "tmp")); len(entries) != 0 {
			t.Fatalf("materialized key left behind: %v", entries)
		}
	}
	f.fail = "read"
	_, err = Apply(context.Background(), s, r, "human:dashboard", "op://PULSE/missing/private key?ssh-format=openssh", false, Deps{})
	var ref *Refusal
	if !errors.As(err, &ref) || strings.Contains(ref.Why, testToken) || ref.What != "The key could not be read" {
		t.Fatalf("op failure %v", err)
	}
}

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
		// Isolate from the host's global/system git config so the fixture
		// never reaches a 1Password/ssh signing agent.
		cmd := exec.Command("git", args...)
		cmd.Env = append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	if _, err := s.DB.Exec("INSERT INTO projects (slug,name,path,default_branch,added_at) VALUES (?,?,?,?,?)", slug, slug, root, "main", s.Now()); err != nil {
		t.Fatal(err)
	}
	return root
}
