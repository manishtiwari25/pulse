package runner

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
)

func TestDaemonWorkerCompletionStartsNextLandingPass(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	_, root := newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")
	setSetting(t, s, "jobs_per_day", "1")
	exec1(s.DB, `INSERT INTO jobs (id, project, kind, title, status, source, created_at)
		VALUES ('daily-reflect', 'proj', 'reflect', 'Already reflected', 'done', ?, ?)`,
		"reflect:proj:"+testNow[:10], testNow)
	job, _, err := addJob(s, "proj", "task", "One fake change", "one isolated test commit", 1, 100, "landing-wake", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	fifo := filepath.Join(t.TempDir(), "worker")
	if err := syscall.Mkfifo(fifo, 0o600); err != nil {
		t.Fatal(err)
	}
	gate, err := os.OpenFile(fifo, os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	defer gate.Close()
	command := fmt.Sprintf("sh -c %s", shellQuote("read gate < "+shellQuote(fifo)+
		"; printf change > value.txt && git add value.txt && git commit -q -m 'Fake change' && echo SUMMARY: fake commit"))
	ctx, cancel := context.WithTimeout(context.Background(), testWait(20*time.Second))
	finished := make(chan struct{})
	defer func() {
		_, _ = gate.WriteString("go\ngo\n")
		cancel()
		<-finished
	}()
	var result string
	go func() {
		defer close(finished)
		result = daemon(ctx, s, daemonOpts{Interval: 300, MaxLoops: 2, WorkerCmd: command, Out: io.Discard})
	}()
	// Finish the first (empty) maintenance pass before releasing the fake worker.
	deadline := time.Now().Add(testWait(10 * time.Second))
	for {
		if _, exists, err := s.Meta("hygiene"); err != nil {
			t.Fatal(err)
		} else if exists {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("initial maintenance did not finish")
		}
		time.Sleep(10 * time.Millisecond)
	}
	if _, err := gate.WriteString("go\n"); err != nil {
		t.Fatal(err)
	}
	select {
	case <-finished:
	case <-ctx.Done():
		t.Fatal("fake completion did not wake the supervisor")
	}
	if result != "done" || !fileExists(filepath.Join(root, "value.txt")) || jobRow(s.DB, job.str("id")).str("branch") != "" {
		t.Fatalf("worker completion waited for an idle interval instead of starting its landing: %s, job=%v", result, jobRow(s.DB, job.str("id")))
	}
}
