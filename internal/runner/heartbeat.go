package runner

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

const heartbeatInterval = board.HeartbeatInterval

var heartbeatTicks = func(period time.Duration) (<-chan time.Time, func()) {
	ticker := time.NewTicker(period)
	return ticker.C, ticker.Stop
}

// The store clock supports --now and the existing deterministic test stores.
var heartbeatNow = func(s *store.Store) (time.Time, error) {
	return time.Parse(time.RFC3339Nano, s.Now())
}

func (e *localExec) execution(env map[string]string) board.ExecutionIdentity {
	return board.ExecutionIdentity{Agent: e.agent, Session: env["PULSE_SESSION"], RunID: env["PULSE_SESSION"],
		JobID: e.job.str("id"), Generation: e.job.int("generation"), Executor: e.name()}
}

// Presence failures are visible but never turn an otherwise valid worker result
// into a failed job, or a heartbeat failure into a lease/retirement action.
func heartbeatReporter(log *runLog) func(error) {
	reported := false
	return func(err error) {
		if err != nil && !reported {
			log.add("presence unavailable: %v", err)
			reported = true
		}
	}
}

func (e *localExec) observer(id board.ExecutionIdentity) *processObserver {
	report := heartbeatReporter(e.log)
	if err := id.Validate(); err != nil {
		report(err)
		return nil
	}
	return &processObserver{
		beat: func() {
			now, err := heartbeatNow(e.s)
			if err == nil {
				_, err = board.RecordHeartbeat(e.s, board.ExecutionHeartbeat{ExecutionIdentity: id, At: now.UTC().Format(time.RFC3339Nano)})
			}
			report(err)
		},
		stop: func() { e.stopHeartbeat(id, report) },
	}
}

func (e *localExec) stopHeartbeat(id board.ExecutionIdentity, report func(error)) {
	now, err := heartbeatNow(e.s)
	if err == nil {
		_, err = board.StopHeartbeat(e.s, id, now)
	}
	report(err)
}

func (e *sshExec) heartbeatCapable(ctx context.Context) bool {
	fields := e.probeCapabilities(ctx)
	return fields != nil && e.heartbeatCapableFrom(fields)
}

func (e *sshExec) probeCapabilities(ctx context.Context) map[string]string {
	rc, out, _, err := sshRunContext(ctx, e.row, []string{"probe"}, nil, 10*time.Second)
	if err != nil || rc != 0 {
		e.log.add("presence unavailable: hands capability probe failed (exit %d): %v", rc, err)
		return nil
	}
	return parseProbe(out)
}

func (e *sshExec) heartbeatCapableFrom(fields map[string]string) bool {
	if fields["heartbeat"] != "1" {
		e.log.add("presence unavailable: these hands do not acknowledge supervised heartbeats")
		return false
	}
	return true
}

func stepsCapable(fields map[string]string) bool {
	return fields["steps"] == "1"
}

// The SSH client being alive is not evidence. Read only snapshots written by
// the existing hands child supervisor, with the exact run and lease provenance.
func (e *sshExec) watchRemote(ctx context.Context, id board.ExecutionIdentity) {
	report := heartbeatReporter(e.log)
	period := time.Second // the first snapshot may not exist until the child starts
	ticks, stop := heartbeatTicks(period)
	defer func() { stop() }()
	for {
		if ctx.Err() != nil {
			return
		}
		rc, raw, _, err := sshRunContext(ctx, e.row, []string{"heartbeat", id.JobID, id.Session}, nil, 10*time.Second)
		if ctx.Err() != nil {
			return
		}
		if err == nil && rc == 0 {
			var h board.ExecutionHeartbeat
			err = json.Unmarshal([]byte(raw), &h)
			if err == nil {
				err = h.Validate()
			}
			now, clockErr := heartbeatNow(e.s)
			if err == nil {
				err = clockErr
			}
			if err == nil {
				at, _ := time.Parse(time.RFC3339Nano, h.At)
				if h.ExecutionIdentity != id || at.After(now) || now.Sub(at) >= board.HeartbeatGrace {
					err = fmt.Errorf("hands heartbeat is stale or belongs to another execution")
				} else if h.StoppedAt != "" {
					stopped, _ := time.Parse(time.RFC3339Nano, h.StoppedAt)
					if stopped.After(now) {
						err = fmt.Errorf("hands heartbeat has a future stop time")
					} else {
						e.stopHeartbeat(id, report)
						return
					}
				} else {
					_, err = board.RecordHeartbeat(e.s, h)
					if err == nil && period != heartbeatInterval {
						stop()
						period = heartbeatInterval
						ticks, stop = heartbeatTicks(period)
					}
				}
			}
		} else if err == nil && rc != 3 {
			err = fmt.Errorf("hands heartbeat unavailable (exit %d)", rc)
		}
		report(err)
		select {
		case <-ctx.Done():
			return
		case <-ticks:
		}
	}
}

func handsHeartbeatPath(dir, session string) (string, error) {
	if !handsJobRe.MatchString(session) || session == "." || session == ".." || len(session) > 160 {
		return "", handsDie("bad heartbeat session")
	}
	return filepath.Join(dir, "presence-"+session+".json"), nil
}

func handsStepsPath(dir, session string) (string, error) {
	if !handsJobRe.MatchString(session) || session == "." || session == ".." || len(session) > 160 {
		return "", handsDie("bad steps session")
	}
	return filepath.Join(dir, "harness-steps-"+session+".jsonl"), nil
}

// A session-specific, atomically replaced snapshot stays outside the clone and
// is removed by ordinary job cleanup. Old hands processes cannot overwrite a
// newer session's evidence, even if the same job is dispatched again.
func writeHandsHeartbeat(path string, h board.ExecutionHeartbeat) error {
	data, err := json.Marshal(h)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), ".presence-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(f.Name(), path)
}

func (h *hands) heartbeatObserver(job string, env map[string]string) (*processObserver, error) {
	generation, err := strconv.Atoi(env["PULSE_GENERATION"])
	if err != nil {
		return nil, handsDie("bad heartbeat generation")
	}
	id := board.ExecutionIdentity{Agent: env["PULSE_AGENT"], Session: env["PULSE_SESSION"], RunID: env["PULSE_SESSION"],
		JobID: job, Generation: generation, Executor: env["PULSE_EXECUTOR"]}
	if err := id.Validate(); err != nil {
		return nil, err
	}
	dir, err := h.jobDir(job)
	if err != nil {
		return nil, err
	}
	path, err := handsHeartbeatPath(dir, id.Session)
	if err != nil {
		return nil, err
	}
	beat := board.ExecutionHeartbeat{ExecutionIdentity: id}
	reported := false
	write := func() {
		if err := writeHandsHeartbeat(path, beat); err != nil && !reported {
			fmt.Fprintf(h.stderr, "pulse-hands: presence unavailable: %v\n", err)
			reported = true
		}
	}
	return &processObserver{
		beat: func() {
			beat.At = time.Now().UTC().Format(time.RFC3339Nano)
			write()
		},
		stop: func() {
			if beat.At != "" {
				beat.StoppedAt = time.Now().UTC().Format(time.RFC3339Nano)
				write()
			}
		},
	}, nil
}

func (h *hands) heartbeat(job, session string) error {
	dir, err := h.jobDir(job)
	if err != nil {
		return err
	}
	path, err := handsHeartbeatPath(dir, session)
	if err != nil {
		return err
	}
	f, err := os.Open(path)
	if os.IsNotExist(err) {
		return &handsExit{3}
	}
	if err != nil {
		return err
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, 4097))
	if err != nil {
		return err
	}
	var beat board.ExecutionHeartbeat
	if len(data) > 4096 || json.Unmarshal(data, &beat) != nil || beat.Validate() != nil ||
		beat.JobID != job || beat.Session != session {
		return handsDie("invalid heartbeat evidence")
	}
	_, err = h.stdout.Write(data)
	return err
}

func (h *hands) steps(job, session, offsetText string) error {
	dir, err := h.jobDir(job)
	if err != nil {
		return err
	}
	path, err := handsStepsPath(dir, session)
	if err != nil {
		return err
	}
	offset, err := strconv.ParseInt(offsetText, 10, 64)
	if err != nil || offset < 0 {
		return handsDie("bad steps offset")
	}
	f, err := os.Open(path)
	if os.IsNotExist(err) {
		return &handsExit{3}
	}
	if err != nil {
		return err
	}
	defer f.Close()
	if _, err := f.Seek(offset, io.SeekStart); err != nil {
		return err
	}
	data, err := io.ReadAll(io.LimitReader(f, 1024*1024))
	if err != nil {
		return err
	}
	if i := bytes.LastIndexByte(data, '\n'); i >= 0 {
		data = data[:i+1]
	} else {
		data = nil
	}
	_, err = h.stdout.Write(data)
	return err
}
