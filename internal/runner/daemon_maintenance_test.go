package runner

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestDaemonSlowMeasurementDoesNotBlockDispatchOrStop(t *testing.T) {
	bus.ResetDefault()
	s, root, selfEnv := selfUpdateFixture(t)
	other := filepath.Join(t.TempDir(), "other")
	if err := os.CopyFS(other, os.DirFS(templateRepo(t))); err != nil {
		t.Fatal(err)
	}

	if _, _, err := addProject(s, other, "Other", "", false, "", humanAgent, "other"); err != nil {
		t.Fatal(err)
	}
	join(t, s, "aurora")
	setSetting(t, s, "landing", "agents")
	setSetting(t, s, "daemon.sweep", "off")
	setSetting(t, s, "minutes_per_job", "1000")
	run(t, root, "git", "branch", "evolve/slow-measurement")
	exec1(s.DB, `INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
		VALUES ('slow-measurement', 'proj', 'experiment', 'Fake measurement', 'done', 'evolve/slow-measurement', ?, ?)`,
		testNow, testNow)
	exec1(s.DB, `INSERT INTO runs (id, job_id, agent, project, started_at, status)
		VALUES ('slow-run', 'slow-measurement', 'aurora', 'proj', ?, 'done')`, testNow)
	fast, _, err := addJob(s, "other", "task", "Other project can work", "fake worker only", 0, 100, "fast-test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	for _, slug := range []string{"proj", "other"} {
		exec1(s.DB, `INSERT INTO jobs (id, project, kind, title, status, source, created_at)
			VALUES (?, ?, 'reflect', 'Already reflected', 'done', ?, ?)`,
			"reflected-"+slug, slug, "reflect:"+slug+":"+testNow[:10], testNow)
	}

	started, release := make(chan struct{}), make(chan struct{})
	var once sync.Once
	var calls atomic.Int32
	old := evolve.Measure
	evolve.Measure = func(ctx context.Context, _ *store.Store, _, _, _ string, _ evolve.MeasureOptions) (*evolve.Scores, error) {
		calls.Add(1)
		once.Do(func() { close(started) })
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-release:
			return nil, errors.New("fake measurement intentionally left unmeasured")
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), testWait(120*time.Second))
	done := make(chan string, 1)
	joined := false
	go func() {
		done <- daemon(ctx, s, daemonOpts{Interval: 300, WorkerCmd: "sh -c 'echo SUMMARY: fake work complete'", Out: io.Discard, SelfUpdate: &selfEnv})
	}()
	var releaseOnce sync.Once
	releaseMeasurement := func() { releaseOnce.Do(func() { close(release) }) }
	stopTimedOut := false
	t.Cleanup(func() {
		releaseMeasurement()
		cancel()
		if !joined {
			select {
			case <-done:
			case <-time.After(testWait(10 * time.Second)):
				if stopTimedOut {
					t.Log("fake daemon still did not finish after the STOP timeout and measurement release")
				} else {
					t.Error("fake daemon did not finish after its blocked measurement was released")
				}
			}
		}
		evolve.Measure = old
	})
	select {
	case <-started:
	case <-time.After(testWait(10 * time.Second)):
		t.Fatal("fake measurement did not start")
	}
	if _, err := bus.Emit(s, humanAgent, "post.question", "wake-control", map[string]any{"project": "other"}); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(testWait(10 * time.Second))
	progress := false
	for time.Now().Before(deadline) {
		var beat struct {
			Loops int `json:"loops"`
		}
		r := mustOne(s.DB, "SELECT value FROM meta WHERE key = 'daemon'")
		if r != nil && json.Unmarshal([]byte(r.str("value")), &beat) == nil &&
			beat.Loops > 1 && count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?", fast.str("id")) > 0 {
			progress = true
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if !progress {
		t.Error("a blocked measurement prevented another project's dispatch or a supervisor heartbeat")
	}
	if got := calls.Load(); got != 1 {
		t.Errorf("started %d measurements while the first was still pending", got)
	}
	if data, err := os.ReadFile(selfEnv.Binary); err != nil || string(data) != "old binary" {
		t.Errorf("self-update installed before measurement/landing finished: %q (%v)", data, err)
	}
	selfUpdateState.Lock()
	building := selfUpdateState.inflight
	selfUpdateState.Unlock()
	if building || fileExists(selfEnv.Binary+".new") {
		t.Error("self-update started while landing verification was pending")
	}
	// The setting's own "setting" event wakes the loop (daemonWake); no extra wake, so that path stays covered.
	setSetting(t, s, "stop", "1")
	// Wait on the daemon's result channel with a generous, load-scaled budget: on a loaded host the
	// stop path (cancelling the measurement, marking runs, writing the heartbeat) has taken well over
	// the 10s the old poll loop allowed even though STOP itself was never stuck.
	select {
	case result := <-done:
		joined = true
		if result != "stopped" {
			t.Errorf("STOP result = %s", result)
		}
		if job := jobRow(s.DB, "slow-measurement"); job.str("branch") != "evolve/slow-measurement" {
			t.Errorf("STOP discarded the unmeasured branch: %v", job)
		}
		return
	case <-time.After(testWait(60 * time.Second)):
	}
	stopTimedOut = true
	t.Error("STOP could not cancel a fake blocked measurement")
}
