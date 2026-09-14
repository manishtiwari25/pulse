package runner

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func TestHandsStepsVerbStreamsWholeLinesByOffset(t *testing.T) {
	workdir := t.TempDir()
	dir := filepath.Join(workdir, "job-steps")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	h := newTestHands(t, workdir, nil, nil)
	if err := h.steps("job-steps", "run-steps", "0"); err == nil {
		t.Fatal("missing steps file returned nil")
	} else {
		var exit *handsExit
		if !errors.As(err, &exit) || exit.code != 3 {
			t.Fatalf("missing steps err = %v", err)
		}
	}
	path, err := handsStepsPath(dir, "run-steps")
	if err != nil {
		t.Fatal(err)
	}
	first := strings.Repeat("a", 1024*1024-1) + "\n"
	if err := os.WriteFile(path, []byte(first+"b\npartial"), 0o600); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	h.stdout = &out
	if err := h.steps("job-steps", "run-steps", "0"); err != nil {
		t.Fatal(err)
	}
	if got := out.String(); got != first {
		t.Fatalf("first chunk len=%d want %d, suffix %q", len(got), len(first), got[len(got)-min(10, len(got)):])
	}
	out.Reset()
	if err := h.steps("job-steps", "run-steps", strconv.Itoa(len(first))); err != nil {
		t.Fatal(err)
	}
	if got := out.String(); got != "b\n" {
		t.Fatalf("offset chunk = %q", got)
	}
	for _, session := range []string{"../run", strings.Repeat("x", 161)} {
		if err := h.steps("job-steps", session, "0"); err == nil {
			t.Fatalf("bad session %q accepted", session)
		}
	}
	if err := h.steps("job-steps", "run-steps", "-1"); err == nil {
		t.Fatal("negative offset accepted")
	}
}

func TestRemoteStepsMirrorAppliesSplitLinesAndMissingFinish(t *testing.T) {
	s := newStore(t)
	e := &sshExec{localExec: &localExec{
		row: row{"name": "remote-one", "address": "ssh://remote-one"},
		s:   s, job: row{"id": "job-steps"}, project: row{"slug": "proj"}, log: &runLog{},
	}}
	m := newRemoteStepsMirror(e, "run-steps")
	begin := `{"event":"begin","session":"run-steps","run_id":"run-steps","job_id":"job-steps","agent":"agent-one","provider":"fake","model":"m1","cwd":"/work","at":"2026-09-11T07:00:00Z"}`
	step := `{"event":"step","n":1,"at":"2026-09-11T07:00:01Z","kind":"assistant","content":"hello","usage":{"prompt_tokens":3,"completion_tokens":5,"cost":0.2},"model":"m1"}`
	if err := m.apply([]byte(begin + "\n" + step[:len(step)/2])); err != nil {
		t.Fatal(err)
	}
	if err := m.apply([]byte(step[len(step)/2:] + "\n")); err != nil {
		t.Fatal(err)
	}
	m.finishIfMissing(outcome{ExitCode: 0}, nil)
	session, steps, err := s.HarnessSession("run-steps")
	if err != nil {
		t.Fatal(err)
	}
	if session == nil || session.Executor != "remote-one" || session.JobID != "job-steps" ||
		session.Agent != "agent-one" || session.Status != "lost" || session.ExitStatus != "provider_error" ||
		session.Error != "worker ended without a finish line" {
		t.Fatalf("session = %+v", session)
	}
	if len(steps) != 1 || steps[0].Kind != "assistant" || steps[0].PromptTokens != 3 || steps[0].CompletionTokens != 5 {
		t.Fatalf("steps = %+v", steps)
	}
}

func TestRemoteStepsMirrorFinishLine(t *testing.T) {
	s := newStore(t)
	e := &sshExec{localExec: &localExec{
		row: row{"name": "remote-one"},
		s:   s, job: row{"id": "job-steps"}, project: row{"slug": "proj"}, log: &runLog{},
	}}
	m := newRemoteStepsMirror(e, "run-steps")
	lines := []string{
		`{"event":"begin","session":"run-steps","run_id":"run-steps","job_id":"job-steps","agent":"agent-one","provider":"fake","model":"m1","cwd":"/work","at":"2026-09-11T07:00:00Z"}`,
		`{"event":"finish","exit_status":"submitted","submission":"SUMMARY: ok","error":"","cost":0.75,"steps":1,"at":"2026-09-11T07:00:02Z"}`,
	}
	if err := m.apply([]byte(strings.Join(lines, "\n") + "\n")); err != nil {
		t.Fatal(err)
	}
	session, _, err := s.HarnessSession("run-steps")
	if err != nil {
		t.Fatal(err)
	}
	if session == nil || session.Status != "submitted" || session.ExitStatus != "submitted" ||
		session.Submission != "SUMMARY: ok" || session.Cost != 0.75 {
		t.Fatalf("session = %+v", session)
	}
}

func TestProbeAdvertisesSteps(t *testing.T) {
	workdir := t.TempDir()
	var out bytes.Buffer
	h := newTestHands(t, workdir, &out, nil)
	if err := h.probe(); err != nil {
		t.Fatal(err)
	}
	fields := parseProbe(out.String())
	if fields["heartbeat"] != "1" || fields["steps"] != "1" {
		t.Fatalf("probe fields = %#v", fields)
	}
}

func TestRemoteStepsMirrorInvalidJSONStops(t *testing.T) {
	s := newStore(t)
	e := &sshExec{localExec: &localExec{s: s, job: row{"id": "job-steps"}, project: row{"slug": "proj"}, log: &runLog{}}}
	m := newRemoteStepsMirror(e, "run-steps")
	if err := m.apply([]byte("{bad}\n")); err == nil {
		t.Fatal("invalid JSON accepted")
	}
}
