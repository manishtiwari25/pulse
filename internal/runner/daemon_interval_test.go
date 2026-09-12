package runner

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestDaemonIntervalFlagDefaults(t *testing.T) {
	for _, name := range []string{"daemon", "service"} {
		t.Run(name, func(t *testing.T) {
			cmd, ok := cli.Lookup(name)
			if !ok {
				t.Fatalf("%s not registered", name)
			}
			fs := flag.NewFlagSet(name, flag.ContinueOnError)
			cmd.Flags(fs)
			f := fs.Lookup("interval")
			if f == nil || f.DefValue != strconv.Itoa(DefaultDaemonIntervalSeconds) {
				t.Fatalf("interval flag = %+v, want default %d", f, DefaultDaemonIntervalSeconds)
			}
			if err := fs.Parse([]string{"--interval", "7"}); err != nil {
				t.Fatal(err)
			}
			if got := intFlag(fs, "interval"); got != 7 {
				t.Fatalf("interval = %d, want 7", got)
			}
		})
	}
}

func daemonRetryFixture(t *testing.T) (*store.Store, row) {
	t.Helper()
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	job, _, err := addJob(s, "proj", "task", "retry", "exercise retry timing", 1, 100, "retry-test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	// Prime the planner before subscribing so its initial job.add cannot wake the first wait.
	if _, err := planJobs(s, "proj", s.Now(), humanAgent); err != nil {
		t.Fatal(err)
	}
	return s, job
}

func TestDaemonRunnerErrorWait(t *testing.T) {
	skipSlow(t)
	for _, tc := range []struct {
		name     string
		interval int
		want     int
	}{
		{"omitted", 0, DefaultDaemonIntervalSeconds},
		{"explicit-default", DefaultDaemonIntervalSeconds, DefaultDaemonIntervalSeconds},
		{"negative", -1, DefaultDaemonIntervalSeconds},
		{"custom", 7, 7},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, job := daemonRetryFixture(t)
			var out bytes.Buffer
			var waits []time.Duration
			var attemptsAtWait []int
			ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
			defer cancel()
			result := daemon(ctx, s, daemonOpts{
				Interval: tc.interval, MaxLoops: 2, Out: &out,
				WorkerCmd: strconv.Quote(filepath.Join(s.Home, "missing-worker")),
				Sleep: func(d time.Duration) bool {
					waits = append(waits, d)
					attemptsAtWait = append(attemptsAtWait, count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?", job.str("id")))
					return true
				},
			})
			if result != "done" || len(waits) != 1 || waits[0] != time.Duration(tc.want)*time.Second {
				t.Fatalf("daemon = %s, waits = %v, want [%ds]\n%s", result, waits, tc.want, out.String())
			}
			if attemptsAtWait[0] != 1 {
				t.Fatalf("attempts before waiting = %v, want [1]", attemptsAtWait)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'error'", job.str("id")); n != 2 {
				t.Fatalf("error attempts = %d, want 2\n%s", n, out.String())
			}
			if !strings.Contains(out.String(), fmt.Sprintf("interval %ds", tc.want)) {
				t.Fatalf("configured interval missing:\n%s", out.String())
			}
		})
	}
}

func TestDaemonRunnerErrorWaitInterrupted(t *testing.T) {
	s, job := daemonRetryFixture(t)
	var out bytes.Buffer
	var waits []time.Duration
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	result := daemon(ctx, s, daemonOpts{
		Interval: 7, MaxLoops: 2, Out: &out,
		WorkerCmd: strconv.Quote(filepath.Join(s.Home, "missing-worker")),
		Sleep: func(d time.Duration) bool {
			waits = append(waits, d)
			cancel()
			return false
		},
	})
	if result != "interrupted" || len(waits) != 1 || waits[0] != 7*time.Second {
		t.Fatalf("daemon = %s, waits = %v\n%s", result, waits, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?", job.str("id")); n != 1 {
		t.Fatalf("attempts after interruption = %d, want 1", n)
	}
}

func TestDaemonSuccessfulRunsDoNotBackOff(t *testing.T) {
	s, first := daemonRetryFixture(t)
	second, _, err := addJob(s, "proj", "task", "next", "exercise successful dispatch", 1, 90, "next-test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	var waits []time.Duration
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	result := daemon(ctx, s, daemonOpts{
		Interval: 7, MaxLoops: 2, Out: &out, WorkerCmd: "sh -c 'echo SUMMARY: ok'",
		Sleep: func(d time.Duration) bool {
			waits = append(waits, d)
			return true
		},
	})
	if result != "done" || len(waits) != 0 {
		t.Fatalf("daemon = %s, waits = %v\n%s", result, waits, out.String())
	}
	for _, job := range []row{first, second} {
		if n := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'done'", job.str("id")); n != 1 {
			t.Fatalf("successful attempts for %s = %d, want 1\n%s", job.str("id"), n, out.String())
		}
	}
}

func TestDaemonRetryUsesPostWaitTime(t *testing.T) {
	fixed, job := daemonRetryFixture(t)
	t.Setenv("PULSE_NOW", testNow)
	s, err := store.Open(store.Options{Home: fixed.Home})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	later := mustISO(testNow).Add(7 * time.Second).Format(time.RFC3339)
	var out bytes.Buffer
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	result := daemon(ctx, s, daemonOpts{
		Interval: 7, MaxLoops: 2, Out: &out,
		WorkerCmd: strconv.Quote(filepath.Join(s.Home, "missing-worker")),
		Sleep: func(time.Duration) bool {
			t.Setenv("PULSE_NOW", later)
			return true
		},
	})
	runs := mustRows(s.DB, "SELECT started_at FROM runs WHERE job_id = ? ORDER BY started_at, id", job.str("id"))
	if result != "done" || len(runs) != 2 || runs[0].str("started_at") != testNow || runs[1].str("started_at") != later {
		t.Fatalf("daemon = %s, starts = %v, want %s then %s\n%s", result, runs, testNow, later, out.String())
	}
}

func TestDaemonStatusDefaultInterval(t *testing.T) {
	for _, tc := range []struct {
		state string
		extra int
	}{
		{"alive", 0},
		{"silent", 1},
	} {
		t.Run(tc.state, func(t *testing.T) {
			s := newStore(t)
			age := time.Duration(2*DefaultDaemonIntervalSeconds+60+tc.extra) * time.Second
			beat, err := json.Marshal(map[string]any{"pid": 42, "at": mustISO(s.Now()).Add(-age).Format(time.RFC3339)})
			if err != nil {
				t.Fatal(err)
			}
			exec1(s.DB, "INSERT INTO meta (key, value) VALUES ('daemon', ?)", string(beat))
			got := daemonStatus(s, s.Now())
			if !strings.HasPrefix(got, tc.state+" ") || !strings.HasSuffix(got, fmt.Sprintf("interval %ds", DefaultDaemonIntervalSeconds)) {
				t.Fatalf("daemon status = %q", got)
			}
		})
	}
}
