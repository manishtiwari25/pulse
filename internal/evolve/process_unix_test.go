//go:build unix

package evolve

import (
	"context"
	"errors"
	"io"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestProcessCancellationStopsDescendantWork(t *testing.T) {
	dir := t.TempDir()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	finished := make(chan error, 1)
	go func() {
		finished <- runProcess(ctx, dir, isolatedEnv(filepath.Join(dir, "empty")),
			[]string{"sh", "-c", "(sleep 1; printf escaped > descendant-survived) & printf ready > started; wait"}, io.Discard)
	}()
	deadline := time.NewTimer(4 * time.Second)
	defer deadline.Stop()
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	for {
		if _, err := os.Stat(filepath.Join(dir, "started")); err == nil {
			break
		} else if !errors.Is(err, os.ErrNotExist) {
			cancel()
			_ = withinTime(t, finished)
			t.Fatal(err)
		}
		select {
		case err := <-finished:
			t.Fatalf("temporary process exited before cancellation: %v", err)
		case <-deadline.C:
			cancel()
			_ = withinTime(t, finished)
			t.Fatal("temporary process did not start")
		case <-ticker.C:
		}
	}
	cancel()
	if err := withinTime(t, finished); !errors.Is(err, context.Canceled) {
		t.Fatalf("process did not preserve cancellation: %v", err)
	}
	time.Sleep(1100 * time.Millisecond)
	if _, err := os.Stat(filepath.Join(dir, "descendant-survived")); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("cancelled harness left a descendant able to perform work")
	}
}
