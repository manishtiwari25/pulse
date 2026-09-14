package runner

import (
	"context"
	"errors"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

type drainWriter struct {
	waiting chan struct{}
	once    sync.Once
	needle  string
}

func (w *drainWriter) Write(p []byte) (int, error) {
	needle := w.needle
	if needle == "" {
		needle = "waiting for landing maintenance to finish"
	}
	if strings.Contains(string(p), needle) {
		w.once.Do(func() { close(w.waiting) })
	}
	return len(p), nil
}

func TestDaemonCancellationDuringMaintenanceDrain(t *testing.T) {
	for _, exit := range []string{"done", "restart"} {
		t.Run(exit, func(t *testing.T) {
			bus.ResetDefault()
			s, root, binary := selfUpdateFixture(t)
			withClosedLoginGate(t)
			setSetting(t, s, "landing", "agents")
			run(t, root, "git", "branch", "evolve/draining")
			exec1(s.DB, `INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
				VALUES ('draining', 'proj', 'experiment', 'Fake pending measurement', 'done', 'evolve/draining', ?, ?)`,
				testNow, testNow)
			started, cancelled, release := make(chan struct{}), make(chan struct{}), make(chan struct{})
			oldMeasure, oldStamp := evolve.Measure, binaryStamp
			evolve.Measure = func(ctx context.Context, _ *store.Store, _, _, _ string, _ evolve.MeasureOptions) (*evolve.Scores, error) {
				close(started)
				select {
				case <-ctx.Done():
					close(cancelled)
					return nil, ctx.Err()
				case <-release:
					return nil, errors.New("fake measurement released by test cleanup")
				}
			}
			var changed atomic.Bool
			binaryStamp = func() string {
				if changed.Load() {
					return "new-fake-binary"
				}
				return "old-fake-binary"
			}
			ctx, cancel := context.WithTimeout(context.Background(), testWait(20*time.Second))
			out := &drainWriter{waiting: make(chan struct{})}
			opts := daemonOpts{Interval: 300, Out: out, SelfUpdate: &binary}
			if exit == "done" {
				opts.MaxLoops = 1
			} else {
				opts.RestartOnChange = true
			}
			done := make(chan string, 1)
			go func() { done <- daemon(ctx, s, opts) }()
			joined := false
			defer func() {
				cancel()
				close(release)
				if !joined {
					select {
					case <-done:
					case <-time.After(testWait(10 * time.Second)):
						t.Error("maintenance lane outlived the fake daemon")
					}
				}
				waitSelfUpdateIdle(t)
				evolve.Measure, binaryStamp = oldMeasure, oldStamp
			}()
			select {
			case <-started:
			case <-ctx.Done():
				t.Fatal("fake measurement never started")
			}
			if exit == "restart" {
				changed.Store(true)
				if _, err := bus.Emit(s, "human", "post.question", "wake-restart", map[string]any{"project": "proj"}); err != nil {
					t.Fatal(err)
				}
			}
			select {
			case <-out.waiting:
			case <-ctx.Done():
				t.Fatal("daemon did not enter its maintenance drain")
			}
			select {
			case <-cancelled:
				t.Fatal("normal drain cancelled the lane before the caller cancelled")
			default:
			}
			cancel()
			select {
			case result := <-done:
				joined = true
				if result != "interrupted" {
					t.Fatalf("late cancellation returned %s instead of interrupted", result)
				}
			case <-time.After(testWait(10 * time.Second)):
				t.Fatal("caller cancellation was ignored after " + exit + " drain began")
			}
			select {
			case <-cancelled:
			default:
				t.Fatal("daemon returned without cancelling and joining its measurement")
			}
			if jobRow(s.DB, "draining").str("branch") != "evolve/draining" {
				t.Fatal("cancelled drain removed the unmeasured branch")
			}
			selfUpdateState.Lock()
			building := selfUpdateState.inflight
			selfUpdateState.Unlock()
			if building || count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", selfUpdateEvent) != 0 {
				t.Fatal("self-update ran before the cancelled measurement finished")
			}
		})
	}
}
