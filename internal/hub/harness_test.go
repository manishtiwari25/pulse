package hub

import (
	"context"
	"errors"
	"net/http"
	"strings"
	"sync"
	"testing"

	"github.com/manishtiwari25/pulse/internal/provider"
	_ "github.com/manishtiwari25/pulse/internal/provider/openai"
	"github.com/manishtiwari25/pulse/internal/store"
)

type hubFakeProvider struct {
	home  string
	state *hubFakeState
}

type hubFakeState struct {
	mu             sync.Mutex
	login          provider.Login
	modelsErr      error
	polls          int
	tokenLoginSeen bool
}

var (
	hubFakeOnce   sync.Once
	hubFakeStates sync.Map
	hubCacheOnce  sync.Once
	hubCacheSeen  sync.Map
)

func registerHubFakeProvider() {
	hubFakeOnce.Do(func() {
		provider.Register("hubfake", "Hub Fake", func(o provider.Options) (provider.Provider, error) {
			stateAny, _ := hubFakeStates.LoadOrStore(o.Home, &hubFakeState{login: provider.Login{
				LoggedIn: true, User: "octocat", Source: "device login", Plan: "individual", ExpiresAt: "2026-09-07T12:00:00Z",
			}})
			return &hubFakeProvider{home: o.Home, state: stateAny.(*hubFakeState)}, nil
		})
	})
}

func registerHubCacheProvider() {
	hubCacheOnce.Do(func() {
		provider.Register("hubcachefake", "Hub Cache Fake", func(o provider.Options) (provider.Provider, error) {
			p := &hubFakeProvider{home: o.Home, state: &hubFakeState{}}
			hubCacheSeen.Store(o.Settings["provider.openai.base_url"], p)
			return p, nil
		})
	})
}

func harnessStore(t *testing.T) *store.Store {
	t.Helper()
	registerHubFakeProvider()
	s := additionsStore(t, false)
	if _, err := s.DB.Exec(`INSERT INTO settings(name,value,updated_at,updated_by) VALUES('provider','hubfake',?,'test')`, s.Now()); err != nil {
		t.Fatal(err)
	}
	return s
}

func fakeState(t *testing.T, s *store.Store) *hubFakeState {
	t.Helper()
	stateAny, ok := hubFakeStates.Load(s.Home)
	if !ok {
		t.Fatal("fake provider was not opened")
	}
	return stateAny.(*hubFakeState)
}

func (p *hubFakeProvider) Name() string  { return "hubfake" }
func (p *hubFakeProvider) Label() string { return "Hub Fake" }

func (p *hubFakeProvider) Chat(context.Context, provider.Request) (provider.Response, error) {
	return provider.Response{}, nil
}

func (p *hubFakeProvider) Models(context.Context) ([]provider.Model, error) {
	p.state.mu.Lock()
	defer p.state.mu.Unlock()
	if p.state.modelsErr != nil {
		return nil, p.state.modelsErr
	}
	return []provider.Model{{ID: "hub-worker", Name: "Hub Worker", Vendor: "fake", Chat: true, ToolCalls: true}}, nil
}

func (p *hubFakeProvider) Status(context.Context) (provider.Login, error) {
	p.state.mu.Lock()
	defer p.state.mu.Unlock()
	return p.state.login, nil
}

func (p *hubFakeProvider) BeginDeviceLogin(context.Context) (provider.DeviceFlow, error) {
	return provider.DeviceFlow{DeviceCode: "device-1", UserCode: "USER-1", VerificationURI: "https://example.test/device", Interval: 5, ExpiresIn: 600}, nil
}

func (p *hubFakeProvider) PollDeviceLogin(_ context.Context, deviceCode string) (string, provider.Login, error) {
	if deviceCode != "device-1" {
		return provider.DeviceDenied, provider.Login{}, nil
	}
	p.state.mu.Lock()
	defer p.state.mu.Unlock()
	p.state.polls++
	if p.state.polls == 1 {
		return provider.DevicePending, provider.Login{}, nil
	}
	p.state.login = provider.Login{LoggedIn: true, User: "octocat", Source: "device login", Plan: "individual"}
	return provider.DeviceComplete, p.state.login, nil
}

func (p *hubFakeProvider) LoginWithToken(_ context.Context, token string) (provider.Login, error) {
	if token == "bad-token" {
		return provider.Login{}, errors.New("hubfake: invalid token")
	}
	p.state.mu.Lock()
	defer p.state.mu.Unlock()
	p.state.tokenLoginSeen = true
	p.state.login = provider.Login{LoggedIn: true, User: "token-user", Source: "token login", Plan: "individual"}
	return p.state.login, nil
}

func (p *hubFakeProvider) Logout() error {
	p.state.mu.Lock()
	defer p.state.mu.Unlock()
	p.state.login = provider.Login{LoggedIn: false}
	return nil
}

func TestAdditionHarnessStatusAndSettings(t *testing.T) {
	s := harnessStore(t)
	h := New(s, nil)
	if err := s.BeginHarnessSession(store.HarnessSession{ID: "HS-running", JobID: "JOB-1", Agent: "alice", Project: "demo"}); err != nil {
		t.Fatal(err)
	}

	if err := s.BeginHarnessSession(store.HarnessSession{ID: "HS-failed", JobID: "JOB-2", Agent: "bob", Project: "demo"}); err != nil {
		t.Fatal(err)
	}
	if err := s.FinishHarnessSession("HS-failed", "failed", "error", "failed submission", "boom", 0, ""); err != nil {
		t.Fatal(err)
	}
	status, result := additionsRequest(t, h, http.MethodGet, "/api/v1/harness", "")
	if status != http.StatusOK {
		t.Fatalf("GET /harness: %d %#v", status, result)
	}
	if result["harness"] != "native" {
		t.Fatalf("harness mode: %#v", result)
	}
	providerRow := result["provider"].(map[string]any)
	if providerRow["name"] != "hubfake" || providerRow["label"] != "Hub Fake" || providerRow["device_login"] != true || providerRow["token_login"] != true {
		t.Fatalf("provider row: %#v", providerRow)
	}
	login := providerRow["login"].(map[string]any)
	if login["logged_in"] != true || login["user"] != "octocat" {
		t.Fatalf("login row: %#v", login)
	}
	limits := result["limits"].(map[string]any)
	if limits["max_steps"] != float64(60) || limits["step_timeout_seconds"] != float64(300) || limits["output_limit"] != float64(20000) || limits["minutes_per_job"] != float64(10) {
		t.Fatalf("limits: %#v", limits)
	}
	sessions := result["sessions"].(map[string]any)
	if sessions["running"] != float64(1) || sessions["today"] != float64(2) || sessions["failed_today"] != float64(1) {
		t.Fatalf("session counts: %#v", sessions)
	}
	settings := result["settings"].([]any)
	foundMaxSteps := false
	for _, raw := range settings {
		row := raw.(map[string]any)
		if row["name"] == "harness.max_steps" && row["value"] == "60" && row["kind"] == "count" && row["description"] != "" {
			foundMaxSteps = true
		}
	}
	if !foundMaxSteps {
		t.Fatalf("harness.max_steps missing from /harness settings: %#v", settings)
	}

	status, result = additionsRequest(t, h, http.MethodGet, "/api/v1/settings", "")
	if status != http.StatusOK {
		t.Fatalf("GET /settings: %d %#v", status, result)
	}
	foundMaxSteps = false
	for _, raw := range result["settings"].([]any) {
		row := raw.(map[string]any)
		if row["name"] == "harness.max_steps" {
			foundMaxSteps = true
		}
	}
	if !foundMaxSteps {
		t.Fatalf("harness.max_steps missing from /settings: %#v", result["settings"])
	}
}

func TestHarnessProviderSummariesIncludeOpenAITokenLogin(t *testing.T) {
	s := harnessStore(t)
	h := New(s, nil)
	providers := h.harnessProviderSummaries()
	for _, row := range providers {
		if row["name"] == "openai" {
			if row["label"] != "OpenAI-compatible" || row["device_login"] != false || row["token_login"] != true {
				t.Fatalf("openai row: %#v", row)
			}
			return
		}
	}
	t.Fatalf("openai provider missing: %#v", providers)
}

func TestHarnessProviderCacheUsesProviderSettingsFingerprint(t *testing.T) {
	registerHubCacheProvider()
	s := harnessStore(t)
	h := New(s, nil)
	if err := s.SetSetting("human", "provider.openai.base_url", "https://one.test/v1"); err != nil {
		t.Fatal(err)
	}
	first, err := h.harnessProvider("hubcachefake")
	if err != nil {
		t.Fatal(err)
	}
	again, err := h.harnessProvider("hubcachefake")
	if err != nil {
		t.Fatal(err)
	}
	if first != again {
		t.Fatal("same settings did not reuse cached provider")
	}
	if err := s.SetSetting("human", "provider.openai.base_url", "https://two.test/v1"); err != nil {
		t.Fatal(err)
	}
	second, err := h.harnessProvider("hubcachefake")
	if err != nil {
		t.Fatal(err)
	}
	if second == first {
		t.Fatal("changed provider settings reused stale provider")
	}
	h.invalidateHarnessProvider("hubcachefake")
	third, err := h.harnessProvider("hubcachefake")
	if err != nil {
		t.Fatal(err)
	}
	if third == second {
		t.Fatal("logout invalidation did not remove settings-keyed provider")
	}
	if _, ok := hubCacheSeen.Load("https://two.test/v1"); !ok {
		t.Fatal("provider did not receive store provider settings")
	}
}

func TestAdditionHarnessModels(t *testing.T) {
	s := harnessStore(t)
	h := New(s, nil)
	status, result := additionsRequest(t, h, http.MethodGet, "/api/v1/harness/models", "")
	if status != http.StatusOK || result["fetched_at"] == "" || len(result["models"].([]any)) != 1 {
		t.Fatalf("models: %d %#v", status, result)
	}
	state := fakeState(t, s)
	state.mu.Lock()
	state.modelsErr = provider.ErrNotLoggedIn
	state.mu.Unlock()
	status, result = additionsRequest(t, h, http.MethodGet, "/api/v1/harness/models", "")
	if status != http.StatusServiceUnavailable || result["error"] != "not signed in" || result["fix"] != "Sign in with GitHub." {
		t.Fatalf("not logged in models: %d %#v", status, result)
	}
}

func TestAdditionHarnessLoginFlowTokenAndLogout(t *testing.T) {
	s := harnessStore(t)
	h := New(s, nil)
	status, result := additionsRequest(t, h, http.MethodPost, "/api/v1/harness/login", `{"provider":"hubfake"}`)
	if status != http.StatusOK || result["device_code"] != "device-1" || result["user_code"] != "USER-1" {
		t.Fatalf("begin login: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, http.MethodPost, "/api/v1/harness/login/poll", `{}`)
	if status != http.StatusBadRequest || result["error"] != "missing device_code" {
		t.Fatalf("missing device_code: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, http.MethodPost, "/api/v1/harness/login/poll", `{"device_code":"device-1"}`)
	if status != http.StatusOK || result["status"] != provider.DevicePending || result["login"].(map[string]any)["logged_in"] != false {
		t.Fatalf("pending poll: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, http.MethodPost, "/api/v1/harness/login/poll", `{"device_code":"device-1"}`)
	if status != http.StatusOK || result["status"] != provider.DeviceComplete || result["login"].(map[string]any)["logged_in"] != true {
		t.Fatalf("complete poll: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, http.MethodPost, "/api/v1/harness/login/token", `{"token":"bad-token"}`)
	if status != http.StatusBadRequest || strings.Contains(strings.ToLower(result["why"].(string)), "bad-token") {
		t.Fatalf("invalid token: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, http.MethodPost, "/api/v1/harness/login/token", `{"token":"good-token"}`)
	if status != http.StatusOK || result["login"].(map[string]any)["source"] != "token login" {
		t.Fatalf("token login: %d %#v", status, result)
	}
	state := fakeState(t, s)
	state.mu.Lock()
	tokenSeen := state.tokenLoginSeen
	state.mu.Unlock()
	if !tokenSeen {
		t.Fatal("fake provider did not receive token login")
	}
	status, result = additionsRequest(t, h, http.MethodPost, "/api/v1/harness/logout", `{}`)
	if status != http.StatusOK || result["ok"] != true || result["login"].(map[string]any)["logged_in"] != false {
		t.Fatalf("logout: %d %#v", status, result)
	}
}

func TestAdditionHarnessSessions(t *testing.T) {
	s := harnessStore(t)
	h := New(s, nil)
	if _, err := s.DB.Exec(`INSERT INTO jobs(id,project,kind,title,created_at) VALUES('JOB-1','demo','task','Fix hub API',?)`, s.Now()); err != nil {
		t.Fatal(err)
	}
	longSubmission := strings.Repeat("x", 2100)
	if err := s.BeginHarnessSession(store.HarnessSession{ID: "HS-1", RunID: "RUN-1", JobID: "JOB-1", Agent: "alice", Project: "demo", Provider: "hubfake", Model: "hub-worker", Submission: longSubmission}); err != nil {
		t.Fatal(err)
	}
	if err := s.AppendHarnessStep(store.HarnessStep{SessionID: "HS-1", N: 1, Kind: "assistant", Content: "hello", Model: "hub-worker", PromptTokens: 3, CompletionTokens: 5}); err != nil {
		t.Fatal(err)
	}
	if err := s.FinishHarnessSession("HS-1", "failed", "error", longSubmission, "boom", 1.25, "trajectory"); err != nil {
		t.Fatal(err)
	}
	if err := s.BeginHarnessSession(store.HarnessSession{ID: "HS-2", JobID: "JOB-2", Agent: "bob", Project: "other"}); err != nil {
		t.Fatal(err)
	}
	status, result := additionsRequest(t, h, http.MethodGet, "/api/v1/harness/sessions?project=demo&agent=alice&status=failed&job=JOB-1&limit=5", "")
	if status != http.StatusOK {
		t.Fatalf("sessions list: %d %#v", status, result)
	}
	rows := result["sessions"].([]any)
	if len(rows) != 1 {
		t.Fatalf("session rows: %#v", rows)
	}
	row := rows[0].(map[string]any)
	if row["id"] != "HS-1" || row["job_title"] != "Fix hub API" || len(row["submission"].(string)) != 2000 {
		t.Fatalf("session row: %#v", row)
	}
	status, result = additionsRequest(t, h, http.MethodGet, "/api/v1/harness/sessions/HS-1", "")
	if status != http.StatusOK {
		t.Fatalf("session detail: %d %#v", status, result)
	}
	detail := result["session"].(map[string]any)
	if detail["job_title"] != "Fix hub API" || len(detail["submission"].(string)) != 2100 {
		t.Fatalf("session detail: %#v", detail)
	}
	steps := result["steps"].([]any)
	if len(steps) != 1 || steps[0].(map[string]any)["kind"] != "assistant" {
		t.Fatalf("steps: %#v", steps)
	}
	status, result = additionsRequest(t, h, http.MethodGet, "/api/v1/harness/sessions/missing", "")
	if status != http.StatusNotFound || result["error"] != "not found" {
		t.Fatalf("missing session: %d %#v", status, result)
	}
}

func TestSessionAfter(t *testing.T) {
	s := harnessStore(t)
	h := New(s, nil)
	if err := s.BeginHarnessSession(store.HarnessSession{ID: "HS-after", JobID: "JOB-after", Agent: "alice", Project: "demo"}); err != nil {
		t.Fatal(err)
	}
	for i, kind := range []string{"assistant", "command", "observation"} {
		if err := s.AppendHarnessStep(store.HarnessStep{SessionID: "HS-after", N: i + 1, Kind: kind, Content: kind}); err != nil {
			t.Fatal(err)
		}
	}
	status, result := additionsRequest(t, h, http.MethodGet, "/api/v1/harness/sessions/HS-after?after=1", "")
	if status != http.StatusOK {
		t.Fatalf("session after: %d %#v", status, result)
	}
	if result["after"] != float64(1) {
		t.Fatalf("after echo: %#v", result)
	}
	steps := result["steps"].([]any)
	if len(steps) != 2 || steps[0].(map[string]any)["n"] != float64(2) || steps[1].(map[string]any)["n"] != float64(3) {
		t.Fatalf("steps after 1: %#v", steps)
	}
	status, result = additionsRequest(t, h, http.MethodGet, "/api/v1/harness/sessions/HS-after?after=x", "")
	if status != http.StatusBadRequest || result["error"] != "Invalid after" {
		t.Fatalf("invalid after: %d %#v", status, result)
	}
}

func TestAdditionHarnessMethods(t *testing.T) {
	h := New(harnessStore(t), nil)
	for _, path := range []string{
		"/api/v1/harness",
		"/api/v1/harness/models",
		"/api/v1/harness/sessions",
		"/api/v1/harness/sessions/HS-1",
	} {
		status, _ := additionsRequest(t, h, http.MethodPost, path, `{}`)
		if status != http.StatusMethodNotAllowed {
			t.Fatalf("POST %s returned %d, want 405", path, status)
		}
	}
	for _, path := range []string{
		"/api/v1/harness/login",
		"/api/v1/harness/login/poll",
		"/api/v1/harness/login/token",
		"/api/v1/harness/logout",
	} {
		status, _ := additionsRequest(t, h, http.MethodGet, path, "")
		if status != http.StatusMethodNotAllowed {
			t.Fatalf("GET %s returned %d, want 405", path, status)
		}
	}
}
