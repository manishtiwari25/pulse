package harness

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
)

type fakeProvider struct {
	responses []provider.Response
	err       error
	requests  []provider.Request
}

func (p *fakeProvider) Name() string  { return "fake" }
func (p *fakeProvider) Label() string { return "Fake" }
func (p *fakeProvider) Chat(ctx context.Context, req provider.Request) (provider.Response, error) {
	p.requests = append(p.requests, req)
	if p.err != nil {
		return provider.Response{}, p.err
	}
	if len(p.responses) == 0 {
		return provider.Response{}, errors.New("no response")
	}
	resp := p.responses[0]
	p.responses = p.responses[1:]
	return resp, nil
}
func (p *fakeProvider) Models(context.Context) ([]provider.Model, error) { return nil, nil }
func (p *fakeProvider) Status(context.Context) (provider.Login, error)   { return provider.Login{}, nil }

type fakeEnv struct {
	observations []Observation
	commands     []string
}

func (e *fakeEnv) Execute(ctx context.Context, command string, timeout time.Duration) Observation {
	e.commands = append(e.commands, command)
	if len(e.observations) == 0 {
		return Observation{Output: "", ExitCode: 0}
	}
	obs := e.observations[0]
	e.observations = e.observations[1:]
	return obs
}
func (e *fakeEnv) Info() map[string]string {
	return map[string]string{"cwd": "/work", "os": "testos", "shell": "/bin/sh"}
}

func baseConfig() Config {
	return Config{Model: "m", Task: "do it", SystemPrompt: "system", MaxSteps: 10}
}

func TestToolCallPathSubmits(t *testing.T) {
	p := &fakeProvider{responses: []provider.Response{
		{
			Message: provider.Message{ToolCalls: []provider.ToolCall{{ID: "call-1", Name: "bash", Arguments: `{"command":"echo hi"}`}}},
			Usage:   provider.Usage{PromptTokens: 1, CompletionTokens: 2, Cost: 0.5},
			Model:   "m-resolved",
		},
		{Message: provider.Message{Content: "SUMMARY: done\nFINDING: ok"}, Usage: provider.Usage{PromptTokens: 3}},
	}}
	env := &fakeEnv{observations: []Observation{{Output: "hi\n", ExitCode: 0, Duration: time.Millisecond}}}
	got, err := New(p, env, baseConfig()).Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitSubmitted || !strings.Contains(got.Submission, "SUMMARY: done") {
		t.Fatalf("unexpected result: %+v", got)
	}
	if got.Steps != 1 || got.Calls != 2 {
		t.Fatalf("steps/calls = %d/%d, want 1/2", got.Steps, got.Calls)
	}
	if p.requests[0].Initiator != "user" || p.requests[1].Initiator != "agent" {
		t.Fatalf("initiators = %q, %q", p.requests[0].Initiator, p.requests[1].Initiator)
	}
	msgs := New(p, env, baseConfig()).Messages()
	_ = msgs
	h := New(&fakeProvider{responses: []provider.Response{
		{Message: provider.Message{ToolCalls: []provider.ToolCall{{ID: "call-1", Name: "bash", Arguments: `{"command":"echo hi"}`}}}},
		{Message: provider.Message{Content: "SUMMARY: done"}},
	}}, &fakeEnv{observations: []Observation{{Output: "hi\n", ExitCode: 0}}}, baseConfig())
	if _, err := h.Run(context.Background()); err != nil {
		t.Fatal(err)
	}
	hmsgs := h.Messages()
	if hmsgs[3].Role != provider.RoleTool || hmsgs[3].ToolCallID != "call-1" || !strings.Contains(hmsgs[3].Content, "<returncode>0</returncode>") {
		t.Fatalf("tool message not recorded: %+v", hmsgs[3])
	}
}

func TestTextModePathAndTwoBlockFormatError(t *testing.T) {
	t.Run("one block runs and appends user observation", func(t *testing.T) {
		h := New(&fakeProvider{responses: []provider.Response{
			{Message: provider.Message{Content: "```bash\necho hi\n```"}},
			{Message: provider.Message{Content: "SUMMARY: done"}},
		}}, &fakeEnv{observations: []Observation{{Output: "hi\n", ExitCode: 0}}}, baseConfig())
		got, err := h.Run(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		if got.ExitStatus != ExitSubmitted {
			t.Fatalf("exit = %s", got.ExitStatus)
		}
		msgs := h.Messages()
		if msgs[3].Role != provider.RoleUser || !strings.Contains(msgs[3].Content, "hi") {
			t.Fatalf("observation user message = %+v", msgs[3])
		}
	})
	t.Run("two blocks nudge", func(t *testing.T) {
		cfg := baseConfig()
		cfg.MaxFormatErrors = 1
		h := New(&fakeProvider{responses: []provider.Response{{Message: provider.Message{Content: "```bash\na\n```\n```sh\nb\n```"}}}}, &fakeEnv{}, cfg)
		got, err := h.Run(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		if got.ExitStatus != ExitFormatError {
			t.Fatalf("exit = %s", got.ExitStatus)
		}
		lastUser := h.Messages()[3]
		if !strings.Contains(lastUser.Content, "write exactly one") {
			t.Fatalf("missing nudge: %+v", lastUser)
		}
	})
}

func TestFormatErrorsStopAfterThreePlanOnlyMessages(t *testing.T) {
	h := New(&fakeProvider{responses: []provider.Response{
		{Message: provider.Message{Content: "I will inspect first."}},
		{Message: provider.Message{Content: "Plan: run tests."}},
		{Message: provider.Message{Content: "Next I will edit."}},
	}}, &fakeEnv{}, baseConfig())
	got, err := h.Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitFormatError || got.Calls != 3 {
		t.Fatalf("unexpected result: %+v", got)
	}
	nudges := 0
	for _, m := range h.Messages() {
		if strings.Contains(m.Content, "no command and no report") {
			nudges++
		}
	}
	if nudges != 3 {
		t.Fatalf("nudges = %d, want 3", nudges)
	}
}

func TestLimits(t *testing.T) {
	t.Run("step limit", func(t *testing.T) {
		cfg := baseConfig()
		cfg.MaxSteps = 1
		h := New(&fakeProvider{responses: []provider.Response{{Message: provider.Message{ToolCalls: []provider.ToolCall{{ID: "c", Name: "bash", Arguments: `{"command":"echo hi"}`}}}}}}, &fakeEnv{}, cfg)
		got, err := h.Run(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		if got.ExitStatus != ExitStepLimit || got.Calls != 1 {
			t.Fatalf("unexpected result: %+v", got)
		}
	})
	t.Run("wall time", func(t *testing.T) {
		cfg := baseConfig()
		cfg.WallTime = 50 * time.Millisecond
		cfg.StepTimeout = time.Second
		h := New(&fakeProvider{responses: []provider.Response{{Message: provider.Message{Content: "```bash\nsleep 5\n```"}}}}, NewLocalEnv(t.TempDir(), nil), cfg)
		got, err := h.Run(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		if got.ExitStatus != ExitTimeLimit {
			t.Fatalf("exit = %s, want %s", got.ExitStatus, ExitTimeLimit)
		}
	})
}

func TestDenyRefusesWithoutExecuting(t *testing.T) {
	env := &fakeEnv{}
	h := New(&fakeProvider{responses: []provider.Response{
		{Message: provider.Message{ToolCalls: []provider.ToolCall{{ID: "c", Name: "bash", Arguments: `{"command":"git push origin main"}`}}}},
		{Message: provider.Message{Content: "SUMMARY: done"}},
	}}, env, baseConfig())
	got, err := h.Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitSubmitted || len(env.commands) != 0 {
		t.Fatalf("result=%+v commands=%v", got, env.commands)
	}
	if !strings.Contains(h.Messages()[3].Content, "refused by policy") {
		t.Fatalf("missing refusal: %q", h.Messages()[3].Content)
	}
}

func TestLocalEnvTimeoutKillsProcessGroup(t *testing.T) {
	start := time.Now()
	obs := NewLocalEnv(t.TempDir(), nil).Execute(context.Background(), "sleep 5", 200*time.Millisecond)
	if !obs.TimedOut || obs.ExitCode != -1 {
		t.Fatalf("obs = %+v", obs)
	}
	if time.Since(start) > 2*time.Second {
		t.Fatalf("timeout took too long: %s", time.Since(start))
	}
}

func TestTruncationKeepsHeadTail(t *testing.T) {
	out := formatObservation(Observation{Output: "abcdefghijklmnopqrstuvwxyz", ExitCode: 0}, 10)
	if !strings.Contains(out, "abcde") || !strings.Contains(out, "vwxyz") || !strings.Contains(out, "characters omitted") {
		t.Fatalf("bad truncation: %q", out)
	}
}

func TestProviderErrors(t *testing.T) {
	for _, tc := range []struct {
		name string
		err  error
		want string
	}{
		{"not logged in", provider.ErrNotLoggedIn, ExitNotLoggedIn},
		{"provider", errors.New("provider failed with   spaces"), ExitProviderError},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got, err := New(&fakeProvider{err: tc.err}, &fakeEnv{}, baseConfig()).Run(context.Background())
			if err != nil {
				t.Fatal(err)
			}
			if got.ExitStatus != tc.want || got.Error == "" {
				t.Fatalf("unexpected result: %+v", got)
			}
		})
	}
}

func TestWriteTrajectory(t *testing.T) {
	h := New(&fakeProvider{responses: []provider.Response{{Message: provider.Message{Content: "SUMMARY: done"}}}}, &fakeEnv{}, baseConfig())
	got, err := h.Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitSubmitted {
		t.Fatal(got.ExitStatus)
	}
	path := filepath.Join(t.TempDir(), "nested", "trajectory.json")
	if err := WriteTrajectory(path, h.Trajectory()); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), `"trajectory_format": "pulse-harness-1"`) || !strings.Contains(string(data), `"exit_status": "submitted"`) {
		t.Fatalf("bad trajectory: %s", data)
	}
	st, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if st.Mode().Perm() != 0600 {
		t.Fatalf("mode = %v", st.Mode().Perm())
	}
}

func TestSinkPanicIsSwallowed(t *testing.T) {
	h := New(&fakeProvider{responses: []provider.Response{{Message: provider.Message{Content: "SUMMARY: done"}}}}, &fakeEnv{}, baseConfig(), SinkFunc(func(Step) {
		panic("boom")
	}))
	got, err := h.Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitSubmitted {
		t.Fatalf("exit = %s", got.ExitStatus)
	}
}

func TestCompleteTaskMarkerFromCommandOutputSubmits(t *testing.T) {
	h := New(&fakeProvider{responses: []provider.Response{{Message: provider.Message{Content: "```bash\necho COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT; echo 'SUMMARY: from marker'\n```"}}}}, NewLocalEnv(t.TempDir(), nil), baseConfig())
	got, err := h.Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitSubmitted || strings.TrimSpace(got.Submission) != "SUMMARY: from marker" {
		t.Fatalf("unexpected result: %+v", got)
	}
}
