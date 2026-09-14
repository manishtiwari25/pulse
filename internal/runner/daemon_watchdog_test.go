package runner

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestDaemonStallBoundScalesWithIntervalAboveTheFloor(t *testing.T) {
	if got := daemonStallBound(300); got != 30*time.Minute {
		t.Fatalf("300s interval: bound %s, want the 30m floor", got)
	}
	if got := daemonStallBound(5); got != 30*time.Minute {
		t.Fatalf("5s interval: bound %s, want the 30m floor", got)
	}
	if got := daemonStallBound(600); got != 60*time.Minute {
		t.Fatalf("600s interval: bound %s, want 6x", got)
	}
}

// newTestWatchdog: a watchdog with a fake clock and a captured exit; returns it and the recorded calls.
func newTestWatchdog(t *testing.T, bound time.Duration) (*daemonWatchdog, *[]string, *time.Time) {
	t.Helper()
	logs := filepath.Join(t.TempDir(), "logs")
	var calls []string
	clock := time.Date(2026, 9, 14, 15, 0, 0, 0, time.UTC)
	w := newDaemonWatchdog(bound, logs,
		func(f string, a ...any) { calls = append(calls, "say") },
		func(note, dump string) { calls = append(calls, "note:"+note+":"+filepath.Base(dump)) },
		func() { calls = append(calls, "kill") })
	w.exit = func(code int) { calls = append(calls, "exit") }
	w.grace = 0
	w.now = func() time.Time { return clock }
	w.beatAt = clock
	w.dump = func() []byte { return []byte("goroutine 1 [running]:\nmain.main()\n") }
	return w, &calls, &clock
}

func TestWatchdogFiresOnceWhenTheLoopStopsBeating(t *testing.T) {
	w, calls, clock := newTestWatchdog(t, 30*time.Minute)
	*clock = clock.Add(29 * time.Minute)
	if w.stalled() != 0 {
		t.Fatal("29m without a beat is within the bound")
	}
	w.beat()
	*clock = clock.Add(31 * time.Minute)
	if w.stalled() != 31*time.Minute {
		t.Fatalf("stalled = %s, want 31m after the last beat", w.stalled())
	}
	w.fire(w.stalled())
	w.fire(w.stalled())
	got := strings.Join(*calls, ",")
	entries, err := os.ReadDir(w.logs)
	if err != nil || len(entries) != 1 || !strings.HasPrefix(entries[0].Name(), "daemon-stall-") {
		t.Fatalf("dump files = %v (%v)", entries, err)
	}
	if got != "say,note:stalled 31m0s; restarting:"+entries[0].Name()+",kill,exit" {
		t.Fatalf("calls = %s", got)
	}
	text, _ := os.ReadFile(filepath.Join(w.logs, entries[0].Name()))
	if !strings.Contains(string(text), "goroutine 1 [running]") || !strings.Contains(string(text), "stall") {
		t.Fatalf("dump = %q", text)
	}
}

func TestWatchdogRunStaysQuietWhileTheLoopBeats(t *testing.T) {
	w, calls, _ := newTestWatchdog(t, 30*time.Minute)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() {
		defer close(done)
		w.run(ctx, time.Millisecond)
	}()
	time.Sleep(20 * time.Millisecond)
	cancel()
	<-done
	if len(*calls) != 0 {
		t.Fatalf("a beating loop drew calls: %v", *calls)
	}
}

func TestWatchdogRunFiresFromItsTick(t *testing.T) {
	w, calls, clock := newTestWatchdog(t, 30*time.Minute)
	*clock = clock.Add(time.Hour)
	done := make(chan struct{})
	go func() {
		defer close(done)
		w.run(context.Background(), time.Millisecond)
	}()
	select {
	case <-done:
	case <-time.After(testWait(20 * time.Second)):
		t.Fatal("run did not return after firing")
	}
	if got := strings.Join(*calls, ","); !strings.HasSuffix(got, "kill,exit") {
		t.Fatalf("calls = %s", got)
	}
}

func TestPruneStallDumpsKeepsTheNewest(t *testing.T) {
	logs := t.TempDir()
	for _, n := range []string{"daemon-stall-20260914T100000Z.log", "daemon-signal-20260914T110000Z.log",
		"daemon-stall-20260914T120000Z.log", "daemon.log", "run-20260914T100000Z-abc.log"} {
		if err := os.WriteFile(filepath.Join(logs, n), []byte("x"), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	pruneStallDumps(logs, 2)
	entries, _ := os.ReadDir(logs)
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	want := "daemon-signal-20260914T110000Z.log daemon-stall-20260914T120000Z.log daemon.log run-20260914T100000Z-abc.log"
	if got := strings.Join(names, " "); got != want {
		t.Fatalf("after prune: %s\nwant: %s", got, want)
	}
}
