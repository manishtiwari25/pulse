package harness

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
	providerfake "github.com/manishtiwari25/pulse/internal/provider/fake"
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

type budgetAwareProvider struct {
	requests []provider.Request
}

func (p *budgetAwareProvider) Name() string  { return "budget-aware" }
func (p *budgetAwareProvider) Label() string { return "Budget Aware" }
func (p *budgetAwareProvider) Chat(ctx context.Context, req provider.Request) (provider.Response, error) {
	_ = ctx
	p.requests = append(p.requests, cloneRequestForTest(req))
	if requestHasBudgetNotice(req) {
		return provider.Response{Message: provider.Message{Content: "SUMMARY: done after notice"}}, nil
	}
	args, _ := json.Marshal(struct {
		Command string `json:"command"`
	}{Command: "true"})
	return provider.Response{Message: provider.Message{ToolCalls: []provider.ToolCall{{ID: "call", Name: "bash", Arguments: string(args)}}}}, nil
}
func (p *budgetAwareProvider) Models(context.Context) ([]provider.Model, error) {
	return nil, nil
}
func (p *budgetAwareProvider) Status(context.Context) (provider.Login, error) {
	return provider.Login{}, nil
}

func requestHasBudgetNotice(req provider.Request) bool {
	for _, msg := range req.Messages {
		if msg.Role == provider.RoleUser && strings.HasPrefix(msg.Content, "Budget notice:") {
			return true
		}
	}
	return false
}

type blockingProvider struct {
	started  chan struct{}
	requests []provider.Request
}

func (p *blockingProvider) Name() string  { return "blocking" }
func (p *blockingProvider) Label() string { return "Blocking" }
func (p *blockingProvider) Chat(ctx context.Context, req provider.Request) (provider.Response, error) {
	p.requests = append(p.requests, cloneRequestForTest(req))
	if p.started != nil {
		close(p.started)
		p.started = nil
	}
	<-ctx.Done()
	return provider.Response{}, ctx.Err()
}
func (p *blockingProvider) Models(context.Context) ([]provider.Model, error) {
	return nil, nil
}
func (p *blockingProvider) Status(context.Context) (provider.Login, error) {
	return provider.Login{}, nil
}

func cloneRequestForTest(req provider.Request) provider.Request {
	req.Messages = cloneMessages(req.Messages)
	req.Tools = append([]provider.Tool(nil), req.Tools...)
	return req
}

type fakeEnv struct {
	observations  []Observation
	commands      []string
	defaultOutput string
}

func (e *fakeEnv) Execute(ctx context.Context, command string, timeout time.Duration) Observation {
	e.commands = append(e.commands, command)
	if len(e.observations) == 0 {
		return Observation{Output: e.defaultOutput, ExitCode: 0}
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

func TestDefaultPromptTemplateRendersLikeDefaultSystemPrompt(t *testing.T) {
	cfg := Config{Cwd: "/repo", Model: "m", Provider: "fake", MaxSteps: 2, StepTimeout: 15 * time.Second, OutputLimit: 1234}
	info := map[string]string{"cwd": "/work", "os": "testos", "shell": "/bin/zsh"}
	rendered, err := RenderPrompt(DefaultPromptTemplate, promptData(info, cfg))
	if err != nil {
		t.Fatal(err)
	}
	if rendered != DefaultSystemPrompt(info, cfg) || !strings.Contains(rendered, "Commands time out after 15s.") || !strings.Contains(rendered, "Output over 1234 characters") {
		t.Fatalf("rendered prompt mismatch:\n%s", rendered)
	}
	custom, err := RenderPrompt("{{.Cwd}} {{.Provider}} {{.Model}} {{.Info.shell}}", promptData(info, cfg))
	if err != nil {
		t.Fatal(err)
	}
	if custom != "/work fake m /bin/zsh" {
		t.Fatalf("custom prompt = %q", custom)
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

func TestCostLimitStopsRun(t *testing.T) {
	p := providerfake.New(providerfake.Script{
		Replies: []providerfake.Reply{{Command: "echo one", Usage: provider.Usage{Cost: 0.6}}},
		Loop:    true,
	})
	cfg := baseConfig()
	cfg.MaxCost = 1
	env := &fakeEnv{observations: []Observation{{Output: "one\n", ExitCode: 0}, {Output: "one\n", ExitCode: 0}}}
	got, err := New(p, env, cfg).Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitCostLimit || got.Calls != 2 || !strings.Contains(got.Error, "cost budget") {
		t.Fatalf("unexpected result: %+v", got)
	}
}

func TestBudgetNoticeAppearsOnceAndCanSubmit(t *testing.T) {
	p := &budgetAwareProvider{}
	cfg := baseConfig()
	cfg.MaxSteps = 20
	var notes []Step
	h := New(p, &fakeEnv{defaultOutput: "ok\n"}, cfg, SinkFunc(func(s Step) {
		if s.Kind == StepNote && strings.HasPrefix(s.Content, "Budget notice:") {
			notes = append(notes, s)
		}
	}))
	got, err := h.Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitSubmitted || got.Calls != 19 || !strings.Contains(got.Submission, "SUMMARY: done after notice") {
		t.Fatalf("unexpected result: %+v", got)
	}
	if len(notes) != 1 {
		t.Fatalf("budget notes = %d, want 1", len(notes))
	}
	noticeReq := cfg.MaxSteps - min(5, cfg.MaxSteps/10)
	if len(p.requests) != got.Calls || !requestHasBudgetNotice(p.requests[noticeReq]) {
		t.Fatalf("notice missing from request %d of %d", noticeReq, len(p.requests))
	}
	for i, req := range p.requests {
		count := 0
		for _, msg := range req.Messages {
			if msg.Role == provider.RoleUser && strings.HasPrefix(msg.Content, "Budget notice:") {
				count++
			}
		}
		if i < noticeReq && count != 0 {
			t.Fatalf("request %d has early budget notice", i)
		}
		if i >= noticeReq && count != 1 {
			t.Fatalf("request %d budget notices = %d, want 1", i, count)
		}
	}
}

func TestBudgetNoticeNotSentBelowTenSteps(t *testing.T) {
	p := providerfake.New(providerfake.Script{Replies: []providerfake.Reply{{Command: "true"}}, Loop: true})
	cfg := baseConfig()
	cfg.MaxSteps = 9
	h := New(p, &fakeEnv{defaultOutput: "ok\n"}, cfg)
	got, err := h.Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitStepLimit || got.Calls != 9 {
		t.Fatalf("unexpected result: %+v", got)
	}
	for _, req := range p.Requests() {
		if requestHasBudgetNotice(req) {
			t.Fatal("budget notice was sent below 10 max steps")
		}
	}
}

func TestLongRunStaysUnderWindow(t *testing.T) {
	var replies []providerfake.Reply
	for i := 0; i < 80; i++ {
		replies = append(replies, providerfake.Reply{Command: "yes | head -c 20000"})
	}
	replies = append(replies, providerfake.Reply{Report: "SUMMARY: done"})
	p := providerfake.New(providerfake.Script{Replies: replies, TokensPerChar: 0.25})
	cfg := baseConfig()
	cfg.MaxSteps = 100
	cfg.ContextWindow = 128000
	cfg.OutputLimit = 20000
	env := &fakeEnv{defaultOutput: strings.Repeat("x\n", 10000)}
	var steps []Step
	got, err := New(p, env, cfg, SinkFunc(func(s Step) { steps = append(steps, s) })).Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitSubmitted {
		t.Fatalf("exit = %s", got.ExitStatus)
	}
	limit := int(0.8 * float64(cfg.ContextWindow))
	for i, req := range p.Requests() {
		if i < 6 {
			continue
		}
		chars := requestCharsForTest(req)
		if int(float64(chars)*0.25) > limit {
			t.Fatalf("request %d has %d chars (~%d tokens), over limit %d", i, chars, int(float64(chars)*0.25), limit)
		}
	}
	notes := 0
	for _, step := range steps {
		if step.Kind == StepNote {
			notes++
		}
	}
	if notes == 0 {
		t.Fatal("no prune note emitted")
	}
}

func TestPruneNoteEmittedOncePerLevel(t *testing.T) {
	replies := []providerfake.Reply{
		{Command: "long"}, {Command: "long"}, {Command: "long"}, {Command: "long"}, {Report: "SUMMARY: done"},
	}
	p := providerfake.New(providerfake.Script{Replies: replies})
	cfg := baseConfig()
	cfg.KeepObservations = 1
	cfg.MaxSteps = 10
	env := &fakeEnv{defaultOutput: strings.Repeat("x\n", 20)}
	var notes []Step
	got, err := New(p, env, cfg, SinkFunc(func(s Step) {
		if s.Kind == StepNote {
			notes = append(notes, s)
		}
	})).Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitSubmitted {
		t.Fatalf("exit = %s", got.ExitStatus)
	}
	if len(notes) != 1 {
		t.Fatalf("notes = %d, want 1: %+v", len(notes), notes)
	}
	if !strings.Contains(notes[0].Content, "level 1, keep 1") {
		t.Fatalf("note content = %q", notes[0].Content)
	}
}

func requestCharsForTest(req provider.Request) int {
	total := 0
	for _, msg := range req.Messages {
		total += len(msg.Content)
		for _, call := range msg.ToolCalls {
			total += len(call.Arguments)
		}
	}
	return total
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

func TestCancellationReportsOwnerReason(t *testing.T) {
	cancelFile := filepath.Join(t.TempDir(), "cancel")
	if err := os.WriteFile(cancelFile, []byte("stop"), 0o600); err != nil {
		t.Fatal(err)
	}
	got, err := New(&fakeProvider{}, &fakeEnv{}, Config{Model: "m", Task: "do it", SystemPrompt: "system", CancelFile: cancelFile}).Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitCancelled || got.Error != "cancelled by the owner" {
		t.Fatalf("cancel file result: %+v", got)
	}

	ctx, cancel := context.WithCancelCause(context.Background())
	cancel(ErrOwnerCancelled)
	got, err = New(&fakeProvider{}, &fakeEnv{}, Config{Model: "m", Task: "do it", SystemPrompt: "system"}).Run(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitCancelled || got.Error != "cancelled by the owner" {
		t.Fatalf("cancel cause result: %+v", got)
	}
}

func TestProviderCallCancellationReportsCancelReason(t *testing.T) {
	for _, tc := range []struct {
		name  string
		cause error
		want  string
	}{
		{"plain", context.Canceled, "interrupted: context canceled"},
		{"owner", ErrOwnerCancelled, "cancelled by the owner"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			p := &blockingProvider{started: make(chan struct{})}
			ctx, cancel := context.WithCancelCause(context.Background())
			done := make(chan Result, 1)
			errs := make(chan error, 1)
			go func() {
				got, err := New(p, &fakeEnv{}, baseConfig()).Run(ctx)
				done <- got
				errs <- err
			}()
			select {
			case <-p.started:
			case <-time.After(time.Second):
				t.Fatal("provider call did not start")
			}
			cancel(tc.cause)
			got := <-done
			if err := <-errs; err != nil {
				t.Fatal(err)
			}
			if got.ExitStatus != ExitCancelled || got.Error != tc.want {
				t.Fatalf("cancel result = %+v, want %q", got, tc.want)
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
}

func TestTrajectoryWrittenEveryStep(t *testing.T) {
	path := filepath.Join(t.TempDir(), "trajectory.json")
	p := &fakeProvider{responses: []provider.Response{
		{Message: provider.Message{ToolCalls: []provider.ToolCall{{ID: "call-1", Name: "bash", Arguments: `{"command":"echo hi"}`}}}},
		{Message: provider.Message{Content: "SUMMARY: done"}},
	}}
	env := &fakeEnv{observations: []Observation{{Output: "hi\n", ExitCode: 0}}}
	trajectoryLog := &trajectorySink{path: path}
	sawCheckpoint := false
	var agent *Agent
	check := SinkFunc(func(s Step) {
		if s.N != 1 {
			return
		}
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var traj Trajectory
		if err := json.Unmarshal(data, &traj); err != nil {
			t.Fatal(err)
		}
		if len(traj.Steps) < 2 {
			t.Fatalf("checkpoint has %d steps, want at least 2", len(traj.Steps))
		}
		sawCheckpoint = true
	})
	agent = New(p, env, baseConfig(), trajectoryLog, check)
	trajectoryLog.agent = agent
	got, err := agent.Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if got.ExitStatus != ExitSubmitted {
		t.Fatalf("exit = %s", got.ExitStatus)
	}
	if !sawCheckpoint {
		t.Fatal("checkpoint was not observed after the second step")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var traj Trajectory
	if err := json.Unmarshal(data, &traj); err != nil {
		t.Fatal(err)
	}
	if len(traj.Steps) != len(agent.Trajectory().Steps) {
		t.Fatalf("final trajectory steps = %d, want %d", len(traj.Steps), len(agent.Trajectory().Steps))
	}
	if _, err := os.Stat(path + ".tmp"); !os.IsNotExist(err) {
		t.Fatalf("tmp file left behind: %v", err)
	}
	_ = agent
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
