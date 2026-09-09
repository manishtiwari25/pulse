package runner

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

// Integration gates include real clone/sign/verify work, not just one observation.
const lifecycleWaitTimeout = time.Minute

type lifecycleGate struct {
	name, command, ready string
	pipe                 *os.File
}

func newLifecycleGate(t *testing.T, name string) *lifecycleGate {
	t.Helper()
	path, _ := heartbeatFIFO(t)
	// Keep a reader open so releasing a failed child's gate cannot block.
	pipe, err := os.OpenFile(path, os.O_RDWR, 0)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := pipe.Close(); err != nil {
			t.Error(err)
		}
	})
	ready := path + ".ready"
	return &lifecycleGate{name: name, ready: ready, pipe: pipe,
		command: "printf ready > " + shellQuote(ready) + "; read value < " + shellQuote(path)}
}

func (g *lifecycleGate) wait(t *testing.T, done <-chan struct{}) {
	t.Helper()
	deadline := time.NewTimer(lifecycleWaitTimeout)
	defer deadline.Stop()
	tick := time.NewTicker(10 * time.Millisecond)
	defer tick.Stop()
	for {
		if _, err := os.Stat(g.ready); err == nil {
			return
		} else if !os.IsNotExist(err) {
			t.Fatal(err)
		}
		select {
		case <-done:
			t.Fatalf("run finished before reaching the %s gate", g.name)
		case <-deadline.C:
			t.Fatalf("run did not reach the %s gate within %s", g.name, lifecycleWaitTimeout)
		case <-tick.C:
		}
	}
}

func waitLifecycleRun(t *testing.T, done <-chan struct{}) {
	t.Helper()
	deadline := time.NewTimer(lifecycleWaitTimeout)
	defer deadline.Stop()
	select {
	case <-done:
	case <-deadline.C:
		t.Fatalf("worker lifecycle did not finish within %s", lifecycleWaitTimeout)
	}
}

func (g *lifecycleGate) release(t *testing.T) {
	t.Helper()
	if _, err := g.pipe.WriteString("continue\n"); err != nil {
		t.Fatal(err)
	}
}

func assertLifecycleSuggestion(t *testing.T, s *store.Store, id, now, status string, listed bool) {
	t.Helper()
	th := tally.Thresholds{Now: now}
	res, err := tally.Compute(s, "proj", "", th)
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, suggestion := range res.Suggestions {
		if suggestion.ID == id {
			found = true
			if suggestion.Status != status {
				t.Fatalf("suggestion status = %q, want %q", suggestion.Status, status)
			}
		}
	}
	if !found {
		t.Fatal("suggestion disappeared from its history")
	}
	onBallot := false
	for _, item := range res.Ballot("observer", "observer-session") {
		if item.Suggestion != nil && item.Suggestion.ID == id {
			onBallot = true
		}
	}
	queue, err := tally.WorkWanted(s, []string{"proj"}, "observer", th)
	if err != nil {
		t.Fatal(err)
	}
	inQueue := false
	for _, item := range queue.Work {
		if item.ID == id {
			inQueue = true
		}
	}
	if onBallot != listed || inQueue != listed {
		t.Fatalf("suggestion on ballot=%v in queue=%v, want both %v", onBallot, inQueue, listed)
	}
}

func assertLifecyclePresence(t *testing.T, s *store.Store, now time.Time, want string) {
	t.Helper()
	presences, err := board.AgentPresences(s, now.UTC().Format(time.RFC3339Nano))
	if err != nil {
		t.Fatal(err)
	}
	if got := presences["quiet"]; got.Presence != want {
		t.Fatalf("presence = %+v, want %s", got, want)
	}
}

func TestWorkerLifecycleDelayedSuggestion(t *testing.T) {
	const startupFailure = "injected missed startup heartbeat"
	for _, tc := range []struct {
		name          string
		storeReported bool
		takeover      string
	}{
		{name: "output_completion"},
		{name: "store_completion_before_exit", storeReported: true},
		{name: "lease_retaken_during_worker", takeover: "worker"},
		{name: "lease_retaken_during_verification", takeover: "verification"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := newStore(t)
			_, root := newRepo(t, s)
			t.Setenv("PULSE_SESSION", "")
			setSetting(t, s, "minutes_per_job", "1")
			setSetting(t, s, "jobs_per_day", "0")
			setSetting(t, s, "quorum", "2")
			for _, agent := range []string{"proposer", "voter-one", "voter-two", "quiet", "rival", "observer"} {
				join(t, s, agent)
			}
			suggestion, err := createPost(s, testNow, postOpts{Kind: "suggestion", Project: "proj", Area: "evolution",
				Thread: "slow-lifecycle", Agent: actorAgent(s, "proposer"), Session: "proposal-session",
				Summary: "Keep slow worker lifecycle stable", Body: "One variable: lifecycle stability. Signal: README.md"})
			if err != nil {
				t.Fatal(err)
			}
			for _, agent := range []string{"voter-one", "voter-two"} {
				if _, err := createPost(s, testNow, postOpts{Kind: "vote", Project: "proj", Area: "evolution",
					Thread: "slow-lifecycle", Agent: actorAgent(s, agent), Session: agent + "-session",
					On: suggestion.str("id"), Vote: "up", Summary: "A bounded lifecycle regression guard"}); err != nil {
					t.Fatal(err)
				}
			}
			assertLifecycleSuggestion(t, s, suggestion.str("id"), testNow, "eligible", true)
			planned, err := planJobs(s, "proj", testNow, "daemon")
			if err != nil || len(planned) != 1 {
				t.Fatalf("plan: %v, %v", planned, err)
			}
			job := planned[0]
			if job.str("kind") != "improve" || job.str("source") != "suggestion:"+suggestion.str("id") {
				t.Fatalf("planned the wrong job: %v", job)
			}
			if again, err := planJobs(s, "proj", testNow, "daemon"); err != nil || len(again) != 0 {
				t.Fatalf("planned twice before dispatch: %v, %v", again, err)
			}

			clock, ticks, periods := fakeHeartbeatClock(t)
			scheduledNow := heartbeatNow
			missedStartup := make(chan struct{})
			var startup atomic.Bool
			heartbeatNow = func(s *store.Store) (time.Time, error) {
				if startup.CompareAndSwap(false, true) {
					close(missedStartup)
					return time.Time{}, errors.New(startupFailure)
				}
				return scheduledNow(s)
			}
			workerGate := newLifecycleGate(t, "worker exit")
			verifyGate := newLifecycleGate(t, "verification")
			check := verifyGate.command + "; test \"$(git show HEAD:lifecycle.txt)\" = 'slow lifecycle'"
			exec1(s.DB, "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'", check)
			script := "set -e; printf 'slow lifecycle\\n' > lifecycle.txt; git add lifecycle.txt; " +
				"git commit -qm 'lifecycle fixture'; " + workerGate.command
			if !tc.storeReported {
				script += "; printf 'SUMMARY: slow lifecycle complete\\n'"
			}
			worker := shellJoin([]string{"sh", "-c", script})
			base := strings.TrimSpace(run(t, root, "git", "rev-parse", "HEAD"))
			ctx, cancel := context.WithTimeout(context.Background(), 3*lifecycleWaitTimeout)
			done := make(chan struct{})
			var result row
			var reason string
			var runErr error
			go func() {
				defer close(done)
				result, reason, runErr = workOnce(s, "quiet", "proj", job.str("id"), testNow, runOpts{WorkerCmd: worker, Ctx: ctx})
			}()
			t.Cleanup(func() {
				cancel()
				waitLifecycleRun(t, done)
				if t.Failed() && result != nil {
					log, err := os.ReadFile(result.str("log"))
					if err != nil {
						t.Errorf("read worker log: %v", err)
					} else {
						t.Logf("worker log:\n%s", log)
					}
				}
			})
			workerGate.wait(t, done)
			if period := heartbeatReceive(t, periods); period != board.HeartbeatInterval {
				t.Fatalf("heartbeat period = %s", period)
			}
			heartbeatReceive(t, missedStartup)
			if h, exists := runnerHeartbeat(t, s, "quiet"); exists {
				t.Fatalf("failed startup observation produced evidence: %+v", h)
			}
			leased := jobRow(s.DB, job.str("id"))

			// Startup presence is best-effort; a scheduled observation must recover
			// without changing the lease or depending on startup I/O timing.
			beatAt := time.Unix(0, clock.Add(int64(board.HeartbeatInterval))).UTC()
			ticks <- beatAt
			heartbeatEventually(t, func() bool {
				h, _ := runnerHeartbeat(t, s, "quiet")
				return h.At == beatAt.Format(time.RFC3339Nano)
			})
			first, _ := runnerHeartbeat(t, s, "quiet")
			if leased.str("status") != "leased" || leased.int("generation") != 1 || leased.int("attempts") != 1 ||
				first.JobID != job.str("id") || first.Agent != "quiet" || first.Generation != 1 || first.StoppedAt != "" {
				t.Fatalf("initial lifecycle: job=%v heartbeat=%+v", leased, first)
			}
			// Six logical minutes pass while a real, quiet child is held open.
			// Presence expires at five missed minutes; the lease expires strictly after its deadline.
			edge := beatAt.Add(board.HeartbeatGrace)
			clock.Store(edge.Add(-time.Nanosecond).UnixNano())
			assertLifecyclePresence(t, s, time.Unix(0, clock.Load()), "online")
			clock.Store(edge.UnixNano())
			assertLifecyclePresence(t, s, edge, "offline")
			if edge.Format(time.RFC3339Nano) != leased.str("lease_until") {
				t.Fatalf("fixture missed the lease boundary: %s vs %s", edge, leased.str("lease_until"))
			}
			if other, err := leaseJob(s, "rival", edge.Format(time.RFC3339Nano), leaseOpts{JobID: job.str("id"), Minutes: 1}); err != nil || other != nil {
				t.Fatalf("lease stolen at its exact deadline: %v, %v", other, err)
			}
			ticks <- edge
			heartbeatEventually(t, func() bool {
				h, _ := runnerHeartbeat(t, s, "quiet")
				return h.At == edge.Format(time.RFC3339Nano)
			})
			assertLifecyclePresence(t, s, edge, "online")
			if current := jobRow(s.DB, job.str("id")); !reflect.DeepEqual(current, leased) {
				t.Fatalf("missed/recovered heartbeat changed the lease: before=%v after=%v", leased, current)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'job.renew' AND subject = ?", job.str("id")); n != 0 {
				t.Fatalf("heartbeat renewed the lease %d time(s)", n)
			}

			var replacement row
			var replacementAt string
			takeover := func() {
				t.Helper()
				current := jobRow(s.DB, job.str("id"))
				until, err := time.Parse(time.RFC3339Nano, current.str("lease_until"))
				if err != nil {
					t.Fatal(err)
				}
				at := time.Unix(0, clock.Load()).UTC()
				if at.Before(until) {
					at = until
				}
				at = at.Add(time.Second)
				clock.Store(at.UnixNano())
				replacementAt = at.Format(time.RFC3339Nano)
				// Reuse the same agent so generation, not just the agent name, must fence old work.
				replacement, err = leaseJob(s, "quiet", replacementAt, leaseOpts{JobID: job.str("id"), Minutes: 1})
				if err != nil || replacement == nil || replacement.int("generation") != first.Generation+1 || replacement.int("attempts") != 2 {
					t.Fatalf("expired lease takeover: %v, %v", replacement, err)
				}
				before, _ := runnerHeartbeat(t, s, "quiet")
				late := board.ExecutionHeartbeat{ExecutionIdentity: first.ExecutionIdentity, At: replacementAt}
				if changed, err := board.RecordHeartbeat(s, late); err != nil || changed {
					t.Fatalf("stale heartbeat accepted: changed=%v, %v", changed, err)
				}
				if changed, err := board.StopHeartbeat(s, first.ExecutionIdentity, at); err != nil || changed {
					t.Fatalf("stale stop accepted: changed=%v, %v", changed, err)
				}
				if renewLease(s.DB, replacementAt, job.str("id"), "quiet", first.Generation, 5) {
					t.Fatal("old generation renewed the replacement lease")
				}
				if _, err := finishJob(s, job.str("id"), "quiet", "done", "late result", "", first.Generation); err == nil || !strings.Contains(err.Error(), "Stale lease") {
					t.Fatalf("old generation finished the replacement job: %v", err)
				}
				if after, _ := runnerHeartbeat(t, s, "quiet"); after != before {
					t.Fatalf("stale observations changed heartbeat: before=%+v after=%+v", before, after)
				}
				if current := jobRow(s.DB, job.str("id")); !reflect.DeepEqual(current, replacement) {
					t.Fatalf("stale work changed the replacement lease: %v", current)
				}
			}
			if tc.takeover == "worker" {
				takeover()
			}
			if tc.storeReported {
				if _, err := finishJob(s, job.str("id"), "quiet", "done", "slow lifecycle complete", "", first.Generation); err != nil {
					t.Fatal(err)
				}
				at := time.Unix(0, clock.Add(int64(board.HeartbeatInterval))).UTC()
				ticks <- at
				heartbeatEventually(t, func() bool {
					h, _ := runnerHeartbeat(t, s, "quiet")
					return h.At == at.Format(time.RFC3339Nano)
				})
				assertLifecyclePresence(t, s, at, "online")
			}
			workerGate.release(t)
			if tc.takeover != "worker" {
				verifyGate.wait(t, done)
				if r := mustOne(s.DB, "SELECT * FROM runs WHERE id = ?", first.RunID); r.str("status") != "running" {
					t.Fatalf("run ended before verification completed: %v", r)
				}
				stopped, _ := runnerHeartbeat(t, s, "quiet")
				if stopped.StoppedAt == "" {
					t.Fatal("worker heartbeat did not stop before verification")
				}
				clock.Add(int64(2 * board.HeartbeatInterval))
				assertLifecyclePresence(t, s, time.Unix(0, clock.Load()), "offline")
				if tc.takeover == "verification" {
					takeover()
				}
				verifyGate.release(t)
			}
			waitLifecycleRun(t, done)
			if runErr != nil || reason != "" || result == nil {
				t.Fatalf("work: result=%v reason=%q err=%v", result, reason, runErr)
			}
			log, err := os.ReadFile(result.str("log"))
			if err != nil || !strings.Contains(string(log), "presence unavailable: "+startupFailure) {
				t.Fatalf("missed startup heartbeat was not reported: %v\n%s", err, log)
			}

			successful, wantRuns := result, 1
			if tc.takeover != "" {
				if result.str("status") != "stale" || result.str("branch") != "" || result.int("commits") != 1 {
					t.Fatalf("old run was not fenced: %v", result)
				}
				if refs := run(t, root, "git", "for-each-ref", "--format=%(refname)", "refs/heads/pulse/"); refs != "" {
					t.Fatalf("stale run landed a branch: %s", refs)
				}
				if current := jobRow(s.DB, job.str("id")); !reflect.DeepEqual(current, replacement) {
					t.Fatalf("old run overwrote replacement lease: %v", current)
				}
				assertLifecycleSuggestion(t, s, suggestion.str("id"), replacementAt, "eligible", true)
				workerGate.release(t)
				verifyGate.release(t)
				successful, err = runJob(s, replacement, "quiet", replacementAt, runOpts{WorkerCmd: worker, Ctx: ctx})
				if err != nil {
					t.Fatal(err)
				}
				wantRuns++
			}
			if successful.str("status") != "done" || successful.int("commits") != 1 || successful.str("verify") != "verify exit 0" ||
				successful.str("branch") != "pulse/quiet/"+job.str("id") || successful.str("finished_at") == "" {
				t.Fatalf("successful run: %v", successful)
			}
			finished := jobRow(s.DB, job.str("id"))
			if finished.str("status") != "done" || finished.str("lease_until") != "" || finished.int("generation") != wantRuns ||
				finished.int("attempts") != wantRuns || finished.str("finished_at") == "" ||
				finished.str("branch") != successful.str("branch") || finished.str("result") != successful.str("summary") ||
				!strings.Contains(finished.str("result"), "slow lifecycle complete") {
				t.Fatalf("job/run completion diverged: job=%v run=%v", finished, successful)
			}
			if got := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?", job.str("id")); got != wantRuns {
				t.Fatalf("runs=%d, want %d", got, wantRuns)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'done'", job.str("id")); n != 1 {
				t.Fatalf("successful runs = %d, want 1", n)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result' AND area = 'jobs' AND thread = ?", board.Slugify(job.str("id"))); n != 1 {
				t.Fatalf("result posts = %d, want 1", n)
			}
			if agent := agentRow(s.DB, "quiet"); agent.int("jobs_done") != 1 || agent.int("jobs_failed") != 0 {
				t.Fatalf("worker completion counted incorrectly: %v", agent)
			}
			finalBeat, _ := runnerHeartbeat(t, s, "quiet")
			if finalBeat.RunID != successful.str("id") || finalBeat.Generation != wantRuns || finalBeat.StoppedAt == "" {
				t.Fatalf("final heartbeat: %+v", finalBeat)
			}
			now := time.Unix(0, clock.Load()).UTC()
			assertLifecyclePresence(t, s, now, "idle")
			if changed, err := board.RecordHeartbeat(s, board.ExecutionHeartbeat{ExecutionIdentity: first.ExecutionIdentity,
				At: now.Add(board.HeartbeatInterval).Format(time.RFC3339Nano)}); err != nil || changed {
				t.Fatalf("finished execution returned online: changed=%v, %v", changed, err)
			}
			if after, _ := runnerHeartbeat(t, s, "quiet"); after != finalBeat {
				t.Fatalf("late heartbeat replaced final evidence: %+v", after)
			}
			if renewLease(s.DB, now.Format(time.RFC3339Nano), job.str("id"), "quiet", finished.int("generation"), 5) {
				t.Fatal("completed job reacquired a lease")
			}
			assertLifecycleSuggestion(t, s, suggestion.str("id"), now.Format(time.RFC3339Nano), "built", false)
			for i := 0; i < 2; i++ {
				if _, err := planJobs(s, "proj", now.Format(time.RFC3339Nano), "daemon"); err != nil {
					t.Fatal(err)
				}
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source = ?", job.str("source")); n != 1 {
				t.Fatalf("suggestion planned %d times", n)
			}
			if again, why, err := workOnce(s, "quiet", "proj", job.str("id"), now.Format(time.RFC3339Nano),
				runOpts{WorkerCmd: worker, Ctx: ctx}); err != nil || again != nil || !strings.Contains(why, "no queued job") {
				t.Fatalf("completed suggestion dispatched again: %v, %q, %v", again, why, err)
			}
			if head := strings.TrimSpace(run(t, root, "git", "rev-parse", "HEAD")); head != base {
				t.Fatal("worker moved the project's default branch")
			}
			if refs := strings.TrimSpace(run(t, root, "git", "for-each-ref", "--format=%(refname:short)", "refs/heads/pulse/")); refs != successful.str("branch") {
				t.Fatalf("expected exactly one result branch, got %q", refs)
			}
			if content := run(t, root, "git", "show", successful.str("branch")+":lifecycle.txt"); content != "slow lifecycle\n" {
				t.Fatalf("landed content = %q", content)
			}
			if entries, err := os.ReadDir(filepath.Join(s.Home, "work")); err != nil || len(entries) != 0 {
				t.Fatalf("worker left temporary state: %v, %v", entries, err)
			}
		})
	}
}
