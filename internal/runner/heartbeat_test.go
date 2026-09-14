package runner

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func fakeHeartbeatClock(t *testing.T) (*atomic.Int64, chan time.Time, chan time.Duration) {
	t.Helper()
	oldNow, oldTicks := heartbeatNow, heartbeatTicks
	t.Cleanup(func() { heartbeatNow, heartbeatTicks = oldNow, oldTicks })
	clock := &atomic.Int64{}
	base, _ := time.Parse(time.RFC3339Nano, testNow)
	clock.Store(base.UnixNano())
	ticks, periods := make(chan time.Time, 8), make(chan time.Duration, 8)
	heartbeatNow = func(*store.Store) (time.Time, error) { return time.Unix(0, clock.Load()).UTC(), nil }
	heartbeatTicks = func(period time.Duration) (<-chan time.Time, func()) {
		periods <- period
		return ticks, func() {}
	}
	return clock, ticks, periods
}

func heartbeatReceive[T any](t *testing.T, ch <-chan T) T {
	t.Helper()
	select {
	case value := <-ch:
		return value
	case <-time.After(10 * time.Second):
		t.Fatal("supervisor did not reach the expected observation")
		var zero T
		return zero
	}
}

func heartbeatEventually(t *testing.T, ready func() bool) {
	t.Helper()
	deadline := time.NewTimer(10 * time.Second)
	defer deadline.Stop()
	tick := time.NewTicker(time.Millisecond)
	defer tick.Stop()
	for {
		if ready() {
			return
		}
		select {
		case <-deadline.C:
			t.Fatal("heartbeat evidence did not arrive")
		case <-tick.C:
		}
	}
}

func heartbeatFIFO(t *testing.T) (string, []string) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "finish")
	if err := syscall.Mkfifo(path, 0o600); err != nil {
		t.Fatal(err)
	}
	return path, []string{"sh", "-c", `read value < "$1"; printf 'SUMMARY: quiet worker finished\n'`, "worker", path}
}

func finishHeartbeatFIFO(t *testing.T, path string) {
	t.Helper()
	if err := os.WriteFile(path, []byte("finish\n"), 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestHeartbeatProcessTimingQuietWorkAndExit(t *testing.T) {
	clock, ticks, periods := fakeHeartbeatClock(t)
	fifo, argv := heartbeatFIFO(t)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	beats, stopped := make(chan int64, 8), make(chan struct{}, 1)
	observer := &processObserver{beat: func() { beats <- clock.Load() }, stop: func() { stopped <- struct{}{} }}
	type result struct {
		out outcome
		err error
	}
	done := make(chan result, 1)
	go func() {
		out, err := runGroupObserved(ctx, argv, "", environ(), nil, time.Minute, observer)
		done <- result{out, err}
	}()
	base := clock.Load()
	if got := heartbeatReceive(t, periods); got != board.HeartbeatInterval {
		t.Fatalf("cadence %s", got)
	}
	if at := heartbeatReceive(t, beats); at != base {
		t.Fatalf("initial heartbeat at %d, want %d", at, base)
	}
	for _, seconds := range []int64{60, 120} {
		clock.Store(base + int64(time.Duration(seconds)*time.Second))
		ticks <- time.Unix(0, clock.Load())
		if at := heartbeatReceive(t, beats); at != clock.Load() {
			t.Fatalf("heartbeat at %d, want t=%ds", at, seconds)
		}
	}
	finishHeartbeatFIFO(t, fifo)
	got := heartbeatReceive(t, done)
	if got.err != nil || got.out.Timeout || got.out.ExitCode != 0 || !strings.Contains(got.out.Stdout, "quiet worker finished") {
		t.Fatalf("outcome %+v, %v", got.out, got.err)
	}
	heartbeatReceive(t, stopped)
	ticks <- time.Unix(0, clock.Load()+int64(time.Minute))
	if len(beats) != 0 || len(stopped) != 0 {
		t.Fatal("heartbeat continued after exit or stopped twice")
	}
}

func TestHeartbeatProcessLaunchFailureCancellationAndTimeout(t *testing.T) {
	for _, mode := range []string{"failed launch", "cancel before launch", "cancel", "timeout"} {
		t.Run(mode, func(t *testing.T) {
			_, _, _ = fakeHeartbeatClock(t)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			beats, stops := make(chan struct{}, 4), make(chan struct{}, 4)
			observer := &processObserver{beat: func() { beats <- struct{}{} }, stop: func() { stops <- struct{}{} }}
			_, argv := heartbeatFIFO(t)
			if mode == "failed launch" {
				argv = []string{filepath.Join(t.TempDir(), "missing-worker")}
			}
			if mode == "cancel before launch" {
				cancel()
			}
			budget := time.Minute
			if mode == "timeout" {
				budget = 100 * time.Millisecond
			}
			done := make(chan outcome, 1)
			errs := make(chan error, 1)
			go func() {
				out, err := runGroupObserved(ctx, argv, "", environ(), nil, budget, observer)
				errs <- err
				done <- out
			}()
			if mode == "cancel" {
				heartbeatReceive(t, beats)
				cancel()
			}
			out, err := heartbeatReceive(t, done), heartbeatReceive(t, errs)
			if mode == "failed launch" {
				if err == nil || len(beats) != 0 || len(stops) != 0 {
					t.Fatalf("failed launch produced presence: %+v %v beats=%d stops=%d", out, err, len(beats), len(stops))
				}
			} else if mode == "cancel before launch" {
				if !out.Timeout || len(beats) != 0 || len(stops) != 0 {
					t.Fatalf("pre-cancelled child launched: %+v", out)
				}
			} else {
				if err != nil || !out.Timeout || len(stops) != 1 {
					t.Fatalf("%s: %+v %v, stops=%d", mode, out, err, len(stops))
				}
			}
		})
	}
}

func runnerHeartbeatFixture(t *testing.T, s *store.Store, executor string) (*localExec, board.ExecutionIdentity, map[string]string) {
	t.Helper()
	join(t, s, "quiet")
	job := row{"id": "job-quiet", "generation": 4}
	exec1(s.DB, `INSERT INTO jobs(id,project,kind,title,status,created_at,lease_agent,lease_until,generation)
		VALUES('job-quiet','commons','task','quiet','leased',?,'quiet','2026-09-07T10:00:00Z',4)`, testNow)
	exec1(s.DB, `INSERT INTO runs(id,job_id,agent,project,started_at,status,executor)
		VALUES('run-quiet','job-quiet','quiet','commons',?,'running',?)`, testNow, executor)
	id := board.ExecutionIdentity{Agent: "quiet", Session: "run-quiet", RunID: "run-quiet", JobID: "job-quiet", Generation: 4, Executor: executor}
	env := map[string]string{"PULSE_AGENT": id.Agent, "PULSE_SESSION": id.Session, "PULSE_JOB": id.JobID,
		"PULSE_GENERATION": strconv.Itoa(id.Generation), "PULSE_EXECUTOR": id.Executor}
	e := newLocalExec(row{"name": executor, "address": "pulse@synthetic-hands"}, s, s.Home, job, row{}, id.Agent, &runLog{})
	e.cloneDir = t.TempDir()
	// runWorker preflights the per-run signing key (ADR-022); give the fixture one.
	if err := os.MkdirAll(filepath.Dir(e.key), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(e.key, []byte("synthetic run key"), 0o600); err != nil {
		t.Fatal(err)
	}
	return e, id, env
}

func runnerHeartbeat(t *testing.T, s *store.Store, agent string) (board.ExecutionHeartbeat, bool) {
	t.Helper()
	raw, exists, err := s.Meta("agent-heartbeat:" + agent)
	if err != nil {
		t.Fatal(err)
	}
	var h board.ExecutionHeartbeat
	if exists {
		if err := json.Unmarshal([]byte(raw), &h); err != nil {
			t.Fatal(err)
		}
	}
	return h, exists
}

func TestHeartbeatLocalExecutorAndRestartFencing(t *testing.T) {
	s := newStore(t)
	clock, ticks, _ := fakeHeartbeatClock(t)
	e, id, env := runnerHeartbeatFixture(t, s, "local")
	fifo, argv := heartbeatFIFO(t)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() {
		out, err := e.runWorker(ctx, argv, env, 1)
		if err == nil && out.ExitCode != 0 {
			err = errors.New("quiet worker failed")
		}
		done <- err
	}()
	heartbeatEventually(t, func() bool { _, exists := runnerHeartbeat(t, s, id.Agent); return exists })
	base := clock.Load()
	clock.Add(int64(time.Minute))
	ticks <- time.Unix(0, clock.Load())
	heartbeatEventually(t, func() bool {
		h, _ := runnerHeartbeat(t, s, id.Agent)
		return h.At == time.Unix(0, clock.Load()).UTC().Format(time.RFC3339Nano)
	})
	finishHeartbeatFIFO(t, fifo)
	if err := heartbeatReceive(t, done); err != nil {
		t.Fatal(err)
	}
	h, _ := runnerHeartbeat(t, s, id.Agent)
	if h.ExecutionIdentity != id || h.At != time.Unix(0, base+int64(time.Minute)).UTC().Format(time.RFC3339Nano) || h.StoppedAt == "" {
		t.Fatalf("local supervision: %+v", h)
	}
	// The existing restart policy still owns lost runs/requeueing. Neither a
	// missed heartbeat nor a later old observer changes its generation fence.
	if n := sweepLostRuns(s, testNow); n != 1 {
		t.Fatalf("lost sweep: %d", n)
	}
	j := jobRow(s.DB, id.JobID)
	if j.str("status") != "queued" || j.str("lease_agent") != "" {
		t.Fatalf("existing restart policy changed: %v", j)
	}
	clock.Add(int64(time.Minute))
	e.observer(id).beat()
	after, _ := runnerHeartbeat(t, s, id.Agent)
	if after != h {
		t.Fatalf("old supervisor changed evidence after restart: %+v", after)
	}
}

func heartbeatSSHArgs(t *testing.T, argv []string) []string {
	t.Helper()
	raw, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(argv[len(argv)-1], "b64 "))
	if err != nil {
		t.Fatal(err)
	}
	return strings.Split(strings.TrimSuffix(string(raw), "\x00"), "\x00")
}

func TestHeartbeatRemoteRequiresEvidenceAndRecoversWithoutLeaseChanges(t *testing.T) {
	s := newStore(t)
	clock, ticks, periods := fakeHeartbeatClock(t)
	local, id, env := runnerHeartbeatFixture(t, s, "hands")
	e := &sshExec{localExec: local}
	oldSSH := runSSH
	t.Cleanup(func() { runSSH = oldSSH })
	type reply struct {
		code int
		raw  string
		err  error
	}
	requests, replies := make(chan struct{}, 1), make(chan reply)
	workerStarted := make(chan struct{}, 1)
	runSSH = func(ctx context.Context, argv []string, _ []byte, _ time.Duration) (int, string, string, error) {
		args := heartbeatSSHArgs(t, argv)
		switch args[0] {
		case "verify": // runWorker's remote signing-key pre-check
			return 0, "", "", nil
		case "probe":
			return 0, "hands=ok heartbeat=1", "", nil
		case "run":
			if !contains(args, "PULSE_HEARTBEAT=1") {
				return -1, "", "", errors.New("no heartbeat request")
			}
			workerStarted <- struct{}{}
			<-ctx.Done()
			return -1, "", "", ctx.Err()
		case "heartbeat":
			requests <- struct{}{}
			select {
			case r := <-replies:
				return r.code, r.raw, "", r.err
			case <-ctx.Done():
				return -1, "", "", ctx.Err()
			}
		}
		return -1, "", "", errors.New("unexpected SSH verb")
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan outcome, 1)
	errs := make(chan error, 1)
	go func() {
		out, err := e.runWorker(ctx, []string{"copilot"}, env, 1)
		errs <- err
		done <- out
	}()
	heartbeatReceive(t, workerStarted)
	if p := heartbeatReceive(t, periods); p != time.Second {
		t.Fatalf("initial observation retry: %s", p)
	}
	heartbeatReceive(t, requests)
	replies <- reply{code: 3} // SSH alive, child not acknowledged
	ticks <- time.Unix(0, clock.Load())
	heartbeatReceive(t, requests)
	if _, exists := runnerHeartbeat(t, s, id.Agent); exists {
		t.Fatal("SSH client alone fabricated an initial heartbeat")
	}
	replies <- reply{raw: `{"at":"malformed"}`}
	ticks <- time.Unix(0, clock.Load())
	heartbeatReceive(t, requests)
	if _, exists := runnerHeartbeat(t, s, id.Agent); exists {
		t.Fatal("malformed remote evidence marked the worker online")
	}
	for _, corrupt := range []func(*board.ExecutionHeartbeat){
		func(h *board.ExecutionHeartbeat) { h.At = "" },
		func(h *board.ExecutionHeartbeat) {
			h.At = time.Unix(0, clock.Load()+int64(time.Hour)).UTC().Format(time.RFC3339Nano)
		},
		func(h *board.ExecutionHeartbeat) {
			h.At = time.Unix(0, clock.Load()-int64(board.HeartbeatGrace)).UTC().Format(time.RFC3339Nano)
		},
		func(h *board.ExecutionHeartbeat) {
			h.StoppedAt = time.Unix(0, clock.Load()+int64(time.Hour)).UTC().Format(time.RFC3339Nano)
		},
		func(h *board.ExecutionHeartbeat) { h.Generation-- },
		func(h *board.ExecutionHeartbeat) { h.Session, h.RunID = "run-old", "run-old" },
		func(h *board.ExecutionHeartbeat) { h.Executor = "other-hands" },
	} {
		bad := board.ExecutionHeartbeat{ExecutionIdentity: id, At: testNow}
		corrupt(&bad)
		data, _ := json.Marshal(bad)
		replies <- reply{raw: string(data)}
		ticks <- time.Unix(0, clock.Load())
		heartbeatReceive(t, requests)
		if _, exists := runnerHeartbeat(t, s, id.Agent); exists {
			t.Fatalf("invalid remote evidence was accepted: %+v", bad)
		}
	}
	sendBeat := func() {
		h := board.ExecutionHeartbeat{ExecutionIdentity: id, At: time.Unix(0, clock.Load()).UTC().Format(time.RFC3339Nano)}
		data, _ := json.Marshal(h)
		replies <- reply{raw: string(data)}
		heartbeatEventually(t, func() bool {
			got, exists := runnerHeartbeat(t, s, id.Agent)
			return exists && got.At == h.At
		})
	}
	sendBeat()
	if p := heartbeatReceive(t, periods); p != time.Minute {
		t.Fatalf("acknowledged observation cadence: %s", p)
	}
	clock.Add(int64(time.Minute))
	ticks <- time.Unix(0, clock.Load())
	heartbeatReceive(t, requests)
	sendBeat()
	jobBefore := jobRow(s.DB, id.JobID)
	agentBefore, _ := s.Agent(id.Agent)
	ticks <- time.Unix(0, clock.Load())
	heartbeatReceive(t, requests)
	replies <- reply{err: errors.New("synthetic evidence connection lost")}
	clock.Add(int64(board.HeartbeatGrace - time.Nanosecond))
	check := func(want string) {
		rows, err := board.AgentPresences(s, time.Unix(0, clock.Load()).UTC().Format(time.RFC3339Nano))
		if err != nil || rows[id.Agent].Presence != want {
			t.Fatalf("remote presence: %+v %v; want %s", rows[id.Agent], err, want)
		}
	}
	check("online")
	clock.Add(1)
	check("offline")
	if j := jobRow(s.DB, id.JobID); j.str("status") != jobBefore.str("status") || j.int("generation") != jobBefore.int("generation") || j.str("lease_until") != jobBefore.str("lease_until") {
		t.Fatalf("remote evidence loss changed lease: %v", j)
	}
	ticks <- time.Unix(0, clock.Load())
	heartbeatReceive(t, requests)
	sendBeat()
	check("online")
	agentAfter, _ := s.Agent(id.Agent)
	if agentAfter.LastActiveAt != agentBefore.LastActiveAt || agentAfter.LastUsefulAt != agentBefore.LastUsefulAt || agentAfter.Posts != agentBefore.Posts {
		t.Fatal("remote heartbeat counted as useful work")
	}
	cancel()
	if out, err := heartbeatReceive(t, done), heartbeatReceive(t, errs); err != nil || !out.Timeout {
		t.Fatalf("remote cancellation: %+v %v", out, err)
	}
	check("offline")
	h, _ := runnerHeartbeat(t, s, id.Agent)
	if h.StoppedAt == "" {
		t.Fatal("remote observer continued after cancellation")
	}
}

func TestHeartbeatLegacyHandsAndProbeFailuresDoNotFailWork(t *testing.T) {
	for _, mode := range []string{"legacy", "probe error", "probe exit"} {
		t.Run(mode, func(t *testing.T) {
			s := newStore(t)
			local, id, env := runnerHeartbeatFixture(t, s, "hands")
			e := &sshExec{localExec: local}
			oldSSH := runSSH
			t.Cleanup(func() { runSSH = oldSSH })
			runSSH = func(_ context.Context, argv []string, _ []byte, _ time.Duration) (int, string, string, error) {
				args := heartbeatSSHArgs(t, argv)
				if args[0] == "verify" { // runWorker's remote signing-key pre-check
					return 0, "", "", nil
				}
				if args[0] == "probe" {
					switch mode {
					case "probe error":
						return -1, "", "", errors.New("synthetic probe failure")
					case "probe exit":
						return 2, "", "", nil
					default:
						return 0, "hands=ok", "", nil
					}
				}
				if args[0] != "run" || contains(args, "PULSE_HEARTBEAT=1") {
					return -1, "", "", errors.New("legacy hands received a heartbeat request")
				}
				return 0, "SUMMARY: completed", "", nil
			}
			out, err := e.runWorker(context.Background(), []string{"copilot"}, env, 1)
			if err != nil || out.ExitCode != 0 || !strings.Contains(out.Stdout, "completed") || e.lastOut == nil || *e.lastOut != out.Stdout {
				t.Fatalf("presence capability affected execution: %+v %v", out, err)
			}
			if _, exists := runnerHeartbeat(t, s, id.Agent); exists {
				t.Fatal("legacy hands fabricated presence")
			}
			if !strings.Contains(e.log.text(), "presence unavailable") {
				t.Fatal("capability failure was hidden")
			}
		})
	}
}

func TestHeartbeatRemoteStopsOnExitTimeoutAndTransportLoss(t *testing.T) {
	for _, tc := range []struct {
		name    string
		code    int
		failure bool
		timeout bool
	}{
		{"exit", 0, false, false},
		{"timeout", remoteExitTimeout, false, true},
		{"remote loss", sshExitUnreach, true, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := newStore(t)
			fakeHeartbeatClock(t)
			local, id, env := runnerHeartbeatFixture(t, s, "hands")
			e := &sshExec{localExec: local}
			oldSSH := runSSH
			t.Cleanup(func() { runSSH = oldSSH })
			end := make(chan struct{})
			runSSH = func(ctx context.Context, argv []string, _ []byte, _ time.Duration) (int, string, string, error) {
				args := heartbeatSSHArgs(t, argv)
				switch args[0] {
				case "verify": // runWorker's remote signing-key pre-check
					return 0, "", "", nil
				case "probe":
					return 0, "hands=ok heartbeat=1", "", nil
				case "heartbeat":
					data, _ := json.Marshal(board.ExecutionHeartbeat{ExecutionIdentity: id, At: testNow})
					return 0, string(data), "", nil
				case "run":
					select {
					case <-end:
						return tc.code, "SUMMARY: finished", "synthetic transport", nil
					case <-ctx.Done():
						return -1, "", "", ctx.Err()
					}
				}
				return -1, "", "", errors.New("unexpected SSH verb")
			}
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			done, errs := make(chan outcome, 1), make(chan error, 1)
			go func() {
				out, err := e.runWorker(ctx, []string{"copilot"}, env, 1)
				errs <- err
				done <- out
			}()
			heartbeatEventually(t, func() bool { _, exists := runnerHeartbeat(t, s, id.Agent); return exists })
			close(end)
			out, err := heartbeatReceive(t, done), heartbeatReceive(t, errs)
			if (err != nil) != tc.failure || out.Timeout != tc.timeout {
				t.Fatalf("%s: %+v %v", tc.name, out, err)
			}
			beat, _ := runnerHeartbeat(t, s, id.Agent)
			if beat.At != testNow || beat.StoppedAt == "" {
				t.Fatalf("observations survived %s: %+v", tc.name, beat)
			}
			j := jobRow(s.DB, id.JobID)
			if j.str("status") != "leased" || j.int("generation") != id.Generation || j.str("lease_agent") != id.Agent {
				t.Fatalf("presence took over lease policy: %v", j)
			}
		})
	}
}

func TestHeartbeatHandsFailedLaunchAndTimeoutSnapshots(t *testing.T) {
	for _, timeout := range []bool{false, true} {
		t.Run(strconv.FormatBool(timeout), func(t *testing.T) {
			fakeHeartbeatClock(t)
			workdir := t.TempDir()
			dir := filepath.Join(workdir, "job-quiet")
			if err := os.MkdirAll(dir, 0o755); err != nil {
				t.Fatal(err)
			}
			var stdout, stderr bytes.Buffer
			h := newTestHands(t, workdir, &stdout, &stderr)
			env := map[string]string{"PULSE_AGENT": "quiet", "PULSE_SESSION": "run-quiet", "PULSE_GENERATION": "1", "PULSE_EXECUTOR": "hands"}
			observer, err := h.heartbeatObserver("job-quiet", env)
			if err != nil {
				t.Fatal(err)
			}
			argv := []string{filepath.Join(dir, "missing-command")}
			want := 127
			if timeout {
				_, argv = heartbeatFIFO(t)
				want = remoteExitTimeout
			}
			if got := h.withTimeoutObserved(100*time.Millisecond, dir, argv, env, observer); got != want {
				t.Fatalf("hands result %d, want %d", got, want)
			}
			path, _ := handsHeartbeatPath(dir, "run-quiet")
			data, err := os.ReadFile(path)
			if !timeout {
				if !os.IsNotExist(err) {
					t.Fatalf("failed launch acquired a heartbeat: %s %v", data, err)
				}
				return
			}
			var beat board.ExecutionHeartbeat
			if err != nil || json.Unmarshal(data, &beat) != nil || beat.At == "" || beat.StoppedAt == "" {
				t.Fatalf("hands timeout retained live evidence: %s %v", data, err)
			}
		})
	}
}

func TestHeartbeatHandsSupervisorSnapshotsAndSessionIsolation(t *testing.T) {
	_, ticks, periods := fakeHeartbeatClock(t)
	workdir := t.TempDir()
	dir := filepath.Join(workdir, "job-quiet")
	clone := filepath.Join(dir, "clone")
	if err := os.MkdirAll(filepath.Join(clone, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PULSE_HANDS_ALLOW_ANY_WORKER", "1")
	var stdout, stderr bytes.Buffer
	h := newTestHands(t, workdir, &stdout, &stderr)
	id := board.ExecutionIdentity{Agent: "quiet", Session: "run-quiet", RunID: "run-quiet", JobID: "job-quiet", Generation: 4, Executor: "hands"}
	env := map[string]string{"PULSE_AGENT": id.Agent, "PULSE_SESSION": id.Session, "PULSE_GENERATION": "4", "PULSE_EXECUTOR": "hands"}
	observer, err := h.heartbeatObserver(id.JobID, env)
	if err != nil {
		t.Fatal(err)
	}
	observed := make(chan struct{}, 8)
	original := observer.beat
	observer.beat = func() { original(); observed <- struct{}{} }
	fifo, argv := heartbeatFIFO(t)
	done := make(chan int, 1)
	go func() { done <- h.withTimeoutObserved(time.Minute, clone, argv, env, observer) }()
	if p := heartbeatReceive(t, periods); p != time.Minute {
		t.Fatalf("hands cadence: %s", p)
	}
	heartbeatReceive(t, observed)
	read := func(session string) (board.ExecutionHeartbeat, error) {
		var out bytes.Buffer
		reader := newTestHands(t, workdir, &out, nil)
		err := reader.heartbeat(id.JobID, session)
		var beat board.ExecutionHeartbeat
		if err == nil {
			err = json.Unmarshal(out.Bytes(), &beat)
		}
		return beat, err
	}
	first, err := read(id.Session)
	if err != nil || first.ExecutionIdentity != id || first.At == "" || first.StoppedAt != "" {
		t.Fatalf("initial hands evidence: %+v %v", first, err)
	}
	ticks <- time.Now()
	heartbeatReceive(t, observed)
	second, err := read(id.Session)
	firstAt, _ := time.Parse(time.RFC3339Nano, first.At)
	secondAt, _ := time.Parse(time.RFC3339Nano, second.At)
	if err != nil || secondAt.Before(firstAt) {
		t.Fatalf("hands refresh: %+v %v", second, err)
	}
	if _, err := read("old-session"); err == nil {
		t.Fatal("another session obtained this execution's evidence")
	}
	finishHeartbeatFIFO(t, fifo)
	if code := heartbeatReceive(t, done); code != 0 {
		t.Fatalf("hands exit %d: %s", code, stderr.String())
	}
	last, err := read(id.Session)
	if err != nil || last.At != second.At || last.StoppedAt == "" {
		t.Fatalf("hands stop fabricated or lost evidence: %+v %v", last, err)
	}
	if err := h.heartbeat(id.JobID, "../outside"); err == nil {
		t.Fatal("heartbeat escaped the job boundary")
	}
	path, _ := handsHeartbeatPath(dir, "malformed")
	if err := os.WriteFile(path, []byte(`{"at":"bad"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := read("malformed"); err == nil {
		t.Fatal("malformed snapshot was acknowledged")
	}
	if err := h.clean(id.JobID); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatalf("heartbeat outlived ordinary cleanup: %v", err)
	}
}

func TestHeartbeatSSHProcessCancellationIsBounded(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	fifo, _ := heartbeatFIFO(t)
	ready := filepath.Join(t.TempDir(), "ready")
	argv := []string{"sh", "-c", `printf ready > "$1"; read value < "$2"`, "client", ready, fifo}
	done := make(chan error, 1)
	go func() {
		_, _, _, err := runProcess(ctx, argv, nil, time.Minute)
		done <- err
	}()
	heartbeatEventually(t, func() bool {
		_, err := os.Stat(ready)
		return err == nil
	})
	cancel()
	if err := heartbeatReceive(t, done); !errors.Is(err, context.Canceled) {
		t.Fatalf("SSH process ignored cancellation: %v", err)
	}
}

// sshExec.runWorker applies the signing-key pre-check itself: when the host's
// `test -s run.key` fails, the run ends with a setup-error and no `run` verb
// is ever issued, so the failure lands at setup rather than at commit time.
func TestSSHRunWorkerFailsAtSetupWhenRemoteKeyMissing(t *testing.T) {
	s := newStore(t)
	local, _, env := runnerHeartbeatFixture(t, s, "hands")
	e := &sshExec{localExec: local}
	oldSSH := runSSH
	t.Cleanup(func() { runSSH = oldSSH })
	ran := false
	runSSH = func(_ context.Context, argv []string, _ []byte, _ time.Duration) (int, string, string, error) {
		args := heartbeatSSHArgs(t, argv)
		switch args[0] {
		case "verify":
			return 1, "", "test: run.key: No such file", nil
		case "run":
			ran = true
		}
		return 0, "", "", nil
	}
	_, err := e.runWorker(context.Background(), []string{"copilot"}, env, 1)
	if err == nil || !strings.Contains(err.Error(), "setup-error") || !strings.Contains(err.Error(), "before the worker starts") {
		t.Fatalf("expected setup-error from the remote key pre-check, got %v", err)
	}
	if ran {
		t.Fatal("worker was started although the remote signing key was missing")
	}
}
