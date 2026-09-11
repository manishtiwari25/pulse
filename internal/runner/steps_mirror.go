package runner

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
	"github.com/manishtiwari25/pulse/internal/store"
)

type remoteStepsMirror struct {
	e        *sshExec
	id       string
	offset   int64
	pending  []byte
	begun    bool
	finished bool
	reported bool
	stopped  bool
}

type remoteStepsBegin struct {
	Event    string `json:"event"`
	Session  string `json:"session"`
	RunID    string `json:"run_id"`
	JobID    string `json:"job_id"`
	Agent    string `json:"agent"`
	Provider string `json:"provider"`
	Model    string `json:"model"`
	Cwd      string `json:"cwd"`
	At       string `json:"at"`
}

type remoteStepsStep struct {
	Event   string         `json:"event"`
	N       int            `json:"n"`
	At      time.Time      `json:"at"`
	Kind    string         `json:"kind"`
	Content string         `json:"content"`
	Command string         `json:"command"`
	Exit    *int           `json:"exit_code"`
	MS      int64          `json:"duration_ms"`
	Usage   provider.Usage `json:"usage"`
	Model   string         `json:"model"`
}

type remoteStepsFinish struct {
	Event      string  `json:"event"`
	ExitStatus string  `json:"exit_status"`
	Submission string  `json:"submission"`
	Error      string  `json:"error"`
	Cost       float64 `json:"cost"`
	Steps      int     `json:"steps"`
	At         string  `json:"at"`
}

func newRemoteStepsMirror(e *sshExec, id string) *remoteStepsMirror {
	return &remoteStepsMirror{e: e, id: id}
}

func (m *remoteStepsMirror) watch(ctx context.Context) {
	period := time.Second
	ticks, stop := heartbeatTicks(period)
	defer stop()
	for {
		if ctx.Err() != nil || m.stopped {
			return
		}
		if m.fetch(ctx) && period != heartbeatInterval {
			stop()
			period = heartbeatInterval
			ticks, stop = heartbeatTicks(period)
		}
		select {
		case <-ctx.Done():
			return
		case <-ticks:
		}
	}
}

func (m *remoteStepsMirror) fetch(ctx context.Context) bool {
	if m.stopped {
		return false
	}
	rc, raw, errText, err := sshRunContext(ctx, m.e.row, []string{"steps", m.e.job.str("id"), m.id, strconv.FormatInt(m.offset, 10)}, nil, 10*time.Second)
	if ctx.Err() != nil {
		return false
	}
	if err != nil {
		m.unavailable(err)
		return false
	}
	if rc == 3 {
		return false
	}
	if rc != 0 {
		if errText == "" {
			errText = raw
		}
		m.unavailable(fmt.Errorf("hands steps unavailable (exit %d): %s", rc, tail(strings.Join(strings.Fields(errText), " "), 300)))
		return false
	}
	if raw == "" {
		return false
	}
	m.offset += int64(len([]byte(raw)))
	if err := m.apply([]byte(raw)); err != nil {
		m.unavailable(err)
		return false
	}
	return true
}

func (m *remoteStepsMirror) apply(chunk []byte) error {
	m.pending = append(m.pending, chunk...)
	for {
		i := bytes.IndexByte(m.pending, '\n')
		if i < 0 {
			return nil
		}
		line := bytes.TrimSpace(m.pending[:i])
		m.pending = m.pending[i+1:]
		if len(line) == 0 {
			continue
		}
		var head struct {
			Event string `json:"event"`
		}
		if err := json.Unmarshal(line, &head); err != nil {
			return fmt.Errorf("invalid harness steps line: %w", err)
		}
		switch head.Event {
		case "begin":
			var ev remoteStepsBegin
			if err := json.Unmarshal(line, &ev); err != nil {
				return fmt.Errorf("invalid harness begin line: %w", err)
			}
			// Steps are keyed by the run's session id whatever the remote harness called its session,
			// so the console finds one session per run.
			if ev.RunID == "" {
				ev.RunID = m.id
			}
			if err := m.e.s.BeginHarnessSession(store.HarnessSession{
				ID: m.id, RunID: ev.RunID, JobID: ev.JobID, Agent: ev.Agent, Project: m.e.project.str("slug"),
				Executor: m.e.name(), Provider: ev.Provider, Model: ev.Model, Cwd: ev.Cwd, Status: "running",
				StartedAt: ev.At, UpdatedAt: ev.At,
			}); err != nil {
				return err
			}
			m.begun = true
		case "step":
			var ev remoteStepsStep
			if err := json.Unmarshal(line, &ev); err != nil {
				return fmt.Errorf("invalid harness step line: %w", err)
			}
			at := ""
			if !ev.At.IsZero() {
				at = ev.At.UTC().Format(time.RFC3339)
			}
			if err := m.e.s.AppendHarnessStep(store.HarnessStep{
				SessionID: m.id, N: ev.N, At: at, Kind: ev.Kind, Content: ev.Content, Command: ev.Command,
				ExitCode: ev.Exit, DurationMS: ev.MS, PromptTokens: ev.Usage.PromptTokens,
				CompletionTokens: ev.Usage.CompletionTokens, Model: ev.Model,
			}); err != nil {
				return err
			}
		case "finish":
			var ev remoteStepsFinish
			if err := json.Unmarshal(line, &ev); err != nil {
				return fmt.Errorf("invalid harness finish line: %w", err)
			}
			if err := m.e.s.FinishHarnessSession(m.id, ev.ExitStatus, ev.ExitStatus, ev.Submission, ev.Error, ev.Cost, ""); err != nil {
				return err
			}
			m.finished = true
		default:
			return fmt.Errorf("unknown harness steps event %q", head.Event)
		}
	}
}

func (m *remoteStepsMirror) finishIfMissing(out outcome, runErr error) {
	if !m.begun || m.finished {
		return
	}
	exitStatus := "provider_error"
	if out.Timeout {
		exitStatus = "time_limit"
	} else if errors.Is(runErr, context.Canceled) || errors.Is(runErr, context.DeadlineExceeded) {
		exitStatus = "cancelled"
	}
	if err := m.e.s.FinishHarnessSession(m.id, "lost", exitStatus, "", "worker ended without a finish line", 0, ""); err != nil {
		m.unavailable(err)
	}
}

func (m *remoteStepsMirror) unavailable(err error) {
	if err == nil || m.reported {
		return
	}
	m.reported = true
	m.stopped = true
	m.e.log.add("harness steps unavailable: %v", err)
}
