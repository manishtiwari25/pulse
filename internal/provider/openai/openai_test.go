package openai

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/manishtiwari25/pulse/internal/provider"
)

const fakeKey = "sk-test-secret-abcdefghijklmnop"

func TestModelsListDefaultPickAndCache(t *testing.T) {
	var calls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		if r.URL.Path != "/models" {
			t.Fatalf("path = %s", r.URL.Path)
		}
		if r.Header.Get("Authorization") != "Bearer "+fakeKey {
			t.Fatalf("auth = %q", r.Header.Get("Authorization"))
		}
		json.NewEncoder(w).Encode(map[string]any{"data": []any{
			map[string]any{"id": "z-local", "owned_by": "local"},
			map[string]any{"id": "gpt-4o-mini", "owned_by": "openai"},
			map[string]any{"id": "a-first"},
		}})
	}))
	defer srv.Close()

	p := newTestProvider(t, srv.URL+"/", map[string]string{"OPENAI_API_KEY": fakeKey}, nil)
	models, err := p.Models(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got := modelIDs(models); !reflect.DeepEqual(got, []string{"a-first", "gpt-4o-mini", "z-local"}) {
		t.Fatalf("models = %v", got)
	}
	for _, m := range models {
		if m.ID == "gpt-4o-mini" && (!m.Default || !m.Chat || !m.ToolCalls || m.ContextWindow != 0) {
			t.Fatalf("bad default model: %+v", m)
		}
		if m.ID == "a-first" && m.Vendor != "openai" {
			t.Fatalf("vendor fallback = %+v", m)
		}
	}
	if _, err := p.Models(context.Background()); err != nil {
		t.Fatal(err)
	}
	if calls.Load() != 1 {
		t.Fatalf("models calls = %d, want cached 1", calls.Load())
	}
}

func TestChatToolCallRoundTrip(t *testing.T) {
	var saw map[string]any
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/chat/completions" {
			t.Fatalf("path = %s", r.URL.Path)
		}
		if r.Header.Get("Authorization") != "Bearer "+fakeKey {
			t.Fatalf("auth = %q", r.Header.Get("Authorization"))
		}
		if err := json.NewDecoder(r.Body).Decode(&saw); err != nil {
			t.Fatal(err)
		}
		w.Header().Set("x-request-id", "req-header")
		json.NewEncoder(w).Encode(map[string]any{
			"id":    "req-body",
			"model": "gpt-4o-mini",
			"choices": []any{map[string]any{
				"finish_reason": "tool_calls",
				"message": map[string]any{
					"role":    "assistant",
					"content": nil,
					"tool_calls": []any{map[string]any{
						"id":   "call-1",
						"type": "function",
						"function": map[string]any{
							"name":      "lookup",
							"arguments": `{"id":1}`,
						},
					}},
				},
			}},
			"usage": map[string]any{"prompt_tokens": 5, "completion_tokens": 3},
		})
	}))
	defer srv.Close()

	p := newTestProvider(t, srv.URL, map[string]string{"OPENAI_API_KEY": fakeKey}, nil)
	temp := 0.3
	resp, err := p.Chat(context.Background(), provider.Request{
		Model: "gpt-4o-mini",
		Messages: []provider.Message{
			{Role: provider.RoleUser, Content: "use a tool"},
			{Role: provider.RoleAssistant, ToolCalls: []provider.ToolCall{{ID: "call-0", Name: "lookup", Arguments: `{"id":0}`}}},
			{Role: provider.RoleTool, ToolCallID: "call-0", Name: "lookup", Content: `{"ok":true}`},
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
	if saw["tool_choice"] != "auto" || saw["stream"] != false || saw["max_tokens"].(float64) != 42 {
		t.Fatalf("bad controls: %#v", saw)
	}
	tools := saw["tools"].([]any)
	if tools[0].(map[string]any)["type"] != "function" {
		t.Fatalf("bad tools: %#v", tools)
	}
	if resp.FinishReason != "tool_calls" || resp.RequestID != "req-header" || resp.Usage.PromptTokens != 5 || resp.Usage.CompletionTokens != 3 || resp.Usage.Cost != 0 {
		t.Fatalf("bad response: %+v", resp)
	}
	if got := resp.Message.ToolCalls; len(got) != 1 || got[0].Name != "lookup" || got[0].Arguments != `{"id":1}` {
		t.Fatalf("tool calls = %+v", got)
	}
}

func TestUnauthorizedErrorAndStatusDetail(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
		json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{"message": "no " + fakeKey}})
	}))
	defer srv.Close()

	home := t.TempDir()
	p := newTestProviderWithHome(t, home, srv.URL, nil, map[string]string{"provider.openai.base_url": srv.URL}).(*openai)
	if err := p.writeStored(storedLogin{APIKey: fakeKey, User: keyUser(fakeKey)}); err != nil {
		t.Fatal(err)
	}
	_, err := p.Chat(context.Background(), provider.Request{Model: "gpt-4o-mini", Messages: []provider.Message{{Role: provider.RoleUser, Content: "hi"}}})
	var perr *provider.Error
	if !errors.As(err, &perr) || perr.Code != "unauthorized" || perr.Retryable {
		t.Fatalf("unauthorized = %#v", err)
	}
	if strings.Contains(err.Error(), fakeKey) {
		t.Fatalf("secret leaked in error: %v", err)
	}
	login, err := p.Status(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if login.LoggedIn || !strings.Contains(login.Detail, "key was refused") {
		t.Fatalf("status = %+v", login)
	}
	if _, err := os.Stat(filepath.Join(home, "private", "providers", "openai.json")); err != nil && !errors.Is(err, os.ErrNotExist) {
		t.Fatal(err)
	}
}

func TestRetryAfter429ThenSuccess(t *testing.T) {
	var calls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) == 1 {
			w.Header().Set("Retry-After", "1")
			w.WriteHeader(http.StatusTooManyRequests)
			json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{"code": "rate_limit", "message": "slow down"}})
			return
		}
		json.NewEncoder(w).Encode(map[string]any{
			"model": "gpt-4o-mini",
			"choices": []any{map[string]any{
				"finish_reason": "stop",
				"message":       map[string]any{"role": "assistant", "content": "ok"},
			}},
		})
	}))
	defer srv.Close()

	p := newTestProvider(t, srv.URL, map[string]string{"OPENAI_API_KEY": fakeKey}, nil)
	resp, err := p.Chat(context.Background(), provider.Request{Model: "gpt-4o-mini", Messages: []provider.Message{{Role: provider.RoleUser, Content: "hi"}}})
	if err != nil {
		t.Fatal(err)
	}
	if resp.Message.Content != "ok" || calls.Load() != 2 {
		t.Fatalf("resp=%+v calls=%d", resp, calls.Load())
	}
}

func TestNetworkErrorRetryable(t *testing.T) {
	p := newTestProvider(t, "https://example.test/v1", map[string]string{"OPENAI_API_KEY": fakeKey}, nil).(*openai)
	p.http = &http.Client{Transport: roundTripFunc(func(*http.Request) (*http.Response, error) {
		return nil, errors.New("dial failed for " + fakeKey)
	})}
	_, err := p.Models(context.Background())
	var perr *provider.Error
	if !errors.As(err, &perr) || !perr.Retryable || perr.Code != "network" {
		t.Fatalf("network error = %#v", err)
	}
	if strings.Contains(err.Error(), fakeKey) {
		t.Fatalf("secret leaked in network error: %v", err)
	}
}

func TestTokenLoginStores0600RefusesBadKeyAndExportsStoredOnly(t *testing.T) {
	var allow atomic.Bool
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/models" {
			t.Fatalf("path = %s", r.URL.Path)
		}
		if !allow.Load() {
			w.WriteHeader(http.StatusForbidden)
			json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{"message": "bad " + fakeKey}})
			return
		}
		json.NewEncoder(w).Encode(map[string]any{"data": []any{map[string]any{"id": "gpt-4o"}}})
	}))
	defer srv.Close()

	home := t.TempDir()
	envProvider := newTestProvider(t, srv.URL, map[string]string{"OPENAI_API_KEY": fakeKey}, nil).(*openai)
	if _, _, err := envProvider.ExportLogin(); err == nil {
		t.Fatal("environment key exported")
	}
	p := newTestProviderWithHome(t, home, srv.URL, nil, nil).(*openai)
	if _, err := p.LoginWithToken(context.Background(), fakeKey); err == nil {
		t.Fatal("bad key was stored")
	} else if strings.Contains(err.Error(), fakeKey) {
		t.Fatalf("secret leaked in bad key error: %v", err)
	}
	if _, err := os.Stat(p.loginPath()); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("bad key store stat = %v", err)
	}
	allow.Store(true)
	login, err := p.LoginWithToken(context.Background(), fakeKey)
	if err != nil {
		t.Fatal(err)
	}
	if login.User != keyUser(fakeKey) || login.Source != "token login" {
		t.Fatalf("login = %+v", login)
	}
	info, err := os.Stat(p.loginPath())
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("file mode = %o", info.Mode().Perm())
	}
	dir, err := os.Stat(filepath.Dir(p.loginPath()))
	if err != nil {
		t.Fatal(err)
	}
	if dir.Mode().Perm() != 0o700 {
		t.Fatalf("dir mode = %o", dir.Mode().Perm())
	}
	tok, exported, err := p.ExportLogin()
	if err != nil {
		t.Fatal(err)
	}
	if tok != fakeKey || exported.User != keyUser(fakeKey) || exported.Source != "stored" {
		t.Fatalf("export = %q %+v", tok, exported)
	}
}

func TestBaseURLPrecedenceAndTrim(t *testing.T) {
	p := newTestProvider(t, "", map[string]string{"OPENAI_BASE_URL": "https://env.test/v1///"}, nil).(*openai)
	if p.base != "https://env.test/v1" {
		t.Fatalf("env base = %q", p.base)
	}
	p = newTestProvider(t, "", map[string]string{"OPENAI_BASE_URL": "https://env.test/v1"}, map[string]string{"provider.openai.base_url": "https://settings.test/v1/"}).(*openai)
	if p.base != "https://settings.test/v1" {
		t.Fatalf("settings base = %q", p.base)
	}
	p = newTestProvider(t, "", nil, nil).(*openai)
	if p.base != defaultBaseURL {
		t.Fatalf("default base = %q", p.base)
	}
}

func TestFakeKeyNeverLeaksInProviderErrors(t *testing.T) {
	rememberSecret(fakeKey)
	err := providerError(http.StatusTooManyRequests, "rate_"+fakeKey, "message "+fakeKey, true)
	if strings.Contains(err.Error(), fakeKey) || strings.Contains(err.Code, fakeKey) || strings.Contains(err.Message, fakeKey) {
		t.Fatalf("secret leaked: %#v", err)
	}
}

func newTestProvider(t *testing.T, base string, env map[string]string, settings map[string]string) provider.Provider {
	t.Helper()
	return newTestProviderWithHome(t, t.TempDir(), base, env, settings)
}

func newTestProviderWithHome(t *testing.T, home, base string, env map[string]string, settings map[string]string) provider.Provider {
	t.Helper()
	if settings == nil {
		settings = map[string]string{}
	}
	if base != "" {
		settings["provider.openai.base_url"] = base
	}
	p, err := New(provider.Options{
		Home:     home,
		Settings: settings,
		Env: func(k string) string {
			if env == nil {
				return ""
			}
			return env[k]
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func modelIDs(models []provider.Model) []string {
	out := make([]string, len(models))
	for i, m := range models {
		out[i] = m.ID
	}
	return out
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) {
	return f(r)
}
