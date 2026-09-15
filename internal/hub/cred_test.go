package hub

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cred"
	"github.com/manishtiwari25/pulse/internal/cred/setup"
)

const credTestKey = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n"
const credTestToken = "ops_abcdefghijklmnopqrstuvwxyz0123456789"

// credTools fakes op, security and ssh-keygen on a linux resolver: the token lives in a 0600 file under
// the store's home, so nothing here touches the machine's Keychain.
type credTools struct {
	calls  []string
	vaults string
	items  string
	readOK bool
}

func (f *credTools) runner() cred.Runner {
	return func(_ context.Context, name string, args []string, _ string, env []string) (string, string, error) {
		f.calls = append(f.calls, name+" "+strings.Join(args, " "))
		switch name {
		case "op":
			tokened := false
			for _, kv := range env {
				if kv == cred.TokenEnv+"="+credTestToken {
					tokened = true
				}
			}
			if !tokened {
				return "", "[ERROR] no token", errors.New("exit 1")
			}
			switch args[0] {
			case "vault":
				return f.vaults, "", nil
			case "item":
				return f.items, "", nil
			case "read":
				if !f.readOK {
					return "", "[ERROR] 2026/09/15 could not read " + credTestToken, errors.New("exit 1")
				}
				return credTestKey, "", nil
			}
		case "ssh-keygen":
			if args[0] == "-y" {
				return "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakePublicKey pulse-push\n", "", nil
			}
			return "256 SHA256:FakeFingerprint0123456789 pulse-push (ED25519)\n", "", nil
		}
		return "", "not faked", errors.New("exit 1")
	}
}

func credRequest(t *testing.T, h http.Handler, method, path, body, host string, confirmed bool) (int, map[string]any, string) {
	t.Helper()
	req := httptest.NewRequest(method, path, strings.NewReader(body))
	req.Host = host
	if method == http.MethodPost {
		req.Header.Set("Content-Type", "application/json")
	}
	if confirmed {
		req.Header.Set(credHeader, "yes")
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	var result map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &result); err != nil {
		t.Fatalf("%s %s: status %d, non-JSON response %q: %v", method, path, rec.Code, rec.Body, err)
	}
	return rec.Code, result, rec.Body.String()
}

func TestCredRoutes(t *testing.T) {
	s := additionsStore(t, true)
	t.Setenv("OP_SERVICE_ACCOUNT_TOKEN", "")
	tools := &credTools{
		vaults: `[{"id":"v1","name":"PULSE"}]`,
		items:  `[{"id":"i1","title":"pulse push key","category":"SSH_KEY","vault":{"id":"v1","name":"PULSE"}}]`,
		readOK: true,
	}
	oldResolver, oldDeps := credResolver, credDeps
	credResolver = func(home string) *cred.Resolver {
		return &cred.Resolver{Home: home, Run: tools.runner(), GOOS: "linux", Getenv: func(string) string { return "" }}
	}
	probeErr := errors.New("ERROR: The key you are authenticating with has been marked as read only.")
	var probes []string
	credDeps = setup.Deps{
		Probe: func(_ context.Context, root, remote, ssh string) error {
			probes = append(probes, remote)
			return probeErr
		},
		Keygen: func(_ context.Context, path, comment string) error {
			if !strings.HasPrefix(comment, "pulse push key") {
				t.Errorf("comment %q", comment)
			}
			return os.WriteFile(path, []byte(credTestKey), 0o600)
		},
	}
	t.Cleanup(func() { credResolver, credDeps = oldResolver, oldDeps })
	h := New(s, nil)
	repo := lifecycleRepo(t)
	credUpstream(t, repo)
	if status, result := additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(repo)+`,"name":"My App"}`); status != 200 {
		t.Fatalf("register: %d %#v", status, result)
	}
	loopback := "127.0.0.1:8765"
	noSecret := func(raw string) {
		t.Helper()
		for _, leak := range []string{"b3BlbnNzaC", credTestToken, "BEGIN OPENSSH"} {
			if strings.Contains(raw, leak) {
				t.Fatalf("a hub response carried a secret: %s", raw)
			}
		}
	}
	post := func(path, body string) (int, map[string]any, string) {
		t.Helper()
		status, result, raw := credRequest(t, h, "POST", "/api/v1/cred/"+path, body, loopback, true)
		noSecret(raw)
		return status, result, raw
	}

	// The state before anything: no reference, no token, three providers, keychain unavailable on linux.
	status, result, raw := credRequest(t, h, "GET", "/api/v1/cred", "", "example.com", false)
	noSecret(raw)
	if status != 200 || result["os"] != "linux" || result["can_edit"] != true {
		t.Fatalf("state: %d %#v", status, result)
	}
	report := result["report"].(map[string]any)
	if report["reference"] != "" || report["token"] != "none" || report["landing_push"] != "off" {
		t.Fatalf("report %#v", report)
	}
	providers := result["providers"].([]any)
	if len(providers) != 3 || providers[0].(map[string]any)["id"] != "op" || providers[1].(map[string]any)["available"] != false || providers[2].(map[string]any)["generate"] != true {
		t.Fatalf("providers %#v", providers)
	}

	// Writes need loopback and the header.
	if status, _, _ := credRequest(t, h, "POST", "/api/v1/cred/token", `{"token":"x"}`, "example.com", true); status != 403 {
		t.Fatalf("non-loopback write: %d", status)
	}
	if status, _, _ := credRequest(t, h, "POST", "/api/v1/cred/set", `{"reference":"file:///x"}`, loopback, false); status != 403 {
		t.Fatalf("write without the header: %d", status)
	}
	if status, _, _ := credRequest(t, h, "GET", "/api/v1/cred/op/vaults", "", loopback, true); status != 405 {
		t.Fatalf("GET on a write route: %d", status)
	}
	if status, _, _ := credRequest(t, h, "POST", "/api/v1/cred/nope", "{}", loopback, true); status != 404 {
		t.Fatalf("unknown sub-route: %d", status)
	}

	// Browsing 1Password before the token is connected fails with a pointer, not a crash.
	if status, result, _ := post("op/vaults", "{}"); status != 409 || !strings.Contains(result["error"].(string), "token") {
		t.Fatalf("vaults without a token: %d %#v", status, result)
	}
	// A pasted key is not a token.
	if status, _, _ := post("token", jsonBody(map[string]any{"token": credTestKey})); status != 400 {
		t.Fatalf("key as token: %d", status)
	}
	status, result, _ = post("token", jsonBody(map[string]any{"token": credTestToken}))
	if status != 200 || !strings.Contains(result["stored_in"].(string), "secrets/op-service-account-token") {
		t.Fatalf("token: %d %#v", status, result)
	}
	if got, _ := os.ReadFile(filepath.Join(s.Home, "secrets", "op-service-account-token")); strings.TrimSpace(string(got)) != credTestToken {
		t.Fatal("token file not written")
	}
	status, result, _ = post("op/vaults", "{}")
	if status != 200 || len(result["vaults"].([]any)) != 1 {
		t.Fatalf("vaults: %d %#v", status, result)
	}
	status, result, _ = post("op/items", `{"vault":"v1"}`)
	if status != 200 || len(result["items"].([]any)) != 1 {
		t.Fatalf("items: %d %#v", status, result)
	}
	item := result["items"].([]any)[0].(map[string]any)
	reference := item["reference"].(string)
	if reference != "op://PULSE/pulse push key/private key?ssh-format=openssh" {
		t.Fatalf("reference %q", reference)
	}
	if status, _, _ := post("op/items", `{}`); status != 400 {
		t.Fatalf("items without a vault: %d", status)
	}

	// The remote refuses: nothing saved, the public key comes back so the owner can add it.
	status, result, _ = post("set", jsonBody(map[string]any{"reference": reference}))
	if status != 409 || !strings.HasPrefix(result["public_key"].(string), "ssh-ed25519 ") || result["fingerprint"] != "SHA256:FakeFingerprint0123456789" {
		t.Fatalf("refused set: %d %#v", status, result)
	}
	if got := result["probes"].([]any); len(got) != 1 || got[0].(map[string]any)["ok"] != false || !strings.Contains(got[0].(map[string]any)["why"].(string), "read only") {
		t.Fatalf("refused probes %#v", got)
	}
	if s.Setting("push.credential", "") != "" || len(probes) != 1 {
		t.Fatalf("refused key saved or not probed: %q %v", s.Setting("push.credential", ""), probes)
	}
	// A pasted key is not a reference; a bad reference is named.
	if status, result, _ := post("set", jsonBody(map[string]any{"reference": credTestKey})); status != 400 || !strings.Contains(result["error"].(string), "not a reference") {
		t.Fatalf("pasted key: %d %#v", status, result)
	}
	if status, _, _ := post("set", `{"reference":"ftp://x"}`); status != 400 {
		t.Fatalf("bad scheme: %d", status)
	}
	// The remote answers: saved, event written, the report shows the fingerprint.
	probeErr = nil
	status, result, _ = post("set", jsonBody(map[string]any{"reference": reference}))
	if status != 200 {
		t.Fatalf("set: %d %#v", status, result)
	}
	res := result["result"].(map[string]any)
	if res["reference"] != reference || res["probed"] != true || len(res["probes"].([]any)) != 1 || res["probes"].([]any)[0].(map[string]any)["ok"] != true {
		t.Fatalf("result %#v", res)
	}
	report = result["report"].(map[string]any)
	if report["reference"] != reference || report["fingerprint"] != "SHA256:FakeFingerprint0123456789" || report["set_by"] != "human:dashboard" || report["scheme"] != "op" {
		t.Fatalf("report after set %#v", report)
	}
	if s.Setting("push.credential", "") != reference {
		t.Fatal("reference not saved")
	}
	var actor string
	if err := s.DB.QueryRow("SELECT actor FROM events WHERE action = 'cred.set' ORDER BY seq DESC LIMIT 1").Scan(&actor); err != nil || actor != "human:dashboard" {
		t.Fatalf("event actor %q %v", actor, err)
	}
	// No key directory is left under the home.
	if entries, _ := os.ReadDir(filepath.Join(s.Home, "tmp")); len(entries) != 0 {
		t.Fatalf("materialized key left behind: %v", entries)
	}
	// The settings API shows the reference (it is not a secret) as kind reference.
	status, settings := additionsRequest(t, h, "GET", "/api/v1/settings", "")
	if status != 200 {
		t.Fatalf("settings: %d", status)
	}
	found := false
	for _, row := range settings["settings"].([]any) {
		m := row.(map[string]any)
		if m["name"] == "push.credential" {
			found = m["value"] == reference && m["kind"] == "reference"
		}
	}
	if !found {
		t.Fatalf("push.credential row missing or wrong in %#v", settings)
	}

	// Generate a key of the daemon's own; the second call keeps it; replace mints anew.
	status, result, _ = post("generate", "{}")
	if status != 200 {
		t.Fatalf("generate: %d %#v", status, result)
	}
	gen := result["generated"].(map[string]any)
	if gen["existed"] != false || gen["reference"] != "file://"+filepath.Join(s.Home, "secrets", "push_key") || !strings.HasPrefix(gen["public_key"].(string), "ssh-ed25519 ") {
		t.Fatalf("generated %#v", gen)
	}
	if info, err := os.Stat(filepath.Join(s.Home, "secrets", "push_key")); err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("generated key mode: %v %v", info, err)
	}
	if status, result, _ = post("generate", "{}"); status != 200 || result["generated"].(map[string]any)["existed"] != true {
		t.Fatalf("second generate must keep the key: %d %#v", status, result)
	}
	if status, result, _ = post("generate", `{"replace":true}`); status != 200 || result["generated"].(map[string]any)["existed"] != false {
		t.Fatalf("replace: %d %#v", status, result)
	}
	if status, _, _ := post("generate", `{"replace":"yes"}`); status != 400 {
		t.Fatalf("replace must be a boolean: %d", status)
	}

	// Clear with the token: the reference is empty again, the token file gone.
	status, result, _ = post("clear", `{"token":true}`)
	if status != 200 {
		t.Fatalf("clear: %d %#v", status, result)
	}
	cleared := result["cleared"].(map[string]any)
	if cleared["was"] != reference || len(cleared["token_removed"].([]any)) != 1 {
		t.Fatalf("cleared %#v", cleared)
	}
	if s.Setting("push.credential", "") != "" {
		t.Fatal("reference not cleared")
	}
	if _, err := os.Stat(filepath.Join(s.Home, "secrets", "op-service-account-token")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("token file still there: %v", err)
	}
	report = result["report"].(map[string]any)
	if report["reference"] != "" || report["token"] != "none" {
		t.Fatalf("report after clear %#v", report)
	}
}

// credUpstream gives a test repo an origin (a bare repo beside it) and tracks main, so the set probes it.
func credUpstream(t *testing.T, repo string) {
	t.Helper()
	bare := filepath.Join(t.TempDir(), "origin.git")
	for _, args := range [][]string{
		{"init", "-q", "--bare", bare},
		{"-C", repo, "remote", "add", "origin", bare},
		{"-C", repo, "push", "-q", "-u", "origin", "main"},
	} {
		if out, err := exec.Command("git", args...).CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
}

func jsonBody(v map[string]any) string {
	b, _ := json.Marshal(v)
	return string(b)
}
