package harness

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/provider"
	_ "github.com/manishtiwari25/pulse/internal/provider/openai"
	"github.com/manishtiwari25/pulse/internal/store"
)

const fakeProviderName = "fake-harness-cli"

var (
	registerFakeProvider sync.Once
	fakeMu               sync.Mutex
	fakeCurrent          *commandFakeProvider
)

type commandFakeProvider struct {
	label    string
	models   []provider.Model
	login    provider.Login
	err      error
	requests []provider.Request
}

func (p *commandFakeProvider) Name() string { return fakeProviderName }
func (p *commandFakeProvider) Label() string {
	if p.label == "" {
		return "Fake Harness"
	}
	return p.label
}
func (p *commandFakeProvider) Chat(_ context.Context, req provider.Request) (provider.Response, error) {
	p.requests = append(p.requests, req)
	if p.err != nil {
		return provider.Response{}, p.err
	}
	return provider.Response{
		Message: provider.Message{Content: "SUMMARY: fake complete\nFINDING: ok"},
		Usage:   provider.Usage{PromptTokens: 7, CompletionTokens: 11, Cost: 0.25},
		Model:   "fake-model",
	}, nil
}
func (p *commandFakeProvider) Models(context.Context) ([]provider.Model, error) {
	if errors.Is(p.err, provider.ErrNotLoggedIn) {
		return nil, p.err
	}
	if p.err != nil {
		return nil, p.err
	}
	if p.models != nil {
		return p.models, nil
	}
	return []provider.Model{{ID: "fake-model", Name: "Fake Model", Vendor: "Fake", Chat: true, ToolCalls: true, Default: true, ContextWindow: 1000, MaxOutput: 200}}, nil
}
func (p *commandFakeProvider) Status(context.Context) (provider.Login, error) {
	return p.login, nil
}

func setFakeProvider(t *testing.T, p *commandFakeProvider) {
	t.Helper()
	registerFakeProvider.Do(func() {
		provider.Register(fakeProviderName, "Fake Harness", func(provider.Options) (provider.Provider, error) {
			fakeMu.Lock()
			defer fakeMu.Unlock()
			if fakeCurrent == nil {
				return &commandFakeProvider{}, nil
			}
			return fakeCurrent, nil
		})
	})
	fakeMu.Lock()
	fakeCurrent = p
	fakeMu.Unlock()
	t.Cleanup(func() {
		fakeMu.Lock()
		if fakeCurrent == p {
			fakeCurrent = nil
		}
		fakeMu.Unlock()
	})
}

func harnessCommandStore(t *testing.T) *store.Store {
	t.Helper()
	home := t.TempDir()
	for _, name := range []string{"PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT", "PULSE_EXECUTOR", "PULSE_NOW", "PULSE_HARNESS_CANCEL", "PULSE_HARNESS_PROMPT_FILE"} {
		t.Setenv(name, "")
	}
	t.Setenv("PULSE_HOME", home)
	s, err := store.Open(store.Options{Home: home, Create: true, Now: "2026-09-11T05:28:22Z"})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func invokeHarness(t *testing.T, home string, args ...string) (string, string, int) {
	t.Helper()
	cmd, ok := cli.Lookup("harness")
	if !ok {
		t.Fatal("harness command is not registered")
	}
	var out, errb bytes.Buffer
	c := &cli.Context{Home: home, Out: &out, Err: &errb, Agent: "tester", Session: "outer-session", Project: "demo"}
	err := cmd.Run(c, flag.NewFlagSet("harness", flag.ContinueOnError), args)
	code := 0
	if err != nil {
		var ce *cli.Error
		if errors.As(err, &ce) {
			code = ce.ExitCode()
			if ce.What != "" {
				errb.WriteString(ce.Error())
			}
		} else {
			code = 1
			errb.WriteString(err.Error())
		}
	}
	return out.String(), errb.String(), code
}

func TestHarnessOpenAIProviderLoginStatusModelsAndLogout(t *testing.T) {
	s := harnessCommandStore(t)
	const token = "sk-cli-smoke-abcdefghijklmnop"
	var paths []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.URL.Path)
		if r.Header.Get("Authorization") != "Bearer "+token {
			t.Fatalf("auth = %q", r.Header.Get("Authorization"))
		}
		if r.URL.Path != "/models" {
			t.Fatalf("path = %s", r.URL.Path)
		}
		json.NewEncoder(w).Encode(map[string]any{"data": []any{map[string]any{"id": "gpt-4o-mini", "owned_by": "openai"}}})
	}))
	defer srv.Close()
	if err := s.SetSetting("human", "provider.openai.base_url", srv.URL+"/"); err != nil {
		t.Fatal(err)
	}

	oldStdin := os.Stdin
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := io.WriteString(w, token+"\n"); err != nil {
		t.Fatal(err)
	}
	_ = w.Close()
	os.Stdin = r
	out, stderr, code := invokeHarness(t, s.Home, "login", "--provider", "openai", "--with-token")
	os.Stdin = oldStdin
	_ = r.Close()
	if code != 0 || !strings.Contains(out, "Signed in as …mnop (OpenAI-compatible)") {
		t.Fatalf("login = code %d stdout %q stderr %q", code, out, stderr)
	}
	out, stderr, code = invokeHarness(t, s.Home, "status", "--provider", "openai", "--json")
	if code != 0 || !strings.Contains(out, `"name": "openai"`) || !strings.Contains(out, `"logged_in": true`) {
		t.Fatalf("status = code %d stdout %q stderr %q", code, out, stderr)
	}
	out, stderr, code = invokeHarness(t, s.Home, "models", "--provider", "openai")
	if code != 0 || !strings.Contains(out, "gpt-4o-mini *") {
		t.Fatalf("models = code %d stdout %q stderr %q", code, out, stderr)
	}
	out, stderr, code = invokeHarness(t, s.Home, "logout", "--provider", "openai")
	if code != 0 || !strings.Contains(out, "removed stored OpenAI-compatible login") {
		t.Fatalf("logout = code %d stdout %q stderr %q", code, out, stderr)
	}
	if len(paths) != 2 {
		t.Fatalf("models endpoint calls = %v", paths)
	}
}

func TestHarnessRunRecordsSubmissionAndTrajectory(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{})
	trajectory := filepath.Join(t.TempDir(), "trajectory.json")
	out, stderr, code := invokeHarness(t, s.Home, "run", "-p", "do it", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--session", "hs-test", "--trajectory", trajectory, "--quiet")
	if code != 0 || !strings.Contains(out, "SUMMARY: fake complete") {
		t.Fatalf("run = code %d stdout %q stderr %q", code, out, stderr)
	}
	h, steps, err := s.HarnessSession("hs-test")
	if err != nil {
		t.Fatal(err)
	}
	if h == nil || h.Status != ExitSubmitted || h.ExitStatus != ExitSubmitted || h.Submission != strings.TrimSuffix(out, "\n") || h.Calls != 1 {
		t.Fatalf("session not recorded: %+v", h)
	}
	if len(steps) < 2 || steps[0].Kind != string(StepAssistant) || steps[len(steps)-1].Kind != string(StepExit) {
		t.Fatalf("steps not recorded: %+v", steps)
	}
	var detail string
	if err := s.DB.QueryRow("SELECT detail FROM events WHERE action = 'harness.step' AND subject = 'hs-test' ORDER BY seq LIMIT 1").Scan(&detail); err != nil {
		t.Fatal(err)
	}
	var event map[string]any
	if err := json.Unmarshal([]byte(detail), &event); err != nil {
		t.Fatal(err)
	}
	if event["session"] != "hs-test" || event["n"] != float64(steps[0].N) || event["kind"] != string(StepAssistant) {
		t.Fatalf("harness.step payload: %#v", event)
	}
	if _, ok := event["exit_code"]; ok {
		t.Fatalf("nil exit_code should be omitted: %#v", event)
	}
	data, err := os.ReadFile(trajectory)
	if err != nil {
		t.Fatal(err)
	}
	var traj Trajectory
	if err := json.Unmarshal(data, &traj); err != nil || traj.Info.ExitStatus != ExitSubmitted {
		t.Fatalf("bad trajectory: %v %+v", err, traj.Info)
	}
}

func TestHarnessRunUsesStoredPromptTemplate(t *testing.T) {
	s := harnessCommandStore(t)
	p := &commandFakeProvider{}
	setFakeProvider(t, p)
	if err := s.SetSetting("human", "harness.system_prompt", "CUSTOM {{.Cwd}} {{.Model}}"); err != nil {
		t.Fatal(err)
	}
	out, stderr, code := invokeHarness(t, s.Home, "run", "-p", "do it", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--session", "hs-prompt", "--trajectory", "none", "--quiet")
	if code != 0 || !strings.Contains(out, "SUMMARY: fake complete") || stderr != "" {
		t.Fatalf("run = code %d stdout %q stderr %q", code, out, stderr)
	}
	if len(p.requests) != 1 || !strings.HasPrefix(p.requests[0].Messages[0].Content, "CUSTOM ") || !strings.Contains(p.requests[0].Messages[0].Content, "fake-model") {
		t.Fatalf("prompt request = %+v", p.requests)
	}
}

func TestHarnessRunPromptFileOverridesInvalidStoredTemplate(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{})
	if _, err := s.DB.Exec(`INSERT INTO settings(name,value,updated_at,updated_by) VALUES('harness.system_prompt','{{.Missing}}',?,'test')`, s.Now()); err != nil {
		t.Fatal(err)
	}
	out, stderr, code := invokeHarness(t, s.Home, "run", "-p", "do it", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--session", "hs-bad-prompt", "--trajectory", "none", "--quiet")
	if code != 2 || !strings.Contains(stderr, "harness.system_prompt render failed") || out != "" {
		t.Fatalf("bad prompt = code %d stdout %q stderr %q", code, out, stderr)
	}
	file := filepath.Join(t.TempDir(), "prompt.txt")
	if err := os.WriteFile(file, []byte("FILE {{.Cwd}}"), 0o600); err != nil {
		t.Fatal(err)
	}
	out, stderr, code = invokeHarness(t, s.Home, "run", "-p", "do it", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--session", "hs-file-prompt", "--prompt-file", file, "--trajectory", "none", "--quiet")
	if code != 0 || !strings.Contains(out, "SUMMARY: fake complete") || stderr != "" {
		t.Fatalf("file prompt = code %d stdout %q stderr %q", code, out, stderr)
	}
}

func TestHarnessRunNotLoggedInExit(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{err: provider.ErrNotLoggedIn})
	out, stderr, code := invokeHarness(t, s.Home, "run", "-p", "do it", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--model", "fake-model", "--session", "hs-login")
	if code != 4 || !strings.Contains(stderr, "not logged in: sign in with GitHub in the console") {
		t.Fatalf("not logged in = code %d stdout %q stderr %q", code, out, stderr)
	}
	if !strings.Contains(out, "SUMMARY: harness stopped: not_logged_in") {
		t.Fatalf("missing synthesized summary: %q", out)
	}
}

// Without a configured model the login failure surfaces while the default
// model is looked up, before the loop starts; the session is still recorded
// so the console shows why nothing ran.
func TestHarnessRunNotLoggedInBeforeModelRecordsSession(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{err: provider.ErrNotLoggedIn})
	out, stderr, code := invokeHarness(t, s.Home, "run", "-p", "do it", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--session", "hs-login-early")
	if code != 4 || !strings.Contains(stderr, "not logged in:") || !strings.Contains(out, "SUMMARY: harness stopped: not_logged_in") {
		t.Fatalf("early not logged in = code %d stdout %q stderr %q", code, out, stderr)
	}
	h, steps, err := s.HarnessSession("hs-login-early")
	if err != nil {
		t.Fatal(err)
	}
	if h == nil || h.Status != ExitNotLoggedIn || h.Provider != fakeProviderName || h.Model != "" || !strings.Contains(h.Error, "not logged in") {
		t.Fatalf("early login failure not recorded: %+v", h)
	}
	if len(steps) != 0 {
		t.Fatalf("no steps expected, got %+v", steps)
	}
}

func TestHarnessRunWithoutStoreStillSubmits(t *testing.T) {
	home := t.TempDir()
	setFakeProvider(t, &commandFakeProvider{})
	out, stderr, code := invokeHarness(t, home, "run", "-p", "do it", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--no-store", "--trajectory", "none", "--quiet")
	if code != 0 || !strings.Contains(out, "SUMMARY: fake complete") || stderr != "" {
		t.Fatalf("no-store run = code %d stdout %q stderr %q", code, out, stderr)
	}
}

func TestHarnessStatusJSONShape(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{login: provider.Login{LoggedIn: true, User: "octocat", Source: "test"}})
	out, stderr, code := invokeHarness(t, s.Home, "status", "--provider", fakeProviderName, "--json")
	if code != 0 || stderr != "" {
		t.Fatalf("status = code %d stderr %q", code, stderr)
	}
	var got struct {
		Harness  string `json:"harness"`
		Provider struct {
			Name  string         `json:"name"`
			Login provider.Login `json:"login"`
		} `json:"provider"`
		Limits struct {
			MaxSteps int `json:"max_steps"`
		} `json:"limits"`
		Sessions struct {
			Running int `json:"running"`
		} `json:"sessions"`
	}
	if err := json.Unmarshal([]byte(out), &got); err != nil {
		t.Fatal(err)
	}
	if got.Harness != "native" || got.Provider.Name != fakeProviderName || got.Provider.Login.User != "octocat" || got.Limits.MaxSteps == 0 {
		t.Fatalf("bad status JSON: %+v", got)
	}
}

func TestHarnessCancelCommand(t *testing.T) {
	s := harnessCommandStore(t)
	if err := s.BeginHarnessSession(store.HarnessSession{ID: "hs-cancel"}); err != nil {
		t.Fatal(err)
	}
	out, stderr, code := invokeHarness(t, s.Home, "cancel", "hs-cancel")
	if code != 0 || out != "cancel requested for hs-cancel\n" || stderr != "" {
		t.Fatalf("cancel running = code %d stdout %q stderr %q", code, out, stderr)
	}
	h, err := s.HarnessSessionHead("hs-cancel")
	if err != nil {
		t.Fatal(err)
	}
	if h.CancelRequestedAt == "" {
		t.Fatalf("cancel not recorded: %+v", h)
	}
	out, stderr, code = invokeHarness(t, s.Home, "cancel", "hs-cancel")
	if code != 0 || out != "cancel already requested for hs-cancel\n" || stderr != "" {
		t.Fatalf("cancel twice = code %d stdout %q stderr %q", code, out, stderr)
	}
	if err := s.FinishHarnessSession("hs-cancel", "cancelled", "cancelled", "", "cancelled by the owner", 0, ""); err != nil {
		t.Fatal(err)
	}
	out, stderr, code = invokeHarness(t, s.Home, "cancel", "hs-cancel")
	if code != 1 || out != "" || !strings.Contains(stderr, "session is not running") {
		t.Fatalf("cancel finished = code %d stdout %q stderr %q", code, out, stderr)
	}
}

func TestHarnessPromptCommand(t *testing.T) {
	s := harnessCommandStore(t)
	out, stderr, code := invokeHarness(t, s.Home, "prompt", "--default")
	if code != 0 || stderr != "" || !strings.Contains(out, "You are an autonomous software engineering agent") || !strings.Contains(out, "{{.StepTimeout}}") {
		t.Fatalf("prompt default = code %d stdout %q stderr %q", code, out, stderr)
	}
	out, stderr, code = invokeHarness(t, s.Home, "prompt", "--raw")
	if code != 0 || out != "\n" || strings.TrimSpace(stderr) != "built-in" {
		t.Fatalf("prompt raw built-in = code %d stdout %q stderr %q", code, out, stderr)
	}
	if err := s.SetSetting("human", "harness.system_prompt", "Stored {{.Cwd}} {{.Provider}}"); err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	out, stderr, code = invokeHarness(t, s.Home, "prompt", "-C", dir, "--provider", "hubfake")
	if code != 0 || stderr != "" || !strings.Contains(out, "Stored "+dir+" hubfake") {
		t.Fatalf("prompt rendered = code %d stdout %q stderr %q", code, out, stderr)
	}
	out, stderr, code = invokeHarness(t, s.Home, "prompt", "--raw")
	if code != 0 || stderr != "" || out != "Stored {{.Cwd}} {{.Provider}}\n" {
		t.Fatalf("prompt raw custom = code %d stdout %q stderr %q", code, out, stderr)
	}
}

func TestHarnessModelsTable(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{})
	out, stderr, code := invokeHarness(t, s.Home, "models", "--provider", fakeProviderName, "--no-color")
	if code != 0 || stderr != "" {
		t.Fatalf("models = code %d stderr %q", code, stderr)
	}
	for _, want := range []string{"ID", "fake-model *", "Fake Model", "yes"} {
		if !strings.Contains(out, want) {
			t.Fatalf("models output missing %q:\n%s", want, out)
		}
	}
}

func TestHarnessRunInputErrors(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{})
	for _, tc := range [][]string{
		{"run", "--provider", fakeProviderName},
		{"nope"},
	} {
		_, _, code := invokeHarness(t, s.Home, tc...)
		if code != 2 {
			t.Fatalf("%v exit code = %d, want 2", tc, code)
		}
	}
}

func TestHarnessRunTaskFileStdin(t *testing.T) {
	s := harnessCommandStore(t)
	setFakeProvider(t, &commandFakeProvider{})
	old := os.Stdin
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.Stdin = old })
	os.Stdin = r
	_, _ = io.WriteString(w, "from stdin")
	_ = w.Close()
	out, stderr, code := invokeHarness(t, s.Home, "run", "--task-file", "-", "-C", t.TempDir(),
		"--provider", fakeProviderName, "--no-store", "--trajectory", "none", "--quiet")
	if code != 0 || !strings.Contains(out, "SUMMARY: fake complete") || stderr != "" {
		t.Fatalf("stdin task = code %d stdout %q stderr %q", code, out, stderr)
	}
}
