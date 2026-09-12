package harness

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
)

func TestUnsuccessfulCommandCannotSubmit(t *testing.T) {
	for _, obs := range []Observation{
		{Output: "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT\nSUMMARY: prematurely done", ExitCode: 1},
		{Output: "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT\nSUMMARY: prematurely done", ExitCode: 0, TimedOut: true, Duration: time.Second},
	} {
		p := &fakeProvider{responses: []provider.Response{
			{Message: provider.Message{Content: "```bash\nrun-check\n```"}},
			{Message: provider.Message{Content: "```bash\nrepair-and-check\n```"}},
			{Message: provider.Message{Content: "SUMMARY: repaired and checked"}},
		}}
		env := &fakeEnv{observations: []Observation{obs, {Output: "check passed", ExitCode: 0}}}
		h := New(p, env, baseConfig())
		got, err := h.Run(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		if got.ExitStatus != ExitSubmitted || got.Submission != "SUMMARY: repaired and checked" || got.Calls != 3 ||
			len(env.commands) != 2 || env.commands[1] != "repair-and-check" {
			t.Fatalf("failed command suppressed the recovery turn: result=%+v commands=%v", got, env.commands)
		}
		if !strings.Contains(h.Messages()[3].Content, obs.Output) {
			t.Fatal("recovery discarded the original failure evidence")
		}
	}
}

func TestRecoveryGuidancePreservesLimitsAndPolicy(t *testing.T) {
	prompt := DefaultSystemPrompt((&fakeEnv{}).Info(), baseConfig())
	for _, want := range []string{"inspect the cause", "peer-help channel", "do not repeat an unchanged failure",
		"Never bypass a policy refusal", "SUMMARY: blocked:", "SUMMARY: failed:"} {
		if !strings.Contains(prompt, want) {
			t.Errorf("missing recovery guidance %q", want)
		}
	}
	out := formatObservation(Observation{Output: "partial failure", ExitCode: 124, TimedOut: true, Duration: time.Second}, 200)
	for _, want := range []string{"partial failure", "<returncode>124</returncode>", "remaining budget", "Do not leave unsupervised background work"} {
		if !strings.Contains(out, want) {
			t.Errorf("timeout recovery lost %q: %s", want, out)
		}
	}
}
