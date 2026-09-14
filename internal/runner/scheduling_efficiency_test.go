package runner

import (
	"bytes"
	"context"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

func schedulingJob(t *testing.T, s *store.Store, project, title string, priority int) row {
	t.Helper()
	job, _, err := addJob(s, project, "task", title, "exercise scheduling", 0, priority,
		"scheduling:"+title, humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	return job
}

func schedulingRun(s *store.Store, job row, agent string) string {
	id := "run-" + job.str("id")
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES (?, ?, ?, ?, ?, 'running')",
		id, job.str("id"), agent, job.str("project"), testNow)
	return id
}

func TestSchedulingConcurrentLeasesKeepOneJobPerAgent(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "parallel_per_project", "2")
	jobs := []row{
		schedulingJob(t, s, "proj", "first", 100),
		schedulingJob(t, s, "proj", "second", 90),
	}
	type result struct {
		job row
		err error
	}
	start := make(chan struct{})
	results := make(chan result, len(jobs))
	for _, job := range jobs {
		go func(id string) {
			<-start
			leased, err := leaseJob(s, "aurora", testNow, leaseOpts{JobID: id})
			results <- result{leased, err}
		}(job.str("id"))
	}
	close(start)
	var leased []row
	var refusals []error
	for range jobs {
		result := <-results
		if result.job != nil {
			leased = append(leased, result.job)
		}
		if result.err != nil {
			refusals = append(refusals, result.err)
		}
	}
	if len(leased) != 1 {
		t.Fatalf("one agent acquired %d simultaneous leases, want 1: %v", len(leased), leased)
	}
	if len(refusals) != 1 || !strings.Contains(refusals[0].Error(), "aurora") ||
		!strings.Contains(refusals[0].Error(), leased[0].str("id")) {
		t.Fatalf("busy agent refusal must identify the held job: %v", refusals)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'job.lease'"); n != 1 {
		t.Fatalf("lease events = %d, want 1", n)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued' AND generation = 0 AND attempts = 0"); n != 1 {
		t.Fatalf("untouched queued jobs = %d, want 1", n)
	}
	if _, err := finishJob(s, leased[0].str("id"), "aurora", "done", "finished", "", leased[0].int("generation")); err != nil {
		t.Fatal(err)
	}
	if next, err := leaseJob(s, "aurora", testNow, leaseOpts{}); err != nil || next == nil {
		t.Fatalf("worker was not reusable after finishing: %v, %v", next, err)
	}
}

func TestSchedulingProvisionalFinishKeepsCapacity(t *testing.T) {
	for _, status := range []string{"done", "failed", "cancelled", "blocked"} {
		for _, scope := range []string{"worker", "project"} {
			t.Run(status+"/"+scope, func(t *testing.T) {
				s := newStore(t)
				newBareRepo(t, s)
				join(t, s, "aurora")
				join(t, s, "borealis")
				first := schedulingJob(t, s, "proj", "first", 100)
				next := schedulingJob(t, s, "proj", "next", 90)
				leased, err := leaseJob(s, "aurora", testNow, leaseOpts{JobID: first.str("id")})
				if err != nil || leased == nil {
					t.Fatalf("first lease: %v, %v", leased, err)
				}
				runID := schedulingRun(s, first, "aurora")
				if _, err := finishJob(s, first.str("id"), "aurora", status, "provisional report", "", leased.int("generation")); err != nil {
					t.Fatal(err)
				}
				agent := "borealis"
				if scope == "worker" {
					setSetting(t, s, "parallel_per_project", "2")
					agent = "aurora"
				}
				got, err := leaseJob(s, agent, testNow, leaseOpts{JobID: next.str("id")})
				if got != nil {
					t.Fatalf("provisional %s released %s capacity before run %s ended: %v", status, scope, runID, got)
				}
				if scope == "worker" {
					if err == nil || !strings.Contains(err.Error(), first.str("id")) {
						t.Fatalf("busy worker refusal must identify its live job: %v", err)
					}
				} else if err != nil {
					t.Fatal(err)
				}
				exec1(s.DB, "UPDATE runs SET status = ?, finished_at = ? WHERE id = ?", status, testNow, runID)
				got, err = leaseJob(s, agent, testNow, leaseOpts{JobID: next.str("id")})
				if err != nil || got == nil || got.int("generation") != 1 || got.int("attempts") != 1 {
					t.Fatalf("capacity did not reopen after the run ended: %v, %v", got, err)
				}
			})
		}
	}
}

func TestSchedulingRunningLeaseCountsOnce(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	setSetting(t, s, "parallel_per_project", "2")
	first := schedulingJob(t, s, "proj", "first", 100)
	second := schedulingJob(t, s, "proj", "second", 90)
	third := schedulingJob(t, s, "proj", "third", 80)
	if got, err := leaseJob(s, "aurora", testNow, leaseOpts{JobID: first.str("id")}); err != nil || got == nil {
		t.Fatalf("first lease: %v, %v", got, err)
	}
	schedulingRun(s, first, "aurora")
	if got, err := leaseJob(s, "borealis", testNow, leaseOpts{JobID: second.str("id")}); err != nil || got == nil {
		t.Fatalf("a running lease consumed two project slots: %v, %v", got, err)
	}
	if got, err := leaseJob(s, "cygnus", testNow, leaseOpts{JobID: third.str("id")}); err != nil || got != nil {
		t.Fatalf("project cap was exceeded: %v, %v", got, err)
	}
}

func TestSchedulingPickWorkerSkipsProvisionalCompletion(t *testing.T) {
	for _, status := range []string{"done", "failed", "cancelled", "blocked"} {
		t.Run(status, func(t *testing.T) {
			s := newStore(t)
			newBareRepo(t, s)
			join(t, s, "aurora")
			join(t, s, "borealis")
			setSetting(t, s, "quorum", "1")
			setSetting(t, s, "parallel_per_project", "2")
			first := schedulingJob(t, s, "proj", "controller-still-running", 100)
			second := schedulingJob(t, s, "proj", "already-finished", 90)
			for _, next := range []struct {
				job   row
				agent string
			}{{first, "aurora"}, {second, "borealis"}} {
				leased, err := leaseJob(s, next.agent, testNow, leaseOpts{JobID: next.job.str("id")})
				if err != nil || leased == nil {
					t.Fatalf("lease: %v %v", leased, err)
				}
				if next.agent == "aurora" {
					schedulingRun(s, next.job, next.agent)
				}
				if _, err := finishJob(s, next.job.str("id"), next.agent, status, "reported", "", leased.int("generation")); err != nil {
					t.Fatal(err)
				}
			}
			if picked, err := pickWorker(s, testNow, "", nil); err != nil || picked != "borealis" {
				t.Fatalf("selection chose a busy controller over an idle worker: %q %v", picked, err)
			}
			exec1(s.DB, "UPDATE runs SET status = ?, finished_at = ? WHERE job_id = ?", status, testNow, first.str("id"))
			if picked, err := pickWorker(s, testNow, "", nil); err != nil || picked != "aurora" {
				t.Fatalf("finished worker was not reused: %q %v", picked, err)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM agents WHERE kind = 'worker'"); n != 2 {
				t.Fatalf("worker selection spawned unnecessary identities: %d", n)
			}
		})
	}
}

func TestSchedulingExpiredLeasePreservesGenerationTakeover(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "aurora")
	job := schedulingJob(t, s, "proj", "expired", 100)
	first, err := leaseJob(s, "aurora", testNow, leaseOpts{JobID: job.str("id"), Minutes: 1})
	if err != nil || first == nil {
		t.Fatalf("first lease: %v, %v", first, err)
	}
	runID := schedulingRun(s, job, "aurora")
	if got, err := leaseJob(s, "aurora", first.str("lease_until"), leaseOpts{JobID: job.str("id")}); err != nil || got != nil {
		t.Fatalf("lease was retaken at its exact deadline: %v, %v", got, err)
	}
	replacement, err := leaseJob(s, "aurora", plusMinutes(testNow, 2), leaseOpts{JobID: job.str("id")})
	if err != nil || replacement == nil || replacement.int("generation") != 2 || replacement.int("attempts") != 2 {
		t.Fatalf("expired same-agent takeover: %v, %v", replacement, err)
	}
	if _, err := finishJob(s, job.str("id"), "aurora", "done", "stale result", "", first.int("generation")); err == nil ||
		!strings.Contains(err.Error(), "Stale lease") {
		t.Fatalf("old generation finished its replacement: %v", err)
	}
	if got := mustOne(s.DB, "SELECT status FROM runs WHERE id = ?", runID).str("status"); got != "running" {
		t.Fatalf("lease acquisition changed the old run: %s", got)
	}
}

func TestSchedulingFairnessIncludesLeasesBeforeRunStart(t *testing.T) {
	s := newStore(t)
	setSetting(t, s, "parallel_per_project", "2")
	projects := []string{"first", "second", "third"}
	for i, project := range projects {
		exec1(s.DB, "INSERT INTO projects (slug, name, path, added_at) VALUES (?, ?, ?, ?)", project, project, t.TempDir(), testNow)
		schedulingJob(t, s, project, project+"-high", 100-i*10)
		schedulingJob(t, s, project, project+"-low", 99-i*10)
	}
	for i, agent := range []string{"aurora", "borealis", "cygnus"} {
		got, err := leaseJob(s, agent, testNow, leaseOpts{})
		if err != nil || got == nil || got.str("project") != projects[i] || got.str("title") != projects[i]+"-high" {
			t.Fatalf("dispatch %d before any run.start = %v, %v; want %s's highest priority job", i+1, got, err, projects[i])
		}
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM runs"); n != 0 {
		t.Fatalf("fixture already recorded %d runs", n)
	}
	got, err := leaseJob(s, "draco", testNow, leaseOpts{Projects: []string{"first"}})
	if err != nil || got == nil || got.str("project") != "first" || got.str("title") != "first-low" {
		t.Fatalf("restricted dispatch must still honor its project and priority: %v, %v", got, err)
	}
}

func TestSchedulingMixedBatchDoesNotPauseOnLastError(t *testing.T) {
	s, first := daemonRetryFixture(t)
	second := schedulingJob(t, s, "proj", "slow-error", 90)
	setSetting(t, s, "workers", "2")
	setSetting(t, s, "parallel_per_project", "2")
	setSetting(t, s, "jobs_per_day", "0")
	reflectJob, _, err := addJob(s, "proj", "reflect", "already reflected", "", 0, 1,
		"reflect:proj:"+testNow[:10], humanAgent, true)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := finishJob(s, reflectJob.str("id"), humanAgent, "done", "already reflected", "", -1); err != nil {
		t.Fatal(err)
	}
	failureGate := newLifecycleGate(t, "last runner error")
	script := "if [ \"$PULSE_JOB\" = " + shellQuote(first.str("id")) + " ]; then echo 'SUMMARY: ok'; exit 0; fi; " +
		"if [ ! -f " + shellQuote(failureGate.ready) + " ]; then " + failureGate.command + "; fi; " +
		"echo 'Error: Authentication token found but could not be validated.' >&2; exit 1"
	ctx, cancel := context.WithTimeout(context.Background(), lifecycleWaitTimeout)
	done := make(chan struct{})
	var result string
	var out bytes.Buffer
	var waits []time.Duration
	go func() {
		defer close(done)
		result = daemon(ctx, s, daemonOpts{Interval: 7, MaxLoops: 3, Grace: time.Second, Out: &out,
			WorkerCmd: shellJoin([]string{"sh", "-c", script}),
			Sleep: func(d time.Duration) bool {
				waits = append(waits, d)
				return true
			}})
	}()
	t.Cleanup(func() {
		cancel()
		waitLifecycleRun(t, done)
	})
	failureGate.wait(t, done)
	heartbeatEventually(t, func() bool {
		beat := mustOne(s.DB, "SELECT json_extract(value, '$.loops') AS loops FROM meta WHERE key = 'daemon'")
		return beat != nil && beat.int("loops") >= 2
	})
	failureGate.release(t)
	waitLifecycleRun(t, done)
	if result != "done" || len(waits) != 0 {
		t.Fatalf("mixed batch paused after its last run errored: result=%s waits=%v\n%s", result, waits, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'done'", first.str("id")); n != 1 {
		t.Fatalf("successful runs = %d, want 1\n%s", n, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'error'", second.str("id")); n != 2 {
		t.Fatalf("prompt retries after mixed batch = %d, want 2\n%s", n, out.String())
	}
}
