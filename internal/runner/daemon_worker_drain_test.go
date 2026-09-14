package runner

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
)

func TestDaemonCancellationDuringWorkerDrainKeepsGrace(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "minutes_per_job", "1000")
	setSetting(t, s, "jobs_per_day", "1")
	job, _, err := addJob(s, "proj", "task", "Fake draining worker", "fake worker only", 0, 100, "drain-test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	fifo, marker := filepath.Join(t.TempDir(), "worker"), filepath.Join(t.TempDir(), "started")
	if err := syscall.Mkfifo(fifo, 0o600); err != nil {
		t.Fatal(err)
	}
	cmd := "sh -c " + shellQuote("printf ready > "+shellQuote(marker)+"; read line < "+shellQuote(fifo))
	out := &drainWriter{waiting: make(chan struct{}), needle: "run(s) in flight to finish"}
	ctx, cancel := context.WithTimeout(context.Background(), testWait(20*time.Second))
	const grace = 100 * time.Millisecond
	done := make(chan string, 1)
	go func() {
		done <- daemon(ctx, s, daemonOpts{Interval: 300, MaxLoops: 1, Grace: grace, WorkerCmd: cmd, Out: out})
	}()
	joined := false
	defer func() {
		cancel()
		writer, err := os.OpenFile(fifo, os.O_WRONLY|syscall.O_NONBLOCK, 0o600)
		if err == nil {
			_, writeErr := writer.WriteString("cleanup\n")
			writer.Close()
			if writeErr != nil && !errors.Is(writeErr, syscall.EPIPE) {
				t.Error(writeErr)
			}
		} else if !errors.Is(err, syscall.ENXIO) {
			t.Error(err)
		}
		if !joined {
			select {
			case <-done:
			case <-time.After(testWait(10 * time.Second)):
				t.Error("fake worker outlived its daemon")
			}
		}
	}()
	select {
	case <-out.waiting:
	case <-ctx.Done():
		t.Fatal("daemon did not enter its normal worker drain")
	}
	deadline := time.Now().Add(testWait(10 * time.Second))
	for !fileExists(marker) && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if !fileExists(marker) {
		t.Fatal("fake worker did not reach its blocking read")
	}
	cancelledAt := time.Now()
	cancel()
	select {
	case result := <-done:
		joined = true
		if result != "interrupted" || time.Since(cancelledAt) < grace {
			t.Fatalf("late cancellation lost its grace or exit state: %s, elapsed %s", result, time.Since(cancelledAt))
		}
	case <-time.After(testWait(10 * time.Second)):
		t.Fatal("late cancellation failed to bound the previously unbounded worker drain")
	}
	if jobRow(s.DB, job.str("id")).str("status") == "leased" ||
		count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE status = 'running'") != 0 {
		t.Fatal("drain returned without joining the cancelled worker")
	}
	if s.Setting("minutes_per_job", "") != "1000" {
		t.Fatal("drain changed the owner's per-job cap")
	}
}
