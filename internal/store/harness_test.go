package store

import (
	"database/sql"
	"errors"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestHarnessSettings(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-11T05:00:00Z"})
	for key, want := range HarnessSettings {
		if got := s.Setting(key, ""); got != want {
			t.Fatalf("%s default = %q, want %q", key, got, want)
		}
	}
	var stored int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM settings WHERE name IN ('harness', 'provider', 'harness.max_steps', 'harness.max_cost', 'harness.keep_observations', 'harness.system_prompt', 'harness.step_timeout', 'harness.output_limit')").Scan(&stored); err != nil || stored != 0 {
		t.Fatalf("harness settings written at Create: %d %v", stored, err)
	}
	if !s.HarnessNative() {
		t.Fatal("HarnessNative false by default")
	}
	if err := s.SetSetting("human", "harness", "copilot-cli"); err != nil {
		t.Fatal(err)
	}
	if s.HarnessNative() {
		t.Fatal("HarnessNative true after copilot-cli")
	}
	if err := s.SetSetting("human", "harness", "moon"); err == nil || !strings.Contains(err.Error(), "harness takes one of native, copilot-cli") {
		t.Fatalf("bad harness choice: %v", err)
	}
	if _, err := ValidateSetting("harness.mode", "native"); err == nil || !strings.Contains(err.Error(), "Harness settings: harness, harness.keep_observations, harness.max_cost, harness.max_steps, harness.output_limit, harness.step_timeout, harness.system_prompt, provider, provider.openai.base_url") {
		t.Fatalf("unknown harness hint: %v", err)
	}
	if v, err := ValidateSetting("provider", "Copilot"); err != nil || v != "copilot" {
		t.Fatalf("provider: %q %v", v, err)
	}
	if v, err := ValidateSetting("provider", "OpenAI"); err != nil || v != "openai" {
		t.Fatalf("openai provider: %q %v", v, err)
	}
	if !reflect.DeepEqual(HarnessRules["provider"].Choices, []string{"copilot", "openai"}) {
		t.Fatalf("provider choices = %v", HarnessRules["provider"].Choices)
	}
	if got := s.Setting("provider.openai.base_url", ""); got != "https://api.openai.com/v1" {
		t.Fatalf("openai base default = %q", got)
	}
	if got := s.ProviderSettings(); !reflect.DeepEqual(got, map[string]string{"provider.openai.base_url": "https://api.openai.com/v1"}) {
		t.Fatalf("provider settings default = %#v", got)
	}
	if err := s.SetSetting("human", "provider.openai.base_url", "http://127.0.0.1:11434/v1"); err != nil {
		t.Fatal(err)
	}
	if got := s.ProviderSettings(); got["provider.openai.base_url"] != "http://127.0.0.1:11434/v1" {
		t.Fatalf("provider settings stored = %#v", got)
	}
	for _, tc := range []struct {
		name string
		bad  string
		good string
	}{
		{"harness.max_steps", "0", "1"},
		{"harness.keep_observations", "0", "1"},
		{"harness.step_timeout", "9", "10"},
		{"harness.output_limit", "999", "1000"},
	} {
		if err := s.SetSetting("human", tc.name, tc.bad); err == nil || !strings.Contains(err.Error(), tc.name+" must be at least") {
			t.Fatalf("%s bad value: %v", tc.name, err)
		}
		if v, err := ValidateSetting("harness.max_cost", "0.5"); err != nil || v != "0.5" {
			t.Fatalf("harness.max_cost valid: %q %v", v, err)
		}
		if _, err := ValidateSetting("harness.max_cost", "abc"); err == nil || !strings.Contains(err.Error(), "harness.max_cost takes a number") {
			t.Fatalf("harness.max_cost invalid text: %v", err)
		}
		if _, err := ValidateSetting("harness.max_cost", "-0.1"); err == nil || !strings.Contains(err.Error(), "harness.max_cost must be at least") {
			t.Fatalf("harness.max_cost below min: %v", err)
		}
		if err := s.SetSetting("human", tc.name, tc.good); err != nil {
			t.Fatalf("%s good value: %v", tc.name, err)
		}
		if got := s.Setting(tc.name, ""); got != tc.good {
			t.Fatalf("%s stored %q, want %q", tc.name, got, tc.good)
		}
	}
	if v, err := ValidateSetting("harness.system_prompt", "Hello {{.Cwd}}"); err != nil || v != "Hello {{.Cwd}}" {
		t.Fatalf("harness.system_prompt valid: %q %v", v, err)
	}
	if _, err := ValidateSetting("harness.system_prompt", "{{if}}"); err == nil || !strings.Contains(err.Error(), "harness.system_prompt is not a valid Go text/template") {
		t.Fatalf("harness.system_prompt invalid: %v", err)
	}
	if !strings.Contains(SettingRules["runner_cmd"].Description, "copilot-cli") {
		t.Fatal("runner_cmd does not describe the copilot-cli harness")
	}
	if !strings.Contains(SettingRules["max_ai_credits"].Description, "copilot CLI") {
		t.Fatal("max_ai_credits does not describe the copilot CLI")
	}
	if names := HarnessSettingNames(); !reflect.DeepEqual(names, []string{"harness", "harness.keep_observations", "harness.max_cost", "harness.max_steps", "harness.output_limit", "harness.step_timeout", "harness.system_prompt", "provider", "provider.openai.base_url"}) {
		t.Fatalf("HarnessSettingNames = %v", names)
	}
}

func TestHarnessSessionAccessors(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-11T05:00:00Z"})
	if err := s.BeginHarnessSession(HarnessSession{
		ID:       "hs-1",
		RunID:    "run-1",
		JobID:    "job-1",
		Agent:    "agent-a",
		Project:  "proj",
		Executor: "local",
		Provider: "copilot",
		Model:    "gpt",
		Cwd:      "/tmp/proj",
	}); err != nil {
		t.Fatal(err)
	}
	for _, step := range []HarnessStep{
		{SessionID: "hs-1", N: 3, Kind: "exit", Content: "done"},
		{SessionID: "hs-1", N: 1, Kind: "assistant", Content: "run tests", PromptTokens: 11, CompletionTokens: 7, Model: "gpt"},
		{SessionID: "hs-1", N: 2, Kind: "command", Command: "go test ./internal/store/..."},
		{SessionID: "hs-1", N: 4, Kind: "observation", Content: "ok", ExitCode: intPtr(0), DurationMS: 1500, PromptTokens: 3},
	} {
		if err := s.AppendHarnessStep(step); err != nil {
			t.Fatalf("append step %d: %v", step.N, err)
		}
	}
	h, steps, err := s.HarnessSession("hs-1")
	if err != nil {
		t.Fatal(err)
	}
	if h == nil || h.Status != "running" || h.StartedAt != "2026-09-11T05:00:00Z" || h.UpdatedAt != "2026-09-11T05:00:00Z" {
		t.Fatalf("session timestamps/status: %+v", h)
	}
	if h.CancelRequestedAt != "" {
		t.Fatalf("fresh session has cancel_requested_at = %q", h.CancelRequestedAt)
	}
	if h.Steps != 1 || h.Calls != 1 || h.PromptTokens != 14 || h.CompletionTokens != 7 {
		t.Fatalf("session totals: %+v", h)
	}
	head, err := s.HarnessSessionHead("hs-1")
	if err != nil {
		t.Fatal(err)
	}
	if head == nil || head.ID != "hs-1" || head.Steps != 1 || head.Calls != 1 {
		t.Fatalf("session head: %+v", head)
	}
	if got := stepNumbers(steps); !reflect.DeepEqual(got, []int{1, 2, 3, 4}) {
		t.Fatalf("steps ordered by n: %v", got)
	}
	after, err := s.HarnessSteps("hs-1", 1)
	if err != nil {
		t.Fatal(err)
	}
	if got := stepNumbers(after); !reflect.DeepEqual(got, []int{2, 3, 4}) {
		t.Fatalf("steps after 1 = %v", got)
	}
	if steps[3].ExitCode == nil || *steps[3].ExitCode != 0 || steps[3].DurationMS != 1500 {
		t.Fatalf("observation scan: %+v", steps[3])
	}
	if err := s.AppendHarnessStep(HarnessStep{SessionID: "hs-1", N: 1, Kind: "assistant", PromptTokens: 5, CompletionTokens: 2}); err != nil {
		t.Fatal(err)
	}
	h, _, err = s.HarnessSession("hs-1")
	if err != nil {
		t.Fatal(err)
	}
	if h.PromptTokens != 8 || h.CompletionTokens != 2 || h.Calls != 1 {
		t.Fatalf("replacement double-counted: %+v", h)
	}
	if err := s.AppendHarnessStep(HarnessStep{SessionID: "hs-1", N: 5, Kind: "note", Content: "collapsed observations"}); err != nil {
		t.Fatal(err)
	}
	h, _, err = s.HarnessSession("hs-1")
	if err != nil {
		t.Fatal(err)
	}
	if h.Steps != 1 || h.Calls != 1 {
		t.Fatalf("note changed command/call counts: %+v", h)
	}
	if missing, missingSteps, err := s.HarnessSession("missing"); err != nil || missing != nil || missingSteps != nil {
		t.Fatalf("missing session = %+v %+v %v", missing, missingSteps, err)
	}
	if missing, err := s.HarnessSessionHead("missing"); err != nil || missing != nil {
		t.Fatalf("missing session head = %+v %v", missing, err)
	}
	if err := s.FinishHarnessSession("hs-1", "submitted", "0", "large submission", "", 1.25, "/tmp/traj.json"); err != nil {
		t.Fatal(err)
	}
	h, _, err = s.HarnessSession("hs-1")
	if err != nil {
		t.Fatal(err)
	}
	if h.Status != "submitted" || h.ExitStatus != "0" || h.Submission != "large submission" || h.Cost != 1.25 || h.Trajectory != "/tmp/traj.json" || h.EndedAt != "2026-09-11T05:00:00Z" {
		t.Fatalf("finished session: %+v", h)
	}
}

func TestRequestHarnessCancel(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-11T05:00:00Z"})
	if err := s.BeginHarnessSession(HarnessSession{ID: "running"}); err != nil {
		t.Fatal(err)
	}
	ok, err := s.RequestHarnessCancel("running")
	if err != nil || !ok {
		t.Fatalf("cancel running = %v %v", ok, err)
	}
	h, err := s.HarnessSessionHead("running")
	if err != nil {
		t.Fatal(err)
	}
	if h.CancelRequestedAt != "2026-09-11T05:00:00Z" {
		t.Fatalf("cancel timestamp = %q", h.CancelRequestedAt)
	}
	ok, err = s.RequestHarnessCancel("running")
	if !errors.Is(err, ErrHarnessCancelPending) || ok {
		t.Fatalf("cancel twice = %v %v", ok, err)
	}
	if err := s.BeginHarnessSession(HarnessSession{ID: "done"}); err != nil {
		t.Fatal(err)
	}
	if err := s.FinishHarnessSession("done", "submitted", "submitted", "SUMMARY: done", "", 0, ""); err != nil {
		t.Fatal(err)
	}
	ok, err = s.RequestHarnessCancel("done")
	if err != nil || ok {
		t.Fatalf("cancel done = %v %v", ok, err)
	}
	ok, err = s.RequestHarnessCancel("missing")
	if !errors.Is(err, sql.ErrNoRows) || ok {
		t.Fatalf("cancel missing = %v %v", ok, err)
	}
}

func TestHarnessSessionListCountsAndLost(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-11T05:00:00Z"})
	fixtures := []HarnessSession{
		{ID: "old-running", JobID: "job-old", Agent: "agent-a", Project: "proj", StartedAt: "2026-09-10T23:00:00Z", UpdatedAt: "2026-09-10T23:10:00Z"},
		{ID: "today-running", JobID: "job-2", Agent: "agent-a", Project: "proj", StartedAt: "2026-09-11T01:00:00Z", UpdatedAt: "2026-09-11T01:10:00Z"},
		{ID: "today-failed", JobID: "job-3", Agent: "agent-b", Project: "proj", StartedAt: "2026-09-11T02:00:00Z", UpdatedAt: "2026-09-11T02:10:00Z"},
		{ID: "other-project", JobID: "job-4", Agent: "agent-a", Project: "other", StartedAt: "2026-09-11T03:00:00Z", UpdatedAt: "2026-09-11T03:10:00Z"},
	}
	for _, h := range fixtures {
		if err := s.BeginHarnessSession(h); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.FinishHarnessSession("today-failed", "provider_error", "1", "", "boom", 0, ""); err != nil {
		t.Fatal(err)
	}
	list, err := s.HarnessSessions(HarnessSessionFilter{Project: "proj", Limit: 2})
	if err != nil {
		t.Fatal(err)
	}
	if got := sessionIDs(list); !reflect.DeepEqual(got, []string{"today-failed", "today-running"}) {
		t.Fatalf("project order/limit = %v", got)
	}
	list, err = s.HarnessSessions(HarnessSessionFilter{Agent: "agent-a", Status: "running", JobID: "job-2"})
	if err != nil {
		t.Fatal(err)
	}
	if got := sessionIDs(list); !reflect.DeepEqual(got, []string{"today-running"}) {
		t.Fatalf("filtered sessions = %v", got)
	}
	counts, err := s.HarnessSessionCounts()
	if err != nil {
		t.Fatal(err)
	}
	if counts.Running != 3 || counts.Today != 3 || counts.FailedToday != 1 || counts.SubmittedToday != 0 || counts.CostToday != 0 {
		t.Fatalf("counts = %+v", counts)
	}
	changed, err := s.MarkLostHarnessSessions("2026-09-11T00:00:00Z")
	if err != nil {
		t.Fatal(err)
	}
	if changed != 1 {
		t.Fatalf("lost changed = %d, want 1", changed)
	}
	lost, _, err := s.HarnessSession("old-running")
	if err != nil {
		t.Fatal(err)
	}
	if lost.Status != "lost" || lost.EndedAt != "2026-09-11T05:00:00Z" {
		t.Fatalf("lost session: %+v", lost)
	}
	counts, err = s.HarnessSessionCounts()
	if err != nil {
		t.Fatal(err)
	}
	if counts.Running != 2 || counts.Today != 3 || counts.FailedToday != 1 {
		t.Fatalf("counts after lost = %+v", counts)
	}
}

func TestHarnessSessionsBeforeAndKPIs(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-11T05:00:00Z"})
	for _, h := range []HarnessSession{
		{ID: "new", StartedAt: "2026-09-11T04:00:00Z", UpdatedAt: "2026-09-11T04:00:00Z"},
		{ID: "submitted", StartedAt: "2026-09-11T03:00:00Z", UpdatedAt: "2026-09-11T03:00:00Z"},
		{ID: "old", StartedAt: "2026-09-11T02:00:00Z", UpdatedAt: "2026-09-11T02:00:00Z"},
	} {
		if err := s.BeginHarnessSession(h); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.FinishHarnessSession("submitted", "submitted", "submitted", "SUMMARY: done", "", 3.5, ""); err != nil {
		t.Fatal(err)
	}
	page, err := s.HarnessSessions(HarnessSessionFilter{Before: "2026-09-11T04:00:00Z", Limit: 2})
	if err != nil {
		t.Fatal(err)
	}
	if got := sessionIDs(page); !reflect.DeepEqual(got, []string{"submitted", "old"}) {
		t.Fatalf("before page = %v", got)
	}
	// Same-second starts: the (started_at, id) cursor pages past siblings instead of skipping them.
	for _, id := range []string{"twin-a", "twin-b", "twin-c"} {
		if err := s.BeginHarnessSession(HarnessSession{ID: id, StartedAt: "2026-09-11T01:00:00Z", UpdatedAt: "2026-09-11T01:00:00Z"}); err != nil {
			t.Fatal(err)
		}
	}
	page, err = s.HarnessSessions(HarnessSessionFilter{Before: HarnessSessionCursor(HarnessSession{ID: "old", StartedAt: "2026-09-11T02:00:00Z"}), Limit: 2})
	if err != nil {
		t.Fatal(err)
	}
	if got := sessionIDs(page); !reflect.DeepEqual(got, []string{"twin-c", "twin-b"}) {
		t.Fatalf("twin page 1 = %v", got)
	}
	page, err = s.HarnessSessions(HarnessSessionFilter{Before: HarnessSessionCursor(page[len(page)-1]), Limit: 2})
	if err != nil {
		t.Fatal(err)
	}
	if got := sessionIDs(page); !reflect.DeepEqual(got, []string{"twin-a"}) {
		t.Fatalf("twin page 2 = %v", got)
	}
	counts, err := s.HarnessSessionCounts()
	if err != nil {
		t.Fatal(err)
	}
	if counts.Running != 5 || counts.Today != 6 || counts.FailedToday != 0 || counts.SubmittedToday != 1 || counts.CostToday != 3.5 {
		t.Fatalf("counts = %+v", counts)
	}
}

func TestHarnessTablesAreAdditive(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := t.TempDir()
	bare, err := sql.Open("sqlite", dsn(filepath.Join(home, StoreName), false))
	if err != nil {
		t.Fatal(err)
	}
	if err := initSchema(bare, "2026-09-11T00:00:00Z"); err != nil {
		t.Fatal(err)
	}
	bare.Close()
	s := openTemp(t, Options{Home: home})
	for _, table := range []string{"harness_sessions", "harness_steps"} {
		if !tableExists(t, s, table) {
			t.Fatalf("%s missing after EnsureRecords", table)
		}
	}
}

func intPtr(n int) *int { return &n }

func stepNumbers(steps []HarnessStep) []int {
	out := make([]int, len(steps))
	for i, step := range steps {
		out[i] = step.N
	}
	return out
}

func sessionIDs(sessions []HarnessSession) []string {
	out := make([]string, len(sessions))
	for i, session := range sessions {
		out[i] = session.ID
	}
	return out
}
