package runner

import (
	"bytes"
	"context"
	"encoding/json"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
)

func TestDaemonWakesOnPostQuestion(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var out bytes.Buffer
	done := make(chan string, 1)
	go func() {
		done <- daemon(ctx, s, daemonOpts{Interval: 30, MaxLoops: 2, WorkerCmd: "sh -c 'echo SUMMARY: ok'", Out: &out})
	}()

	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		var beat map[string]any
		if r := mustOne(s.DB, "SELECT value FROM meta WHERE key = 'daemon'"); r != nil && json.Unmarshal([]byte(r.str("value")), &beat) == nil && beat["loops"] == float64(1) {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if _, err := bus.Emit(s, "human", "post.question", "q1", map[string]any{"project": "proj"}); err != nil {
		t.Fatal(err)
	}
	select {
	case result := <-done:
		if result != "done" {
			t.Fatalf("daemon = %s\n%s", result, out.String())
		}
	case <-time.After(5 * time.Second):
		cancel()
		t.Fatalf("the daemon did not wake on post.question:\n%s", out.String())
	}
}
