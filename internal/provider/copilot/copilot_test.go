package copilot

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
)

func TestDeviceFlowBeginPendingSlowDownComplete(t *testing.T) {
	var poll atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/login/device/code":
			if r.Header.Get("Accept") != "application/json" {
				t.Fatalf("missing accept header")
			}
			json.NewEncoder(w).Encode(map[string]any{
				"device_code":      "dev-1",
				"user_code":        "ABCD-EFGH",
				"verification_uri": "https://example.test/device",
				"interval":         2,
				"expires_in":       900,
			})
		case "/login/oauth/access_token":
			switch poll.Add(1) {
			case 1:
				json.NewEncoder(w).Encode(map[string]any{"error": "authorization_pending"})
			case 2:
				json.NewEncoder(w).Encode(map[string]any{"error": "slow_down"})
			default:
				json.NewEncoder(w).Encode(map[string]any{"access_token": "gho_abcdefghijklmnopqrstuvwxyz"})
			}
		case "/user":
			if r.Header.Get("Authorization") != "token gho_abcdefghijklmnopqrstuvwxyz" {
				t.Fatalf("unexpected user auth header %q", r.Header.Get("Authorization"))
			}
			json.NewEncoder(w).Encode(map[string]any{"login": "octocat"})
		case "/copilot_internal/v2/token":
			json.NewEncoder(w).Encode(map[string]any{
				"token":      "tid=device;exp=9999999999;",
				"expires_at": time.Now().Add(time.Hour).Unix(),
				"endpoints":  map[string]string{"api": srvURL(r)},
				"sku":        "pro",
			})
		default:
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
	}))
	defer srv.Close()
	cleanup := resetGlobals(t, srv.URL, srv.URL, srv.URL)
	defer cleanup()

	home := t.TempDir()
	p := newTestProvider(t, home, nil).(*copilot)
	flow, err := p.BeginDeviceLogin(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if flow.DeviceCode != "dev-1" || flow.UserCode != "ABCD-EFGH" || flow.Interval != 2 || flow.ExpiresIn != 900 {
		t.Fatalf("bad flow: %+v", flow)
	}
	state, _, err := p.PollDeviceLogin(context.Background(), flow.DeviceCode)
	if err != nil || state != provider.DevicePending {
		t.Fatalf("pending = %q, %v", state, err)
	}
	state, _, err = p.PollDeviceLogin(context.Background(), flow.DeviceCode)
	if err != nil || state != provider.DeviceSlowDown {
		t.Fatalf("slow_down = %q, %v", state, err)
	}
	state, login, err := p.PollDeviceLogin(context.Background(), flow.DeviceCode)
	if err != nil || state != provider.DeviceComplete {
		t.Fatalf("complete = %q, %+v, %v", state, login, err)
	}
	if login.User != "octocat" || login.Source != "device login" || !login.LoggedIn {
		t.Fatalf("bad login: %+v", login)
	}
	info, err := os.Stat(filepath.Join(home, "private", "providers", "copilot.json"))
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("private file mode = %o", info.Mode().Perm())
	}
	dir, err := os.Stat(filepath.Join(home, "private", "providers"))
	if err != nil {
		t.Fatal(err)
	}
	if dir.Mode().Perm() != 0o700 {
		t.Fatalf("private dir mode = %o", dir.Mode().Perm())
	}
}

func TestTokenExchangeCacheAndChatRefreshOn401(t *testing.T) {
	var exchanges atomic.Int32
	var chats atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/copilot_internal/v2/token":
			n := exchanges.Add(1)
			json.NewEncoder(w).Encode(map[string]any{
				"token":      map[bool]string{true: "tid=one;exp=999;", false: "tid=two;exp=999;"}[n == 1],
				"expires_at": time.Now().Add(time.Hour).Unix(),
				"endpoints":  map[string]string{"api": srvURL(r)},
			})
		case "/chat/completions":
			n := chats.Add(1)
			if n == 1 {
				if r.Header.Get("Authorization") != "Bearer tid=one;exp=999;" {
					t.Fatalf("first auth = %q", r.Header.Get("Authorization"))
				}
				w.WriteHeader(http.StatusUnauthorized)
				json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{"code": "bad_token", "message": "no"}})
				return
			}
			if r.Header.Get("Authorization") != "Bearer tid=two;exp=999;" {
				t.Fatalf("second auth = %q", r.Header.Get("Authorization"))
			}
			json.NewEncoder(w).Encode(chatContent("chatcmpl-1", "gpt-4.1", "ok"))
		default:
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
	}))
	defer srv.Close()
	cleanup := resetGlobals(t, srv.URL, srv.URL, srv.URL)
	defer cleanup()

	p := newTestProvider(t, t.TempDir(), map[string]string{"COPILOT_GITHUB_TOKEN": "gho_abcdefghijklmnopqrstuvwxyz"}).(*copilot)
	resp, err := p.Chat(context.Background(), provider.Request{
		Model:    "gpt-4.1",
		Messages: []provider.Message{{Role: provider.RoleUser, Content: "hi"}},
	})
	if err != nil {
		t.Fatal(err)
	}
	if resp.Message.Content != "ok" || exchanges.Load() != 2 || chats.Load() != 2 {
		t.Fatalf("resp=%+v exchanges=%d chats=%d", resp, exchanges.Load(), chats.Load())
	}
	if _, _, err := p.sessionToken(context.Background(), "gho_abcdefghijklmnopqrstuvwxyz", false); err != nil {
		t.Fatal(err)
	}
	if exchanges.Load() != 2 {
		t.Fatalf("cached exchange count = %d", exchanges.Load())
	}
}

func TestChatRequestMappingResponseToolCallsAndInitiator(t *testing.T) {
	var sawBody map[string]any
	var sawInitiator string
	srv := copilotTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		sawInitiator = r.Header.Get("X-Initiator")
		if err := json.NewDecoder(r.Body).Decode(&sawBody); err != nil {
			t.Fatal(err)
		}
		json.NewEncoder(w).Encode(map[string]any{
			"id":    "req-1",
			"model": "gpt-4.1",
			"choices": []any{map[string]any{
				"finish_reason": "tool_calls",
				"message": map[string]any{
					"role":    "assistant",
					"content": nil,
					"tool_calls": []any{map[string]any{
						"id":   "call-2",
						"type": "function",
						"function": map[string]any{
							"name":      "lookup",
							"arguments": `{"id":2}`,
						},
					}},
				},
			}},
			"usage": map[string]any{"prompt_tokens": 11, "completion_tokens": 7},
		})
	})
	defer srv.Close()

	p := newTestProvider(t, t.TempDir(), map[string]string{"COPILOT_GITHUB_TOKEN": "gho_abcdefghijklmnopqrstuvwxyz"})
	temp := 0.2
	resp, err := p.Chat(context.Background(), provider.Request{
		Model: "gpt-4.1",
		Messages: []provider.Message{
			{Role: provider.RoleUser, Content: "use a tool"},
			{Role: provider.RoleAssistant, ToolCalls: []provider.ToolCall{{ID: "call-1", Name: "lookup", Arguments: `{"id":1}`}}},
			{Role: provider.RoleTool, ToolCallID: "call-1", Name: "lookup", Content: `{"ok":true}`},
		},
		Tools: []provider.Tool{{
			Name:        "lookup",
			Description: "Looks up an item.",
			Parameters:  json.RawMessage(`{"type":"object"}`),
		}},
		MaxTokens:   42,
		Temperature: &temp,
	})
	if err != nil {
		t.Fatal(err)
	}
	if sawInitiator != "agent" {
		t.Fatalf("initiator = %q", sawInitiator)
	}
	if sawBody["tool_choice"] != "auto" || sawBody["stream"] != false || sawBody["max_tokens"].(float64) != 42 {
		t.Fatalf("bad body controls: %#v", sawBody)
	}
	tools := sawBody["tools"].([]any)
	if tools[0].(map[string]any)["type"] != "function" {
		t.Fatalf("bad tools: %#v", tools)
	}
	messages := sawBody["messages"].([]any)
	assistant := messages[1].(map[string]any)
	call := assistant["tool_calls"].([]any)[0].(map[string]any)
	fn := call["function"].(map[string]any)
	if call["id"] != "call-1" || fn["name"] != "lookup" || fn["arguments"] != `{"id":1}` {
		t.Fatalf("bad tool call mapping: %#v", call)
	}
	tool := messages[2].(map[string]any)
	if tool["tool_call_id"] != "call-1" || tool["name"] != "lookup" {
		t.Fatalf("bad tool message: %#v", tool)
	}
	if resp.FinishReason != "tool_calls" || resp.RequestID != "req-1" || resp.Model != "gpt-4.1" {
		t.Fatalf("bad response: %+v", resp)
	}
	if got := resp.Message.ToolCalls; len(got) != 1 || got[0].Name != "lookup" || got[0].Arguments != `{"id":2}` {
		t.Fatalf("bad response tool calls: %+v", got)
	}
	if resp.Usage.PromptTokens != 11 || resp.Usage.CompletionTokens != 7 {
		t.Fatalf("bad usage: %+v", resp.Usage)
	}
}

func TestChatPlainContentAndUserInitiator(t *testing.T) {
	var sawInitiator string
	srv := copilotTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		sawInitiator = r.Header.Get("X-Initiator")
		json.NewEncoder(w).Encode(chatContent("req-2", "gpt-4.1", "plain"))
	})
	defer srv.Close()

	p := newTestProvider(t, t.TempDir(), map[string]string{"COPILOT_GITHUB_TOKEN": "gho_abcdefghijklmnopqrstuvwxyz"})
	resp, err := p.Chat(context.Background(), provider.Request{
		Model:    "gpt-4.1",
		Messages: []provider.Message{{Role: provider.RoleUser, Content: "hello"}},
	})
	if err != nil {
		t.Fatal(err)
	}
	if sawInitiator != "user" || resp.Message.Content != "plain" || resp.FinishReason != "stop" {
		t.Fatalf("initiator=%q resp=%+v", sawInitiator, resp)
	}
}

func TestChat429RetryAfterAnd400ModelError(t *testing.T) {
	var rateCalls atomic.Int32
	srv := copilotTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		if rateCalls.Add(1) == 1 {
			w.Header().Set("Retry-After", "0")
			w.WriteHeader(http.StatusTooManyRequests)
			json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{"type": "rate_limit", "message": "slow down"}})
			return
		}
		json.NewEncoder(w).Encode(chatContent("req-3", "gpt-4.1", "retried"))
	})
	defer srv.Close()

	p := newTestProvider(t, t.TempDir(), map[string]string{"COPILOT_GITHUB_TOKEN": "gho_abcdefghijklmnopqrstuvwxyz"})
	resp, err := p.Chat(context.Background(), provider.Request{Model: "gpt-4.1", Messages: []provider.Message{{Role: provider.RoleUser, Content: "hi"}}})
	if err != nil {
		t.Fatal(err)
	}
	if resp.Message.Content != "retried" || rateCalls.Load() != 2 {
		t.Fatalf("retry failed resp=%+v calls=%d", resp, rateCalls.Load())
	}

	var modelCalls atomic.Int32
	srv2 := copilotTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		modelCalls.Add(1)
		w.WriteHeader(http.StatusBadRequest)
		json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{"code": "bad_model", "message": "model no-such-model is not available"}})
	})
	defer srv2.Close()
	p2 := newTestProvider(t, t.TempDir(), map[string]string{"COPILOT_GITHUB_TOKEN": "gho_abcdefghijklmnopqrstuvwxyz"})
	_, err = p2.Chat(context.Background(), provider.Request{Model: "no-such-model", Messages: []provider.Message{{Role: provider.RoleUser, Content: "hi"}}})
	if err == nil {
		t.Fatal("expected model error")
	}
	var perr *provider.Error
	if !errors.As(err, &perr) || perr.Retryable || !strings.Contains(perr.Message, "no-such-model") || modelCalls.Load() != 1 {
		t.Fatalf("bad model error: %#v calls=%d", err, modelCalls.Load())
	}
}

func TestModelsFilteringMappingSortingAndCache(t *testing.T) {
	var calls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/copilot_internal/v2/token":
			json.NewEncoder(w).Encode(map[string]any{
				"token":      "tid=models;exp=999;",
				"expires_at": time.Now().Add(time.Hour).Unix(),
				"endpoints":  map[string]string{"api": srvURL(r)},
			})
		case "/models":
			calls.Add(1)
			disabled := false
			enabled := true
			json.NewEncoder(w).Encode(map[string]any{"data": []any{
				modelJSON("zeta", "Zeta", "Vendor", true, true, true, 2, 100, 10, nil),
				modelJSON("alpha", "Alpha", "Vendor", true, false, false, 0, 200, 20, &enabled),
				modelJSON("hidden", "Hidden", "Vendor", true, false, false, 0, 1, 1, &disabled),
				modelJSON("embed", "Embed", "Vendor", false, false, false, 0, 1, 1, nil),
			}})
		default:
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
	}))
	defer srv.Close()
	cleanup := resetGlobals(t, srv.URL, srv.URL, srv.URL)
	defer cleanup()

	p := newTestProvider(t, t.TempDir(), map[string]string{"COPILOT_GITHUB_TOKEN": "gho_abcdefghijklmnopqrstuvwxyz"})
	models, err := p.Models(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	want := []provider.Model{
		{ID: "alpha", Name: "Alpha", Vendor: "Vendor", Chat: true, ContextWindow: 200, MaxOutput: 20},
		{ID: "zeta", Name: "Zeta", Vendor: "Vendor", Chat: true, ToolCalls: true, Premium: 2, Preview: true, ContextWindow: 100, MaxOutput: 10, Default: true},
	}
	if !reflect.DeepEqual(models, want) {
		t.Fatalf("models\n got: %#v\nwant: %#v", models, want)
	}
	if _, err := p.Models(context.Background()); err != nil {
		t.Fatal(err)
	}
	if calls.Load() != 1 {
		t.Fatalf("models cache missed: %d", calls.Load())
	}
}

func TestRedactPatternsAndKnownSecrets(t *testing.T) {
	cleanup := resetGlobals(t, "http://example.test", "http://example.test", "http://example.test")
	defer cleanup()
	rememberSecret("0123456789abcdef0123456789abcdef01234567")
	in := "gho_abcdefghijklmnopqrstuvwxyz github_pat_ABCDEFGHIJKLMNOPQRSTUVWXYZ_0123456789 Bearer abcdefghijklmnop.qrst tid=abc123;exp=456; 0123456789abcdef0123456789abcdef01234567"
	out := redact(in)
	if strings.Contains(out, "gho_") || strings.Contains(out, "github_pat_") || strings.Contains(out, "Bearer abc") || strings.Contains(out, "tid=abc123") || strings.Contains(out, "0123456789abcdef") {
		t.Fatalf("not redacted: %s", out)
	}
	if strings.Count(out, "[redacted]") != 5 {
		t.Fatalf("redaction count: %s", out)
	}
}

func TestStatusPrecedenceFileEnvEditorNone(t *testing.T) {
	var userCalls, tokenCalls atomic.Int32
	// exchange decides what the Copilot token service answers per GitHub token.
	exchange := map[string]int{}
	var mu sync.Mutex
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		token := strings.TrimPrefix(r.Header.Get("Authorization"), "token ")
		switch r.URL.Path {
		case "/user":
			userCalls.Add(1)
			if strings.Contains(token, "stale") {
				w.WriteHeader(http.StatusUnauthorized)
				json.NewEncoder(w).Encode(map[string]any{"message": "Bad credentials", "status": "401"})
				return
			}
			json.NewEncoder(w).Encode(map[string]any{"login": "from-network"})
		case "/copilot_internal/v2/token":
			tokenCalls.Add(1)
			mu.Lock()
			status := exchange[token]
			mu.Unlock()
			switch status {
			case http.StatusNotFound:
				w.WriteHeader(status)
				json.NewEncoder(w).Encode(map[string]any{"message": "Not Found", "status": "404"})
			case http.StatusForbidden:
				w.WriteHeader(status)
				fmt.Fprint(w, "403 Forbidden. For more on scraping GitHub ...")
			default:
				json.NewEncoder(w).Encode(map[string]any{
					"token":      "tid=status;exp=999;",
					"expires_at": time.Now().Add(time.Hour).Unix(),
					"sku":        "copilot_for_business_seat",
				})
			}
		default:
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
	}))
	defer srv.Close()
	cleanup := resetGlobals(t, srv.URL, srv.URL, srv.URL)
	defer cleanup()

	// 1. The stored login wins and answers without the network.
	home := t.TempDir()
	p := newTestProvider(t, home, map[string]string{"COPILOT_GITHUB_TOKEN": "gho_envabcdefghijklmnopqrstuvwxyz"}).(*copilot)
	if err := p.writeStored(storedLogin{GitHubToken: "gho_fileabcdefghijklmnopqrstuvwxyz", User: "from-file", Source: "device login", SavedAt: time.Now().Format(time.RFC3339)}); err != nil {
		t.Fatal(err)
	}
	login, err := p.Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if login.User != "from-file" || login.Source != "device login" || userCalls.Load() != 0 || tokenCalls.Load() != 0 {
		t.Fatalf("file status = %+v user calls=%d token calls=%d", login, userCalls.Load(), tokenCalls.Load())
	}

	// 2. An environment token is checked for Copilot access once, then cached.
	envProvider := newTestProvider(t, t.TempDir(), map[string]string{"GH_TOKEN": "gho_envabcdefghijklmnopqrstuvwxyz"})
	login, err = envProvider.Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if !login.LoggedIn || login.User != "from-network" || login.Source != "environment" || login.Plan != "copilot_for_business_seat" || login.ExpiresAt == "" {
		t.Fatalf("env status = %+v", login)
	}
	if _, err := envProvider.Status(context.Background()); err != nil || tokenCalls.Load() != 1 {
		t.Fatalf("second status reused the session token: err=%v token calls=%d", err, tokenCalls.Load())
	}

	// 3. The editor plugins' apps.json is borrowed; github.com entries only.
	editorDir := writeEditorLogin(t, "apps.json", map[string]any{
		"ghe.example:Iv1.x":               map[string]any{"user": "elsewhere", "oauth_token": "gho_elsewhereabcdefghijklmnopqrstuv"},
		"github.com:Iv1.b507a08c87ecfe98": map[string]any{"user": "editor-user", "oauth_token": "gho_editorabcdefghijklmnopqrstuvwxyz"},
	})
	editorProvider := newTestProvider(t, t.TempDir(), map[string]string{"XDG_CONFIG_HOME": editorDir})
	login, err = editorProvider.Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if !login.LoggedIn || login.User != "from-network" || login.Source != "editor login" {
		t.Fatalf("editor status = %+v", login)
	}

	// 4. A stale editor token (GitHub says 401) is "not signed in", not a fault.
	staleDir := writeEditorLogin(t, "hosts.json", map[string]any{
		"github.com": map[string]any{"user": "old", "oauth_token": "gho_staleabcdefghijklmnopqrstuvwxyz"},
	})
	staleProvider := newTestProvider(t, t.TempDir(), map[string]string{"XDG_CONFIG_HOME": staleDir})
	login, err = staleProvider.Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if login.LoggedIn || login.Source != "editor login" || !strings.Contains(login.Detail, "stale") || !strings.Contains(login.Detail, "pulse harness login") {
		t.Fatalf("stale status = %+v", login)
	}

	// 5. A login without a Copilot seat: GitHub answers 404 on the exchange; the
	// refusal is remembered so console polling stays quiet, and Models says why.
	mu.Lock()
	exchange["gho_noseatabcdefghijklmnopqrstuvwxyz"] = http.StatusNotFound
	exchange["gho_ghclitokenabcdefghijklmnopqrstuv"] = http.StatusForbidden
	mu.Unlock()
	seatlessProvider := newTestProvider(t, t.TempDir(), map[string]string{"COPILOT_GITHUB_TOKEN": "gho_noseatabcdefghijklmnopqrstuvwxyz"})
	before := tokenCalls.Load()
	login, err = seatlessProvider.Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if login.LoggedIn || login.User != "from-network" || login.Source != "environment" || !strings.Contains(login.Detail, "no Copilot access") || !strings.Contains(login.Detail, "pulse harness login") {
		t.Fatalf("seatless status = %+v", login)
	}
	if _, err := seatlessProvider.Status(context.Background()); err != nil || tokenCalls.Load() != before+1 {
		t.Fatalf("refusal was not remembered: err=%v token calls=%d", err, tokenCalls.Load()-before)
	}
	_, err = seatlessProvider.Models(context.Background())
	if !errors.Is(err, provider.ErrNotLoggedIn) || !strings.Contains(err.Error(), "no Copilot access") {
		t.Fatalf("seatless models error = %v", err)
	}

	// 6. A token from an OAuth app that is not Copilot-enabled (gh's): 403.
	ghProvider := newTestProvider(t, t.TempDir(), map[string]string{"GITHUB_TOKEN": "gho_ghclitokenabcdefghijklmnopqrstuv"})
	login, err = ghProvider.Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if login.LoggedIn || !strings.Contains(login.Detail, "not Copilot-enabled") {
		t.Fatalf("gh token status = %+v", login)
	}

	// 7. Nothing anywhere: not signed in, with the fix.
	noneProvider := newTestProvider(t, t.TempDir(), nil)
	login, err = noneProvider.Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if login.LoggedIn || !strings.Contains(login.Detail, "pulse harness login") {
		t.Fatalf("none status = %+v", login)
	}
}

func TestLoginWithTokenRejectsBadShape(t *testing.T) {
	p := newTestProvider(t, t.TempDir(), nil)
	_, err := p.(provider.TokenLoginer).LoginWithToken(context.Background(), "not-a-token")
	if err == nil {
		t.Fatal("expected error")
	}
	var perr *provider.Error
	if !errors.As(err, &perr) || perr.Code != "invalid_token" {
		t.Fatalf("bad error: %#v", err)
	}
}

func newTestProvider(t *testing.T, home string, env map[string]string) provider.Provider {
	t.Helper()
	if env == nil {
		env = map[string]string{}
	}
	if _, ok := env["COPILOT_HOME"]; !ok {
		env["COPILOT_HOME"] = t.TempDir()
	}
	// Keep the developer's own editor login out of the tests.
	if _, ok := env["XDG_CONFIG_HOME"]; !ok {
		env["XDG_CONFIG_HOME"] = t.TempDir()
	}
	p, err := New(provider.Options{
		Home: home,
		HTTP: &http.Client{Timeout: 5 * time.Second},
		Env: func(k string) string {
			return env[k]
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	return p
}

// writeEditorLogin writes the apps.json the Copilot editor plugins keep, under
// a fresh XDG_CONFIG_HOME, and returns that directory.
func writeEditorLogin(t *testing.T, file string, hosts map[string]any) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "github-copilot"), 0o700); err != nil {
		t.Fatal(err)
	}
	data, err := json.Marshal(hosts)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "github-copilot", file), data, 0o600); err != nil {
		t.Fatal(err)
	}
	return dir
}

func resetGlobals(t *testing.T, github, oauth, api string) func() {
	t.Helper()
	oldGitHub, oldOAuth, oldAPI := githubBase, oauthBase, apiBase
	githubBase, oauthBase, apiBase = github, oauth, api
	secretsMu.Lock()
	knownSecrets = nil
	secretsMu.Unlock()
	return func() {
		githubBase, oauthBase, apiBase = oldGitHub, oldOAuth, oldAPI
		secretsMu.Lock()
		knownSecrets = nil
		secretsMu.Unlock()
	}
}

func copilotTestServer(t *testing.T, chat func(http.ResponseWriter, *http.Request)) *httptest.Server {
	t.Helper()
	var srv *httptest.Server
	srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/copilot_internal/v2/token":
			json.NewEncoder(w).Encode(map[string]any{
				"token":      "tid=test;exp=999;",
				"expires_at": time.Now().Add(time.Hour).Unix(),
				"endpoints":  map[string]string{"api": srv.URL},
			})
		case "/chat/completions":
			chat(w, r)
		default:
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
	}))
	cleanup := resetGlobals(t, srv.URL, srv.URL, srv.URL)
	t.Cleanup(cleanup)
	return srv
}

func chatContent(id, model, content string) map[string]any {
	return map[string]any{
		"id":    id,
		"model": model,
		"choices": []any{map[string]any{
			"finish_reason": "stop",
			"message": map[string]any{
				"role":    "assistant",
				"content": content,
			},
		}},
		"usage": map[string]any{"prompt_tokens": 3, "completion_tokens": 4},
	}
}

func modelJSON(id, name, vendor string, chat, tools, preview bool, premium float64, contextWindow, maxOutput int, enabled *bool) map[string]any {
	typ := "embedding"
	if chat {
		typ = "chat"
	}
	m := map[string]any{
		"id":                   id,
		"name":                 name,
		"vendor":               vendor,
		"model_picker_enabled": enabled,
		"preview":              preview,
		"is_chat_default":      id == "zeta",
		"capabilities":         map[string]any{"type": typ, "supports": map[string]any{"tool_calls": tools}, "limits": map[string]any{"max_context_window_tokens": contextWindow, "max_output_tokens": maxOutput}},
		"billing":              map[string]any{"multiplier": premium},
	}
	if enabled == nil {
		delete(m, "model_picker_enabled")
	}
	return m
}

func srvURL(r *http.Request) string {
	scheme := "http"
	if r.TLS != nil {
		scheme = "https"
	}
	return scheme + "://" + r.Host
}
